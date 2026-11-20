//! Holds every provider's catalog in memory, refreshes it on a timer, and
//! answers searches by combining the local catalogs with live per-query
//! searches against the providers that are too big to download.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::future::join_all;
use reqwest::Client;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

use crate::model::{Item, Kind, LiveEntry, Program};
use crate::providers::Provider;
use crate::search::{Indexed, Query, rank};

const SEARCH_TIMEOUT: Duration = Duration::from_secs(7);
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const SEARCH_CACHE_MAX: usize = 500;
const GUIDE_EVERY: Duration = Duration::from_secs(20 * 60);
/// How far ahead the guide view shows.
const GUIDE_AHEAD_SECS: u64 = 6 * 3600;
const GUIDE_MAX_PROGRAMS: usize = 8;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Status {
    pub id: String,
    pub name: String,
    /// "catalog", "search" or "catalog+search".
    pub mode: String,
    pub catalog_items: usize,
    /// Unix seconds of the last catalog download that worked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_ok_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_ok_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_error: Option<String>,
    /// Live channels that currently have a schedule.
    #[serde(skip_serializing_if = "is_zero")]
    pub guide_channels: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guide_error: Option<String>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Default)]
struct State {
    catalogs: HashMap<String, Vec<Indexed>>,
    status: HashMap<String, Status>,
    /// provider id -> item id -> upcoming programs.
    guides: HashMap<String, HashMap<String, Vec<Program>>>,
}

pub struct Store {
    providers: Vec<Arc<dyn Provider>>,
    http: Client,
    state: RwLock<State>,
    search_cache: Mutex<HashMap<(String, String), (Instant, Vec<Item>)>>,
}

#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub kind: Option<Kind>,
    pub provider: Option<String>,
}

impl Filter {
    fn allows_provider(&self, id: &str) -> bool {
        self.provider.as_deref().is_none_or(|p| p == id)
    }
    fn allows(&self, item: &Item) -> bool {
        self.allows_provider(&item.provider) && self.kind.is_none_or(|k| k == item.kind)
    }
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub query: String,
    pub items: Vec<Item>,
    /// Display names of providers whose live search failed this time.
    pub unavailable: Vec<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Keep at most `limit` items across groups without letting the first group
/// crowd out the rest: each group gets an equal share, and any share a group
/// can't fill goes to the others. Group order and order within each group
/// are kept.
fn fair_truncate<T>(groups: Vec<Vec<T>>, limit: usize) -> Vec<T> {
    let mut take = vec![0; groups.len()];
    let mut left = limit;
    loop {
        let open: Vec<usize> = (0..groups.len())
            .filter(|&i| take[i] < groups[i].len())
            .collect();
        if open.is_empty() || left == 0 {
            break;
        }
        let share = (left / open.len()).max(1);
        for i in open {
            let n = share.min(groups[i].len() - take[i]).min(left);
            take[i] += n;
            left -= n;
            if left == 0 {
                break;
            }
        }
    }
    groups
        .into_iter()
        .zip(take)
        .flat_map(|(g, n)| g.into_iter().take(n))
        .collect()
}

fn short(e: &anyhow::Error) -> String {
    let s = format!("{e:#}");
    if s.len() > 300 {
        format!("{}…", &s[..s.floor_char_boundary(300)])
    } else {
        s
    }
}

impl Store {
    pub fn new(providers: Vec<Arc<dyn Provider>>, http: Client) -> Arc<Store> {
        let mut state = State::default();
        for p in &providers {
            let mode = match (p.has_catalog(), p.has_search()) {
                (true, true) => "catalog+search",
                (true, false) => "catalog",
                _ => "search",
            };
            state.status.insert(
                p.id().to_string(),
                Status {
                    id: p.id().into(),
                    name: p.name().into(),
                    mode: mode.into(),
                    ..Default::default()
                },
            );
        }
        Arc::new(Store {
            providers,
            http,
            state: RwLock::new(state),
            search_cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn providers(&self) -> &[Arc<dyn Provider>] {
        &self.providers
    }

    fn name_of(&self, id: &str) -> String {
        self.providers
            .iter()
            .find(|p| p.id() == id)
            .map(|p| p.name().to_string())
            .unwrap_or_default()
    }

    /// Download one provider's catalog. On failure the previous catalog is
    /// kept, so a provider having a bad hour doesn't empty the app.
    pub async fn refresh(&self, p: &Arc<dyn Provider>) {
        if !p.has_catalog() {
            return;
        }
        let started = Instant::now();
        let res = p.catalog(&self.http).await;
        let mut st = self.state.write().await;
        let status = st.status.entry(p.id().to_string()).or_default();
        match res {
            Ok(items) if !items.is_empty() => {
                status.catalog_items = items.len();
                status.catalog_ok_at = Some(now());
                status.catalog_error = None;
                tracing::info!(
                    "{}: {} items in {:?}",
                    p.id(),
                    items.len(),
                    started.elapsed()
                );
                let indexed = items.into_iter().map(Indexed::new).collect();
                st.catalogs.insert(p.id().to_string(), indexed);
            }
            Ok(_) => {
                status.catalog_error =
                    Some("returned no items (the format may have changed)".into());
                tracing::warn!("{}: catalog came back empty", p.id());
            }
            Err(e) => {
                status.catalog_error = Some(short(&e));
                tracing::warn!("{}: catalog failed: {e:#}", p.id());
            }
        }
    }

    pub async fn refresh_all(&self) {
        join_all(self.providers.iter().map(|p| self.refresh(p))).await;
        self.refresh_guides().await;
    }

    /// Download schedules for every provider that has one. On failure the
    /// previous schedule is kept (it simply runs out, never goes blank).
    pub async fn refresh_guides(&self) {
        join_all(self.providers.iter().filter(|p| p.has_guide()).map(|p| async move {
            let channels: Vec<Item> = {
                let st = self.state.read().await;
                st.catalogs
                    .get(p.id())
                    .map(|c| {
                        c.iter()
                            .filter(|i| i.item.kind == Kind::Live)
                            .map(|i| i.item.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            };
            if channels.is_empty() {
                return;
            }
            let res = p.guide(&self.http, &channels).await;
            let mut st = self.state.write().await;
            match res {
                Ok(map) if !map.is_empty() => {
                    tracing::info!("{}: schedule for {} channels", p.id(), map.len());
                    let status = st.status.entry(p.id().to_string()).or_default();
                    status.guide_channels = map.len();
                    status.guide_error = None;
                    st.guides.insert(p.id().to_string(), map);
                }
                Ok(_) => {
                    let status = st.status.entry(p.id().to_string()).or_default();
                    status.guide_error = Some("returned no schedule".into());
                }
                Err(e) => {
                    tracing::warn!("{}: schedule failed: {e:#}", p.id());
                    let status = st.status.entry(p.id().to_string()).or_default();
                    status.guide_error = Some(short(&e));
                }
            }
        }))
        .await;
    }

    /// Refresh every `every`, forever.
    pub fn spawn_refresher(self: &Arc<Self>, every: Duration) {
        let store = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                store.refresh_all().await;
                tokio::time::sleep(every).await;
            }
        });
        // Schedules go stale much faster than channel lists.
        let store = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(GUIDE_EVERY).await;
                store.refresh_guides().await;
            }
        });
    }

    pub async fn statuses(&self) -> Vec<Status> {
        let st = self.state.read().await;
        self.providers
            .iter()
            .filter_map(|p| st.status.get(p.id()).cloned())
            .collect()
    }

    /// Live channels from the downloaded catalogs, for browsing with no query.
    /// Grouped by provider, each in channel order. If there are more than
    /// `limit`, every provider still gets a fair share (see `fair_truncate`).
    pub async fn live(&self, filter: &Filter, limit: usize) -> Vec<Item> {
        let st = self.state.read().await;
        let mut groups = Vec::new();
        for p in &self.providers {
            if !filter.allows_provider(p.id()) {
                continue;
            }
            if let Some(cat) = st.catalogs.get(p.id()) {
                let mut live: Vec<Item> = cat
                    .iter()
                    .filter(|i| i.item.kind == Kind::Live)
                    .map(|i| i.item.clone())
                    .collect();
                live.sort_by_key(|i| {
                    (i.channel_number.unwrap_or(u32::MAX), i.title.to_lowercase())
                });
                groups.push(live);
            }
        }
        fair_truncate(groups, limit)
    }

    /// Live channels with what's on now and next, for the TV-guide view.
    pub async fn live_guide(&self, filter: &Filter, limit: usize) -> Vec<LiveEntry> {
        let items = self.live(filter, limit).await;
        let st = self.state.read().await;
        let t = now();
        items
            .into_iter()
            .map(|item| {
                let programs = st
                    .guides
                    .get(&item.provider)
                    .and_then(|g| g.get(&item.id))
                    .map(|ps| {
                        ps.iter()
                            .filter(|p| p.stop > t && p.start < t + GUIDE_AHEAD_SECS)
                            .take(GUIDE_MAX_PROGRAMS)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                LiveEntry { item, programs }
            })
            .collect()
    }

    async fn remote_search(&self, p: &Arc<dyn Provider>, q: &str) -> Result<Vec<Item>, ()> {
        let key = (p.id().to_string(), q.to_string());
        if let Some((at, items)) = self.search_cache.lock().await.get(&key)
            && at.elapsed() < SEARCH_CACHE_TTL
        {
            return Ok(items.clone());
        }
        let res = match tokio::time::timeout(SEARCH_TIMEOUT, p.search(&self.http, q)).await {
            Ok(r) => r,
            Err(_) => Err(anyhow::anyhow!("timed out after {:?}", SEARCH_TIMEOUT)),
        };
        {
            let mut st = self.state.write().await;
            let status = st.status.entry(p.id().to_string()).or_default();
            match &res {
                Ok(_) => {
                    status.search_ok_at = Some(now());
                    status.search_error = None;
                }
                Err(e) => status.search_error = Some(short(e)),
            }
        }
        match res {
            Ok(items) => {
                let mut cache = self.search_cache.lock().await;
                if cache.len() >= SEARCH_CACHE_MAX {
                    cache.retain(|_, (at, _)| at.elapsed() < SEARCH_CACHE_TTL);
                    if cache.len() >= SEARCH_CACHE_MAX {
                        cache.clear();
                    }
                }
                cache.insert(key, (Instant::now(), items.clone()));
                Ok(items)
            }
            Err(e) => {
                tracing::warn!("{}: search failed: {e:#}", p.id());
                Err(())
            }
        }
    }

    pub async fn search(&self, raw: &str, filter: &Filter, limit: usize) -> SearchResult {
        let q = Query::new(raw);
        let mut result = SearchResult {
            query: q.text.clone(),
            items: Vec::new(),
            unavailable: Vec::new(),
        };
        if q.is_empty() {
            return result;
        }

        // Live searches (only worth it from 2 characters up).
        let remote: Vec<_> = self
            .providers
            .iter()
            .filter(|p| p.has_search() && filter.allows_provider(p.id()) && q.text.len() >= 2)
            .collect();
        let remote_results = join_all(remote.iter().map(|p| self.remote_search(p, &q.text))).await;

        let mut scored: Vec<(u32, Item)> = Vec::new();
        {
            let st = self.state.read().await;
            for (pid, cat) in &st.catalogs {
                if !filter.allows_provider(pid) {
                    continue;
                }
                for (s, ix) in rank(cat.iter(), &q) {
                    if filter.allows(&ix.item) {
                        scored.push((s, ix.item.clone()));
                    }
                }
            }
        }
        // A provider with both (Roku's live channels) can return the same
        // item twice; the downloaded copy has the channel number and schedule.
        let from_catalog: std::collections::HashSet<String> =
            scored.iter().map(|(_, i)| i.id.clone()).collect();
        for (p, res) in remote.iter().zip(remote_results) {
            match res {
                Ok(items) => {
                    for (pos, item) in items.into_iter().enumerate() {
                        if !filter.allows(&item) || from_catalog.contains(&item.id) {
                            continue;
                        }
                        let ix = Indexed::new(item);
                        // The provider matched it for a reason we can't see
                        // (cast, alternate title), so keep it, ranked lower.
                        let s = crate::search::score(&ix, &q)
                            .max(60u32.saturating_sub(pos as u32).max(1));
                        scored.push((s, ix.item));
                    }
                }
                Err(()) => result.unavailable.push(self.name_of(p.id())),
            }
        }

        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.title.len().cmp(&b.1.title.len())));
        let mut seen = std::collections::HashSet::new();
        result.items = scored
            .into_iter()
            .map(|(_, i)| i)
            .filter(|i| seen.insert(i.id.clone()))
            .take(limit)
            .collect();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::fair_truncate;

    #[test]
    fn fair_truncate_shares_the_limit() {
        // Under the limit: everything, in order.
        assert_eq!(fair_truncate(vec![vec![1, 2], vec![3]], 10), vec![1, 2, 3]);
        // A big first group can't crowd out the second.
        let big: Vec<u32> = (0..450).collect();
        let out = fair_truncate(vec![big, vec![1000, 1001, 1002]], 400);
        assert_eq!(out.len(), 400);
        assert_eq!(&out[397..], &[1000, 1001, 1002], "small group kept whole");
        assert_eq!(out[396], 396, "big group gets the rest, in order");
        // Even split when both are big.
        let out = fair_truncate(vec![vec![0; 10], vec![1; 10]], 7);
        assert_eq!(out.iter().filter(|&&x| x == 0).count(), 4);
        assert_eq!(out.iter().filter(|&&x| x == 1).count(), 3);
        assert!(fair_truncate(vec![vec![1, 2]], 0).is_empty());
        assert!(fair_truncate::<u8>(vec![], 5).is_empty());
    }
}
