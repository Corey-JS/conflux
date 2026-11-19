# Conflux

Search free live TV and on-demand movies and shows across several free
(FAST) streaming services in one place. Tap a result and Conflux sends you to
that provider's own page or app to watch. Conflux never streams or proxies
video itself.

It's a small Rust server plus a lightweight installable web app (PWA) built
for iPhone Safari.

## How it works

```
iPhone (PWA)  ──/api/search──▶  Rust server  ──▶  Pluto TV, Plex, Tubi, Roku Channel
   tap result ──────────────────────────────────▶  provider's own watch page / app
```

Browsers can't read these catalogs directly (CORS), and none of the
providers offer a public API, so the server calls the same JSON endpoints
each provider's own web app uses, then normalizes everything into one shape.

There are two kinds of provider:

- **Catalog** providers have lists small enough to download whole: live
  channel lineups and Pluto's on-demand library. The server refreshes them
  hourly and searches them in memory.
- **Search** providers have libraries far too big to download (Tubi has
  hundreds of thousands of titles), so the server forwards each query to
  them and caches the answer for 15 minutes.

Every provider sits in its own module. If one breaks, the others keep
working, and `/api/providers` shows which one failed and why.

## Providers

| Provider | What Conflux gets | How | Fragility |
| --- | --- | --- | --- |
| Pluto TV | Live channels + on-demand library | Web app's boot token, then its guide and on-demand APIs; falls back to Pluto's older token-free channel list | Medium. Pluto changes app versions now and then. |
| Plex | Live channels + on-demand search | Guest token (the same one plex.tv gives signed-out visitors, not an account), then its EPG and on-demand search | Medium |
| Tubi | On-demand search (also live channels if they match) | Web app's search endpoint | Medium |
| The Roku Channel | On-demand + live search | Web app's CSRF token, then search; response read leniently | **High.** Least certain endpoint; expect to fix after a first real run. US only. |
| Samsung TV Plus | Live channels | Channel list mirrored by the open-source i.mjh.nz project | **Off by default.** Samsung's website reportedly closed and there's no iPhone app, so a tap has nowhere to go. |

None of these were reachable from the machine this was built on, so the
parsers are tested against hand-written samples of each response shape
(`tests/fixtures/`). Run `conflux check` on the server to test the real ones.

## Run it locally

Needs Rust (https://rustup.rs).

```
cargo run --release              # http://127.0.0.1:8080
cargo run --release -- check     # fetch every provider once and report
cargo test
```

On a phone the app must be served over HTTPS to install; see
[docs/DEPLOY.md](docs/DEPLOY.md) for the Google Cloud VM setup.

## API

| Endpoint | Returns |
| --- | --- |
| `GET /api/search?q=star+trek&kind=live\|movie\|series&provider=pluto&limit=60` | `{ query, items: [Item], unavailable: [provider names] }` |
| `GET /api/live?provider=pluto` | `{ items: [Item] }`, live channels in channel order |
| `GET /api/providers` | Each provider's mode, item count, last success and last error |
| `GET /healthz` | `ok` |

An `Item` is `{ id, provider, kind, title, description?, image?, genres, year?, channel_number?, watch_url }`.

## Layout

```
src/
  main.rs            server and `check` command
  api.rs             HTTP routes and static files
  catalog.rs         per-provider caches, refresh timer, merged search
  search.rs          text normalization and ranking
  model.rs           the shared Item type
  providers/         one file per provider
web/                 the PWA (plain HTML, CSS, JS, service worker)
deploy/              systemd unit, Caddy config, install script
tests/               API tests with fake providers; parser tests with fixtures
```

## Adding a provider

1. Add `src/providers/<name>.rs` implementing `Provider` (`catalog` for a
   downloadable list, `search` for per-query search, or both).
2. Keep the JSON parsing in a `pub fn parse_…(&Value) -> Vec<Item>` and test
   it against a fixture in `tests/fixtures/`.
3. Register it in `providers::build` and add a dot color in `web/app.css`.
