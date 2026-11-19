//! HTTP API plus the static PWA.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
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

async fn search(State(store): State<Arc<Store>>, Query(p): Query<Params>) -> Response {
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
    let limit = p.limit.unwrap_or(400).clamp(1, 2000);
    let mut items = store.live(&f).await;
    items.truncate(limit);
    Json(json!({ "items": items })).into_response()
}

async fn providers(State(store): State<Arc<Store>>) -> Response {
    Json(json!({ "providers": store.statuses().await })).into_response()
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
        .with_state(store);

    // Small files; the service worker does the offline caching.
    let static_files = Router::new()
        .fallback_service(ServeDir::new(web_dir).append_index_html_on_directories(true))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ));

    api.merge(static_files)
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
