//! HTTP API plus the static PWA.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::json;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::catalog::{Filter, Store};
use crate::model::Kind;

const MAX_QUERY_LEN: usize = 100;
const MAX_LIVE: usize = 2000;

#[derive(Deserialize, Default)]
pub struct Params {
    #[serde(default)]
    q: String,
    kind: Option<String>,
    provider: Option<String>,
    limit: Option<usize>,
}

fn filter(store: &Store, p: &Params) -> Result<Filter, Response> {
    let kind = match p.kind.as_deref().filter(|k| !k.is_empty() && *k != "all") {
        Some(k) => Some(Kind::parse(k).ok_or_else(|| bad("unknown kind"))?),
        None => None,
    };
    let provider = match p
        .provider
        .as_deref()
        .filter(|s| !s.is_empty() && *s != "all")
    {
        Some(id) => {
            if !store.providers().iter().any(|x| x.id() == id) {
                return Err(bad("unknown provider"));
            }
            Some(id.to_string())
        }
        None => None,
    };
    Ok(Filter { kind, provider })
}

fn bad(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

/// Who's asking, for the per-visitor search limit. Behind Caddy that's the
/// last X-Forwarded-For entry: the address Caddy itself saw (Caddy replaces
/// whatever a client sends, but the last entry is the safe one either way).
fn visitor(headers: &HeaderMap) -> String {
    headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .rfind(|s| !s.is_empty())
        .unwrap_or("direct")
        .to_string()
}

async fn search(State(store): State<Arc<Store>>, headers: HeaderMap, Query(p): Query<Params>) -> Response {
    if let Err(wait) = store.visitor_may_search(&visitor(&headers)) {
        let secs = wait.as_secs().max(1);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, secs.to_string())],
            Json(json!({ "error": "too many searches", "retry_after": secs })),
        )
            .into_response();
    }
    let f = match filter(&store, &p) {
        Ok(f) => f,
        Err(r) => return r,
    };
    if p.q.chars().count() > MAX_QUERY_LEN {
        return bad("query too long");
    }
    let limit = p.limit.unwrap_or(60).clamp(1, 200);
    Json(store.search(&p.q, &f, limit).await).into_response()
}

async fn live(State(store): State<Arc<Store>>, Query(p): Query<Params>) -> Response {
    let f = match filter(&store, &p) {
        Ok(f) => f,
        Err(r) => return r,
    };
    // Whole lineups by default: Pluto alone has 450+ channels, and a lower
    // cap used to hide every provider listed after it.
    let limit = p.limit.unwrap_or(MAX_LIVE).clamp(1, MAX_LIVE);
    let items = store.live_guide(&f, limit).await;
    Json(json!({ "items": items, "now": now_secs() })).into_response()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn providers(State(store): State<Arc<Store>>) -> Response {
    Json(json!({ "providers": store.statuses().await })).into_response()
}

/// A shrunken copy of a show's art (see `art`). The key comes from the
/// guide and stands for one image URL, so the copy can be cached for good.
async fn art(State(store): State<Arc<Store>>, Path(key): Path<String>) -> Response {
    match store.art().get(&key).await {
        Some(jpeg) => (
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "public, max-age=604800, immutable"),
            ],
            jpeg,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")]).into_response(),
    }
}

async fn health() -> &'static str {
    "ok"
}

pub fn router(store: Arc<Store>, web_dir: PathBuf) -> Router {
    let api = Router::new()
        .route("/api/search", get(search))
        .route("/api/live", get(live))
        .route("/api/providers", get(providers))
        .route("/healthz", get(health))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .with_state(store.clone());

    // Images change only with their URL, so these may be cached (unlike the
    // rest of the API).
    let art = Router::new()
        .route("/api/art/{key}", get(art))
        .with_state(store);

    // Small files; the service worker does the offline caching.
    let static_files = Router::new()
        .fallback_service(ServeDir::new(web_dir).append_index_html_on_directories(true))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ));

    api.merge(art)
        .merge(static_files)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(CompressionLayer::new())
}
