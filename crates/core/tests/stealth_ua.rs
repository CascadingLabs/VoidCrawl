//! The default stealth preset must not leak "HeadlessChrome" in the
//! User-Agent string — that's a one-line bot signal for any WAF that
//! inspects UA. `chrome_like()` is advertised as the anti-detection
//! default; if it ships with a UA that announces "I am a headless
//! browser", the preset is false advertising.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use void_crawl_core::BrowserSession;

async fn headless_session() -> BrowserSession {
    BrowserSession::builder()
        .headless()
        .no_sandbox()
        .launch()
        .await
        .expect("failed to launch headless browser")
}

#[tokio::test]
async fn default_stealth_strips_headless_from_user_agent() {
    let session = headless_session().await;
    // Use a data URL so no network is needed and the test is hermetic.
    let page = session
        .new_page("data:text/html,<html><body>ua probe</body></html>")
        .await
        .expect("new_page");

    let ua_value = page.evaluate_js("navigator.userAgent").await.expect("evaluate_js");
    let ua = ua_value.as_str().expect("userAgent must be a string");
    assert!(
        !ua.contains("Headless"),
        "User-Agent leaks headless fingerprint: {ua:?} — stealth preset should strip \"HeadlessChrome\""
    );
    assert!(ua.contains("Chrome/"), "User-Agent should still identify as Chrome/<version>: {ua:?}");

    page.close().await.expect("page close");
    session.close().await.expect("session close");
}
