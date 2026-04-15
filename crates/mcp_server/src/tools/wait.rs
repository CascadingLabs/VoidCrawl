//! Parser + applicator for the shared `wait_for` string knob.
//!
//! Accepted values:
//!   - `"networkidle"` (default when arg is omitted) — wait for Chrome to
//!     report the network-idle lifecycle event.
//!   - `"selector:<css>"` — wait until the CSS selector matches at least one
//!     element.
//!   - `"ms:<n>"` — fixed-duration sleep (coarse fallback).

use std::time::Duration;

use tokio::time::{Instant, sleep};
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
        return wait_for_selector(page, css.trim(), timeout).await;
    }
    if let Some(ms) = raw.strip_prefix("ms:") {
        let parsed: u64 = ms
            .trim()
            .parse()
            .map_err(|_| VoidCrawlError::Other(format!("invalid ms value in wait_for: {ms}")))?;
        sleep(Duration::from_millis(parsed)).await;
        return Ok(());
    }
    Err(VoidCrawlError::Other(format!(
        "unknown wait_for spec: {raw:?} (expected 'networkidle', 'selector:<css>', or 'ms:<n>')"
    )))
}

async fn wait_for_selector(
    page: &Page,
    css: &str,
    timeout: Duration,
) -> Result<(), VoidCrawlError> {
    let deadline = Instant::now() + timeout;
    let poll = Duration::from_millis(100);
    loop {
        if page.query_selector(css).await?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(VoidCrawlError::Timeout(format!(
                "selector {css:?} did not appear within {}s",
                timeout.as_secs()
            )));
        }
        sleep(poll).await;
    }
}
