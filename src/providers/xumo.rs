//! Xumo Play: live channels and schedules, downloaded whole.
//!
//! Reads the same channel list and guide that play.xumo.com's own web app
//! loads (`valencia-app-mds.xumo.com`, no token). The guide comes in pages
//! of 50 channels per six-hour block of the day (blocks 0-3, UTC).
//! Checked against the live service in Nov 2026. US only.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use super::json::{arr, at, str_of, u32_of};
use super::{Provider, get_json};
use crate::model::{Item, Kind, Program, clean, iso_to_unix};

const API: &str = "https://valencia-app-mds.xumo.com/v2";
/// Xumo Play's id for its US web lineup.
const LINEUP: &str = "10006";
const PAGE: usize = 50;
/// Never ask for more guide pages than this per block, whatever the
/// channel count says.
const MAX_PAGES: usize = 20;

pub struct Xumo;

#[async_trait]
impl Provider for Xumo {
    fn id(&self) -> &'static str {
        "xumo"
    }
    fn name(&self) -> &'static str {
        "Xumo Play"
    }
    fn has_catalog(&self) -> bool {
        true
    }
    async fn catalog(&self, http: &Client) -> Result<Vec<Item>> {
        let v = get_json(
            http.get(format!("{API}/channels/list/{LINEUP}.json"))
                .query(&[("sort", "hybrid"), ("geoId", "unknown")]),
        )
        .await
        .context("Xumo channel list")?;
        Ok(parse_channels(&v))
    }
    fn has_guide(&self) -> bool {
        true
    }
    async fn guide(&self, http: &Client, channels: &[Item]) -> Result<HashMap<String, Vec<Program>>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // The block airing now, plus the next one once this one is two-thirds
        // done, so "next" never runs dry near a block's end.
        let mut blocks = vec![block_of(now)];
        if now % (6 * 3600) >= 4 * 3600 {
            blocks.push(block_of(now + 6 * 3600));
        }
        let pages = channels.len().div_ceil(PAGE).clamp(1, MAX_PAGES);

        let mut out: HashMap<String, Vec<Program>> = HashMap::new();
        let mut last_err = None;
        for (date, block) in blocks {
            // One page at a time: ten small requests every 20 minutes.
            for page in 0..pages {
                let req = http
                    .get(format!("{API}/epg/{LINEUP}/{date}/{block}.json"))
                    .query(&[
                        ("limit", PAGE.to_string()),
                        ("offset", (page * PAGE).to_string()),
                        ("f", "asset.title".into()),
                        ("f", "asset.descriptions".into()),
                    ]);
                match get_json(req).await {
                    Ok(v) => {
                        let last = arr(&v, &["channels"]).len() < PAGE;
                        for (id, progs) in parse_epg(&v) {
                            out.entry(id).or_default().extend(progs);
                        }
                        if last {
                            break;
                        }
                    }
                    Err(e) => {
                        if super::throttled(&e).is_some() {
                            return Err(e.context("Xumo guide"));
                        }
                        last_err = Some(e);
                    }
                }
            }
        }
        for progs in out.values_mut() {
            progs.sort_by_key(|p| p.start);
            progs.dedup_by_key(|p| p.start);
        }
        if out.is_empty()
            && let Some(e) = last_err
        {
            return Err(e.context("Xumo guide"));
        }
        Ok(out)
    }
}

/// `{ channel: { item: [ { title, description, guid: { value }, number,
/// genre: [ { value } ], properties: { is_live } } ] } }`
pub fn parse_channels(v: &Value) -> Vec<Item> {
    let mut out = Vec::new();
    for c in arr(v, &["channel", "item"]) {
        let (Some(id), Some(title)) = (
            at(c, &["guid"]).and_then(|g| str_of(g, &["value"])),
            str_of(c, &["title"]),
        ) else {
            continue;
        };
        if !id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // Xumo also lists on-demand "pages" here; only real channels play live.
        let live = at(c, &["properties", "is_live"])
            .and_then(Value::as_str)
            .is_some_and(|s| s.eq_ignore_ascii_case("true"));
        if !live {
            continue;
        }
        // Xumo resolves the page by id; the slug only makes the link readable.
        let url = format!("https://play.xumo.com/networks/{}/{id}", slug(&title));
        let mut item = Item::new("xumo", &id, Kind::Live, &title, url);
        item.description = clean(str_of(c, &["description"]));
        item.channel_number = u32_of(c, &["number"]);
        item.image = Some(format!(
            "https://image.xumo.com/v1/channels/channel/{id}/168x168.png?type=color_onBlack"
        ));
        item.genres = arr(c, &["genre"])
            .iter()
            .filter_map(|g| str_of(g, &["value"]))
            .take(3)
            .collect();
        out.push(item);
    }
    out.sort_by_key(|i| i.channel_number.unwrap_or(u32::MAX));
    out
}

/// `{ channels: [ { channelId, schedule: [ { assetId, start, end } ] } ],
/// assets: { "<assetId>": { title, descriptions: { small, medium, tiny } } } }`
/// Keyed by item id ("xumo:<channelId>").
pub fn parse_epg(v: &Value) -> HashMap<String, Vec<Program>> {
    let assets = v.get("assets");
    let mut out = HashMap::new();
    for ch in arr(v, &["channels"]) {
        let Some(cid) = str_of(ch, &["channelId"]) else {
            continue;
        };
        let mut progs = Vec::new();
        for s in arr(ch, &["schedule"]) {
            let (Some(start), Some(stop)) = (
                str_of(s, &["start"]).and_then(|t| utc(&t)),
                str_of(s, &["end"]).and_then(|t| utc(&t)),
            ) else {
                continue;
            };
            let asset = str_of(s, &["assetId"]).and_then(|a| assets?.get(a.as_str()));
            let Some(title) = asset.and_then(|a| str_of(a, &["title"])) else {
                continue;
            };
            if stop <= start {
                continue;
            }
            let description = asset
                .and_then(|a| a.get("descriptions"))
                .and_then(|d| str_of(d, &["medium", "small", "tiny", "large"]));
            progs.push(Program {
                start,
                stop,
                title,
                subtitle: None,
                description: clean(description),
                rating: None,
                image: None,
            });
        }
        if !progs.is_empty() {
            out.insert(format!("xumo:{cid}"), progs);
        }
    }
    out
}

/// "2026-11-20T00:00:00+0000". Only UTC times are read; anything else is
/// skipped rather than shown at the wrong hour.
fn utc(s: &str) -> Option<u64> {
    let zone = s.get(19..)?;
    let zone = zone.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if !matches!(zone, "" | "Z" | "+0000" | "+00:00" | "-0000") {
        return None;
    }
    iso_to_unix(s)
}

/// Xumo's own slugs spell out "&" and "+" ("law-and-order", "bloomberg-tv-plus").
fn slug(title: &str) -> String {
    crate::model::slugify(&title.replace('&', " and ").replace('+', " plus "))
}

/// (YYYYMMDD, block 0-3) for a Unix time, in UTC.
fn block_of(t: u64) -> (String, u64) {
    let (y, m, d) = civil(t / 86_400);
    (format!("{y:04}{m:02}{d:02}"), (t % 86_400) / (6 * 3600))
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_and_dates() {
        // 2026-11-20 19:47 UTC is block 3 of that day.
        assert_eq!(block_of(1_795_204_020), ("20261120".into(), 3));
        // Six hours on is just past midnight: block 0 of the next day.
        assert_eq!(block_of(1_795_204_020 + 6 * 3600), ("20261121".into(), 0));
        assert_eq!(block_of(0), ("19700101".into(), 0));
        assert_eq!(block_of(1_709_208_000), ("20240229".into(), 2));
    }

    #[test]
    fn only_utc_times() {
        assert_eq!(utc("2026-11-20T00:00:00+0000"), Some(1_795_132_800));
        assert_eq!(utc("2026-11-20T00:00:00Z"), Some(1_795_132_800));
        assert_eq!(utc("2026-11-20T00:00:00-0500"), None);
    }

    #[test]
    fn slugs_match_xumo() {
        assert_eq!(slug("Law & Order"), "law-and-order");
        assert_eq!(slug("Bloomberg TV+"), "bloomberg-tv-plus");
        assert_eq!(slug("Dateline 24/7"), "dateline-24-7");
    }
}
