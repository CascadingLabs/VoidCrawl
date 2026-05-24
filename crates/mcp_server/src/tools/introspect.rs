//! Pool + session introspection.

use schemars::JsonSchema;
use serde::Serialize;
use void_crawl_core::Result;

use crate::server::VoidCrawlServer;

#[derive(Debug, Serialize, JsonSchema)]
pub struct PoolStatus {
    pub browsers:          usize,
    pub tabs_per_browser:  usize,
    pub max_tabs:          usize,
    /// Free tab slots right now — how many fetches an agent can fan out
    /// without queueing. Read this before a big `fetch_many` to size the
    /// batch. A live snapshot, so it races with concurrent calls; treat it as
    /// guidance, not a reservation.
    pub available:         usize,
    /// Tabs currently checked out (`max_tabs - available`): in-flight fetches
    /// plus any held session tabs.
    pub in_flight:         usize,
    pub tab_max_uses:      u32,
    pub tab_max_idle_secs: u64,
    pub sessions_open:     usize,
}

pub async fn pool_status(server: &VoidCrawlServer) -> Result<PoolStatus> {
    let pool = server.state().pool().await?;
    let cfg = pool.config();
    let sessions_open = server.state().sessions.len().await;
    let max_tabs = cfg.browsers.saturating_mul(cfg.tabs_per_browser);
    let available = pool.available_permits().min(max_tabs);
    Ok(PoolStatus {
        browsers: cfg.browsers,
        tabs_per_browser: cfg.tabs_per_browser,
        max_tabs,
        available,
        in_flight: max_tabs.saturating_sub(available),
        tab_max_uses: cfg.tab_max_uses,
        tab_max_idle_secs: cfg.tab_max_idle_secs,
        sessions_open,
    })
}
