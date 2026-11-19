//! Samsung TV Plus: live channels, downloaded whole. OFF BY DEFAULT.
//!
//! Samsung publishes no web catalog we can read, so this reads the channel
//! list that the open-source "i.mjh.nz" project mirrors from Samsung's apps.
//! Samsung's own website reportedly closed in Dec 2024 and there is no iPhone
//! app, so on a phone there is nowhere to send a tap. Turn it on with
//! CONFLUX_PROVIDERS if that changes.

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use std::io::Read;

use super::Provider;
use super::json::{at, str_of, u32_of};
use crate::model::{Item, Kind, clean, is_web_url};

const LIST_URL: &str = "https://i.mjh.nz/SamsungTVPlus/.channels.json.gz";

pub struct Samsung {
    region: String,
}

impl Samsung {
    pub fn from_env() -> Samsung {
        Samsung {
            region: std::env::var("CONFLUX_SAMSUNG_REGION").unwrap_or_else(|_| "us".into()),
        }
    }
}

#[async_trait]
impl Provider for Samsung {
    fn id(&self) -> &'static str {
        "samsung"
    }
    fn name(&self) -> &'static str {
        "Samsung TV Plus"
    }
    fn has_catalog(&self) -> bool {
        true
    }
    async fn catalog(&self, http: &Client) -> Result<Vec<Item>> {
        let resp = http.get(LIST_URL).send().await?.error_for_status()?;
        let bytes = resp.bytes().await?;
        let v = decode(&bytes).context("Samsung channel list")?;
        Ok(parse_channels(&v, &self.region))
    }
}

/// The file is gzip; some servers already undo that for us.
fn decode(bytes: &[u8]) -> Result<Value> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut s = String::new();
        flate2::read::GzDecoder::new(bytes).read_to_string(&mut s)?;
        Ok(serde_json::from_str(&s)?)
    } else {
        Ok(serde_json::from_slice(bytes)?)
    }
}

/// `{ regions: { us: { channels: { "<id>": { name, chno, group, logo, description } } } } }`
pub fn parse_channels(v: &Value, region: &str) -> Vec<Item> {
    let Some(Value::Object(chans)) = at(v, &["regions", region, "channels"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, c) in chans {
        let Some(name) = str_of(c, &["name"]) else {
            continue;
        };
        let url = "https://www.samsungtvplus.com/".to_string();
        let mut item = Item::new("samsung", id, Kind::Live, &name, url);
        item.description = clean(str_of(c, &["description"]));
        item.channel_number = u32_of(c, &["chno"]);
        item.image = str_of(c, &["logo"]).filter(|u| is_web_url(u));
        item.genres = str_of(c, &["group"]).into_iter().collect();
        out.push(item);
    }
    out.sort_by_key(|i| i.channel_number.unwrap_or(u32::MAX));
    out
}
