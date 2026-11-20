//! The HTTP API end to end, with fake providers standing in for real ones:
//! one with a catalog, one searched live, one that always fails.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use conflux::catalog::Store;
use conflux::model::{Item, Kind, Program};
use conflux::providers::{Provider, Throttled};
use reqwest::Client;
use serde_json::Value;

struct CatalogFake;
#[async_trait]
impl Provider for CatalogFake {
    fn id(&self) -> &'static str {
        "pluto"
    }
    fn name(&self) -> &'static str {
        "Pluto TV"
    }
    fn has_catalog(&self) -> bool {
        true
    }
    async fn catalog(&self, _: &Client) -> Result<Vec<Item>> {
        let mut a = Item::new(
            "pluto",
            "1",
            Kind::Live,
            "Star Trek Channel",
            "https://pluto.tv/live-tv/1".into(),
        );
        a.channel_number = Some(5);
        let mut b = Item::new(
            "pluto",
            "2",
            Kind::Live,
            "News Now",
            "https://pluto.tv/live-tv/2".into(),
        );
        b.channel_number = Some(1);
        let c = Item::new(
            "pluto",
            "3",
            Kind::Movie,
            "Star Trek II",
            "https://pluto.tv/on-demand/movies/x".into(),
        );
        Ok(vec![a, b, c])
    }
}

struct SearchFake;
#[async_trait]
impl Provider for SearchFake {
    fn id(&self) -> &'static str {
        "tubi"
    }
    fn name(&self) -> &'static str {
        "Tubi"
    }
    fn has_search(&self) -> bool {
        true
    }
    async fn search(&self, _: &Client, q: &str) -> Result<Vec<Item>> {
        Ok(vec![
            Item::new(
                "tubi",
                "9",
                Kind::Movie,
                &format!("{q} (Tubi)"),
                "https://tubitv.com/movies/9".into(),
            ),
            // Matched on something we can't see, e.g. cast. Still kept.
            Item::new(
                "tubi",
                "10",
                Kind::Series,
                "Unrelated Title",
                "https://tubitv.com/series/10".into(),
            ),
        ])
    }
}

struct Broken;
#[async_trait]
impl Provider for Broken {
    fn id(&self) -> &'static str {
        "roku"
    }
    fn name(&self) -> &'static str {
        "The Roku Channel"
    }
    fn has_catalog(&self) -> bool {
        true
    }
    async fn catalog(&self, _: &Client) -> Result<Vec<Item>> {
        anyhow::bail!("403 Forbidden")
    }
    fn has_search(&self) -> bool {
        true
    }
    async fn search(&self, _: &Client, _: &str) -> Result<Vec<Item>> {
        anyhow::bail!("403 Forbidden")
    }
}

/// A provider with `n` live channels, numbered from 1.
struct ManyLive(&'static str, &'static str, usize);
#[async_trait]
impl Provider for ManyLive {
    fn id(&self) -> &'static str {
        self.0
    }
    fn name(&self) -> &'static str {
        self.1
    }
    fn has_catalog(&self) -> bool {
        true
    }
    async fn catalog(&self, _: &Client) -> Result<Vec<Item>> {
        Ok((0..self.2)
            .map(|n| {
                let mut it = Item::new(
                    self.0,
                    &n.to_string(),
                    Kind::Live,
                    &format!("{} {n}", self.1),
                    format!("https://example.com/{n}"),
                );
                it.channel_number = Some(n as u32 + 1);
                it
            })
            .collect())
    }
}

/// Catalog and search both return the same live channel, like Roku, under
/// slightly different names.
struct Both;
#[async_trait]
impl Provider for Both {
    fn id(&self) -> &'static str {
        "roku"
    }
    fn name(&self) -> &'static str {
        "The Roku Channel"
    }
    fn has_catalog(&self) -> bool {
        true
    }
    async fn catalog(&self, _: &Client) -> Result<Vec<Item>> {
        let mut it = Item::new(
            "roku",
            "abc",
            Kind::Live,
            "News 24 Live",
            "https://app.roku.com/watch/abc".into(),
        );
        it.channel_number = Some(103);
        Ok(vec![it])
    }
    fn has_search(&self) -> bool {
        true
    }
    async fn search(&self, _: &Client, _: &str) -> Result<Vec<Item>> {
        Ok(vec![Item::new(
            "roku",
            "abc",
            Kind::Live,
            "News 24",
            "https://app.roku.com/watch/abc".into(),
        )])
    }
}

/// Show art at full size, like Plex: the guide should link shrunken copies.
struct BigArt;
#[async_trait]
impl Provider for BigArt {
    fn id(&self) -> &'static str {
        "plex"
    }
    fn name(&self) -> &'static str {
        "Plex"
    }
    fn has_catalog(&self) -> bool {
        true
    }
    async fn catalog(&self, _: &Client) -> Result<Vec<Item>> {
        Ok(vec![Item::new("plex", "live:1", Kind::Live, "CW Forever", "https://watch.plex.tv/live-tv".into())])
    }
    fn has_guide(&self) -> bool {
        true
    }
    async fn guide(&self, _: &Client, _: &[Item]) -> Result<HashMap<String, Vec<Program>>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let slot = |start: u64, image: &str| Program {
            start,
            stop: start + 1800,
            title: "Lost Girl".into(),
            subtitle: None,
            description: None,
            rating: None,
            image: Some(image.into()),
        };
        Ok(HashMap::from([(
            "plex:live:1".to_string(),
            vec![
                // .invalid never resolves, so fetching it fails without a network.
                slot(now - 600, "https://art.invalid/lost-girl-2000x3000.jpg"),
                slot(now + 1200, "http://169.254.169.254/latest/meta-data"),
            ],
        )]))
    }
    fn shrink_art(&self) -> bool {
        true
    }
}

async fn start() -> String {
    start_with(vec![
        Arc::new(CatalogFake),
        Arc::new(SearchFake),
        Arc::new(Broken),
    ])
    .await
}

async fn start_with(providers: Vec<Arc<dyn Provider>>) -> String {
    let store = Store::new(providers, Client::new());
    store.refresh_all().await;
    let web = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/web"));
    let app = conflux::api::router(store, web);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

async fn get(base: &str, path: &str) -> (u16, Value) {
    let c = Client::builder().no_proxy().build().unwrap();
    let r = c.get(format!("{base}{path}")).send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

fn titles(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn search_merges_catalog_and_live_results() {
    let base = start().await;
    let (s, v) = get(&base, "/api/search?q=Star%20Trek").await;
    assert_eq!(s, 200);
    assert_eq!(
        titles(&v),
        vec![
            "Star Trek II",
            "star trek (Tubi)",
            "Star Trek Channel",
            "Unrelated Title"
        ],
        "{v}"
    );
    assert_eq!(v["unavailable"], serde_json::json!(["The Roku Channel"]));
    assert!(
        v["items"][0]["watch_url"]
            .as_str()
            .unwrap()
            .starts_with("https://")
    );
}

#[tokio::test]
async fn search_filters() {
    let base = start().await;
    let (_, v) = get(&base, "/api/search?q=star&kind=live").await;
    assert_eq!(titles(&v), vec!["Star Trek Channel"]);
    let (_, v) = get(&base, "/api/search?q=star&provider=tubi").await;
    assert_eq!(titles(&v), vec!["star (Tubi)", "Unrelated Title"]);
    assert_eq!(get(&base, "/api/search?q=star&provider=nope").await.0, 400);
    assert_eq!(get(&base, "/api/search?q=star&kind=nope").await.0, 400);
    let long = "a".repeat(101);
    assert_eq!(get(&base, &format!("/api/search?q={long}")).await.0, 400);
    let (_, v) = get(&base, "/api/search?q=").await;
    assert!(titles(&v).is_empty());
}

#[tokio::test]
async fn live_lists_channels_in_number_order() {
    let base = start().await;
    let (_, v) = get(&base, "/api/live").await;
    assert_eq!(titles(&v), vec!["News Now", "Star Trek Channel"]);
}

#[tokio::test]
async fn live_guide_includes_every_provider() {
    // Pluto has 450+ channels and is listed first; Plex must still show up.
    let base = start_with(vec![
        Arc::new(ManyLive("pluto", "Pluto TV", 450)),
        Arc::new(ManyLive("plex", "Plex", 3)),
    ])
    .await;
    let (_, v) = get(&base, "/api/live").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 453, "whole lineups by default");

    let (_, v) = get(&base, "/api/live?limit=400").await;
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 400);
    assert_eq!(
        items.iter().filter(|i| i["provider"] == "plex").count(),
        3,
        "a cap still leaves room for providers listed later"
    );
}

#[tokio::test]
async fn a_channel_found_both_ways_shows_once_with_its_number() {
    let base = start_with(vec![Arc::new(Both)]).await;
    // The search copy's title is an exact match, so it would outrank the
    // downloaded copy if both were kept.
    let (_, v) = get(&base, "/api/search?q=news%2024").await;
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["title"], "News 24 Live", "the downloaded copy wins");
    assert_eq!(items[0]["channel_number"], 103);
}

#[tokio::test]
async fn full_size_art_is_served_shrunk_and_only_from_the_guide() {
    let base = start_with(vec![Arc::new(BigArt)]).await;
    let (_, v) = get(&base, "/api/live").await;
    let programs = v["items"][0]["programs"].as_array().unwrap();
    let art = programs[0]["image"].as_str().unwrap();
    assert!(art.starts_with("/api/art/") && art.len() == "/api/art/".len() + 16, "{art}");
    assert!(programs[1].get("image").is_none(), "not https from a named host: dropped");

    let c = Client::builder().no_proxy().build().unwrap();
    let r = c.get(format!("{base}{art}")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 404, "the original can't be fetched");
    assert_eq!(r.headers()["cache-control"], "no-store");
    for path in ["/api/art/0123456789abcdef", "/api/art/..%2F..%2Fetc", "/api/art/https%3A%2F%2Fexample.com"] {
        let (status, _) = get(&base, path).await;
        assert_eq!(status, 404, "{path}: only keys from the current guide");
    }
}

#[tokio::test]
async fn provider_status_reports_errors() {
    let base = start().await;
    let _ = get(&base, "/api/search?q=star").await;
    let (_, v) = get(&base, "/api/providers").await;
    let ps = v["providers"].as_array().unwrap();
    assert_eq!(ps.len(), 3);
    assert_eq!(ps[0]["catalog_items"], 3);
    assert_eq!(ps[1]["mode"], "search");
    assert!(ps[1]["search_ok_at"].is_u64());
    assert!(ps[2]["catalog_error"].as_str().unwrap().contains("403"));
    assert!(ps[2]["search_error"].as_str().unwrap().contains("403"));
}

#[tokio::test]
async fn serves_the_app() {
    let base = start().await;
    let c = Client::builder().no_proxy().build().unwrap();
    let r = c.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("Conflux"));
    let r = c
        .get(format!("{base}/manifest.webmanifest"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

// ---- Rate limits ----

/// Searched live; counts the searches that actually reach it.
struct Counted(Arc<AtomicUsize>);
#[async_trait]
impl Provider for Counted {
    fn id(&self) -> &'static str {
        "roku"
    }
    fn name(&self) -> &'static str {
        "The Roku Channel"
    }
    fn has_search(&self) -> bool {
        true
    }
    async fn search(&self, _: &Client, q: &str) -> Result<Vec<Item>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![Item::new("roku", q, Kind::Movie, q, "https://therokuchannel.roku.com/x".into())])
    }
}

/// Searched live; always says "too many requests".
struct Throttling(Arc<AtomicUsize>);
#[async_trait]
impl Provider for Throttling {
    fn id(&self) -> &'static str {
        "roku"
    }
    fn name(&self) -> &'static str {
        "The Roku Channel"
    }
    fn has_search(&self) -> bool {
        true
    }
    async fn search(&self, _: &Client, _: &str) -> Result<Vec<Item>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(Throttled { host: "roku.test".into(), wait: Duration::from_secs(600) }.into())
    }
}

async fn search_as(base: &str, q: &str, ip: &str) -> (u16, Value) {
    let c = Client::builder().no_proxy().build().unwrap();
    let r = c
        .get(format!("{base}/api/search"))
        .query(&[("q", q)])
        .header("X-Forwarded-For", ip)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn live_searches_to_a_provider_are_capped() {
    let calls = Arc::new(AtomicUsize::new(0));
    let base = start_with(vec![Arc::new(Counted(calls.clone()))]).await;
    let mut skipped = 0;
    // Different visitors, so only the provider's own cap applies.
    for i in 0..20 {
        let (s, v) = search_as(&base, &format!("query {i}"), &format!("198.51.100.{i}")).await;
        assert_eq!(s, 200);
        if v["unavailable"].as_array().unwrap().iter().any(|n| n == "The Roku Channel") {
            skipped += 1;
        }
    }
    let sent = calls.load(Ordering::SeqCst);
    assert!(sent <= 7, "a burst of 5 then about one a second, got {sent}");
    assert_eq!(sent + skipped, 20, "skipped searches are reported as unavailable");
    // A repeat is answered from the cache and costs nothing.
    let (_, v) = search_as(&base, "query 0", "198.51.100.99").await;
    assert_eq!(titles(&v), vec!["query 0"]);
    assert_eq!(calls.load(Ordering::SeqCst), sent);
}

#[tokio::test]
async fn a_provider_that_says_slow_down_is_left_alone() {
    let calls = Arc::new(AtomicUsize::new(0));
    let base = start_with(vec![Arc::new(CatalogFake), Arc::new(Throttling(calls.clone()))]).await;
    for q in ["star", "trek", "news"] {
        let (s, v) = search_as(&base, q, "198.51.100.1").await;
        assert_eq!(s, 200);
        assert_eq!(v["unavailable"][0], "The Roku Channel");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no more requests while paused");
    let (_, v) = get(&base, "/api/providers").await;
    let roku = &v["providers"][1];
    assert!(roku["paused_until"].as_u64().unwrap() > 0);
    assert!(roku["search_error"].as_str().unwrap().contains("slow down"));
}

#[tokio::test]
async fn one_visitor_cannot_flood_search() {
    let base = start().await;
    let mut limited = None;
    for i in 0..40 {
        let (s, v) = search_as(&base, "star", "203.0.113.7").await;
        if s == 429 {
            assert!(v["retry_after"].as_u64().unwrap() >= 1);
            limited = Some(i);
            break;
        }
    }
    let at = limited.expect("a flood is cut off");
    assert!((30..=32).contains(&at), "a burst of 30 is allowed, cut off at {at}");
    // Someone else is unaffected.
    assert_eq!(search_as(&base, "star", "203.0.113.8").await.0, 200);
}

#[tokio::test]
async fn too_many_requests_is_recognized_with_its_wait() {
    use axum::http::{StatusCode, header};
    use conflux::providers::{get_json, throttled};
    let app = axum::Router::new()
        .route("/slow", axum::routing::get(|| async { (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "120")], "slow down") }))
        .route("/bare", axum::routing::get(|| async { StatusCode::TOO_MANY_REQUESTS }))
        .route("/gone", axum::routing::get(|| async { StatusCode::NOT_FOUND }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let c = Client::builder().no_proxy().build().unwrap();

    let e = get_json(c.get(format!("http://{addr}/slow"))).await.unwrap_err();
    assert_eq!(throttled(&e), Some(Duration::from_secs(120)));
    let e = get_json(c.get(format!("http://{addr}/bare"))).await.unwrap_err();
    assert_eq!(throttled(&e), Some(Duration::from_secs(15 * 60)), "no Retry-After: the default pause");
    // Through error_for_status, as the mirror and art downloads use.
    let e: anyhow::Error = c.get(format!("http://{addr}/bare")).send().await.unwrap().error_for_status().unwrap_err().into();
    assert!(throttled(&e).is_some());
    let e = get_json(c.get(format!("http://{addr}/gone"))).await.unwrap_err();
    assert_eq!(throttled(&e), None, "other errors aren't a slow-down");
}
