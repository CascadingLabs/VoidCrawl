//! Shared application state carried inside the MCP server.

use std::{
    env, fmt,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex as StdMutex},
};

use tokio::sync::OnceCell;
use void_crawl_core::{
    BrowserPool, BrowserSession, PoolConfig, ProfileHandle, Result, VoidCrawlError,
};

use crate::sessions::SessionRegistry;

/// Pre-acquired profile that pins the whole server.
///
/// Built by `main.rs` when the user passes `--profile NAME` or sets
/// `VOIDCRAWL_PROFILE`. The owned `BrowserSession` is moved into the
/// pool on first `AppState::pool()` call so every `fetch` /
/// `fetch_many` / `screenshot` tool call inherits the profile's
/// cookies. The `ProfileHandle` is kept alive (inside the `AppState`)
/// so its `fs2` advisory lock stays held for the server lifetime.
pub struct PinnedProfile {
    pub handle:         ProfileHandle,
    /// Extracted once, at AppState::pool() first-call time.
    pub session:        StdMutex<Option<BrowserSession>>,
    /// Mirrors the profile's Chrome-facing name for user_data_dir
    /// defaulting in `session_open`.
    pub name:           String,
    pub user_data_root: PathBuf,
}

impl fmt::Debug for PinnedProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PinnedProfile")
            .field("name", &self.name)
            .field("user_data_root", &self.user_data_root)
            .finish_non_exhaustive()
    }
}

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
    pub pinned:   Option<Arc<PinnedProfile>>,
}

impl AppState {
    pub fn new(sessions: Arc<SessionRegistry>) -> Self {
        Self { pool: Arc::new(OnceCell::new()), sessions, pinned: None }
    }

    /// Construct with a pre-acquired profile that will back the pool.
    pub fn with_pinned_profile(sessions: Arc<SessionRegistry>, pinned: PinnedProfile) -> Self {
        Self { pool: Arc::new(OnceCell::new()), sessions, pinned: Some(Arc::new(pinned)) }
    }

    /// Construct with a pre-built pool already initialized. Useful for tests
    /// that want to inject a specific `BrowserPool` instead of letting the
    /// state launch Chrome itself.
    pub fn with_pool(pool: Arc<BrowserPool>, sessions: Arc<SessionRegistry>) -> Self {
        let cell = OnceCell::new_with(Some(pool));
        Self { pool: Arc::new(cell), sessions, pinned: None }
    }

    /// Get the pool, launching Chrome on first call. Subsequent calls reuse
    /// the same pool instance.
    ///
    /// When a profile is pinned, the pool is built from the pinned
    /// session — `browsers=1` (Chrome's SingletonLock prevents a second
    /// process on the same user_data_dir), `tabs_per_browser` still
    /// gives real concurrency. Without a pinned profile, the old
    /// `BrowserPool::from_env()` path runs unchanged.
    pub async fn pool(&self) -> Result<Arc<BrowserPool>> {
        let pinned = self.pinned.clone();
        self.pool
            .get_or_try_init(|| async move {
                let pool = if let Some(p) = pinned {
                    // Take the session out of the PinnedProfile. The
                    // handle (and its lock guard) stays alive inside
                    // `self.pinned` for the server lifetime.
                    let session = p
                        .session
                        .lock()
                        .map_err(|_| {
                            VoidCrawlError::Other("PinnedProfile session mutex poisoned".into())
                        })?
                        .take()
                        .ok_or_else(|| {
                            VoidCrawlError::Other("PinnedProfile session already consumed".into())
                        })?;
                    Arc::new(BrowserPool::new(pool_config_from_env(), vec![session]))
                } else {
                    Arc::new(BrowserPool::from_env().await?)
                };
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

/// Read the subset of `PoolConfig` values from env that apply to a
/// pinned-profile pool. Matches `BrowserPool::from_env` for these
/// knobs but ignores `BROWSER_COUNT` / `CHROME_WS_URLS` (single Chrome,
/// no attach).
fn pool_config_from_env() -> PoolConfig {
    fn parse<T: FromStr>(key: &str, default: T) -> T {
        env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
    }
    PoolConfig {
        browsers:             1,
        tabs_per_browser:     parse("TABS_PER_BROWSER", 4),
        tab_max_uses:         parse("TAB_MAX_USES", 50),
        tab_max_idle_secs:    parse("TAB_MAX_IDLE_SECS", 60),
        acquire_timeout_secs: parse("ACQUIRE_TIMEOUT_SECS", 30),
        auto_evict:           env::var("AUTO_EVICT").map_or(true, |v| v != "0"),
    }
}
