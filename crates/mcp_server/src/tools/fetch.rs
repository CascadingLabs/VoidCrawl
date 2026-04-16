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

#[derive(Debug, Serialize, JsonSchema)]
pub struct FetchResult {
    pub url:         String,
    pub status_code: Option<u16>,
    pub redirected:  bool,
    pub html:        String,
    pub title:       Option<String>,
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
    let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let resp = tab.page.goto_and_wait_for_idle(&args.url, timeout).await?;
    wait::apply(&tab.page, args.wait_for.as_deref(), timeout).await?;
    let title = tab.page.title().await.ok().flatten();
    let extracted = match args.extract {
        Some(js) => Some(tab.page.evaluate_js(&js).await?),
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
