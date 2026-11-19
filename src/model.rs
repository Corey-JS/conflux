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
    fn kind_parse() {
        assert_eq!(Kind::parse("Movies"), Some(Kind::Movie));
        assert_eq!(Kind::parse("show"), Some(Kind::Series));
        assert_eq!(Kind::parse("nope"), None);
    }
}
