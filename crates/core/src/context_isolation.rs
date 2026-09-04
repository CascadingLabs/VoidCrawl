//! Explicit browser-state binding and disposable isolated-context primitives.
//!
//! A Chromium browser context is the only reset boundary in this crate that
//! clears cookies, cache, origin storage, service workers, permissions, and
//! context-scoped network state together. Reusable tabs intentionally do not
//! claim that property: they share their browser profile and retain origin
//! state across checkouts.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use chromiumoxide::{
    Browser,
    cdp::browser_protocol::{browser::BrowserContextId, target::DisposeBrowserContextParams},
};
use serde::Serialize;
use tokio::{runtime::Handle, sync::Mutex};

use crate::Page;

/// Where mutable browser state is bound for a page or capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserStateBinding {
    /// Reusable tabs in one launched browser share its ephemeral profile.
    SharedBrowserProfile,
    /// State belongs only to one disposable Chromium browser context.
    IsolatedBrowserContext,
    /// State is deliberately retained in a caller-selected managed profile.
    ManagedProfile,
    /// State belongs to an externally owned browser VoidCrawl attached to.
    AttachedBrowser,
}

impl BrowserStateBinding {
    /// Whether disposal provides a browser-enforced state-isolation boundary.
    #[must_use]
    pub const fn is_isolated(self) -> bool {
        matches!(self, Self::IsolatedBrowserContext)
    }
}

/// Stable outcome of disposing an isolated browser context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextDisposalState {
    Disposed,
    ProviderDisconnected,
    ProviderRejected,
}

/// Secret-safe cleanup report for an isolated context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextCleanupReport {
    pub state_binding:    BrowserStateBinding,
    pub disposal_state:   ContextDisposalState,
    pub cleanup_complete: bool,
}

impl ContextCleanupReport {
    pub(crate) const fn disposed() -> Self {
        Self {
            state_binding:    BrowserStateBinding::IsolatedBrowserContext,
            disposal_state:   ContextDisposalState::Disposed,
            cleanup_complete: true,
        }
    }

    pub(crate) const fn failed(disconnected: bool) -> Self {
        Self {
            state_binding:    BrowserStateBinding::IsolatedBrowserContext,
            disposal_state:   if disconnected {
                ContextDisposalState::ProviderDisconnected
            } else {
                ContextDisposalState::ProviderRejected
            },
            cleanup_complete: false,
        }
    }
}

/// A page owned by a fresh disposable Chromium browser context.
///
/// Call [`dispose`](Self::dispose) to receive an observable cleanup result.
/// Dropping the handle schedules the same context disposal best-effort when a
/// Tokio runtime is available, but cannot return a report to the caller.
pub struct IsolatedBrowserContext {
    page:          Arc<Page>,
    browser:       Arc<Mutex<Browser>>,
    context_id:    BrowserContextId,
    disposed:      AtomicBool,
    handler_alive: Arc<AtomicBool>,
}

impl fmt::Debug for IsolatedBrowserContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IsolatedBrowserContext")
            .field("state_binding", &BrowserStateBinding::IsolatedBrowserContext)
            .field("disposed", &self.disposed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl IsolatedBrowserContext {
    pub(crate) fn new(
        page: Page,
        browser: Arc<Mutex<Browser>>,
        context_id: BrowserContextId,
        handler_alive: Arc<AtomicBool>,
    ) -> Self {
        Self {
            page: Arc::new(page),
            browser,
            context_id,
            disposed: AtomicBool::new(false),
            handler_alive,
        }
    }

    /// Page scoped to this isolated context.
    #[must_use]
    pub fn page(&self) -> &Page {
        &self.page
    }

    /// Cloned page handle for language bindings. It becomes unusable after
    /// this context is disposed; cloning it does not extend context lifetime.
    #[must_use]
    pub fn page_handle(&self) -> Arc<Page> {
        Arc::clone(&self.page)
    }

    #[must_use]
    pub const fn binding(&self) -> BrowserStateBinding {
        BrowserStateBinding::IsolatedBrowserContext
    }

    /// Dispose the entire context, atomically deleting all pages and mutable
    /// context state without running page `beforeunload` handlers.
    pub async fn dispose(self) -> ContextCleanupReport {
        let browser = Arc::clone(&self.browser);
        let context_id = self.context_id.clone();
        let handler_alive = Arc::clone(&self.handler_alive);
        // Detach cleanup from the caller's future before the first await. If
        // the caller is cancelled while waiting, Chromium disposal continues
        // and `Drop` does not race a duplicate request.
        let cleanup = tokio::spawn(async move {
            browser.lock().await.execute(DisposeBrowserContextParams::new(context_id)).await
        });
        self.disposed.store(true, Ordering::Release);
        match cleanup.await {
            Ok(Ok(_)) => ContextCleanupReport::disposed(),
            Ok(Err(_)) | Err(_) => {
                ContextCleanupReport::failed(!handler_alive.load(Ordering::Acquire))
            }
        }
    }
}

impl Drop for IsolatedBrowserContext {
    fn drop(&mut self) {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(runtime) = Handle::try_current() else {
            return;
        };
        let browser = Arc::clone(&self.browser);
        let context_id = self.context_id.clone();
        runtime.spawn(async move {
            let _ =
                browser.lock().await.execute(DisposeBrowserContextParams::new(context_id)).await;
        });
    }
}
