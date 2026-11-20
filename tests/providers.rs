//! Each provider's parser against a sample of its responses.

use conflux::model::Kind;
use conflux::providers::{plex, pluto, roku, samsung, tubi};
use serde_json::Value;

fn text_fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

fn fixture(name: &str) -> Value {
    serde_json::from_str(&text_fixture(name)).unwrap()
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
fn plex_channels_from_mirror_with_links_from_page() {
    let mirror = fixture("plex_mirror.json");
    let page = plex::parse_page(&text_fixture("plex_live_tv.html"));
    let items = plex::build(Some(&mirror), &page, "us");
    let mut titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
    titles.sort();
    assert_eq!(
        titles,
        vec!["A&E Alaska State Troopers", "CW Forever", "WSOC Channel 9"],
        "other regions left out; page-only channels aren't added when the mirror works"
    );
    let by = |t: &str| items.iter().find(|i| i.title == t).unwrap();

    let cw = by("CW Forever");
    assert_eq!(cw.id, "plex:live:6a1610bebdf296985fd95603-66635fc0ca7b22f4c3a2d743");
    assert_eq!(cw.kind, Kind::Live);
    assert_eq!(cw.watch_url, "https://watch.plex.tv/live-tv/channel/cw-forever");
    assert_eq!(cw.genres, vec!["Bingeworthy"], "a later non-genre row doesn't clear it");
    assert!(cw.image.as_deref().unwrap().ends_with("PLEX_CWFOREVER_UPDATEDLOGO_RDX.png"));

    let ae = by("A&E Alaska State Troopers");
    assert_eq!(
        ae.watch_url,
        "https://watch.plex.tv/live-tv/channel/aande-alaska-state-troopers",
        "the page's slug, not one guessed from the name"
    );
    assert_eq!(ae.genres, vec!["Food, Home & Culture"]);

    let local = by("WSOC Channel 9");
    assert_eq!(
        local.watch_url, "https://watch.plex.tv/live-tv",
        "not on the page: link to the guide rather than guess"
    );
    assert!(local.genres.is_empty());
}

#[test]
fn plex_page_alone_when_mirror_is_down() {
    let page = plex::parse_page(&text_fixture("plex_live_tv.html"));
    assert_eq!(page.len(), 3, "bad ids skipped, repeats merged: {page:?}");
    let items = plex::build(None, &page, "us");
    let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, vec!["CW Forever", "A&E Alaska State Troopers", "No Reservations"]);
    assert_eq!(items[2].watch_url, "https://watch.plex.tv/live-tv/channel/no-reservations");
    assert_eq!(items[2].genres, vec!["Food, Home & Culture"]);
    assert!(items[2].image.as_deref().unwrap().starts_with("https://provider-static.plex.tv/"));
}

#[test]
fn plex_schedules_from_mirror() {
    let mirror = fixture("plex_mirror.json");
    let map = plex::parse_programs(&mirror, "us");
    let cw = &map["plex:live:6a1610bebdf296985fd95603-66635fc0ca7b22f4c3a2d743"];
    let slots: Vec<(u64, u64)> = cw.iter().map(|p| (p.start, p.stop)).collect();
    assert_eq!(
        slots,
        vec![
            (1795165969, 1795169033),
            (1795169033, 1795172116),
            (1795172116, 1795175205)
        ],
        "sorted, repeats merged, each ends when the next starts, the open-ended last one dropped"
    );
    assert_eq!(cw[0].title, "Lost Girl");

    let ae = &map["plex:live:6a1610bebdf296985fd95603-6a024c8e981d28c38fb16f21"];
    assert_eq!(ae.len(), 1, "untitled slot skipped");
    assert_eq!((ae[0].start, ae[0].stop), (1795164089, 1795167461));

    assert!(
        !map.contains_key("plex:live:6a1610bebdf296985fd95603-61f9a6aec68d91e31d4cc86e"),
        "one slot has no end, so no schedule"
    );
    assert_eq!(map.len(), 2, "other regions left out");
    assert_eq!(plex::parse_programs(&mirror, "gb").len(), 1);
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
    let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["The Mask", "The Masked Singer"],
        "paid-only (Paramount+, Roku Premium Subscriptions), trailers, \
         no-view-option, duplicate and bad-id entries are all dropped"
    );
    assert_eq!(
        items[0].watch_url,
        "https://app.roku.com/details/dfaed5480b2f5cc7a97d0eb514e790ce"
    );
    assert_eq!(items[0].year, Some(1994));
    assert_eq!(
        items[0].image.as_deref(),
        Some("https://images.sr.roku.com/grid/mask.jpg")
    );
    assert_eq!(
        items[0].description.as_deref(),
        Some("Bank clerk Stanley Ipkiss is transformed into a manic superhero when he wears a mysterious mask."),
        "longest description wins"
    );
    assert_eq!(items[0].genres, vec!["Comedy", "Action"]);
    assert_eq!(items[1].kind, Kind::Live);
    assert_eq!(
        items[1].watch_url,
        "https://app.roku.com/watch/34b686f999ecaacfbcd91bb5195c3124"
    );
    assert_eq!(items[1].description, None, "empty descriptions map");
}

#[test]
fn roku_live_channels_from_mirror() {
    let mirror = fixture("roku_mirror.json");
    let items = roku::parse_channels(&mirror);
    let mut titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
    titles.sort();
    assert_eq!(
        titles,
        vec!["LiveNOW from FOX", "Real America's Voice en Español", "The Masked Singer"],
        "the bad id is dropped"
    );
    let fox = items.iter().find(|i| i.title == "LiveNOW from FOX").unwrap();
    assert_eq!(fox.id, "roku:f9c0d48b540a55dcb98b04d2cc8abf25");
    assert_eq!(fox.kind, Kind::Live);
    assert_eq!(
        fox.watch_url,
        "https://app.roku.com/watch/f9c0d48b540a55dcb98b04d2cc8abf25",
        "same link as the channel's search result"
    );
    assert_eq!(fox.channel_number, Some(103));
    assert_eq!(fox.genres, vec!["News"]);
    assert!(fox.description.as_deref().unwrap().starts_with("Always LIVE."));
    assert!(fox.image.as_deref().unwrap().starts_with("https://images.sr.roku.com/"));

    let map = roku::parse_programs(&mirror);
    let slots: Vec<(u64, u64)> = map["roku:f9c0d48b540a55dcb98b04d2cc8abf25"]
        .iter()
        .map(|p| (p.start, p.stop))
        .collect();
    assert_eq!(
        slots,
        vec![(1795154400, 1795168800), (1795168800, 1795183200), (1795183200, 1795197600)]
    );
    assert_eq!(map.len(), 2, "no schedule for a channel without programs");
}

#[test]
fn samsung_channels() {
    let items = samsung::parse_channels(&fixture("samsung_channels.json"), "us");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].title, "ABC News Live", "sorted by channel number");
    assert_eq!(items[1].genres, vec!["Entertainment"]);
    assert!(samsung::parse_channels(&fixture("samsung_channels.json"), "zz").is_empty());
}

#[test]
fn pluto_timelines_become_sorted_programs() {
    let map = pluto::parse_timelines(&fixture("pluto_timelines.json"));
    let ps = &map["pluto:abc123"];
    assert_eq!(ps.len(), 2, "the entry with a bad time is skipped");
    assert_eq!(ps[0].title, "Paddington");
    assert_eq!(ps[0].subtitle.as_deref(), Some("A Visit"));
    assert_eq!(ps[0].rating.as_deref(), Some("TV-Y"));
    assert_eq!(ps[0].stop, ps[1].start, "sorted by start time");
    assert_eq!(ps[1].title, "Some Movie");
    assert_eq!(ps[1].subtitle, None, "movie name isn't repeated as a subtitle");
    assert_eq!(ps[1].description.as_deref(), Some("A movie."));
    assert_eq!(
        ps[0].image.as_deref(),
        Some("https://images.pluto.tv/e/poster.jpg?fill=blur&fm=jpg&h=300&w=208"),
        "art is requested at phone size"
    );
    assert_eq!(ps[1].image, None);
}
