These files are samples of what each provider returns, trimmed to a few
items. Most are hand-written in the right shape, not captured responses.
The exceptions, trimmed from real responses in Nov 2026:

- `roku_search.json`: an `app.roku.com` search, including its `viewOptions`
  free/paid markers.
- `plex_mirror.json` and `plex_live_tv.html`: the i.mjh.nz Plex channel file
  and Plex's live TV page. The mirror's request headers and token are left
  out.
- `roku_mirror.json`: the i.mjh.nz Roku channel file, plus one made-up
  entry with a bad id. The mirror's request headers are left out.

Use `conflux check` against the real services to confirm the shapes still
hold, and replace a sample with a real (trimmed) response when a parser
changes.
