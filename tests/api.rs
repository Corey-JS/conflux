//! The HTTP API end to end, with fake providers standing in for real ones:
//! one with a catalog, one searched live, one that always fails.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use conflux::catalog::Store;
use conflux::model::{Item, Kind};
use conflux::providers::Provider;
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

async fn start() -> String {
    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(CatalogFake),
        Arc::new(SearchFake),
        Arc::new(Broken),
    ];
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
