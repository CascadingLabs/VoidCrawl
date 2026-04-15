//! Pool + session introspection.

use schemars::JsonSchema;
use serde::Serialize;

use crate::server::VoidCrawlServer;

#[derive(Debug, Serialize, JsonSchema)]
pub struct PoolStatus {
    pub browsers:          usize,
    pub tabs_per_browser:  usize,
    pub max_tabs:          usize,
    pub tab_max_uses:      u32,
    pub tab_max_idle_secs: u64,
    pub sessions_open:     usize,
}

pub async fn pool_status(server: &VoidCrawlServer) -> PoolStatus {
    let cfg = server.state().pool.config();
    let sessions_open = server.state().sessions.len().await;
    PoolStatus {
        browsers: cfg.browsers,
        tabs_per_browser: cfg.tabs_per_browser,
        max_tabs: cfg.browsers.saturating_mul(cfg.tabs_per_browser),
        tab_max_uses: cfg.tab_max_uses,
        tab_max_idle_secs: cfg.tab_max_idle_secs,
        sessions_open,
    }
}
