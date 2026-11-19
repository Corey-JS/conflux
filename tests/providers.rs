//! Each provider's parser against a hand-written sample of its JSON.

use conflux::model::Kind;
use conflux::providers::{plex, pluto, roku, samsung, tubi};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn pluto_live_channels() {
    let items = pluto::parse_channels(
        &fixture("pluto_channels.json"),
        &fixture("pluto_categories.json"),
    );
    assert_eq!(items.len(), 2);
    let trek = &items[0];
    assert_eq!(trek.id, "pluto:5c45f5a9d40d58066869fa60");
    assert_eq!(trek.kind, Kind::Live);
    assert_eq!(trek.channel_number, Some(146));
    assert_eq!(
        trek.image.as_deref(),
        Some("https://images.pluto.tv/x/logo.png")
    );
    assert_eq!(trek.genres, vec!["Sci-Fi"]);
    assert_eq!(
        trek.watch_url,
        "https://pluto.tv/live-tv/5c45f5a9d40d58066869fa60"
    );
    assert_eq!(items[1].channel_number, Some(200));
    assert_eq!(items[1].description, None);
    assert_eq!(items[1].genres, vec!["News"]);
}

#[test]
fn pluto_on_demand() {
    let items = pluto::parse_vod(&fixture("pluto_vod.json"));
    assert_eq!(items.len(), 2, "dupes and clips dropped: {items:?}");
    assert_eq!(items[0].kind, Kind::Movie);
    assert_eq!(
        items[0].watch_url,
        "https://pluto.tv/on-demand/movies/star-trek-ii-the-wrath-of-khan-1982-1-1"
    );
    assert_eq!(
        items[0].image.as_deref(),
        Some("https://images.pluto.tv/k/poster.jpg")
    );
    assert_eq!(items[0].genres, vec!["Sci-Fi & Fantasy"]);
    assert_eq!(items[1].kind, Kind::Series);
    assert_eq!(
        items[1].watch_url,
        "https://pluto.tv/on-demand/series/star-trek-the-original-series"
    );
    assert_eq!(
        items[1].genres,
        vec!["Sci-Fi Movies"],
        "falls back to category name"
    );
}

#[test]
fn pluto_legacy_fallback() {
    let items = pluto::parse_legacy_channels(&fixture("pluto_legacy.json"));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "Pluto TV Westerns");
    assert_eq!(
        items[0].image.as_deref(),
        Some("https://images.pluto.tv/w/logo.png")
    );
}

#[test]
fn plex_live_channels() {
    let items = plex::parse_live(&fixture("plex_live.json"));
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0].watch_url,
        "https://watch.plex.tv/live-tv/channel/plex-movie-picks"
    );
    assert_eq!(items[0].genres, vec!["Movies"]);
    assert_eq!(
        items[1].watch_url,
        "https://watch.plex.tv/live-tv/channel/weather-now"
    );
    assert_eq!(items[1].image, None, "relative image urls are dropped");
    assert_eq!(items[1].genres, vec!["News"]);
}

#[test]
fn plex_on_demand_search() {
    let items = plex::parse_vod_hubs(&fixture("plex_vod_hubs.json"));
    assert_eq!(items.len(), 2, "episodes skipped");
    assert_eq!(
        items[0].watch_url,
        "https://watch.plex.tv/movie/star-trek-nemesis"
    );
    assert_eq!(items[0].year, Some(2002));
    assert_eq!(items[0].genres, vec!["Science Fiction"]);
    assert_eq!(items[1].kind, Kind::Series);
    assert_eq!(
        items[1].watch_url,
        "https://watch.plex.tv/show/star-trek-enterprise"
    );
    assert_eq!(items[1].year, Some(2001));
    assert_eq!(
        items[1].image.as_deref(),
        Some("https://metadata-static.plex.tv/e/poster.jpg")
    );

    let d = plex::parse_discover(&fixture("plex_discover.json"));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].watch_url, "https://watch.plex.tv/movie/the-mask");
}

#[test]
fn tubi_search_results() {
    let items = tubi::parse_search(&fixture("tubi_search.json"));
    assert_eq!(items.len(), 3, "episodes skipped");
    assert_eq!(items[0].watch_url, "https://tubitv.com/movies/589292");
    assert_eq!(
        items[0].image.as_deref(),
        Some("https://canvas-lb.tubitv.com/opts/mask-poster.jpg")
    );
    assert_eq!(items[0].genres.len(), 3);
    assert_eq!(items[1].kind, Kind::Series);
    assert_eq!(items[1].watch_url, "https://tubitv.com/series/300012");
    assert_eq!(
        items[1].image.as_deref(),
        Some("https://canvas-lb.tubitv.com/opts/hk.jpg")
    );
    assert_eq!(items[2].kind, Kind::Live);

    // Same items wrapped in an object.
    let wrapped = serde_json::json!({ "contents": fixture("tubi_search.json") });
    assert_eq!(tubi::parse_search(&wrapped).len(), 3);
}

#[test]
fn roku_search_results() {
    let items = roku::parse_search(&fixture("roku_search.json"));
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(
        items[0].watch_url,
        "https://therokuchannel.roku.com/details/0a1b2c3d4e5f60718293a4b5c6d7e8f9"
    );
    assert_eq!(items[0].year, Some(1994));
    assert_eq!(
        items[0].image.as_deref(),
        Some("https://image.roku.com/grid/mask.jpg")
    );
    assert_eq!(items[1].kind, Kind::Live);
    assert_eq!(
        items[1].watch_url,
        "https://therokuchannel.roku.com/watch/ffeeddccbbaa99887766554433221100"
    );
}

#[test]
fn samsung_channels() {
    let items = samsung::parse_channels(&fixture("samsung_channels.json"), "us");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].title, "ABC News Live", "sorted by channel number");
    assert_eq!(items[1].genres, vec!["Entertainment"]);
    assert!(samsung::parse_channels(&fixture("samsung_channels.json"), "zz").is_empty());
}
