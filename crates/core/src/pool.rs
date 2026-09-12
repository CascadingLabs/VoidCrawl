//! `BrowserPool` — a pool of reusable browser tabs backed by long-lived Chrome
//! sessions.
//!
//! The pool creates tabs **lazily** on first `acquire()` and recycles them on
//! `release()`. Release clears the live document before a tab re-enters the
//! ready queue while intentionally retaining shared profile/origin state.
//! Hard recycling (close + reopen) kicks in after `tab_max_uses`, and idle
//! eviction cleans up stale tabs.
//!
//! `warmup()` is **optional** — calling it pre-creates tabs for faster first
//! acquires, but the pool works correctly without it.

use std::{
    collections::VecDeque,
    env, fmt,
    sync::{
        Arc, Mutex as StdMutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use futures::future;
use serde::Serialize;
use tokio::{
    runtime::Handle,
    sync::{Mutex, Notify, OwnedSemaphorePermit, RwLock, Semaphore},
    task::JoinHandle,
    time::{interval, timeout},
};

/// Shutdown is bounded so a forgotten checkout cannot hang process teardown.
/// After this interval sessions are closed even if a caller still holds a tab.
const POOL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const TAB_CLOSE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Default)]
struct CleanupTracker {
    active:  AtomicUsize,
    changed: Notify,
}

impl CleanupTracker {
    fn start(self: &Arc<Self>) -> CleanupGuard {
        self.active.fetch_add(1, Ordering::AcqRel);
        CleanupGuard(Arc::clone(self))
    }

    fn is_idle(&self) -> bool {
        self.active.load(Ordering::Acquire) == 0
    }
}

struct CleanupGuard(Arc<CleanupTracker>);

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
        self.0.changed.notify_waiters();
    }
}

use crate::{
    context_isolation::BrowserStateBinding,
    error::{Result, VoidCrawlError},
    page::Page,
    session::BrowserSession,
};

/// Configuration for a [`BrowserPool`].
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Number of Chrome processes (sessions) in the pool.
    pub browsers:             usize,
    /// Maximum concurrent tabs per browser session.
    pub tabs_per_browser:     usize,
    /// Close and reopen a tab after this many uses.
    pub tab_max_uses:         u32,
    /// Evict idle tabs after this many seconds.
    pub tab_max_idle_secs:    u64,
    /// Maximum seconds to wait for a tab in [`BrowserPool::acquire()`].
    ///
    /// When all tabs are checked out, `acquire()` blocks on the semaphore
    /// for at most this many seconds before returning
    /// [`VoidCrawlError::Timeout`].  `0` means wait indefinitely (the
    /// pre-v0.2 behaviour).
    pub acquire_timeout_secs: u64,
    /// Automatically run idle eviction in a background task.
    ///
    /// When `true` (the default), calling
    /// [`BrowserPool::spawn_eviction_task`] will start a Tokio task that
    /// calls [`BrowserPool::evict_idle`] every `tab_max_idle_secs / 2`
    /// seconds.  The task is cancelled when the handle returned by
    /// `spawn_eviction_task` is aborted (typically on pool close).
    pub auto_evict:           bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            browsers:             1,
            tabs_per_browser:     4,
            tab_max_uses:         50,
            tab_max_idle_secs:    60,
            acquire_timeout_secs: 30,
            auto_evict:           true,
        }
    }
}

/// A tab checked out from the pool.
///
/// Holds the underlying [`Page`] plus bookkeeping metadata.
/// Return it to the pool via [`BrowserPool::release()`].
pub struct PooledTab {
    /// The CDP page / tab.
    pub page:               Page,
    /// How many times this tab has been used (incremented on release).
    pub use_count:          u32,
    /// When this tab was last returned to the ready queue.
    pub last_used:          Instant,
    /// Index into `BrowserPool::sessions` identifying which browser owns this
    /// tab.
    pub(crate) browser_idx: usize,
    /// Checkout ownership. Dropping an unreleased tab restores pool capacity.
    permit:                 Option<OwnedSemaphorePermit>,
    cleanup_tracker:        Arc<CleanupTracker>,
}

impl PooledTab {
    /// Pooled tabs are reusable shared-profile pages, never isolated contexts.
    #[must_use]
    pub const fn state_binding(&self) -> BrowserStateBinding {
        self.page.state_binding()
    }
}

impl Drop for PooledTab {
    fn drop(&mut self) {
        if self.permit.is_none() {
            return;
        }
        let Ok(runtime) = Handle::try_current() else {
            return;
        };
        let page = self.page.clone_handle();
        let cleanup_guard = self.cleanup_tracker.start();
        runtime.spawn(async move {
            let _cleanup_guard = cleanup_guard;
            let _ = page.close().await;
        });
    }
}

impl fmt::Debug for PooledTab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledTab")
            .field("page", &self.page)
            .field("use_count", &self.use_count)
            .field("last_used", &self.last_used)
            .field("browser_idx", &self.browser_idx)
            .field("checked_out", &self.permit.is_some())
            .finish_non_exhaustive()
    }
}

/// Stable cleanup strategy used when returning a pooled tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolReleaseStrategy {
    /// Clear the live document and abandoned download behavior, while
    /// deliberately retaining browser-profile and page instrumentation state.
    BlankDocumentAndReuseSharedState,
    /// The tab could not be reset safely and was disposed instead of reused.
    DisposeTabAfterResetFailure,
    /// Shutdown began while the tab was being returned, so it was disposed.
    DisposeTabAfterPoolClosed,
}

/// Observable result of returning a shared-state tab to the pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[expect(clippy::struct_excessive_bools, reason = "independent cleanup facts are explicit")]
pub struct PoolReleaseReport {
    pub state_binding:           BrowserStateBinding,
    pub strategy:                PoolReleaseStrategy,
    pub cleanup_complete:        bool,
    pub tab_reused:              bool,
    pub document_cleared:        bool,
    pub download_behavior_reset: bool,
    /// Cookies, HTTP cache, local/session storage, IndexedDB, service workers,
    /// permissions, headers, viewport, and init scripts remain shared.
    pub shared_state_retained:   bool,
}

/// A pool of reusable browser tabs spread across one or more Chrome sessions.
///
/// Tabs are created lazily on first `acquire()`. Call
/// [`warmup()`](Self::warmup) to optionally pre-create tabs for faster first
/// acquires.
///
/// # Usage
///
/// ```rust,no_run
/// # async fn example() -> void_crawl_core::Result<()> {
/// use void_crawl_core::pool::BrowserPool;
///
/// let pool = BrowserPool::from_env().await?;
/// // warmup() is optional — tabs are created on demand
///
/// let tab = pool.acquire().await?;
/// tab.page.navigate("https://example.com").await?;
/// let html = tab.page.content().await?;
/// pool.release(tab).await;
///
/// pool.close().await?;
/// # Ok(())
/// # }
/// ```
pub struct BrowserPool {
    sessions:          Arc<Vec<BrowserSession>>,
    ready:             Arc<Mutex<VecDeque<PooledTab>>>,
    semaphore:         Arc<Semaphore>,
    config:            PoolConfig,
    /// Round-robin counter for distributing new tabs across sessions.
    next_session:      AtomicUsize,
    /// Background eviction task handle.  `StdMutex` (not tokio) because we
    /// only set/take the handle in sync contexts — no `.await` inside the lock.
    eviction_task:     Arc<StdMutex<Option<JoinHandle<()>>>>,
    /// Retained owned shutdown work, so cancelling a close caller cannot lose
    /// drained ready tabs or detach teardown.
    shutdown_task:     Mutex<Option<JoinHandle<Result<()>>>>,
    /// Prevents a checkout from crossing the boundary where shutdown begins.
    lifecycle:         Arc<RwLock<()>>,
    /// Shared with detached release workers so they cannot repopulate a closed
    /// pool.
    closed:            Arc<AtomicBool>,
    shutdown_complete: Arc<AtomicBool>,
    /// Includes detached release and abandoned-checkout cleanup workers.
    cleanup_tracker:   Arc<CleanupTracker>,
}

impl fmt::Debug for BrowserPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserPool")
            .field("config", &self.config)
            .field("sessions", &self.sessions.len())
            .finish_non_exhaustive()
    }
}

impl BrowserPool {
    /// Create a new pool from pre-built sessions and config.
    ///
    /// The pool starts with **no tabs** — they are created lazily on
    /// [`acquire()`](Self::acquire) or optionally pre-created via
    /// [`warmup()`](Self::warmup).
    pub fn new(config: PoolConfig, sessions: Vec<BrowserSession>) -> Self {
        let total_tabs = config.browsers * config.tabs_per_browser;
        Self {
            sessions: Arc::new(sessions),
            ready: Arc::new(Mutex::new(VecDeque::with_capacity(total_tabs))),
            // Permits = max concurrency. Tabs created lazily within this limit.
            semaphore: Arc::new(Semaphore::new(total_tabs)),
            config,
            next_session: AtomicUsize::new(0),
            eviction_task: Arc::new(StdMutex::new(None)),
            shutdown_task: Mutex::new(None),
            lifecycle: Arc::new(RwLock::new(())),
            closed: Arc::new(AtomicBool::new(false)),
            shutdown_complete: Arc::new(AtomicBool::new(false)),
            cleanup_tracker: Arc::new(CleanupTracker::default()),
        }
    }

    /// Build a pool from environment variables.
    ///
    /// | Variable | Description | Default |
    /// |---|---|---|
    /// | `CHROME_WS_URLS` | Comma-separated `ws://` or `http://` URLs (connect mode) | — |
    /// | `BROWSER_COUNT` | Number of Chrome processes to launch | `1` |
    /// | `TABS_PER_BROWSER` | Max concurrent tabs per browser | `4` |
    /// | `TAB_MAX_USES` | Hard recycle threshold | `50` |
    /// | `TAB_MAX_IDLE_SECS` | Idle eviction timeout | `60` |
    /// | `CHROME_NO_SANDBOX` | Set to `"1"` to pass `--no-sandbox` | — |
    /// | `CHROME_HEADLESS` | Set to `"0"` for headful mode | `1` |
    /// | `ACQUIRE_TIMEOUT_SECS` | Max seconds to wait in acquire() | `30` |
    /// | `VIEWPORT_WIDTH` | Stealth viewport width | `1920` |
    /// | `VIEWPORT_HEIGHT` | Stealth viewport height | `1080` |
    /// | `CDP_PORT_BASE` | Pin Chrome's `--remote-debugging-port` for launched browsers. Browser `i` uses `base + i`. Unset = OS-assigned (recommended; can't conflict). | — |
    pub async fn from_env() -> Result<Self> {
        let tabs_per_browser: usize =
            env::var("TABS_PER_BROWSER").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
        let tab_max_uses: u32 =
            env::var("TAB_MAX_USES").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
        let tab_max_idle_secs: u64 =
            env::var("TAB_MAX_IDLE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
        let acquire_timeout_secs: u64 =
            env::var("ACQUIRE_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
        let no_sandbox = env::var("CHROME_NO_SANDBOX").is_ok_and(|v| v == "1");
        let headless = env::var("CHROME_HEADLESS").ok().is_none_or(|v| v != "0");
        let viewport_width: Option<u32> =
            env::var("VIEWPORT_WIDTH").ok().and_then(|v| v.parse().ok());
        let viewport_height: Option<u32> =
            env::var("VIEWPORT_HEIGHT").ok().and_then(|v| v.parse().ok());
        // Leave `None` to use chromiumoxide's default (port 0 = OS-assigned),
        // which avoids every port-conflict failure mode. Only set this when
        // a firewall / container only exposes specific ports.
        let cdp_port_base: Option<u16> =
            env::var("CDP_PORT_BASE").ok().and_then(|v| v.parse().ok());

        let sessions = if let Ok(urls) = env::var("CHROME_WS_URLS") {
            // Connect mode: attach to pre-existing Chrome instances **in
            // parallel**
            let futs: Vec<_> = urls
                .split(',')
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(|url| BrowserSession::connect(url.to_string()))
                .collect();

            if futs.is_empty() {
                return Err(VoidCrawlError::Other(
                    "CHROME_WS_URLS is set but contains no valid URLs".into(),
                ));
            }

            let results = future::join_all(futs).await;
            results.into_iter().collect::<Result<Vec<_>>>()?
        } else {
            // Launch mode: start Chrome processes **in parallel**
            let browser_count: usize =
                env::var("BROWSER_COUNT").ok().and_then(|v| v.parse().ok()).unwrap_or(1);

            let futs: Vec<_> = (0..browser_count)
                .map(|i| {
                    let mut builder = if headless {
                        BrowserSession::builder().headless()
                    } else {
                        BrowserSession::builder().headful()
                    };
                    if no_sandbox {
                        builder = builder.no_sandbox();
                    }
                    if let (Some(w), Some(h)) = (viewport_width, viewport_height) {
                        builder = builder.viewport(w, h);
                    } else if let Some(w) = viewport_width {
                        builder = builder.viewport(w, 1080);
                    } else if let Some(h) = viewport_height {
                        builder = builder.viewport(1920, h);
                    }
                    // Browser N gets base+N so launching multiple browsers
                    // with a pinned base doesn't collide. `base + i` can
                    // overflow `u16::MAX`; clamp via `saturating_add` and
                    // let the OS reject it rather than wrap silently.
                    if let Some(base) = cdp_port_base {
                        builder =
                            builder.port(base.saturating_add(u16::try_from(i).unwrap_or(u16::MAX)));
                    }
                    builder.launch()
                })
                .collect();

            let results = future::join_all(futs).await;
            results.into_iter().collect::<Result<Vec<_>>>()?
        };

        let config = PoolConfig {
            browsers: sessions.len(),
            tabs_per_browser,
            tab_max_uses,
            tab_max_idle_secs,
            acquire_timeout_secs,
            auto_evict: true,
        };

        Ok(Self::new(config, sessions))
    }

    /// Pick the next session index (round-robin).
    fn next_browser_idx(&self) -> usize {
        if self.sessions.len() == 1 {
            return 0;
        }
        self.next_session.fetch_add(1, Ordering::Relaxed) % self.sessions.len()
    }

    /// Create a fresh tab on a round-robin browser session.
    async fn create_tab(&self) -> Result<PooledTab> {
        let idx = self.next_browser_idx();
        let page = self.sessions[idx].new_blank_page().await?;
        Ok(PooledTab {
            page,
            use_count: 0,
            last_used: Instant::now(),
            browser_idx: idx,
            permit: None,
            cleanup_tracker: Arc::clone(&self.cleanup_tracker),
        })
    }

    /// Optionally pre-open tabs across all sessions and fill the ready queue.
    ///
    /// Tabs are created **in parallel** across sessions, then inserted into
    /// the ready queue. Successful tabs are kept even when some fail; the
    /// first error (if any) is returned after all successful tabs are stored.
    ///
    /// This is **optional** — if not called, tabs are created lazily on
    /// first [`acquire()`](Self::acquire).
    pub async fn warmup(&self) -> Result<()> {
        let _lifecycle = self.lifecycle.read().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(VoidCrawlError::Other("browser pool is closed".into()));
        }
        // Build futures for all tabs across all sessions
        let mut futs = Vec::with_capacity(self.config.browsers * self.config.tabs_per_browser);
        for (idx, session) in self.sessions.iter().enumerate() {
            for _ in 0..self.config.tabs_per_browser {
                let cleanup_tracker = Arc::clone(&self.cleanup_tracker);
                futs.push(async move {
                    let page = session.new_blank_page().await?;
                    Ok::<_, VoidCrawlError>(PooledTab {
                        page,
                        use_count: 0,
                        last_used: Instant::now(),
                        browser_idx: idx,
                        permit: None,
                        cleanup_tracker,
                    })
                });
            }
        }

        // Create all tabs in parallel
        let results = future::join_all(futs).await;

        // The semaphore tracks checked-out tabs, not ready-queue occupancy.
        // Tabs in the ready queue are "available" — they hold no permit.
        // Just push successful tabs directly; failed tabs are skipped and
        // their capacity remains available for lazy creation via acquire().
        let mut ready = self.ready.lock().await;
        let mut first_err: Option<VoidCrawlError> = None;
        for result in results {
            match result {
                Ok(tab) => ready.push_back(tab),
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }

        // If any tabs failed, report the first error but keep the
        // successfully created tabs in the pool.
        drop(ready);
        if let Some(e) = first_err { Err(e) } else { Ok(()) }
    }

    /// Check out a tab from the pool.
    ///
    /// If an idle tab is available, it is returned immediately. Otherwise,
    /// a new tab is created on demand (up to `tabs_per_browser * browsers`
    /// total). Blocks only when all tabs are currently in use.
    ///
    /// Tabs that have exceeded `tab_max_uses` are silently hard-recycled.
    ///
    /// The semaphore permit is returned on every error path so that
    /// failures never permanently shrink pool concurrency.
    pub async fn acquire(&self) -> Result<PooledTab> {
        self.acquire_timed().await.map(|(tab, _waited_ms)| tab)
    }

    /// Like [`acquire`](Self::acquire), but also returns the milliseconds
    /// spent blocked on the concurrency semaphore — the *pure queueing wait*,
    /// excluding any lazy tab-creation latency that happens after a permit is
    /// granted. A near-zero value means a slot was free immediately; a
    /// non-trivial value means the caller queued behind other in-flight work.
    /// Surfaced to MCP clients so an agent can tell when it has oversubscribed
    /// the pool and should throttle or cap a batch at `max_tabs`.
    pub async fn acquire_timed(&self) -> Result<(PooledTab, u64)> {
        let _lifecycle = self.lifecycle.read().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(VoidCrawlError::Other("browser pool is closed".into()));
        }
        let wait_start = Instant::now();
        let permit = if self.config.acquire_timeout_secs == 0 {
            // No timeout — wait indefinitely (legacy behaviour).
            Arc::clone(&self.semaphore)
                .acquire_owned()
                .await
                .map_err(|_| VoidCrawlError::Other("pool semaphore closed".into()))?
        } else {
            let deadline = Duration::from_secs(self.config.acquire_timeout_secs);
            match timeout(deadline, Arc::clone(&self.semaphore).acquire_owned()).await {
                Ok(Ok(permit)) => permit,
                Ok(Err(_)) => {
                    return Err(VoidCrawlError::Other("pool semaphore closed".into()));
                }
                Err(_) => {
                    return Err(VoidCrawlError::Timeout(format!(
                        "pool.acquire() timed out after {}s — all {} tabs are checked out",
                        self.config.acquire_timeout_secs,
                        self.config.browsers * self.config.tabs_per_browser,
                    )));
                }
            }
        };
        if self.closed.load(Ordering::Acquire) {
            return Err(VoidCrawlError::Other("browser pool is closed".into()));
        }
        // Pure semaphore queueing time — captured before the ready-queue pop
        // and any lazy `create_tab()` round-trip, so tab-creation latency is
        // never misreported as contention.
        let waited_ms = u64::try_from(wait_start.elapsed().as_millis()).unwrap_or(u64::MAX);
        // Try the ready queue first (fast path: reuse an existing tab)
        let maybe_tab = {
            let mut ready = self.ready.lock().await;
            ready.pop_front()
        };

        let tab = match maybe_tab {
            Some(tab) => tab,
            // No idle tab — create one on demand (lazy growth)
            None => self.create_tab().await?,
        };

        // Hard recycle if this tab is worn out
        if tab.use_count >= self.config.tab_max_uses {
            let browser_idx = tab.browser_idx;
            let cleanup_tracker = Arc::clone(&tab.cleanup_tracker);
            let _ = tab.page.close().await;
            match self.sessions[browser_idx].new_blank_page().await {
                Ok(page) => {
                    return Ok((
                        PooledTab {
                            page,
                            use_count: 0,
                            last_used: Instant::now(),
                            browser_idx,
                            permit: Some(permit),
                            cleanup_tracker,
                        },
                        waited_ms,
                    ));
                }
                Err(e) => return Err(e),
            }
        }

        // No about:blank cleanup — the caller's navigate(url) will replace
        // the prior page content, and stealth scripts persist across
        // navigations. This saves 50-200ms of CDP round-trip per reused
        // tab.
        let mut tab = tab;
        tab.permit = Some(permit);
        Ok((tab, waited_ms))
    }

    /// Return a tab to the pool, discarding the observable cleanup report.
    ///
    /// Compatibility wrapper around [`release_checked`](Self::release_checked).
    pub async fn release(&self, tab: PooledTab) {
        let _ = self.release_checked(tab).await;
    }

    /// Return a tab and report whether it was safe to reuse.
    ///
    /// The live document and abandoned download behavior are reset. Origin and
    /// browser-profile state is intentionally retained: pooled tabs are a
    /// shared scraping primitive, not an isolation boundary. If either reset
    /// fails, the tab is closed and is never handed to the next caller; the
    /// semaphore permit is still restored so capacity can grow lazily again.
    pub async fn release_checked(&self, mut tab: PooledTab) -> PoolReleaseReport {
        tab.use_count += 1;
        tab.last_used = Instant::now();
        let state_binding = tab.state_binding();
        let ready = Arc::clone(&self.ready);
        let closed = Arc::clone(&self.closed);
        let permit = tab.permit.take();
        let cleanup_guard = self.cleanup_tracker.start();

        // Cleanup owns the checked-out slot in a detached task. Cancelling the
        // caller's await therefore cannot drop the tab before restoring pool
        // capacity. The permit guard also restores capacity if cleanup panics
        // or the runtime drops the task before its first poll.
        let worker = tokio::spawn(async move {
            let _cleanup_guard = cleanup_guard;
            let _permit = permit;
            let download_behavior_reset = if tab.page.is_download_armed() {
                tab.page.reset_download_behavior_checked().await.is_ok()
            } else {
                true
            };
            let document_cleared = tab.page.navigate("about:blank").await.is_ok();
            let reset_complete = download_behavior_reset && document_cleared;
            // The queue-lock recheck is authoritative: shutdown can begin
            // between the optimistic check and queue insertion, in which case
            // this tab is disposed and the report must say so.
            let (strategy, tab_reused) = if reset_complete {
                let mut ready = ready.lock().await;
                if closed.load(Ordering::Acquire) {
                    drop(ready);
                    let _ = tab.page.close().await;
                    (PoolReleaseStrategy::DisposeTabAfterPoolClosed, false)
                } else {
                    ready.push_back(tab);
                    (PoolReleaseStrategy::BlankDocumentAndReuseSharedState, true)
                }
            } else {
                let _ = tab.page.close().await;
                (PoolReleaseStrategy::DisposeTabAfterResetFailure, false)
            };

            PoolReleaseReport {
                state_binding,
                strategy,
                cleanup_complete: reset_complete,
                tab_reused,
                document_cleared,
                download_behavior_reset,
                shared_state_retained: true,
            }
        });

        worker.await.unwrap_or(PoolReleaseReport {
            state_binding,
            strategy: PoolReleaseStrategy::DisposeTabAfterResetFailure,
            cleanup_complete: false,
            tab_reused: false,
            document_cleared: false,
            download_behavior_reset: false,
            shared_state_retained: true,
        })
    }

    /// Close idle tabs that have exceeded `tab_max_idle_secs` and open fresh
    /// replacements.
    ///
    /// Intended to be called periodically from a background tokio task.
    pub async fn evict_idle(&self) -> Result<()> {
        let _lifecycle = self.lifecycle.read().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(VoidCrawlError::Other("browser pool is closed".into()));
        }
        let max_idle = Duration::from_secs(self.config.tab_max_idle_secs);
        let now = Instant::now();

        // Partition the ready queue into keep vs. evict
        let to_evict: Vec<PooledTab> = {
            let mut ready = self.ready.lock().await;
            let mut keep = VecDeque::with_capacity(ready.len());
            let mut evict = Vec::new();

            while let Some(tab) = ready.pop_front() {
                if now.duration_since(tab.last_used) > max_idle {
                    evict.push(tab);
                } else {
                    keep.push_back(tab);
                }
            }
            *ready = keep;
            evict
        };

        // Close evicted tabs and create replacements in parallel.
        //
        // If the owning session is dead (handler exited), skip the
        // replacement — closing the old tab and failing to replace it
        // would permanently shrink the pool.
        let futs: Vec<_> = to_evict
            .into_iter()
            .map(|tab| {
                let browser_idx = tab.browser_idx;
                let session = &self.sessions[browser_idx];
                let cleanup_tracker = Arc::clone(&tab.cleanup_tracker);
                async move {
                    if !session.is_alive() {
                        // Session is dead — return the old tab unchanged so
                        // the pool doesn't lose capacity.
                        return Ok::<_, VoidCrawlError>(tab);
                    }
                    let _ = tab.page.close().await;
                    match session.new_blank_page().await {
                        Ok(page) => Ok(PooledTab {
                            page,
                            use_count: 0,
                            last_used: Instant::now(),
                            browser_idx,
                            permit: None,
                            cleanup_tracker,
                        }),
                        Err(e) => Err(e),
                    }
                }
            })
            .collect();

        let results = future::join_all(futs).await;
        let mut ready = self.ready.lock().await;
        let mut first_err: Option<VoidCrawlError> = None;
        for result in results {
            match result {
                Ok(tab) if !self.closed.load(Ordering::Acquire) => ready.push_back(tab),
                Ok(tab) => {
                    let _ = tab.page.close().await;
                }
                // Replacement failed — the old tab is already closed, so
                // this slot is lost.  The semaphore still has its permit,
                // so acquire() can still create a tab on-demand if the
                // session recovers.
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        // Release the Mutex before returning, then surface the first error.
        drop(ready);
        if let Some(e) = first_err { Err(e) } else { Ok(()) }
    }

    /// Access the pool configuration.
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    /// Free concurrency permits right now — how many more tabs could be
    /// acquired without queueing. `max_tabs - available_permits()` is the
    /// count currently checked out (in-flight fetches plus any held session
    /// tabs). A live snapshot an agent can read via `pool_status` to size a
    /// fan-out before submitting it.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Start a background Tokio task that periodically calls
    /// [`evict_idle`](Self::evict_idle).
    ///
    /// **Idempotent** — if an eviction task is already running, this is a
    /// no-op.  The handle is stored inside the pool and cancelled
    /// automatically by [`close`](Self::close).  Call
    /// [`stop_eviction_task`](Self::stop_eviction_task) to cancel early.
    ///
    /// # Panics
    ///
    /// Must be called from within a Tokio runtime context.
    pub fn start_eviction_task(self: Arc<Self>) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        // Recover from a poisoned mutex: if a previous panic occurred while
        // holding this lock, the inner value is still usable.
        let mut slot = self.eviction_task.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return; // Already running — ignore duplicate call.
        }
        let pool = Arc::clone(&self);
        let cadence = Duration::from_secs((self.config.tab_max_idle_secs / 2).max(1));
        let handle = tokio::spawn(async move {
            let mut ticker = interval(cadence);
            // Tokio intervals tick immediately; eviction's first run remains one full cadence away.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                // Ignore errors — a single eviction failure (e.g. a tab
                // failing to close) should not kill the background task.
                let _ = pool.evict_idle().await;
            }
        });
        *slot = Some(handle);
    }

    /// Stop the background eviction task (if running).
    ///
    /// Called automatically by [`close`](Self::close).
    pub fn stop_eviction_task(&self) {
        let mut slot = self.eviction_task.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(task) = slot.take() {
            task.abort();
        }
    }

    /// Drain all tabs and close all browser sessions.
    ///
    /// Returns the first error encountered during tab or session teardown,
    /// but always attempts to close everything regardless of individual
    /// failures.
    pub async fn close(&self) -> Result<()> {
        if self.shutdown_complete.load(Ordering::Acquire) {
            return Ok(());
        }

        // The transition is synchronous and one-way, immediately waking
        // queued acquires. All state moved during shutdown belongs to the
        // retained task rather than this cancellable caller future.
        self.closed.store(true, Ordering::Release);
        self.semaphore.close();

        let mut slot = self.shutdown_task.lock().await;
        if slot.is_none() {
            let state = PoolShutdownState {
                sessions:          Arc::clone(&self.sessions),
                ready:             Arc::clone(&self.ready),
                semaphore:         Arc::clone(&self.semaphore),
                eviction_task:     Arc::clone(&self.eviction_task),
                lifecycle:         Arc::clone(&self.lifecycle),
                cleanup_tracker:   Arc::clone(&self.cleanup_tracker),
                shutdown_complete: Arc::clone(&self.shutdown_complete),
                total_tabs:        self
                    .config
                    .browsers
                    .saturating_mul(self.config.tabs_per_browser),
            };
            *slot = Some(tokio::spawn(shutdown_pool(state)));
        }
        let Some(task) = slot.as_mut() else {
            return Err(VoidCrawlError::Other("pool shutdown task was not installed".into()));
        };
        let outcome = task.await;
        // Keep the handle in the slot throughout a cancellable await, then
        // consume it exactly once after completion. Failures remain truthful:
        // shutdown_complete is false and the next call launches a retry.
        slot.take();
        match outcome {
            Ok(result) => result,
            Err(error) => Err(VoidCrawlError::Other(format!("pool shutdown task failed: {error}"))),
        }
    }
}

struct PoolShutdownState {
    sessions:          Arc<Vec<BrowserSession>>,
    ready:             Arc<Mutex<VecDeque<PooledTab>>>,
    semaphore:         Arc<Semaphore>,
    eviction_task:     Arc<StdMutex<Option<JoinHandle<()>>>>,
    lifecycle:         Arc<RwLock<()>>,
    cleanup_tracker:   Arc<CleanupTracker>,
    shutdown_complete: Arc<AtomicBool>,
    total_tabs:        usize,
}

async fn shutdown_pool(state: PoolShutdownState) -> Result<()> {
    let eviction = state.eviction_task.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(task) = eviction {
        task.abort();
        let _ = task.await;
    }

    let _lifecycle = state.lifecycle.write().await;
    let mut first_err = None;
    let wait_for_cleanup = async {
        loop {
            // Register before checking state so a cleanup notification cannot
            // race between the condition check and awaiting the notification.
            let changed = state.cleanup_tracker.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if state.cleanup_tracker.is_idle()
                && state.semaphore.available_permits() == state.total_tabs
            {
                return;
            }
            changed.await;
        }
    };
    if timeout(POOL_SHUTDOWN_TIMEOUT, wait_for_cleanup).await.is_err() {
        first_err = Some(VoidCrawlError::Timeout(
            "browser pool shutdown timed out waiting for checked-out tabs and cleanup workers"
                .into(),
        ));
    }

    let tabs: Vec<PooledTab> = state.ready.lock().await.drain(..).collect();
    let tab_futs = tabs.into_iter().map(|tab| async move {
        match timeout(TAB_CLOSE_TIMEOUT, tab.page.close()).await {
            Ok(result) => result,
            Err(_) => {
                Err(VoidCrawlError::Timeout("browser pool shutdown timed out closing a tab".into()))
            }
        }
    });
    for result in future::join_all(tab_futs).await {
        if let Err(error) = result
            && first_err.is_none()
        {
            first_err = Some(error);
        }
    }

    for result in future::join_all(state.sessions.iter().map(BrowserSession::close)).await {
        if let Err(error) = result
            && first_err.is_none()
        {
            first_err = Some(error);
        }
    }

    if let Some(error) = first_err {
        return Err(error);
    }
    state.shutdown_complete.store(true, Ordering::Release);
    Ok(())
}
