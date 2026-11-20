//! Plex: free live channels and their schedules, downloaded whole.
//!
//! Plex's own APIs need an X-Plex-Token even for free content, and since
//! Nov 2026 plex.tv answers 404 when anything but Plex's own apps asks for a
//! signed-out guest token. Conflux doesn't pretend to be Plex's app, so it
//! reads two public sources instead:
//!
//! - The channel list and schedules that the open-source i.mjh.nz project
//!   mirrors from Plex (see `mjh`).
//! - Plex's public live TV page, watch.plex.tv/live-tv, for each channel's
//!   link and genre. Links can't be guessed from names ("A&E Alaska State
//!   Troopers" is `aande-alaska-state-troopers`, "VICE" is `vice-2`), so a
//!   channel the page doesn't list links to the live TV page as a whole.
//!
//! Either source alone still gives a channel list. On-demand search needed the
//! guest token, so Plex no longer has it.

use std::collections::HashMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use super::json::str_of;
use super::{Provider, mjh};
use crate::model::{Item, Kind, Program, clean, is_web_url};

const MIRROR_URL: &str = "https://i.mjh.nz/Plex/.channels.json.gz";
const PAGE_URL: &str = "https://watch.plex.tv/live-tv";
const CHANNEL_URL: &str = "https://watch.plex.tv/live-tv/channel/";

pub struct Plex {
    region: String,
    schedules: mjh::Schedules,
}

impl Default for Plex {
    fn default() -> Self {
        Self::new()
    }
}

impl Plex {
    pub fn new() -> Plex {
        Plex {
            region: std::env::var("CONFLUX_PLEX_REGION")
                .map(|r| r.trim().to_lowercase())
                .unwrap_or_else(|_| "us".into()),
            schedules: mjh::Schedules::default(),
        }
    }

    /// Download the mirror and keep its schedules for `guide`.
    async fn fetch_mirror(&self, http: &Client) -> Result<Value> {
        let v = mjh::fetch(http, MIRROR_URL)
            .await
            .context("Plex channel list (i.mjh.nz)")?;
        self.schedules.set(parse_programs(&v, &self.region)).await;
        Ok(v)
    }
}

async fn fetch_page(http: &Client) -> Result<Vec<PageChannel>> {
    let html = http
        .get(PAGE_URL)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let channels = parse_page(&html);
    if channels.is_empty() {
        anyhow::bail!("no channels in Plex's live TV page (its layout may have changed)");
    }
    Ok(channels)
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
        let (mirror, page) = tokio::join!(self.fetch_mirror(http), fetch_page(http));
        let page = match page {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("plex: live TV page unreadable, channels link to the guide: {e:#}");
                if let Err(m) = &mirror {
                    anyhow::bail!("{m:#}; live TV page: {e:#}");
                }
                Vec::new()
            }
        };
        let mirror = match mirror {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("plex: mirror failed, using Plex's page (no schedules): {e:#}");
                None
            }
        };
        let items = build(mirror.as_ref(), &page, &self.region);
        if items.is_empty() {
            anyhow::bail!("no Plex channels for region '{}'", self.region);
        }
        Ok(items)
    }
    fn has_guide(&self) -> bool {
        true
    }
    async fn guide(&self, http: &Client, _channels: &[Item]) -> Result<HashMap<String, Vec<Program>>> {
        if let Some(map) = self.schedules.recent().await {
            return Ok(map);
        }
        self.fetch_mirror(http).await?;
        Ok(self.schedules.recent().await.unwrap_or_default())
    }
}

fn item_id(raw: &str) -> String {
    format!("live:{raw}")
}

/// Channels from the mirror, with links and genres from Plex's page. If the
/// mirror has none (it's down), the page's own list is used.
pub fn build(mirror: Option<&Value>, page: &[PageChannel], region: &str) -> Vec<Item> {
    let mut items = mirror.map(|v| parse_mirror(v, region)).unwrap_or_default();
    if items.is_empty() {
        items = page
            .iter()
            .filter_map(|p| {
                let title = p.title.as_deref()?;
                let mut it = Item::new("plex", &item_id(&p.id), Kind::Live, title, PAGE_URL.into());
                it.image = p.thumb.clone();
                Some(it)
            })
            .collect();
    }
    let by_id: HashMap<&str, &PageChannel> = page.iter().map(|p| (p.id.as_str(), p)).collect();
    for it in &mut items {
        let raw = it.id.strip_prefix("plex:live:").unwrap_or_default();
        if let Some(p) = by_id.get(raw) {
            it.watch_url = format!("{CHANNEL_URL}{}", p.slug);
            it.genres = p.genre.iter().cloned().collect();
            if it.image.is_none() {
                it.image = p.thumb.clone();
            }
        }
    }
    items
}

/// `{ channels: { "<id>": { name, logo, regions: ["us"], programs } } }`
pub fn parse_mirror(v: &Value, region: &str) -> Vec<Item> {
    mjh::channels(v, region)
        .filter_map(|(id, c)| {
            let name = str_of(c, &["name"])?;
            let mut it = Item::new("plex", &item_id(id), Kind::Live, &name, PAGE_URL.into());
            it.image = str_of(c, &["logo"]).filter(|u| is_web_url(u));
            it.description = clean(str_of(c, &["description"]));
            Some(it)
        })
        .collect()
}

/// Schedules keyed by `Item::id` (see `mjh::programs`).
pub fn parse_programs(v: &Value, region: &str) -> HashMap<String, Vec<Program>> {
    mjh::channels(v, region)
        .filter_map(|(id, c)| {
            let programs = mjh::programs(c);
            (!programs.is_empty()).then(|| (format!("plex:{}", item_id(id)), programs))
        })
        .collect()
}

/// One channel as listed on Plex's live TV page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageChannel {
    pub id: String,
    pub slug: String,
    pub title: Option<String>,
    pub thumb: Option<String>,
    /// The genre row it's listed in, e.g. "Food, Home & Culture".
    pub genre: Option<String>,
}

/// After `"title":"<row name>"`; the row is a genre if its URL has `genre%3D`.
const ROW_MARK: &str = "\",\"channels\":[";
const CHANNEL_MARK: &str = "{\"id\":\"";

/// Plex's live TV page carries its guide as escaped JSON inside the page's
/// scripts. Each genre row looks like
/// `…genre%3Dnews","title":"News","channels":[{"id":"…","slug":"…","title":"…","thumb":"…",…},…]`.
/// This reads only each channel's id, slug, title, logo and genre row, and
/// skips anything that doesn't look like a channel.
pub fn parse_page(html: &str) -> Vec<PageChannel> {
    let text = html.replace("\\\"", "\"");
    let mut out: Vec<PageChannel> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut genre: Option<String> = None;
    let mut pos = 0;
    loop {
        let row = text[pos..].find(ROW_MARK).map(|i| pos + i);
        let chan = text[pos..].find(CHANNEL_MARK).map(|i| pos + i);
        match (row, chan) {
            (Some(r), c) if c.is_none_or(|c| r < c) => {
                genre = row_genre(&text, r);
                pos = r + ROW_MARK.len();
            }
            (_, Some(c)) => {
                pos = c + CHANNEL_MARK.len();
                let Some(ch) = read_channel(&text[pos..], genre.clone()) else {
                    continue;
                };
                match index.get(&ch.id) {
                    Some(&i) => {
                        if out[i].genre.is_none() {
                            out[i].genre = ch.genre;
                        }
                    }
                    None => {
                        index.insert(ch.id.clone(), out.len());
                        out.push(ch);
                    }
                }
            }
            _ => break,
        }
    }
    out
}

/// The genre name of the row whose channel list starts at `mark`.
fn row_genre(text: &str, mark: usize) -> Option<String> {
    const TITLE: &str = "\"title\":\"";
    let t = text[..mark].rfind(TITLE)?;
    let before = &text.as_bytes()[t.saturating_sub(300)..t];
    if !before.windows(8).any(|w| w == b"genre%3D") {
        return None;
    }
    let (name, _) = read_str(&text[t + TITLE.len()..])?;
    clean(Some(name))
}

/// `s` starts just after `{"id":"`.
fn read_channel(s: &str, genre: Option<String>) -> Option<PageChannel> {
    let (id, rest) = s.split_once('"')?;
    let id_ok = (8..=100).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    if !id_ok {
        return None;
    }
    let (slug, rest) = rest.strip_prefix(",\"slug\":\"")?.split_once('"')?;
    let slug_ok = !slug.is_empty()
        && slug.len() <= 120
        && slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !slug_ok {
        return None;
    }
    let (title, rest) = match rest.strip_prefix(",\"title\":\"").and_then(read_str) {
        Some((t, r)) => (clean(Some(t)), r),
        None => (None, rest),
    };
    let thumb = rest
        .strip_prefix(",\"thumb\":\"")
        .and_then(read_str)
        .map(|(u, _)| u)
        .filter(|u| is_web_url(u));
    Some(PageChannel {
        id: id.to_string(),
        slug: slug.to_string(),
        title,
        thumb,
        genre,
    })
}

/// A JSON string body up to its closing quote, unescaped. Gives up on
/// anything longer than a title or URL should be.
fn read_str(s: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &s[i + 1..])),
            '\\' => match chars.next()?.1 {
                'u' => {
                    let hex: String = chars.by_ref().take(4).map(|(_, c)| c).collect();
                    let ch = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32);
                    out.push(ch.unwrap_or('\u{fffd}'));
                }
                'n' | 'r' | 't' => out.push(' '),
                other => out.push(other),
            },
            c => out.push(c),
        }
        if out.len() > 500 {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_str_unescapes() {
        assert_eq!(
            read_str("A\\u0026E \\/ News\",rest"),
            Some(("A&E / News".to_string(), ",rest"))
        );
        assert_eq!(read_str("no closing quote"), None);
    }

    #[test]
    fn non_genre_rows_and_bad_ids_are_skipped() {
        let page = r#"\"title\":\"Popular\",\"channels\":[{\"id\":\"../../x\",\"slug\":\"evil\"},{\"id\":\"abc12345\",\"slug\":\"Bad Slug\"},{\"id\":\"abc12345\",\"slug\":\"ok-1\"}]"#;
        let chans = parse_page(page);
        assert_eq!(chans.len(), 1);
        assert_eq!(chans[0].slug, "ok-1");
        assert_eq!(chans[0].genre, None);
        assert_eq!(chans[0].title, None);
    }
}
