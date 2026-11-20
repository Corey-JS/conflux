These files are samples of what each provider returns, trimmed to a few
items. Most are hand-written in the right shape, not captured responses.
The exceptions, trimmed from real responses in Nov 2026:

- `roku_search.json`: an `app.roku.com` search, including its `viewOptions`
  free/paid markers.
- `plex_mirror.json` and `plex_live_tv.html`: the i.mjh.nz Plex channel file
  and Plex's live TV page. The mirror's request headers and token are left
  out.
- `plex_art.xml`: two channels from epgshare01's Plex guide (XMLTV), the
  same ones as `plex_mirror.json`, with their programmes for the same hours.
- `roku_art.xml`: the same two Roku channels from the mirror's Roku guide
  (XMLTV), with their programmes for one afternoon.
- `roku_mirror.json`: the i.mjh.nz Roku channel file, plus one made-up
  entry with a bad id. The mirror's request headers are left out.
- `samsung_channels.json`: two US channels and one UK channel from the
  i.mjh.nz Samsung TV Plus file, schedules cut to two slots. Its request
  headers are left out.
- `xumo_channels.json` and `xumo_epg.json`: Xumo Play's channel list (three
  channels plus one on-demand "page" that isn't a channel) and one channel's
  slice of a guide block, with the assets it names.

Still hand-written: the Pluto samples and `tubi_search.json`. Tubi's old
search endpoint answers 401 for everyone (Nov 2026), so there's no real
response left to capture; its sample is kept in case the endpoint returns.

Use `conflux check` against the real services to confirm the shapes still
hold, and replace a sample with a real (trimmed) response when a parser
changes.
