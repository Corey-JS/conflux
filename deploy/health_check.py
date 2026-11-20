#!/usr/bin/env python3
"""Checks a running Conflux and exits 1 if anything looks broken.

    python3 deploy/health_check.py [https://your-conflux]

Reads only Conflux's own /healthz and /api/providers, so running it often
costs the providers nothing. Run hourly by .github/workflows/health.yml;
a failing run is what sends the alert (GitHub emails on failed runs).

A provider counts as broken when:
- its catalog is empty, or hasn't refreshed for STALE_HOURS (it refreshes
  hourly and keeps the old copy when a refresh fails, so search keeps
  working but quietly goes out of date);
- it has schedules but none are loaded;
- it's paused because it answered "too many requests" (reported, but not
  a failure on its own unless it also goes stale).
"""

import json
import os
import sys
import time
import urllib.request

DEFAULT_URL = "https://132-145-136-144.sslip.io"
STALE_HOURS = 3
TIMEOUT = 30


def get(url):
    req = urllib.request.Request(url, headers={"User-Agent": "conflux-health-check"})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return r.status, r.read()


def check(base):
    problems, notes = [], []
    try:
        status, body = get(f"{base}/healthz")
        if status != 200 or body.strip() != b"ok":
            problems.append(f"/healthz answered {status}: {body[:100]!r}")
    except Exception as e:  # noqa: BLE001 - any failure to reach it is the alert
        return [f"server unreachable: /healthz failed: {e}"], notes

    try:
        _, body = get(f"{base}/api/providers")
        providers = json.loads(body)["providers"]
    except Exception as e:  # noqa: BLE001
        return problems + [f"/api/providers failed: {e}"], notes

    if not providers:
        problems.append("no providers are turned on")
    now = time.time()
    for p in providers:
        name = p.get("name") or p.get("id", "?")
        mode = p.get("mode", "")
        if "catalog" in mode:
            items = p.get("catalog_items", 0)
            ok_at = p.get("catalog_ok_at")
            err = p.get("catalog_error")
            if items == 0:
                problems.append(f"{name}: catalog is empty ({err or 'no error given'})")
            elif ok_at is None or now - ok_at > STALE_HOURS * 3600:
                age = "never" if ok_at is None else f"{(now - ok_at) / 3600:.1f} h ago"
                problems.append(f"{name}: catalog last refreshed {age} ({err or 'no error given'})")
            else:
                notes.append(f"{name}: {items} items, refreshed {(now - ok_at) / 60:.0f} min ago")
        if p.get("has_guide") and not p.get("guide_channels"):
            problems.append(f"{name}: no schedules loaded ({p.get('guide_error') or 'no error given'})")
        elif p.get("guide_error"):
            notes.append(f"{name}: last schedule refresh failed: {p['guide_error']}")
        if "search" in mode and p.get("search_error"):
            # Searches can fail now and then (rate limits); worth seeing, not paging.
            notes.append(f"{name}: last search failed: {p['search_error']}")
        if p.get("paused_until"):
            mins = max(0, (p["paused_until"] - now) / 60)
            notes.append(f"{name}: paused for {mins:.0f} more min (provider said slow down)")
    return problems, notes


def main():
    base = (sys.argv[1] if len(sys.argv) > 1 else os.environ.get("CONFLUX_URL") or DEFAULT_URL).rstrip("/")
    problems, notes = check(base)
    lines = [f"Conflux health: {base}", ""]
    lines += [f"FAIL  {p}" for p in problems]
    lines += [f"ok    {n}" for n in notes]
    report = "\n".join(lines)
    print(report)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write("```\n" + report + "\n```\n")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
