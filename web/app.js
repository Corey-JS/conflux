// Conflux: search box -> /api/search, tap a result -> the provider's own page.
"use strict";

const $ = (id) => document.getElementById(id);
const els = {
  form: $("search-form"), q: $("q"), clear: $("clear"), providers: $("providers"),
  results: $("results"), heading: $("heading"), note: $("note"),
};

const state = { q: "", kind: "all", provider: "all" };
const names = {}; // provider id -> display name
let inflight = null;
let timer = null;

const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode */ } },
};

function el(tag, attrs = {}, ...children) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") n.className = v;
    else n.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children) if (c != null) n.append(c);
  return n;
}

function svg(path) {
  const ns = "http://www.w3.org/2000/svg";
  const s = document.createElementNS(ns, "svg");
  s.setAttribute("viewBox", "0 0 24 24");
  s.setAttribute("class", "go");
  s.setAttribute("aria-hidden", "true");
  const p = document.createElementNS(ns, "path");
  p.setAttribute("d", path);
  s.append(p);
  return s;
}

const KIND_LABEL = { movie: "Movie", series: "Show", live: "Live" };

function row(item) {
  if (!/^https:\/\//.test(item.watch_url)) return null;
  const isLive = item.kind === "live";
  const thumb = el("div", { class: "thumb " + (isLive ? "logo" : "poster") });
  if (item.image) {
    const img = el("img", { src: item.image, alt: "", loading: "lazy", decoding: "async", referrerpolicy: "no-referrer" });
    img.addEventListener("error", () => { img.remove(); thumb.textContent = initial(item.title); }, { once: true });
    thumb.append(img);
  } else {
    thumb.textContent = initial(item.title);
  }

  const meta = el("div", { class: "meta" }, el("i", { class: "dot " + item.provider }));
  const bits = [names[item.provider] || item.provider];
  if (isLive) {
    meta.append(el("b", { class: "live-badge" }, "LIVE"));
    if (item.channel_number) bits.push("Ch " + item.channel_number);
  } else {
    bits.push(KIND_LABEL[item.kind] || "");
    if (item.year) bits.push(String(item.year));
  }
  if (item.genres && item.genres[0]) bits.push(item.genres[0]);
  meta.append(el("span", {}, bits.filter(Boolean).join(" · ")));

  const label = `${item.title}, ${names[item.provider] || item.provider}`;
  const a = el("a", { href: item.watch_url, target: "_blank", rel: "noopener noreferrer", "aria-label": label },
    thumb,
    el("div", { class: "info" }, el("div", { class: "title" }, item.title), meta),
    svg("M7 17L17 7M9 7h8v8"));
  return el("li", {}, a);
}

// ---- TV guide (live channels with what's on now and next) ----
const fmtTime = new Intl.DateTimeFormat([], { hour: "numeric", minute: "2-digit" });
const live = { items: [], skew: 0, fetchedAt: 0 }; // skew: server clock minus ours, in ms
const expanded = new Set();
let guideRows = []; // what renderGuide drew: [{ item, key, li, bar, start, stop }]
let view = null; // "guide" or "results" once drawn, so refreshes can skip the skeleton
const serverNow = () => Date.now() + live.skew;
const clock = (sec) => fmtTime.format(new Date(sec * 1000));
const progress = (start, stop, t) => Math.min(100, Math.max(0, ((t - start) / (stop - start)) * 100));

// Which program a row shows and whether it has started. A row only needs
// rebuilding when this changes; otherwise just its progress bar moves.
function guideKey(item, t) {
  const p = (item.programs || []).find((x) => x.stop > t);
  return p ? `${p.start}${p.start <= t ? "+" : "-"}` : "";
}

function guideRow(item, t) {
  const progs = (item.programs || []).filter((p) => p.stop > t);
  if (!progs.length) return { li: row(item) };
  const [now, ...later] = progs;
  const airing = now.start <= t;
  const pct = airing ? progress(now.start, now.stop, t) : 0;

  // Art for the show that's on, like album art; the channel logo is the fallback.
  const art = now.image || item.image;
  const thumb = el("div", { class: "thumb " + (now.image ? "poster" : "logo") });
  if (art) {
    const img = el("img", { src: art, alt: "", loading: "lazy", decoding: "async", referrerpolicy: "no-referrer" });
    img.addEventListener("error", () => {
      img.remove();
      if (item.image && art !== item.image) {
        thumb.className = "thumb logo";
        thumb.append(el("img", { src: item.image, alt: "", referrerpolicy: "no-referrer" }));
      } else thumb.textContent = initial(item.title);
    }, { once: true });
    thumb.append(img);
  } else thumb.textContent = initial(item.title);

  const chan = el("div", { class: "meta" }, el("i", { class: "dot " + item.provider }),
    el("span", {}, [item.channel_number ? "Ch " + item.channel_number : null, item.title].filter(Boolean).join(" · ")));
  const when = airing ? `${clock(now.start)} – ${clock(now.stop)}` : `Starts ${clock(now.start)}`;
  const sub = [now.subtitle, when, now.rating].filter(Boolean).join(" · ");
  const info = el("div", { class: "info" }, chan,
    el("div", { class: "title" }, now.title),
    el("div", { class: "g-sub" }, sub));
  const bar = airing ? el("i", { style: `width:${pct.toFixed(1)}%` }) : null;
  if (bar) info.append(el("div", { class: "bar", "aria-hidden": "true" }, bar));
  if (later[0]) info.append(el("div", { class: "g-next" }, el("b", {}, "Next "), `${clock(later[0].start)}  ${later[0].title}`));

  const watch = el("a", { class: "watch", href: item.watch_url, target: "_blank", rel: "noopener noreferrer", "aria-label": `Watch ${item.title}` },
    svg("M7 17L17 7M9 7h8v8"));
  const open = expanded.has(item.id);
  const main = el("div", { class: "g-main", role: "button", tabindex: "0", "aria-expanded": String(open) }, thumb, info, watch);

  const sched = el("ol", { class: "g-sched" }, ...progs.map((p) =>
    el("li", {}, el("time", {}, clock(p.start)),
      el("div", {}, el("b", {}, p.title), p.subtitle ? el("span", {}, " · " + p.subtitle) : null,
        p.description ? el("p", {}, p.description) : null))));
  sched.hidden = !open;

  const toggle = () => {
    const o = main.getAttribute("aria-expanded") !== "true";
    main.setAttribute("aria-expanded", String(o));
    sched.hidden = !o;
    if (o) expanded.add(item.id); else expanded.delete(item.id);
  };
  main.addEventListener("click", (e) => { if (!e.target.closest("a")) toggle(); });
  main.addEventListener("keydown", (e) => {
    if ((e.key === "Enter" || e.key === " ") && e.target === main) { e.preventDefault(); toggle(); }
  });
  return { li: el("li", { class: "guide" }, main, sched), bar, start: now.start, stop: now.stop };
}

function renderGuide() {
  const t = serverNow() / 1000;
  guideRows = [];
  for (const item of live.items) {
    const r = guideRow(item, t);
    if (r.li) guideRows.push({ item, key: guideKey(item, t), ...r });
  }
  els.results.replaceChildren(...guideRows.map((r) => r.li));
  view = "guide";
  if (!live.items.length) note("No live channels loaded yet. Try again in a minute.");
}

// Move progress bars, and rebuild only the rows whose show has changed.
// Rebuilding all 1,000+ rows every 30s would drop focus and redo every image.
function tickGuide() {
  const t = serverNow() / 1000;
  for (const r of guideRows) {
    const key = guideKey(r.item, t);
    if (key !== r.key) {
      const fresh = guideRow(r.item, t);
      if (!fresh.li) continue;
      r.li.replaceWith(fresh.li);
      Object.assign(r, { bar: null }, fresh, { key });
    } else if (r.bar) {
      r.bar.style.width = `${progress(r.start, r.stop, t).toFixed(1)}%`;
    }
  }
}

function initial(title) {
  return (title || "?").trim().charAt(0).toUpperCase();
}

function skeleton(n = 6) {
  view = null;
  els.results.replaceChildren(...Array.from({ length: n }, () =>
    el("li", { class: "skeleton" }, el("a", {},
      el("div", { class: "thumb" }), el("div", { class: "info" },
        el("div", { class: "title" }, "Loading title"), el("div", { class: "meta" }, "Loading")), el("span")))));
}

function note(text) {
  els.note.hidden = !text;
  els.note.textContent = text || "";
}

function render(items, emptyText) {
  els.results.replaceChildren(...items.map(row).filter(Boolean));
  view = "results";
  if (!items.length) note(emptyText);
}

async function getJSON(url, signal) {
  const r = await fetch(url, { signal, headers: { Accept: "application/json" } });
  if (!r.ok) throw new Error(r.status === 503 ? "offline" : `HTTP ${r.status}`);
  return r.json();
}

function params(extra) {
  const p = new URLSearchParams(extra);
  if (state.kind !== "all") p.set("kind", state.kind);
  if (state.provider !== "all") p.set("provider", state.provider);
  return p;
}

async function update() {
  if (inflight) inflight.abort();
  inflight = new AbortController();
  const signal = inflight.signal;
  note("");
  const q = state.q.trim();
  els.clear.hidden = !q;
  const url = new URL(location.href);
  if (q) url.searchParams.set("q", q); else url.searchParams.delete("q");
  history.replaceState(null, "", url);

  try {
    if (!q) {
      if (state.kind === "movie" || state.kind === "series") {
        els.heading.textContent = state.kind === "movie" ? "Movies" : "Shows";
        view = null;
        els.results.replaceChildren();
        note("Type a title to search free movies and shows.");
        return;
      }
      els.heading.textContent = "Live channels";
      // A background refresh redraws in place instead of flashing the skeleton.
      if (view !== "guide") skeleton();
      const p = params({}); // no limit: the server sends whole lineups
      p.delete("kind");
      const data = await getJSON("/api/live?" + p, signal);
      live.items = data.items;
      live.skew = data.now ? data.now * 1000 - Date.now() : 0;
      live.fetchedAt = Date.now();
      renderGuide();
      return;
    }
    els.heading.textContent = "Results";
    skeleton(4);
    const data = await getJSON("/api/search?" + params({ q }), signal);
    render(data.items, `Nothing free found for “${q}”.`);
    if (data.unavailable && data.unavailable.length) {
      const msg = `${data.unavailable.join(" and ")} didn't answer this time.`;
      note(data.items.length ? msg : `Nothing found for “${q}”. ${msg}`);
    }
  } catch (e) {
    if (e.name === "AbortError") return;
    view = null;
    els.results.replaceChildren();
    note(navigator.onLine === false || e.message === "offline"
      ? "You're offline. Search needs a connection."
      : "Couldn't reach the Conflux server. Try again.");
  }
}

function schedule(delay = 250) {
  clearTimeout(timer);
  timer = setTimeout(update, delay);
}

function setPressed(container, attr, value) {
  for (const b of container.querySelectorAll("button")) {
    b.setAttribute("aria-pressed", String(b.dataset[attr] === value));
  }
}

async function loadProviders() {
  let list = [];
  try {
    list = (await getJSON("/api/providers")).providers || [];
  } catch { /* chips are optional */ }
  const chip = (id, label, down) => {
    const b = el("button", { type: "button", "data-provider": id, "aria-pressed": String(state.provider === id), class: down ? "down" : null });
    if (id !== "all") b.append(el("i", { class: "dot " + id }));
    b.append(label);
    return b;
  };
  const chips = [chip("all", "All providers")];
  for (const p of list) {
    names[p.id] = p.name;
    const down = Boolean(p.catalog_error && !p.catalog_items) || Boolean(p.search_error && !p.mode.includes("catalog"));
    chips.push(chip(p.id, p.name, down));
  }
  if (state.provider !== "all" && !names[state.provider]) state.provider = "all";
  els.providers.replaceChildren(...chips);
  setPressed(els.providers, "provider", state.provider);
}

function init() {
  state.kind = store.get("kind") || "all";
  state.provider = store.get("provider") || "all";
  state.q = new URL(location.href).searchParams.get("q") || "";
  els.q.value = state.q;
  setPressed(document.querySelector(".kinds"), "kind", state.kind);

  els.q.addEventListener("input", () => { state.q = els.q.value; schedule(); });
  els.form.addEventListener("submit", (e) => { e.preventDefault(); els.q.blur(); schedule(0); });
  els.clear.addEventListener("click", () => { els.q.value = state.q = ""; els.q.focus(); schedule(0); });

  document.querySelector(".kinds").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    state.kind = b.dataset.kind;
    store.set("kind", state.kind);
    setPressed(e.currentTarget, "kind", state.kind);
    schedule(0);
  });
  els.providers.addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    state.provider = b.dataset.provider;
    store.set("provider", state.provider);
    setPressed(els.providers, "provider", state.provider);
    schedule(0);
  });

  loadProviders().then(() => update());

  // Keep "now" honest: move progress every 30s, refetch schedules every 5 min.
  setInterval(() => {
    if (document.hidden || view !== "guide") return;
    if (Date.now() - live.fetchedAt > 5 * 60 * 1000) update(); else tickGuide();
  }, 30 * 1000);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) schedule(0); });

  if ("serviceWorker" in navigator) {
    navigator.serviceWorker.register("/sw.js").catch(() => {});
  }
}

init();
