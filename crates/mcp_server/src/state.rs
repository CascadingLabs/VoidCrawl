//! Shared application state carried inside the MCP server.

use std::sync::Arc;

use void_crawl_core::BrowserPool;

use crate::sessions::SessionRegistry;

/// Bundle of shared state passed into the `VoidCrawlServer`. Cheap to
/// clone (two `Arc`s).
#[derive(Debug, Clone)]
pub struct AppState {
    pub pool:     Arc<BrowserPool>,
    pub sessions: Arc<SessionRegistry>,
}

impl AppState {
    pub fn new(pool: Arc<BrowserPool>, sessions: Arc<SessionRegistry>) -> Self {
        Self { pool, sessions }
    }
}
