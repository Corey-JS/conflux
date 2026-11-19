//! Pluto TV: live channels plus the on-demand library, downloaded whole.
//!
//! Uses the endpoints Pluto's own web app uses: a boot call that hands out a
//! session token, then the channel guide and on-demand catalog with that
//! token as a Bearer header. If boot fails, falls back to Pluto's older
//! token-free channel list.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use super::json::{arr, at, str_of, strings, u32_of};
use super::{Provider, get_json};
use crate::model::{Item, Kind, clean, is_web_url};

const BOOT_URL: &str = "https://boot.pluto.tv/v4/start";
const DEFAULT_CHANNELS: &str = "https://service-channels.clusters.pluto.tv";
const DEFAULT_VOD: &str = "https://service-vod.clusters.pluto.tv";
const LEGACY_CHANNELS: &str = "https://api.pluto.tv/v2/channels";

pub struct Pluto {
    client_id: String,
}

impl Default for Pluto {
    fn default() -> Self {
        Self::new()
    }
}

impl Pluto {
    pub fn new() -> Pluto {
        Pluto {
            client_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    async fn boot(&self, http: &Client) -> Result<Session> {
        let v = get_json(http.get(BOOT_URL).query(&[
            ("appName", "web"),
            ("appVersion", "9.12.0"),
            ("deviceVersion", "17.5.0"),
            ("deviceModel", "web"),
            ("deviceMake", "safari"),
            ("deviceType", "web"),
            ("clientID", self.client_id.as_str()),
            ("clientModelNumber", "1.0.0"),
            ("serverSideAds", "false"),
        ]))
        .await?;
        let token = str_of(&v, &["sessionToken"]).context("Pluto boot gave no sessionToken")?;
        let server = |key: &str, default: &str| {
            at(&v, &["servers", key])
                .and_then(Value::as_str)
                .filter(|s| is_web_url(s))
                .unwrap_or(default)
                .trim_end_matches('/')
                .to_string()
        };
        Ok(Session {
            token,
            channels: server("channels", DEFAULT_CHANNELS),
            vod: server("vod", DEFAULT_VOD),
        })
    }

    async fn modern(&self, http: &Client) -> Result<Vec<Item>> {
        let s = self.boot(http).await?;
        let auth = format!("Bearer {}", s.token);

        let channels = get_json(
            http.get(format!("{}/v2/guide/channels", s.channels))
                .header("Authorization", &auth)
                .query(&[
                    ("channelIds", ""),
                    ("offset", "0"),
                    ("limit", "1000"),
                    ("lineupCondensed", "0"),
                ]),
        )
        .await
        .context("Pluto channel guide")?;

        // Category names are a nice-to-have; ignore failures.
        let categories = get_json(
            http.get(format!("{}/v2/guide/categories", s.channels))
                .header("Authorization", &auth),
        )
        .await
        .unwrap_or(Value::Null);

        let mut items = parse_channels(&channels, &categories);

        match get_json(
            http.get(format!("{}/v4/vod/categories", s.vod))
                .header("Authorization", &auth)
                .query(&[
                    ("includeItems", "true"),
                    ("offset", "1000"),
                    ("page", "1"),
                    ("sort", "number:asc"),
                ]),
        )
        .await
        {
            Ok(vod) => items.extend(parse_vod(&vod)),
            Err(e) => tracing::warn!("pluto: on-demand catalog failed, live only: {e:#}"),
        }
        Ok(items)
    }
}

struct Session {
    token: String,
    channels: String,
    vod: String,
}

#[async_trait]
impl Provider for Pluto {
    fn id(&self) -> &'static str {
        "pluto"
    }
    fn name(&self) -> &'static str {
        "Pluto TV"
    }
    fn has_catalog(&self) -> bool {
        true
    }
    async fn catalog(&self, http: &Client) -> Result<Vec<Item>> {
        let first = match self.modern(http).await {
            Ok(items) if !items.is_empty() => return Ok(items),
            Ok(_) => "web app API returned no channels".to_string(),
            Err(e) => format!("{e:#}"),
        };
        tracing::warn!("pluto: web-app API failed, trying legacy list: {first}");
        match get_json(http.get(LEGACY_CHANNELS)).await {
            Ok(legacy) => Ok(parse_legacy_channels(&legacy)),
            Err(e) => anyhow::bail!("web app API: {first}; legacy list: {e:#}"),
        }
    }
}

fn live_url(id: &str) -> String {
    format!("https://pluto.tv/live-tv/{id}")
}

/// `/v2/guide/channels` → `{ data: [ { id, name, slug, number, summary, images:[{type,url}], categoryID } ] }`
pub fn parse_channels(v: &Value, categories: &Value) -> Vec<Item> {
    let mut cat_names: HashMap<String, String> = HashMap::new();
    let mut chan_cat: HashMap<String, String> = HashMap::new();
    for c in arr(categories, &["data"]) {
        if let (Some(id), Some(name)) = (str_of(c, &["id", "_id"]), str_of(c, &["name"])) {
            for ch in arr(c, &["channelIDs"]).iter().filter_map(Value::as_str) {
                chan_cat.insert(ch.to_string(), name.clone());
            }
            cat_names.insert(id, name);
        }
    }

    let mut out = Vec::new();
    for c in arr(v, &["data"]) {
        let (Some(id), Some(name)) = (str_of(c, &["id", "_id"]), str_of(c, &["name"])) else {
            continue;
        };
        let mut item = Item::new("pluto", &id, Kind::Live, &name, live_url(&id));
        item.description = clean(str_of(c, &["summary", "description"]));
        item.channel_number = u32_of(c, &["number"]);
        item.image = pick_image(
            arr(c, &["images"]),
            &["colorLogoPNG", "logo", "colorLogoSVG"],
        );
        let cat = str_of(c, &["categoryID"])
            .and_then(|cid| cat_names.get(&cid).cloned())
            .or_else(|| chan_cat.get(&id).cloned())
            .or_else(|| str_of(c, &["category"]));
        item.genres = cat.into_iter().collect();
        out.push(item);
    }
    out
}

/// `api.pluto.tv/v2/channels` → `[ { _id, slug, name, number, summary, category, colorLogoPNG:{path} } ]`
pub fn parse_legacy_channels(v: &Value) -> Vec<Item> {
    let list = v.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut out = Vec::new();
    for c in list {
        let (Some(id), Some(name)) = (str_of(c, &["_id", "id"]), str_of(c, &["name"])) else {
            continue;
        };
        if c.get("isStitched").and_then(Value::as_bool) == Some(false) {
            continue; // not a playable channel on the web
        }
        let mut item = Item::new("pluto", &id, Kind::Live, &name, live_url(&id));
        item.description = clean(str_of(c, &["summary"]));
        item.channel_number = u32_of(c, &["number"]);
        item.image = ["colorLogoPNG", "logo", "thumbnail"]
            .iter()
            .find_map(|k| at(c, &[k, "path"]).and_then(Value::as_str))
            .filter(|s| is_web_url(s))
            .map(str::to_string);
        item.genres = str_of(c, &["category"]).into_iter().collect();
        out.push(item);
    }
    out
}

/// `/v4/vod/categories?includeItems=true` →
/// `{ categories: [ { name, items: [ { _id, slug, name, summary, type, genre, covers:[{aspectRatio,url}] } ] } ] }`
pub fn parse_vod(v: &Value) -> Vec<Item> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for cat in arr(v, &["categories"]) {
        let cat_name = str_of(cat, &["name"]);
        for it in arr(cat, &["items"]) {
            let (Some(id), Some(name)) = (str_of(it, &["_id", "id"]), str_of(it, &["name"])) else {
                continue;
            };
            if !seen.insert(id.clone()) {
                continue;
            }
            let kind = match str_of(it, &["type"]).as_deref() {
                Some("series") => Kind::Series,
                Some("movie") | None => Kind::Movie,
                Some(_) => continue,
            };
            let slug = str_of(it, &["slug"]).unwrap_or_else(|| id.clone());
            let path = if kind == Kind::Series {
                "series"
            } else {
                "movies"
            };
            let url = format!("https://pluto.tv/on-demand/{path}/{slug}");
            let mut item = Item::new("pluto", &id, kind, &name, url);
            item.description = clean(str_of(it, &["summary", "description"]));
            item.image = pick_cover(arr(it, &["covers"])).or_else(|| {
                ["poster", "featuredImage", "screenshot4_3"]
                    .iter()
                    .find_map(|k| at(it, &[k, "path"]).and_then(Value::as_str))
                    .filter(|s| is_web_url(s))
                    .map(str::to_string)
            });
            let mut genres = strings(it.get("genre"));
            if genres.is_empty() {
                genres.extend(cat_name.clone());
            }
            item.genres = genres;
            out.push(item);
        }
    }
    out
}

fn pick_image(images: &[Value], prefer: &[&str]) -> Option<String> {
    for want in prefer {
        if let Some(url) = images
            .iter()
            .find(|i| i.get("type").and_then(Value::as_str) == Some(want))
            .and_then(|i| str_of(i, &["url", "path"]))
            && is_web_url(&url)
        {
            return Some(url);
        }
    }
    images
        .iter()
        .find_map(|i| str_of(i, &["url", "path"]))
        .filter(|u| is_web_url(u))
}

fn pick_cover(covers: &[Value]) -> Option<String> {
    // Posters (portrait) first, then anything.
    covers
        .iter()
        .find(|c| c.get("aspectRatio").and_then(Value::as_str) == Some("347:500"))
        .or_else(|| covers.first())
        .and_then(|c| str_of(c, &["url", "path"]))
        .filter(|u| is_web_url(u))
}
