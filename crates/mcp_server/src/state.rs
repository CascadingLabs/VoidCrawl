//! Shared application state carried inside the MCP server.

use std::sync::Arc;

use tokio::sync::OnceCell;
use void_crawl_core::{BrowserPool, Result};

use crate::sessions::SessionRegistry;

/// Bundle of shared state passed into the `VoidCrawlServer`. Cheap to
/// clone (two `Arc`s).
///
/// The `BrowserPool` is lazy: Chrome is not launched until the first tool
/// call that needs it. This keeps the MCP `initialize` handshake fast so the
/// harness can register tools without timing out on browser startup.
#[derive(Debug, Clone)]
pub struct AppState {
    pool:         Arc<OnceCell<Arc<BrowserPool>>>,
    pub sessions: Arc<SessionRegistry>,
}

impl AppState {
    pub fn new(sessions: Arc<SessionRegistry>) -> Self {
        Self { pool: Arc::new(OnceCell::new()), sessions }
    }

    /// Construct with a pre-built pool already initialized. Useful for tests
    /// that want to inject a specific `BrowserPool` instead of letting the
    /// state launch Chrome itself.
    pub fn with_pool(pool: Arc<BrowserPool>, sessions: Arc<SessionRegistry>) -> Self {
        let cell = OnceCell::new_with(Some(pool));
        Self { pool: Arc::new(cell), sessions }
    }

    /// Get the pool, launching Chrome on first call. Subsequent calls reuse
    /// the same pool instance.
    pub async fn pool(&self) -> Result<Arc<BrowserPool>> {
        self.pool
            .get_or_try_init(|| async {
                let pool = Arc::new(BrowserPool::from_env().await?);
                Arc::clone(&pool).start_eviction_task();
                Ok(pool)
            })
            .await
            .map(Arc::clone)
    }

    /// Returns the initialized pool, if any. Used on shutdown to close without
    /// forcing a launch.
    pub fn pool_if_initialized(&self) -> Option<Arc<BrowserPool>> {
        self.pool.get().map(Arc::clone)
    }
}
