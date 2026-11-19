//! Plex: free live channels (downloaded whole) and free on-demand titles
//! (searched per query).
//!
//! Plex's provider APIs need an X-Plex-Token even for its free content. The
//! Plex web app gets one for signed-out visitors from the anonymous-user
//! endpoint, so we do the same. That is a guest token, not a Plex account.

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::Value;
use tokio::sync::Mutex;

use super::Provider;
use super::json::{arr, str_of, strings, year_of};
use crate::model::{Item, Kind, clean, is_web_url, slugify};

const ANON_URL: &str = "https://plex.tv/api/v2/users/anonymous";
const LIVE_URL: &str = "https://epg.provider.plex.tv/lineups/plex/channels";
const VOD_SEARCH_URL: &str = "https://vod.provider.plex.tv/hubs/search";
const DISCOVER_SEARCH_URL: &str = "https://discover.provider.plex.tv/library/search";

pub struct Plex {
    client_id: String,
    token: Mutex<Option<String>>,
}

impl Default for Plex {
    fn default() -> Self {
        Self::new()
    }
}

impl Plex {
    pub fn new() -> Plex {
        Plex {
            client_id: uuid::Uuid::new_v4().to_string(),
            token: Mutex::new(None),
        }
    }

    fn headers(&self, req: RequestBuilder) -> RequestBuilder {
        req.header("Accept", "application/json")
            .header("X-Plex-Product", "Conflux")
            .header("X-Plex-Version", env!("CARGO_PKG_VERSION"))
            .header("X-Plex-Client-Identifier", &self.client_id)
            .header("X-Plex-Platform", "Safari")
            .header("X-Plex-Device", "Web")
            .header("X-Plex-Language", "en")
    }

    async fn token(&self, http: &Client, fresh: bool) -> Result<String> {
        let mut guard = self.token.lock().await;
        if !fresh && let Some(t) = guard.as_ref() {
            return Ok(t.clone());
        }
        let resp = self.headers(http.post(ANON_URL)).send().await?;
        let status = resp.status();
        let v: Value = resp.json().await.context("Plex guest token response")?;
        let t = str_of(&v, &["authToken", "authenticationToken"])
            .with_context(|| format!("Plex guest token missing ({status})"))?;
        *guard = Some(t.clone());
        Ok(t)
    }

    /// GET with the guest token, renewing it once if Plex says it expired.
    async fn get(&self, http: &Client, url: &str, query: &[(&str, &str)]) -> Result<Value> {
        for attempt in 0..2 {
            let token = self.token(http, attempt > 0).await?;
            let resp = self
                .headers(http.get(url))
                .header("X-Plex-Token", token)
                .query(query)
                .send()
                .await?;
            if resp.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                continue;
            }
            let status = resp.status();
            if !status.is_success() {
                anyhow::bail!("{status} from {url}");
            }
            return Ok(resp.json().await?);
        }
        anyhow::bail!("Plex kept refusing the guest token")
    }
}

#[async_trait]
impl Provider for Plex {
    fn id(&self) -> &'static str {
        "plex"
    }
    fn name(&self) -> &'static str {
        "Plex"
    }
    fn has_catalog(&self) -> bool {
        true
    }
    async fn catalog(&self, http: &Client) -> Result<Vec<Item>> {
        let v = self
            .get(http, LIVE_URL, &[])
            .await
            .context("Plex live channels")?;
        Ok(parse_live(&v))
    }
    fn has_search(&self) -> bool {
        true
    }
    async fn search(&self, http: &Client, query: &str) -> Result<Vec<Item>> {
        match self
            .get(
                http,
                VOD_SEARCH_URL,
                &[("query", query), ("limit", "20"), ("includeMetadata", "1")],
            )
            .await
        {
            Ok(v) => Ok(parse_vod_hubs(&v)),
            Err(e) => {
                tracing::debug!("plex: vod hub search failed, trying discover: {e:#}");
                let v = self
                    .get(
                        http,
                        DISCOVER_SEARCH_URL,
                        &[
                            ("query", query),
                            ("limit", "20"),
                            ("searchTypes", "movies,tv"),
                            ("searchProviders", "PLEXAVOD"),
                            ("includeMetadata", "1"),
                        ],
                    )
                    .await?;
                Ok(parse_discover(&v))
            }
        }
    }
}

/// `{ MediaContainer: { Channel: [ { id, title, slug, thumb, callSign, genre } ] } }`
pub fn parse_live(v: &Value) -> Vec<Item> {
    let mut out = Vec::new();
    for c in arr(v, &["MediaContainer", "Channel"]) {
        let Some(title) = str_of(c, &["title", "callSign"]) else {
            continue;
        };
        let slug = str_of(c, &["slug"]).unwrap_or_else(|| slugify(&title));
        let id = str_of(c, &["id", "gridKey"]).unwrap_or_else(|| slug.clone());
        let url = format!("https://watch.plex.tv/live-tv/channel/{slug}");
        let mut item = Item::new("plex", &format!("live:{id}"), Kind::Live, &title, url);
        item.description = clean(str_of(c, &["summary", "description"]));
        item.image = str_of(c, &["thumb", "art"]).filter(|u| is_web_url(u));
        let mut genres = strings(c.get("genre"));
        if genres.is_empty() {
            genres = strings(c.get("Genre"));
        }
        item.genres = genres;
        out.push(item);
    }
    out
}

/// `{ MediaContainer: { Hub: [ { Metadata: [ ... ] } ] } }`
pub fn parse_vod_hubs(v: &Value) -> Vec<Item> {
    let mut out = Vec::new();
    for hub in arr(v, &["MediaContainer", "Hub"]) {
        out.extend(arr(hub, &["Metadata"]).iter().filter_map(metadata_item));
    }
    out
}

/// `{ MediaContainer: { SearchResults: [ { SearchResult: [ { Metadata: {...} } ] } ] } }`
pub fn parse_discover(v: &Value) -> Vec<Item> {
    let mut out = Vec::new();
    for group in arr(v, &["MediaContainer", "SearchResults"]) {
        for r in arr(group, &["SearchResult"]) {
            if let Some(item) = r.get("Metadata").and_then(metadata_item) {
                out.push(item);
            }
        }
    }
    out
}

fn metadata_item(m: &Value) -> Option<Item> {
    let title = str_of(m, &["title"])?;
    let (kind, path) = match str_of(m, &["type"]).as_deref() {
        Some("movie") => (Kind::Movie, "movie"),
        Some("show") => (Kind::Series, "show"),
        _ => return None, // episodes, people, collections
    };
    let slug = str_of(m, &["slug"]).unwrap_or_else(|| slugify(&title));
    let id = str_of(m, &["ratingKey", "guid", "key"]).unwrap_or_else(|| slug.clone());
    let url = format!("https://watch.plex.tv/{path}/{slug}");
    let mut item = Item::new("plex", &id, kind, &title, url);
    item.description = clean(str_of(m, &["summary", "tagline"]));
    item.year = year_of(m, &["year"]);
    item.image = str_of(m, &["thumb"]).filter(|u| is_web_url(u)).or_else(|| {
        // Newer responses: Image: [ { type: "coverPoster", url } ]
        arr(m, &["Image"])
            .iter()
            .filter_map(|i| str_of(i, &["url"]))
            .find(|u| is_web_url(u))
    });
    item.genres = strings(m.get("Genre"));
    Some(item)
}
