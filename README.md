# Conflux

Search free live TV and on-demand movies and shows across several free
(FAST) streaming services in one place. Tap a result and Conflux sends you to
that provider's own page or app to watch. Conflux never streams or proxies
video itself.

It's a small Rust server plus a lightweight installable web app (PWA) built
for iPhone Safari.

## How it works

```
iPhone (PWA)  ──/api/search──▶  Rust server  ──▶  Pluto TV, Plex, Roku Channel, Xumo Play
   tap result ──────────────────────────────────▶  provider's own watch page / app
```

Browsers can't read these catalogs directly (CORS), and none of the
providers offer a public API, so the server calls the same JSON endpoints
each provider's own web app uses, then normalizes everything into one shape.

There are two kinds of provider:

- **Catalog** providers have lists small enough to download whole: live
  channel lineups and Pluto's on-demand library. The server refreshes them
  hourly and searches them in memory.
- **Search** providers have libraries far too big to download (The Roku
  Channel's on-demand library), so the server forwards each query to them
  and caches the answer for 15 minutes.

To stay a polite guest, Conflux limits what it asks of providers:

- Live searches to any one provider are capped at a burst of 5, then about
  one a second. Past that, results come from the other providers and that
  one is shown as unavailable for the search. Cached answers don't count.
- A provider that answers "too many requests" (HTTP 429) is left alone,
  catalog, schedules and search, for as long as its Retry-After says
  (15 minutes if it doesn't say; 1 minute to 6 hours). `/api/providers`
  shows `paused_until` meanwhile. Image hosts get the same treatment.
- Each visitor may search 30 times in a burst, then once a second; past
  that `/api/search` answers 429. The app searches once typing pauses, not
  on every letter.

Every provider sits in its own module. If one breaks, the others keep
working, and `/api/providers` shows which one failed and why.

## Providers

| Provider | What Conflux gets | How | Fragility |
| --- | --- | --- | --- |
| Pluto TV | Live channels + on-demand library | Web app's boot token, then its guide and on-demand APIs; falls back to Pluto's older token-free channel list | Medium. Pluto changes app versions now and then. |
| Plex | Live channels + schedules with show art | Channel list and schedules mirrored by the open-source i.mjh.nz project; each channel's link and genre from Plex's public live TV page; show art, episode names and ratings from epgshare01's free Plex guide (XMLTV, matched by channel name and time). No Plex token. | Medium. Since Nov 2026 plex.tv gives signed-out guest tokens only to Plex's own apps, so on-demand search is gone. `CONFLUX_PLEX_REGION` picks the region (default `us`). |
| Tubi | On-demand search (also live channels if they match) | Web app's old `/oz/search` endpoint | **Off by default (Nov 2026).** The endpoint now answers 401. Tubi's web app moved to a search service whose signed-out token needs a request signature only Tubi's own client is meant to produce. The module and its parser are kept; add `tubi` to `CONFLUX_PROVIDERS` to try it. |
| The Roku Channel | Live channels + schedules with show art; on-demand and live search | Live lineup and schedules mirrored by the open-source i.mjh.nz project; show art, episode names and descriptions from the same project's Roku guide file (XMLTV, matched by Roku's channel id). Search: web app's CSRF token, then search on `app.roku.com`; response read leniently; only items Roku marks free (`rokuavod`) are kept, because its search also returns Paramount+ and other paid channels | Medium. Checked against the live service in Nov 2026. US only. |
| Xumo Play | Live channels + schedules | Channel list and guide from the same `valencia-app-mds.xumo.com` endpoints play.xumo.com's web app loads; no token. The guide comes in pages of 50 channels per six-hour block, so a refresh is about ten small requests. Taps go to `play.xumo.com/networks/<slug>/<id>` (Xumo finds the page by id) or the Xumo Play app. | Medium. Checked against the live service in Nov 2026. US only. |
| Samsung TV Plus | Live channels | Channel list mirrored by the open-source i.mjh.nz project | **Off by default.** Samsung's website reportedly closed and there's no iPhone app, so a tap has nowhere to go. |

The parsers are tested against samples of each response shape
(`tests/fixtures/`). Pluto, Plex, Roku and Xumo were checked against the real
services in Nov 2026 (the Plex and Roku samples are trimmed from real
responses); Xumo and Samsung samples are trimmed from real responses too; the rest are still hand-written. Run `conflux check` on the server to test the real ones.

## Run it locally

Needs Rust (https://rustup.rs).

```
cargo run --release              # http://127.0.0.1:8080
cargo run --release -- check     # fetch every provider once and report
cargo test
```

On a phone the app must be served over HTTPS to install; see
[docs/DEPLOY.md](docs/DEPLOY.md) for the Oracle Cloud VM setup and how to update it.

## API

| Endpoint | Returns |
| --- | --- |
| `GET /api/search?q=star+trek&kind=live\|movie\|series&provider=pluto&limit=60` | `{ query, items: [Item], unavailable: [provider names] }` |
| `GET /api/live?provider=pluto&limit=2000` | `{ items: [Item + programs], now }`: whole lineups by default, grouped by provider, each in channel order. A lower `limit` gives every provider a fair share instead of cutting off the later ones. |
| `GET /api/art/<key>` | A 168×240 JPEG of a show's art, for providers whose art is full size (Plex, Roku). Keys come from `/api/live`; only images in the current guide can be fetched. Made ahead of time for what's on now and next, kept on disk (`CONFLUX_ART_DIR`), and cached by browsers for a week. |
| `GET /api/providers` | Each provider's mode, item count, schedules, last success and last error |
| `GET /healthz` | `ok` |

An `Item` is `{ id, provider, kind, title, description?, image?, genres, year?, channel_number?, watch_url }`.

## Layout

```
src/
  main.rs            server and `check` command
  api.rs             HTTP routes and static files
  art.rs             shrunken copies of full-size show art
  catalog.rs         per-provider caches, refresh timer, merged search
  search.rs          text normalization and ranking
  model.rs           the shared Item type
  providers/         one file per provider
web/                 the PWA (plain HTML, CSS, JS, service worker)
deploy/              systemd unit, Caddy config, install script, health check
tests/               API tests with fake providers; parser tests with fixtures
```

## Adding a provider

1. Add `src/providers/<name>.rs` implementing `Provider` (`catalog` for a
   downloadable list, `search` for per-query search, or both).
2. Keep the JSON parsing in a `pub fn parse_…(&Value) -> Vec<Item>` and test
   it against a fixture in `tests/fixtures/`.
3. Register it in `providers::build` and add a dot color in `web/app.css`.
