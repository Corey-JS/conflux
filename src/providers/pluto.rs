//! Pluto TV: live channels plus the on-demand library, downloaded whole.
//!
//! Uses the endpoints Pluto's own web app uses: a boot call that hands out a
//! session token, then the channel guide and on-demand catalog with that
//! token as a Bearer header. If boot fails, falls back to Pluto's older
//! token-free channel list.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use super::json::{arr, at, str_of, strings, u32_of};
use super::{Provider, get_json};
use crate::model::{Item, Kind, Program, clean, is_web_url, iso_to_unix};

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
    fn has_guide(&self) -> bool {
        true
    }
    async fn guide(&self, http: &Client, channels: &[Item]) -> Result<HashMap<String, Vec<Program>>> {
        self.timelines(http, channels).await
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

impl Pluto {
    /// What's on now and the next few hours, for the given live channels.
    async fn timelines(&self, http: &Client, channels: &[Item]) -> Result<HashMap<String, Vec<Program>>> {
        let s = self.boot(http).await?;
        let auth = format!("Bearer {}", s.token);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let start = iso_from_unix(now - now % 60);
        let ids: Vec<&str> = channels
            .iter()
            .filter_map(|c| c.id.strip_prefix("pluto:"))
            .collect();

        // Modest chunks keep each response (and its parse) small on a 1 GB VM.
        let requests = ids.chunks(50).map(|chunk| {
            let list = chunk.join(",");
            let req = http
                .get(format!("{}/v2/guide/timelines", s.channels))
                .header("Authorization", &auth)
                .query(&[
                    ("start", start.as_str()),
                    ("channelIds", list.as_str()),
                    ("duration", "360"),
                ]);
            async move { get_json(req).await }
        });
        let mut out = HashMap::new();
        let mut last_err = None;
        for res in futures::future::join_all(requests).await {
            match res {
                Ok(v) => out.extend(parse_timelines(&v)),
                Err(e) => last_err = Some(e),
            }
        }
        if out.is_empty()
            && let Some(e) = last_err
        {
            return Err(e.context("Pluto timelines"));
        }
        Ok(out)
    }
}

/// `/v2/guide/timelines` → `{ data: [ { channelId, timelines: [ { start, stop, title, episode:{ name, description, rating, series:{name} } } ] } ] }`
/// Keyed by item id ("pluto:<channelId>").
pub fn parse_timelines(v: &Value) -> HashMap<String, Vec<Program>> {
    let mut out = HashMap::new();
    for ch in arr(v, &["data"]) {
        let Some(cid) = str_of(ch, &["channelId"]) else {
            continue;
        };
        let mut progs: Vec<Program> = Vec::new();
        for t in arr(ch, &["timelines"]) {
            let (Some(start), Some(stop)) = (
                str_of(t, &["start"]).and_then(|s| iso_to_unix(&s)),
                str_of(t, &["stop"]).and_then(|s| iso_to_unix(&s)),
            ) else {
                continue;
            };
            let ep = t.get("episode").unwrap_or(&Value::Null);
            let series = at(ep, &["series", "name"])
                .and_then(Value::as_str)
                .map(str::to_string);
            let title = clean(series.clone().or_else(|| str_of(t, &["title"])))
                .or_else(|| clean(str_of(ep, &["name"])));
            let Some(title) = title else { continue };
            let subtitle = clean(str_of(ep, &["name"])).filter(|n| *n != title);
            let description = clean(str_of(ep, &["description"]))
                .map(|d| d.chars().take(400).collect::<String>());
            progs.push(Program {
                start,
                stop,
                title,
                subtitle,
                description,
                rating: clean(str_of(ep, &["rating"])).filter(|r| r != "Not Rated"),
                image: ["poster", "thumbnail", "poster16_9"]
                    .iter()
                    .find_map(|k| at(ep, &[k, "path"]).and_then(Value::as_str))
                    .filter(|u| is_web_url(u))
                    .map(|u| resize_image(u, 208, 300)),
            });
        }
        progs.sort_by_key(|p| p.start);
        out.insert(format!("pluto:{cid}"), progs);
    }
    out
}

/// Pluto's image URLs take `w`/`h` query params; ask for phone-sized art
/// instead of the 700x1000 originals so a 450-channel guide stays light.
fn resize_image(url: &str, w: u32, h: u32) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let mut params: Vec<String> = query
        .split('&')
        .filter(|p| !p.starts_with("w=") && !p.starts_with("h="))
        .map(str::to_string)
        .collect();
    params.push(format!("h={h}"));
    params.push(format!("w={w}"));
    format!("{base}?{}", params.join("&"))
}

fn iso_from_unix(t: u64) -> String {
    let (days, rem) = (t / 86_400, t % 86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
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
