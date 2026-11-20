These JSON files are samples in the shape each provider's web app returns,
trimmed to a few items. Most are hand-written, not captured responses.
`roku_search.json` is trimmed from a real `app.roku.com` search response
(Nov 2026), including its `viewOptions` free/paid markers. Use
`conflux check` against the real services to confirm the shapes still hold,
and replace a sample with a real (trimmed) response when a parser changes.
