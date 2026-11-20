//! The shape every provider's data is normalized into.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A live, always-on channel.
    Live,
    /// An on-demand movie.
    Movie,
    /// An on-demand series.
    Series,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s.to_ascii_lowercase().as_str() {
            "live" | "channel" | "channels" => Some(Kind::Live),
            "movie" | "movies" => Some(Kind::Movie),
            "series" | "show" | "shows" | "tv" => Some(Kind::Series),
            _ => None,
        }
    }
}

/// One thing a person can watch, and where to go to watch it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    /// Unique across providers: "<provider>:<provider's own id>".
    pub id: String,
    pub provider: String,
    pub kind: Kind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_number: Option<u32>,
    /// The provider's own page (or app link) for this item.
    pub watch_url: String,
}

impl Item {
    pub fn new(provider: &str, raw_id: &str, kind: Kind, title: &str, watch_url: String) -> Item {
        Item {
            id: format!("{provider}:{raw_id}"),
            provider: provider.to_string(),
            kind,
            title: title.trim().to_string(),
            description: None,
            image: None,
            genres: Vec::new(),
            year: None,
            channel_number: None,
            watch_url,
        }
    }
}

/// One slot in a live channel's schedule. Times are Unix seconds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Program {
    pub start: u64,
    pub stop: u64,
    pub title: String,
    /// Episode name, when the title is the series.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<String>,
    /// Poster / episode art for what's airing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// A live channel plus what's on it now and next.
#[derive(Debug, Clone, Serialize)]
pub struct LiveEntry {
    #[serde(flatten)]
    pub item: Item,
    pub programs: Vec<Program>,
}

/// Parse "2026-11-19T21:51:00.000Z" (UTC) to Unix seconds.
pub fn iso_to_unix(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

/// Trim and drop empty strings, so the frontend never shows blank lines.
pub fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// A URL-safe slug from a title, used only when a provider gives none.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Only ever hand the browser http(s) links.
pub fn is_web_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("The Mask (1994)!"), "the-mask-1994");
        assert_eq!(slugify("  Hell's Kitchen "), "hell-s-kitchen");
    }

    #[test]
    fn iso_parse() {
        assert_eq!(iso_to_unix("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(iso_to_unix("2026-11-19T21:51:00.000Z"), Some(1_795_125_060));
        assert_eq!(iso_to_unix("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        assert_eq!(iso_to_unix("garbage"), None);
    }

    #[test]
    fn kind_parse() {
        assert_eq!(Kind::parse("Movies"), Some(Kind::Movie));
        assert_eq!(Kind::parse("show"), Some(Kind::Series));
        assert_eq!(Kind::parse("nope"), None);
    }
}
