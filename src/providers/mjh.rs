//! Reader for the open-source i.mjh.nz project, which mirrors the channel
//! lists and schedules of free TV apps (Plex, Samsung TV Plus and others) as
//! `.channels.json.gz` files. Conflux reads only the channel data. Some files
//! also carry request headers or tokens for those services; Conflux never
//! uses them.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};

use anyhow::Result;
use reqwest::Client;
use serde_json::Value;
use tokio::sync::Mutex;

use super::json::arr;
use crate::model::Program;

/// Download and decode one mirror file. The URLs redirect to GitHub, which
/// reqwest follows.
pub async fn fetch(http: &Client, url: &str) -> Result<Value> {
    let resp = http.get(url).send().await?.error_for_status()?;
    decode(&resp.bytes().await?)
}

/// The files are gzip; some servers already undo that for us.
pub fn decode(bytes: &[u8]) -> Result<Value> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut s = String::new();
        flate2::read::GzDecoder::new(bytes).read_to_string(&mut s)?;
        Ok(serde_json::from_str(&s)?)
    } else {
        Ok(serde_json::from_slice(bytes)?)
    }
}

/// `{ channels: { "<id>": { …, regions: ["us"] } } }`: the channels offered in
/// `region`. A file without regions (Roku's) covers one region only.
pub fn channels<'a>(v: &'a Value, region: &'a str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    v.get("channels")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(move |(_, c)| match c.get("regions").and_then(Value::as_array) {
            Some(rs) => rs.iter().any(|r| r.as_str() == Some(region)),
            None => true,
        })
}

/// `programs: [[start, title], …]`. Each slot ends when the next one
/// starts, so the last one (no known end) is dropped, as are untitled slots.
pub fn programs(c: &Value) -> Vec<Program> {
    let mut slots: Vec<(u64, &str)> = arr(c, &["programs"])
        .iter()
        .filter_map(|p| Some((p.get(0)?.as_u64()?, p.get(1)?.as_str()?.trim())))
        .collect();
    slots.sort_by_key(|s| s.0);
    slots.dedup_by_key(|s| s.0);
    slots
        .windows(2)
        .filter(|w| !w[0].1.is_empty())
        .map(|w| Program {
            start: w[0].0,
            stop: w[1].0,
            title: w[0].1.to_string(),
            subtitle: None,
            description: None,
            rating: None,
            image: None,
        })
        .collect()
}

/// Schedules from the last download, keyed by `Item::id`, so the guide
/// refresh that runs right after a catalog refresh doesn't fetch the same
/// file again.
#[derive(Default)]
pub struct Schedules(Mutex<Option<(Instant, HashMap<String, Vec<Program>>)>>);

impl Schedules {
    const REUSE: Duration = Duration::from_secs(10 * 60);

    pub async fn set(&self, map: HashMap<String, Vec<Program>>) {
        *self.0.lock().await = Some((Instant::now(), map));
    }

    /// The last schedules, if they're recent enough to reuse.
    pub async fn recent(&self) -> Option<HashMap<String, Vec<Program>>> {
        let g = self.0.lock().await;
        g.as_ref()
            .filter(|(at, _)| at.elapsed() < Self::REUSE)
            .map(|(_, m)| m.clone())
    }
}
