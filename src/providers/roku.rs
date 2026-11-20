//! The Roku Channel: live channels downloaded whole, everything searched per
//! query.
//!
//! Search only finds live channels that match a query, so the full lineup
//! and its schedules come from the channel list the open-source i.mjh.nz
//! project mirrors from The Roku Channel (see `mjh`). Its ids are Roku's own,
//! so a channel found both ways links to the same page and shows up once.
//!
//! The Roku Channel's web app gets a CSRF token, then searches with it. Its
//! response shape is not documented, so the parser walks the JSON and picks
//! out anything that looks like a titled movie, series or channel.
//!
//! Roku's search covers every channel on Roku, paid ones included, so each
//! result is kept only if The Roku Channel itself offers it free.
//!
//! The channel list's schedules have titles only. Show art, episode names
//! and descriptions come from the mirror's Roku guide file (XMLTV, same ids;
//! see `showart`). That art is full size, so Conflux serves shrunken copies.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::json::{arr, at, str_of, strings, u32_of, year_of};
use super::showart::{self, ShowArt};
use super::{Provider, get_json, mjh};
use crate::model::{Item, Kind, Program, clean, is_web_url};

/// The web app moved here from therokuchannel.roku.com, which now answers
/// with a 301. A redirected POST arrives as a GET with no body, so the old
/// host can't be used for search.
const BASE: &str = "https://app.roku.com";

/// The view option that means "free with ads on The Roku Channel". Others
/// are paid: "rokusvod" (Roku's Premium Subscriptions), "paramountplus",
/// "shudder" and so on.
const FREE_PROVIDER: &str = "rokuavod";

const MIRROR_URL: &str = "https://i.mjh.nz/Roku/.channels.json.gz";
const ART_URL: &str = "https://i.mjh.nz/Roku/all.xml.gz";
/// The file is about 3.5 MB and covers about three days ahead.
const ART_EVERY: Duration = Duration::from_secs(3 * 3600);

pub struct Roku {
    csrf: Mutex<Option<String>>,
    schedules: mjh::Schedules,
    art: showart::Feed,
}

impl Default for Roku {
    fn default() -> Self {
        Self::new()
    }
}

impl Roku {
    pub fn new() -> Roku {
        Roku {
            csrf: Mutex::new(None),
            schedules: mjh::Schedules::default(),
            art: showart::Feed::new("roku", "i.mjh.nz", ART_URL, ART_EVERY, showart::by_id),
        }
    }

    async fn programs(&self, http: &Client) -> Result<HashMap<String, Vec<Program>>> {
        if let Some(map) = self.schedules.recent().await {
            return Ok(map);
        }
        let v = mjh::fetch(http, MIRROR_URL)
            .await
            .context("Roku schedules (i.mjh.nz)")?;
        let map = parse_programs(&v);
        self.schedules.set(map.clone()).await;
        Ok(map)
    }

    async fn csrf(&self, http: &Client, fresh: bool) -> Result<String> {
        let mut g = self.csrf.lock().await;
        if !fresh && let Some(t) = g.as_ref() {
            return Ok(t.clone());
        }
        let v = get_json(
            http.get(format!("{BASE}/api/v1/csrf"))
                .header("Referer", format!("{BASE}/")),
        )
        .await
        .context("Roku CSRF token")?;
        let t = str_of(&v, &["csrf", "csrfToken", "token"]).context("Roku CSRF token missing")?;
        *g = Some(t.clone());
        Ok(t)
    }
}

#[async_trait]
impl Provider for Roku {
    fn id(&self) -> &'static str {
        "roku"
    }
    fn name(&self) -> &'static str {
        "The Roku Channel"
    }
    fn has_catalog(&self) -> bool {
        true
    }
    async fn catalog(&self, http: &Client) -> Result<Vec<Item>> {
        let v = mjh::fetch(http, MIRROR_URL)
            .await
            .context("Roku channel list (i.mjh.nz)")?;
        self.schedules.set(parse_programs(&v)).await;
        Ok(parse_channels(&v))
    }
    fn has_guide(&self) -> bool {
        true
    }
    async fn guide(&self, http: &Client, channels: &[Item]) -> Result<HashMap<String, Vec<Program>>> {
        let (map, art) = tokio::join!(self.programs(http), self.art.get(http));
        let mut map = map?;
        if let Some(art) = art {
            let n = add_art(&mut map, channels, &art);
            tracing::info!("roku: show art for {n} programs");
        }
        Ok(map)
    }
    fn shrink_art(&self) -> bool {
        true
    }
    fn has_search(&self) -> bool {
        true
    }
    async fn search(&self, http: &Client, query: &str) -> Result<Vec<Item>> {
        let mut last_err = None;
        for attempt in 0..2 {
            let token = self.csrf(http, attempt > 0).await?;
            let res = get_json(
                http.post(format!("{BASE}/api/v1/search"))
                    .header("csrf-token", &token)
                    .header("Origin", BASE)
                    .header("Referer", format!("{BASE}/"))
                    .json(&json!({ "query": query })),
            )
            .await;
            match res {
                Ok(v) => return Ok(parse_search(&v)),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap()).context("Roku search")
    }
}

/// The mirror's live lineup:
/// `{ channels: { "<id>": { name, description, chno, logo, groups, programs } } }`.
pub fn parse_channels(v: &Value) -> Vec<Item> {
    mjh::channels(v, "us")
        .filter(|(id, _)| valid_id(id))
        .filter_map(|(id, c)| {
            let name = str_of(c, &["name"])?;
            let mut item = Item::new("roku", id, Kind::Live, &name, format!("{BASE}/watch/{id}"));
            item.description = clean(str_of(c, &["description"]));
            item.channel_number = u32_of(c, &["chno"]);
            item.image = str_of(c, &["logo"]).filter(|u| is_web_url(u));
            item.genres = strings(c.get("groups")).into_iter().take(3).collect();
            Some(item)
        })
        .collect()
}

/// Schedules keyed by `Item::id` (see `mjh::programs`).
pub fn parse_programs(v: &Value) -> HashMap<String, Vec<Program>> {
    mjh::channels(v, "us")
        .filter(|(id, _)| valid_id(id))
        .filter_map(|(id, c)| {
            let programs = mjh::programs(c);
            (!programs.is_empty()).then(|| (format!("roku:{id}"), programs))
        })
        .collect()
}

/// Show art from the mirror's Roku guide (`bytes`, gzip or not; see `showart`).
pub fn parse_art(bytes: &[u8], now: u64) -> Result<ShowArt> {
    ShowArt::parse(bytes, now, showart::by_id)
}

/// Add show art to Roku's schedules, matching channels by Roku's id.
pub fn add_art(guide: &mut HashMap<String, Vec<Program>>, channels: &[Item], art: &ShowArt) -> usize {
    showart::add_art(guide, channels, art, |ch| ch.id.strip_prefix("roku:").map(str::to_string))
}

/// Ids go into URLs, so only plain ones are accepted.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Walk any JSON and collect titled content objects.
pub fn parse_search(v: &Value) -> Vec<Item> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    walk(v, &mut |o| {
        if let Some(item) = content_item(o)
            && seen.insert(item.id.clone())
        {
            out.push(item);
        }
    });
    out
}

fn walk(v: &Value, f: &mut dyn FnMut(&Value)) {
    match v {
        Value::Object(map) => {
            f(v);
            for child in map.values() {
                walk(child, f);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| walk(x, f)),
        _ => {}
    }
}

fn content_item(o: &Value) -> Option<Item> {
    let title = str_of(o, &["title"])?;
    let kind_str = str_of(o, &["type", "mediaType", "contentType"]).or_else(|| {
        at(o, &["meta", "mediaType"])
            .and_then(Value::as_str)
            .map(str::to_string)
    })?;
    let kind = match kind_str.to_ascii_lowercase().as_str() {
        "movie" | "tvspecial" => Kind::Movie,
        "series" | "tvseries" | "show" => Kind::Series,
        "linear" | "livefeed" | "live" | "channel" => Kind::Live,
        // "shortformvideo" is trailers and clips; episodes, people, etc.
        _ => return None,
    };
    if !is_free(o) {
        return None;
    }
    let id = at(o, &["meta", "id"])
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| str_of(o, &["id", "contentId"]))?;
    if !valid_id(&id) {
        return None;
    }
    let url = if kind == Kind::Live {
        format!("{BASE}/watch/{id}")
    } else {
        format!("{BASE}/details/{id}")
    };
    let mut item = Item::new("roku", &id, kind, &title, url);
    item.description = description(o);
    item.year = year_of(o, &["releaseYear", "year"]);
    item.image = ["imageMap", "images", "image"]
        .iter()
        .find_map(|k| o.get(k).and_then(find_image));
    // Roku sends genres in lowercase ("comedy").
    item.genres = strings(o.get("genres"))
        .iter()
        .take(3)
        .map(|g| capitalize(g))
        .collect();
    Some(item)
}

/// `viewOptions: [ { providerId: "rokuavod" }, { providerId: "paramountplus", … } ]`
fn is_free(o: &Value) -> bool {
    arr(o, &["viewOptions"])
        .iter()
        .any(|v| v.get("providerId").and_then(Value::as_str) == Some(FREE_PROVIDER))
}

/// `descriptions: { "100": { size, text }, "250": { … } }`; the longest wins.
fn description(o: &Value) -> Option<String> {
    let longest = o
        .get("descriptions")
        .and_then(Value::as_object)
        .and_then(|sizes| {
            sizes
                .values()
                .filter_map(|d| d.get("text").and_then(Value::as_str))
                .max_by_key(|t| t.len())
                .map(str::to_string)
        });
    clean(longest.or_else(|| str_of(o, &["description", "shortDescription"])))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn find_image(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if is_web_url(s) => Some(s.clone()),
        Value::Object(m) => m.values().find_map(find_image),
        Value::Array(a) => a.iter().find_map(find_image),
        _ => None,
    }
}
