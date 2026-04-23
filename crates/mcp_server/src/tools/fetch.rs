//! Stateless fetch over the shared `BrowserPool`.

use std::time::Duration;

use futures::future::join_all;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use void_crawl_core::{PooledTab, VoidCrawlError};

use crate::{server::VoidCrawlServer, tools::wait};

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct FetchArgs {
    /// Absolute URL to load.
    pub url:          String,
    /// Optional wait strategy: "networkidle" (default) or "selector:<css>".
    /// Both are event-driven — no polling, no sleeps.
    #[serde(default)]
    pub wait_for:     Option<String>,
    /// Optional JavaScript expression evaluated after the wait. Its
    /// return value is serialized into `extracted`.
    #[serde(default)]
    pub extract:      Option<String>,
    /// Navigation + wait timeout in seconds (default 30).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// JSON Schema helper: emit `{}` (any-value) instead of `true`.
/// Claude Code's validator rejects boolean schemas in outputSchema.properties.
fn any_value_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::Map::new().into()
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FetchResult {
    pub url:         String,
    pub status_code: Option<u16>,
    pub redirected:  bool,
    pub html:        String,
    pub title:       Option<String>,
    #[schemars(schema_with = "any_value_schema")]
    pub extracted:   Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct FetchManyArgs {
    /// List of fetch requests to run concurrently. The server's pool
    /// semaphore caps in-flight work — passing more URLs than the
    /// pool can serve simply queues the rest.
    pub requests: Vec<FetchArgs>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FetchManyItem {
    pub ok:     bool,
    pub result: Option<FetchResult>,
    pub error:  Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FetchManyResult {
    pub results: Vec<FetchManyItem>,
}

pub async fn run(server: &VoidCrawlServer, args: FetchArgs) -> Result<FetchResult, VoidCrawlError> {
    let pool = server.state().pool().await?;
    let tab = pool.acquire().await?;
    let result = fetch_on_tab(&tab, args).await;
    pool.release(tab).await;
    result
}

pub async fn run_many(server: &VoidCrawlServer, args: FetchManyArgs) -> FetchManyResult {
    let futures = args.requests.into_iter().map(|req| run(server, req));
    let outcomes = join_all(futures).await;
    let results = outcomes
        .into_iter()
        .map(|r| match r {
            Ok(result) => FetchManyItem { ok: true, result: Some(result), error: None },
            Err(e) => FetchManyItem { ok: false, result: None, error: Some(e.to_string()) },
        })
        .collect();
    FetchManyResult { results }
}

async fn fetch_on_tab(tab: &PooledTab, args: FetchArgs) -> Result<FetchResult, VoidCrawlError> {
    use tokio::time::{Instant, timeout};

    let total_timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let start = Instant::now();
    let resp = tab.page.goto_and_wait_for_idle(&args.url, total_timeout).await?;
    wait::apply_post_navigate(&tab.page, args.wait_for.as_deref(), total_timeout).await?;
    let remaining = total_timeout.saturating_sub(start.elapsed());
    // Cap title + extract JS at the remaining budget so a runaway
    // user-supplied `extract` (e.g. `while(1){}`) can't pin a pool tab
    // indefinitely — a hung script would otherwise survive every
    // `release`/`acquire` cycle and eventually drain the pool.
    let title = timeout(remaining, tab.page.title())
        .await
        .map_err(|_| VoidCrawlError::Timeout("title read exceeded timeout_secs".into()))?
        .ok()
        .flatten();
    let extracted = match args.extract {
        Some(js) => {
            let value = timeout(remaining, tab.page.evaluate_js(&js)).await.map_err(|_| {
                VoidCrawlError::Timeout(format!(
                    "extract evaluate_js exceeded {}s",
                    total_timeout.as_secs()
                ))
            })??;
            Some(value)
        }
        None => None,
    };
    Ok(FetchResult {
        url: resp.url,
        status_code: resp.status_code,
        redirected: resp.redirected,
        html: resp.html,
        title,
        extracted,
    })
}
