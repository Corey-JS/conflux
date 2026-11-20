//! Show art, episode names, descriptions and ratings from an XMLTV guide,
//! added to schedules that have titles only (the i.mjh.nz channel files).
//!
//! - Plex's comes from epgshare01, whose channel ids are made from names
//!   ("plex.tv.CW.Forever.plex"), so channels are matched by name.
//! - Roku's comes from the mirror's own guide file, which uses Roku's ids.

use std::collections::HashMap;
use std::io::BufReader;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use reqwest::Client;
use tokio::sync::Mutex;

use super::xmltv;
use crate::model::{Item, Program};

/// Programmes that have art, by channel key (see `Keys`), each with its
/// title's `name_key`.
#[derive(Debug, Default)]
pub struct ShowArt(HashMap<String, Vec<(String, xmltv::Programme)>>);

/// The keys an XMLTV channel (its id and contents) can be found by.
pub type Keys = fn(&str, &xmltv::Channel) -> Vec<String>;

/// Channels found by name (see `name_key`).
pub fn by_name(_id: &str, c: &xmltv::Channel) -> Vec<String> {
    c.names.iter().map(|n| name_key(n)).collect()
}

/// Channels found by their XMLTV id.
pub fn by_id(id: &str, _c: &xmltv::Channel) -> Vec<String> {
    vec![id.to_string()]
}

impl ShowArt {
    /// `bytes` is the XMLTV file, gzip or not. Keeps programmes from 6 hours
    /// before `now` to 36 hours after (the mirror's schedules span about a day).
    pub fn parse(bytes: &[u8], now: u64, keys: Keys) -> Result<ShowArt> {
        let (from, to) = (now.saturating_sub(6 * 3600), now + 36 * 3600);
        let channels = if bytes.starts_with(&[0x1f, 0x8b]) {
            xmltv::parse(BufReader::new(flate2::read::GzDecoder::new(bytes)), from, to)?
        } else {
            xmltv::parse(bytes, from, to)?
        };
        let mut by_key = HashMap::new();
        for (id, c) in channels {
            if c.programmes.is_empty() {
                continue;
            }
            let mut ks = keys(&id, &c);
            let Some(last) = ks.pop() else { continue };
            let slots: Vec<_> = c.programmes.into_iter().map(|p| (name_key(&p.title), p)).collect();
            for k in ks {
                by_key.entry(k).or_insert_with(|| slots.clone());
            }
            by_key.entry(last).or_insert(slots);
        }
        by_key.remove("");
        Ok(ShowArt(by_key))
    }

    pub fn channels(&self) -> usize {
        self.0.len()
    }

    /// The programme for `p` on the channel with this key: the one airing
    /// halfway through `p` if the titles match (`true`: the same airing).
    /// Otherwise the nearest one with the same title, for its show art.
    fn find(&self, channel: &str, p: &Program) -> Option<(&xmltv::Programme, bool)> {
        let slots = self.0.get(channel)?;
        let title = name_key(&p.title);
        let mid = p.start + p.stop.saturating_sub(p.start) / 2;
        let same_title = || slots.iter().filter(|(t, _)| *t == title).map(|(_, s)| s);
        if let Some(s) = same_title().find(|s| s.start <= mid && mid < s.stop) {
            return Some((s, true));
        }
        same_title().min_by_key(|s| s.start.abs_diff(p.start)).map(|s| (s, false))
    }
}

/// Lowercase letters and digits only, so "A&E Alaska State Troopers" and
/// "A and E alaska state troopers" match.
pub fn name_key(s: &str) -> String {
    s.replace('&', " and ")
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Add show art to `guide`'s programs (and, for the same airing, the episode
/// name, description and rating). `key` gives each channel's key in `art`.
/// Returns how many programs got art.
pub fn add_art(
    guide: &mut HashMap<String, Vec<Program>>,
    channels: &[Item],
    art: &ShowArt,
    key: impl Fn(&Item) -> Option<String>,
) -> usize {
    let mut n = 0;
    for ch in channels {
        let (Some(k), Some(programs)) = (key(ch), guide.get_mut(&ch.id)) else {
            continue;
        };
        for p in programs {
            let Some((s, same_airing)) = art.find(&k, p) else {
                continue;
            };
            if p.image.is_none() {
                p.image = Some(s.image.clone());
                n += 1;
            }
            if same_airing {
                // Some channels repeat their own name as the episode name.
                let episode = s.subtitle.clone().filter(|e| {
                    let e = name_key(e);
                    e != name_key(&p.title) && e != name_key(&ch.title)
                });
                p.subtitle = p.subtitle.take().or(episode);
                p.description = p.description.take().or_else(|| s.description.clone());
                p.rating = p.rating.take().or_else(|| s.rating.clone());
            }
        }
    }
    n
}

/// An XMLTV guide downloaded every `every`, keeping the last one that worked.
pub struct Feed {
    provider: &'static str,
    /// Who publishes it, for the logs.
    source: &'static str,
    url: &'static str,
    every: Duration,
    keys: Keys,
    held: Mutex<Option<(Instant, Arc<ShowArt>)>>,
}

impl Feed {
    pub fn new(provider: &'static str, source: &'static str, url: &'static str, every: Duration, keys: Keys) -> Feed {
        Feed {
            provider,
            source,
            url,
            every,
            keys,
            held: Mutex::new(None),
        }
    }

    /// The art, downloaded again once it's `every` old. If a download fails
    /// the last art is kept, and the next guide refresh tries again.
    pub async fn get(&self, http: &Client) -> Option<Arc<ShowArt>> {
        let mut held = self.held.lock().await;
        if !held.as_ref().is_some_and(|(at, _)| at.elapsed() < self.every) {
            // One retry: a dropped connection would otherwise mean no art
            // until the next guide refresh.
            let got = match self.fetch(http).await {
                Ok(art) => Ok(art),
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    self.fetch(http).await
                }
            };
            match got {
                Ok(art) => {
                    tracing::info!("{}: show art for {} channels ({})", self.provider, art.channels(), self.source);
                    *held = Some((Instant::now(), Arc::new(art)));
                }
                Err(e) => tracing::warn!("{}: no new show art ({}): {e:#}", self.provider, self.source),
            }
        }
        held.as_ref().map(|(_, art)| Arc::clone(art))
    }

    async fn fetch(&self, http: &Client) -> Result<ShowArt> {
        let bytes = http
            .get(self.url)
            .timeout(Duration::from_secs(90))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let keys = self.keys;
        let art = tokio::task::spawn_blocking(move || ShowArt::parse(&bytes, now, keys)).await??;
        if art.channels() == 0 {
            anyhow::bail!("no programmes with art (the file's format may have changed)");
        }
        Ok(art)
    }
}
