//! The Roku Channel: searched per query.
//!
//! The Roku Channel's web app gets a CSRF token, then searches with it. Its
//! response shape is not documented, so the parser walks the JSON and picks
//! out anything that looks like a titled movie, series or channel.
//!
//! Roku's search covers every channel on Roku, paid ones included, so each
//! result is kept only if The Roku Channel itself offers it free.

use std::collections::HashSet;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::json::{arr, at, str_of, strings, year_of};
use super::{Provider, get_json};
use crate::model::{Item, Kind, clean, is_web_url};

/// The web app moved here from therokuchannel.roku.com, which now answers
/// with a 301. A redirected POST arrives as a GET with no body, so the old
/// host can't be used for search.
const BASE: &str = "https://app.roku.com";

/// The view option that means "free with ads on The Roku Channel". Others
/// are paid: "rokusvod" (Roku's Premium Subscriptions), "paramountplus",
/// "shudder" and so on.
const FREE_PROVIDER: &str = "rokuavod";

pub struct Roku {
    csrf: Mutex<Option<String>>,
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
        }
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
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
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
