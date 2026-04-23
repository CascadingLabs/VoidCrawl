//! Parser + applicator for the shared `wait_for` string knob.
//!
//! Event-driven only. No sleeps. Accepted values:
//!   - `"networkidle"` (default) — wait for Chrome's network-idle lifecycle
//!     event.
//!   - `"selector:<css>"` — wait until a CSS selector matches, driven by an
//!     in-page `MutationObserver`.

use std::time::Duration;

use void_crawl_core::{Page, VoidCrawlError};

/// Apply a `wait_for` spec to a page that has NOT been pre-settled.
/// Use this when you need to wait from scratch (no preceding navigation).
pub async fn apply(
    page: &Page,
    spec: Option<&str>,
    timeout: Duration,
) -> Result<(), VoidCrawlError> {
    apply_with_idle_state(page, spec, timeout, /* already_idle */ false).await
}

/// Apply a `wait_for` spec after a navigation that already completed
/// its networkIdle wait (e.g. via `goto_and_wait_for_idle`). When the
/// caller leaves `spec` at the default, this returns immediately —
/// chromiumoxide event listeners don't replay past events, so a
/// second subscription to `networkIdle` on a settled page would just
/// stall until `timeout`.
pub async fn apply_post_navigate(
    page: &Page,
    spec: Option<&str>,
    timeout: Duration,
) -> Result<(), VoidCrawlError> {
    apply_with_idle_state(page, spec, timeout, /* already_idle */ true).await
}

async fn apply_with_idle_state(
    page: &Page,
    spec: Option<&str>,
    timeout: Duration,
    already_idle: bool,
) -> Result<(), VoidCrawlError> {
    let raw = spec.unwrap_or("networkidle").trim();
    if raw.eq_ignore_ascii_case("networkidle") {
        if already_idle {
            // Page already reached networkIdle via goto_and_wait_for_idle.
            // Subscribing again would drain the full timeout.
            return Ok(());
        }
        page.wait_for_network_idle(timeout).await?;
        return Ok(());
    }
    if let Some(css) = raw.strip_prefix("selector:") {
        return page.wait_for_selector(css.trim(), timeout).await;
    }
    Err(VoidCrawlError::Other(format!(
        "unknown wait_for spec: {raw:?} (expected 'networkidle' or 'selector:<css>')"
    )))
}
