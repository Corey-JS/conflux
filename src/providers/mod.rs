//! One module per provider. Each one turns that provider's own web-app JSON
//! into `Item`s. A provider failing never affects the others: errors are
//! caught per provider and shown on /api/providers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use reqwest::{Client, StatusCode};

use crate::model::{Item, Program};

pub mod json;
pub mod mjh;
pub mod plex;
pub mod pluto;
pub mod roku;
pub mod samsung;
pub mod showart;
pub mod tubi;
pub mod xmltv;

#[async_trait]
pub trait Provider: Send + Sync {
    /// Short id used in URLs and item ids, e.g. "pluto".
    fn id(&self) -> &'static str;
    /// Display name, e.g. "Pluto TV".
    fn name(&self) -> &'static str;

    /// True if this provider has a catalog small enough to download whole
    /// and search locally (live channel lineups, Pluto's on-demand library).
    fn has_catalog(&self) -> bool {
        false
    }
    /// Download the whole catalog. Called on a timer.
    async fn catalog(&self, _http: &Client) -> Result<Vec<Item>> {
        Ok(Vec::new())
    }

    /// True if this provider can say what's on each live channel right now.
    fn has_guide(&self) -> bool {
        false
    }
    /// Upcoming programs for these live channels, keyed by `Item::id`.
    async fn guide(&self, _http: &Client, _channels: &[Item]) -> Result<HashMap<String, Vec<Program>>> {
        Ok(HashMap::new())
    }
    /// True if this provider's show art is full size (often megabytes), so
    /// Conflux serves shrunken copies instead (see `art`).
    fn shrink_art(&self) -> bool {
        false
    }

    /// True if this provider is searched live, per query, because its
    /// library is too big to download (Roku, Tubi).
    fn has_search(&self) -> bool {
        false
    }
    async fn search(&self, _http: &Client, _query: &str) -> Result<Vec<Item>> {
        Ok(Vec::new())
    }
}

pub const ALL_IDS: &[&str] = &["pluto", "plex", "tubi", "roku", "samsung"];

/// Off unless asked for:
/// - Tubi: its old search endpoint answers 401 (Nov 2026), and the service
///   that replaced it only takes requests signed the way Tubi's own client
///   signs them, so every search would just report Tubi as unavailable.
/// - Samsung TV Plus: its website reportedly closed and it has no iPhone app,
///   so there is nowhere on a phone to send a tap.
pub const DEFAULT_IDS: &[&str] = &["pluto", "plex", "roku"];

pub fn build(ids: &[String]) -> Vec<Arc<dyn Provider>> {
    let mut out: Vec<Arc<dyn Provider>> = Vec::new();
    for id in ids {
        match id.as_str() {
            "pluto" => out.push(Arc::new(pluto::Pluto::new())),
            "plex" => out.push(Arc::new(plex::Plex::new())),
            "tubi" => out.push(Arc::new(tubi::Tubi)),
            "roku" => out.push(Arc::new(roku::Roku::new())),
            "samsung" => out.push(Arc::new(samsung::Samsung::from_env())),
            other => tracing::warn!("unknown provider '{other}' ignored"),
        }
    }
    out
}

/// Browsers are what these endpoints expect, so present as one.
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15";

pub fn http_client() -> Client {
    Client::builder()
        .user_agent(USER_AGENT)
        .cookie_store(true)
        .gzip(true)
        .timeout(std::time::Duration::from_secs(20))
        .connect_timeout(std::time::Duration::from_secs(8))
        .build()
        .expect("http client")
}

/// How long to leave a provider alone after it says "too many requests"
/// without saying for how long, and the shortest and longest pause allowed.
pub const DEFAULT_PAUSE: Duration = Duration::from_secs(15 * 60);
const MIN_PAUSE: Duration = Duration::from_secs(60);
const MAX_PAUSE: Duration = Duration::from_secs(6 * 3600);

/// A provider asked us to slow down (HTTP 429, or 503 with Retry-After).
/// Carried inside `anyhow::Error`; see `throttled`.
#[derive(Debug)]
pub struct Throttled {
    pub host: String,
    pub wait: Duration,
}

impl std::fmt::Display for Throttled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} asked us to slow down; pausing {} min",
            self.host,
            self.wait.as_secs().div_ceil(60)
        )
    }
}

impl std::error::Error for Throttled {}

/// If this error means "slow down", how long to wait before asking again.
pub fn throttled(e: &anyhow::Error) -> Option<Duration> {
    e.chain().find_map(|cause| {
        if let Some(t) = cause.downcast_ref::<Throttled>() {
            Some(t.wait)
        } else if let Some(r) = cause.downcast_ref::<reqwest::Error>() {
            (r.status() == Some(StatusCode::TOO_MANY_REQUESTS)).then_some(DEFAULT_PAUSE)
        } else {
            None
        }
    })
}

/// Retry-After in seconds (the HTTP-date form is rare and gets the default).
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let secs = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(secs))
}

/// GET a URL and parse JSON, with a readable error if the status is bad.
pub async fn get_json(req: reqwest::RequestBuilder) -> Result<serde_json::Value> {
    let resp = req.send().await?;
    let status = resp.status();
    let url = resp.url().clone();
    let wait = retry_after(resp.headers());
    if status == StatusCode::TOO_MANY_REQUESTS || (status == StatusCode::SERVICE_UNAVAILABLE && wait.is_some()) {
        return Err(Throttled {
            host: url.host_str().unwrap_or("provider").to_string(),
            wait: wait.unwrap_or(DEFAULT_PAUSE).clamp(MIN_PAUSE, MAX_PAUSE),
        }
        .into());
    }
    let body = resp.bytes().await?;
    if !status.is_success() {
        let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]).to_string();
        anyhow::bail!(
            "{status} from {}{}: {snippet}",
            url.host_str().unwrap_or(""),
            url.path()
        );
    }
    serde_json::from_slice(&body).map_err(|e| {
        anyhow::anyhow!(
            "bad JSON from {}{}: {e}",
            url.host_str().unwrap_or(""),
            url.path()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_known_and_skip_the_off_providers() {
        let ids: Vec<String> = DEFAULT_IDS.iter().map(|s| s.to_string()).collect();
        assert_eq!(build(&ids).len(), DEFAULT_IDS.len(), "every default builds");
        assert!(DEFAULT_IDS.iter().all(|id| ALL_IDS.contains(id)));
        assert!(!DEFAULT_IDS.contains(&"tubi"), "Tubi search answers 401");
        assert!(!DEFAULT_IDS.contains(&"samsung"), "no iPhone app to send taps to");
        assert_eq!(build(&["tubi".into()]).len(), 1, "Tubi can still be turned on");
    }
}
