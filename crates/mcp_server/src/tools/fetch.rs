//! Stateless fetch over the shared `BrowserPool`.

use std::time::{Duration, Instant};

use futures::future::join_all;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use void_crawl_core::{AntibotVerdict, PooledTab, VoidCrawlError};

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

/// Anti-bot / CDN vendor fingerprint of the fetched response, surfaced so an
/// agent can route deterministically (rotate proxy/profile, go headful) instead
/// of retrying blind. The raw headers it was derived from are available on the
/// Python `PageResponse`; only the actionable verdict is surfaced here to keep
/// the agent response small. See `crate::antibot` (core).
#[derive(Debug, Serialize, JsonSchema)]
pub struct AntibotInfo {
    /// Canonical vendor tags detected (e.g. `cloudflare`, `datadome`), sorted.
    pub vendors:          Vec<String>,
    /// `true` when an active wall/challenge fired (rotate), vs. mere CDN
    /// presence (no action needed).
    pub challenged:       bool,
    /// Vendor whose challenge fired, when `challenged`.
    pub challenge_vendor: Option<String>,
    /// Signature corpus version the verdict was produced against.
    pub corpus_version:   String,
    /// Which tier matched: `none` / `headers` / `body`.
    pub evidence:         String,
}

impl From<void_crawl_core::AntibotVerdict> for AntibotInfo {
    fn from(v: void_crawl_core::AntibotVerdict) -> Self {
        let evidence = match v.evidence {
            void_crawl_core::AntibotEvidence::None => "none",
            void_crawl_core::AntibotEvidence::Headers => "headers",
            void_crawl_core::AntibotEvidence::Body => "body",
        };
        Self {
            vendors:          v.vendors,
            challenged:       v.challenged,
            challenge_vendor: v.challenge_vendor,
            corpus_version:   v.corpus_version.to_string(),
            evidence:         evidence.to_string(),
        }
    }
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
    /// Anti-bot / CDN vendor fingerprint, or `null` when no vendor was detected
    /// (or no network response was captured).
    pub antibot:     Option<AntibotInfo>,
    /// Milliseconds this request spent queued for a free pool tab before work
    /// began. ~0 means a tab was free immediately; a larger value means the
    /// pool was saturated and this request waited behind other in-flight work.
    pub waited_ms:   u64,
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

/// Batch-level concurrency summary so an agent driving `fetch_many` can see
/// whether it oversubscribed the pool and adjust — without a separate
/// `pool_status` round-trip.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PoolMeta {
    /// Server concurrency ceiling: `browsers × tabs_per_browser`.
    pub max_tabs:      usize,
    /// Requests submitted in this batch.
    pub submitted:     usize,
    /// How many of them had to queue for a tab (waited measurably for a
    /// permit). `0` means everything ran fully in parallel.
    pub queued:        usize,
    /// Worst per-request queue wait observed in the batch, milliseconds.
    pub max_waited_ms: u64,
    /// Present only when the batch oversubscribed the pool — a plain-language
    /// hint the agent can act on (cap the next batch at `max_tabs`, or raise
    /// the pool size).
    pub note:          Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FetchManyResult {
    pub results: Vec<FetchManyItem>,
    pub pool:    PoolMeta,
}

/// Queue waits at or below this (scheduler jitter) don't count as "queued".
const QUEUE_WAIT_THRESHOLD_MS: u64 = 5;

pub async fn run(server: &VoidCrawlServer, args: FetchArgs) -> Result<FetchResult, VoidCrawlError> {
    run_timed(server, args).await.1
}

/// One fetch, returning the pool queue-wait alongside the result so callers
/// can report concurrency even when the request itself failed.
async fn run_timed(
    server: &VoidCrawlServer,
    args: FetchArgs,
) -> (u64, Result<FetchResult, VoidCrawlError>) {
    let pool = match server.state().pool().await {
        Ok(p) => p,
        Err(e) => return (0, Err(e)),
    };
    // On the error path (e.g. acquire timeout) the precise semaphore-only wait
    // isn't returned, so fall back to wall-clock around the acquire.
    let started = Instant::now();
    let (tab, waited_ms) = match pool.acquire_timed().await {
        Ok((tab, waited)) => (tab, waited),
        Err(e) => {
            let waited = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            return (waited, Err(e));
        }
    };
    let mut result = fetch_on_tab(&tab, args).await;
    pool.release(tab).await;
    if let Ok(ref mut r) = result {
        r.waited_ms = waited_ms;
    }
    (waited_ms, result)
}

pub async fn run_many(server: &VoidCrawlServer, args: FetchManyArgs) -> FetchManyResult {
    let submitted = args.requests.len();
    let max_tabs = server.state().pool().await.map_or(0, |p| {
        let c = p.config();
        c.browsers.saturating_mul(c.tabs_per_browser)
    });

    let futures = args.requests.into_iter().map(|req| run_timed(server, req));
    let outcomes = join_all(futures).await;

    let mut max_waited_ms = 0u64;
    let mut queued = 0usize;
    let results = outcomes
        .into_iter()
        .map(|(waited, r)| {
            max_waited_ms = max_waited_ms.max(waited);
            if waited > QUEUE_WAIT_THRESHOLD_MS {
                queued += 1;
            }
            match r {
                Ok(result) => FetchManyItem { ok: true, result: Some(result), error: None },
                Err(e) => {
                    FetchManyItem { ok: false, result: None, error: Some(e.to_string()) }
                }
            }
        })
        .collect();

    let note = (queued > 0 && max_tabs > 0).then(|| {
        format!(
            "{queued} of {submitted} requests queued behind the pool's {max_tabs}-tab limit \
             (worst wait {max_waited_ms}ms). For full parallelism, submit at most {max_tabs} \
             per batch, or raise TABS_PER_BROWSER / BROWSER_COUNT."
        )
    });

    FetchManyResult { results, pool: PoolMeta { max_tabs, submitted, queued, max_waited_ms, note } }
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
    // Surface the verdict only when a vendor was actually detected — keeps the
    // common (un-walled) response free of empty `antibot` noise.
    let antibot = resp.antibot.filter(AntibotVerdict::detected).map(AntibotInfo::from);
    Ok(FetchResult {
        url: resp.url,
        status_code: resp.status_code,
        redirected: resp.redirected,
        html: resp.html,
        title,
        extracted,
        antibot,
        // Overwritten by `run_timed` with the real pool queue-wait.
        waited_ms: 0,
    })
}
