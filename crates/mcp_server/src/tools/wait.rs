//! Parser + applicator for the shared `wait_for` string knob.
//!
//! Event-driven only. No sleeps. Accepted values:
//!   - `"networkidle"` (default) — wait for Chrome's network-idle lifecycle
//!     event.
//!   - `"selector:<css>"` — wait until a CSS selector matches, driven by an
//!     in-page `MutationObserver`.

use std::time::Duration;

use void_crawl_core::{Page, VoidCrawlError};

pub async fn apply(
    page: &Page,
    spec: Option<&str>,
    timeout: Duration,
) -> Result<(), VoidCrawlError> {
    let raw = spec.unwrap_or("networkidle").trim();
    if raw.eq_ignore_ascii_case("networkidle") {
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
