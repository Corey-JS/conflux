//! Conflux server.
//!
//!   conflux              run the server (same as `conflux serve`)
//!   conflux check [q]    fetch every provider once, search for q, print what worked

use std::path::PathBuf;
use std::time::Duration;

use conflux::catalog::{Filter, Store};
use conflux::providers::{self, DEFAULT_IDS};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn provider_ids() -> Vec<String> {
    let raw = env_or("CONFLUX_PROVIDERS", &DEFAULT_IDS.join(","));
    raw.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `check` prints its own report, so keep the log quiet there.
    let default_log = if args.first().map(String::as_str) == Some("check") {
        "error"
    } else {
        "conflux=info,warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_log.into()),
        )
        .init();

    let store = Store::new(providers::build(&provider_ids()), providers::http_client());

    match args.first().map(String::as_str) {
        Some("check") => {
            let q = if args.len() > 1 {
                args[1..].join(" ")
            } else {
                "star trek".into()
            };
            std::process::exit(check(&store, &q).await);
        }
        None | Some("serve") => serve(store).await,
        Some(other) => {
            eprintln!("unknown command '{other}'. Use: conflux [serve | check [query]]");
            std::process::exit(2);
        }
    }
}

async fn serve(store: std::sync::Arc<Store>) -> anyhow::Result<()> {
    let bind = env_or("CONFLUX_BIND", "127.0.0.1:8080");
    let web_dir = PathBuf::from(env_or("CONFLUX_WEB_DIR", "web"));
    let minutes: u64 = env_or("CONFLUX_REFRESH_MINUTES", "60")
        .parse()
        .unwrap_or(60)
        .max(5);

    if !web_dir.join("index.html").exists() {
        tracing::warn!(
            "no index.html in {} (set CONFLUX_WEB_DIR)",
            web_dir.display()
        );
    }
    store.spawn_refresher(Duration::from_secs(minutes * 60));

    let app = conflux::api::router(store, web_dir);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("listening on http://{bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// Returns a process exit code: 0 if every provider returned something.
async fn check(store: &Store, q: &str) -> i32 {
    println!("Fetching catalogs…");
    store.refresh_all().await;
    let result = store.search(q, &Filter::default(), 200).await;

    let mut failures = 0;
    for s in store.statuses().await {
        let found = result.items.iter().filter(|i| i.provider == s.id).count();
        let samples: Vec<&str> = result
            .items
            .iter()
            .filter(|i| i.provider == s.id)
            .take(3)
            .map(|i| i.title.as_str())
            .collect();
        let catalog_bad = s.mode.contains("catalog") && s.catalog_items == 0;
        let search_bad = s.mode.contains("search") && s.search_error.is_some();
        let ok = !catalog_bad && !search_bad;
        if !ok {
            failures += 1;
        }
        println!(
            "\n{} {} ({})",
            if ok { "OK  " } else { "FAIL" },
            s.name,
            s.mode
        );
        if s.mode.contains("catalog") {
            println!("     catalog: {} items", s.catalog_items);
            if let Some(e) = &s.catalog_error {
                println!("     catalog error: {e}");
            }
        }
        if let Some(e) = &s.search_error {
            println!("     search error: {e}");
        }
        println!("     \"{q}\": {found} results {samples:?}");
    }
    println!();
    if failures == 0 { 0 } else { 1 }
}
