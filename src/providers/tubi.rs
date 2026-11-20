//! Tubi: searched per query (the library is far too big to download).
//! OFF BY DEFAULT.
//!
//! Uses the search endpoint Tubi's web app used to call. Results carry a type
//! letter: "v" video/movie, "s" series, "l" live/linear channel.
//!
//! Since Nov 2026 this endpoint answers 401. Tubi's web app now searches
//! through a service whose signed-out token needs a request signature only
//! Tubi's own client is meant to produce, so this module is kept (with its
//! parser and tests) for the day Tubi offers an open route again, but isn't
//! in `DEFAULT_IDS`. Set `CONFLUX_PROVIDERS=...,tubi` to try it.

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use super::json::{arr, str_of, strings, year_of};
use super::{Provider, get_json};
use crate::model::{Item, Kind, clean, is_web_url};

const SEARCH_URL: &str = "https://tubitv.com/oz/search/";

pub struct Tubi;

#[async_trait]
impl Provider for Tubi {
    fn id(&self) -> &'static str {
        "tubi"
    }
    fn name(&self) -> &'static str {
        "Tubi"
    }
    fn has_search(&self) -> bool {
        true
    }
    async fn search(&self, http: &Client, query: &str) -> Result<Vec<Item>> {
        let mut url = reqwest::Url::parse(SEARCH_URL)?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("bad url"))?
            .pop_if_empty()
            .push(query);
        let v = get_json(http.get(url).query(&[
            ("isKidsMode", "false"),
            ("useLinearHeader", "true"),
            ("isMobile", "true"),
        ]))
        .await
        .context("Tubi search")?;
        Ok(parse_search(&v))
    }
}

/// Either a bare array of items, or an object holding one under
/// `contents` / `items` / `results`.
pub fn parse_search(v: &Value) -> Vec<Item> {
    let list: &[Value] = match v {
        Value::Array(a) => a,
        _ => ["contents", "items", "results", "data"]
            .iter()
            .map(|k| arr(v, &[k]))
            .find(|a| !a.is_empty())
            .unwrap_or(&[]),
    };
    list.iter().filter_map(item).collect()
}

fn item(c: &Value) -> Option<Item> {
    let id = str_of(c, &["id", "ID", "content_id"])?;
    let title = str_of(c, &["title", "Title"])?;
    let (kind, url) = match str_of(c, &["type"]).as_deref() {
        Some("s") => (Kind::Series, format!("https://tubitv.com/series/{id}")),
        Some("l") | Some("linear") => (Kind::Live, format!("https://tubitv.com/live/{id}")),
        Some("v") | None => (Kind::Movie, format!("https://tubitv.com/movies/{id}")),
        Some(_) => return None, // episodes and anything unknown
    };
    let mut it = Item::new("tubi", &id, kind, &title, url);
    it.description = clean(str_of(c, &["description"]));
    it.year = year_of(c, &["year", "Year"]);
    it.image = first_url(c.get("posterarts"))
        .or_else(|| first_url(c.get("thumbnails")))
        .or_else(|| first_url(c.get("landscape_images")));
    it.genres = strings(c.get("tags"));
    it.genres.truncate(3);
    Some(it)
}

fn first_url(v: Option<&Value>) -> Option<String> {
    strings(v).into_iter().find(|u| is_web_url(u))
}
