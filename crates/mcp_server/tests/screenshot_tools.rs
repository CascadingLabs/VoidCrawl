#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration tests for `session_screenshot`.
//!
//! Requires Chromium. Run with:
//!
//!     cargo test -p voidcrawl-mcp --test screenshot_tools -- --test-threads=1

use std::sync::Arc;

use rmcp::model::{CallToolResult, RawContent};
use tokio::sync::Mutex;
use void_crawl_core::BrowserSession;
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::{
        screenshot,
        session::{self, SessionIdArgs},
    },
};

const SID: &str = "screenshot-session";

fn data_url(html: &str) -> String {
    let encoded = html
        .replace('%', "%25")
        .replace('"', "%22")
        .replace('#', "%23")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace(' ', "%20")
        .replace('\n', "%0A");
    format!("data:text/html,{encoded}")
}

fn fixture_html() -> String {
    r#"
    <!doctype html>
    <html>
      <head><title>Screenshot Fixture</title></head>
      <body style="background:#123456"><h1>Hello Screenshot</h1></body>
    </html>
    "#
    .to_string()
}

async fn server_with_page(html: &str) -> VoidCrawlServer {
    let session =
        BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch chromium");
    let page = session.new_page(&data_url(html)).await.expect("navigate fixture");
    let handle = Arc::new(DedicatedSession {
        session:          Arc::new(session),
        page:             Mutex::new(page),
        profile_lease:    None,
        last_navigation:  Mutex::new(None),
        challenge:        Mutex::new(None),
        pending_download: Mutex::new(None),
    });
    let sessions = Arc::new(SessionRegistry::default());
    sessions.insert(SID.to_string(), handle).await;
    VoidCrawlServer::new(Arc::new(AppState::new(sessions)))
}

async fn teardown(server: &VoidCrawlServer) {
    session::close(server, SessionIdArgs { session_id: SID.to_string() }).await.ok();
}

fn text_content(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|c| match &c.raw {
            RawContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .expect("text content block")
}

fn image_content(result: &CallToolResult) -> (String, String) {
    result
        .content
        .iter()
        .find_map(|c| match &c.raw {
            RawContent::Image(i) => Some((i.data.clone(), i.mime_type.clone())),
            _ => None,
        })
        .expect("image content block")
}

#[tokio::test]
async fn session_screenshot_captures_current_page_without_navigating() {
    let server = server_with_page(&fixture_html()).await;

    let before =
        session::content(&server, SessionIdArgs { session_id: SID.to_string() }).await.unwrap();

    let result = screenshot::session(&server, SessionIdArgs { session_id: SID.to_string() })
        .await
        .expect("session_screenshot ok");

    let after =
        session::content(&server, SessionIdArgs { session_id: SID.to_string() }).await.unwrap();
    assert_eq!(before.url, after.url, "session_screenshot must not navigate the page");

    let (data, mime) = image_content(&result);
    assert_eq!(mime, "image/png");
    assert!(!data.is_empty());

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_reports_device_pixel_ratio() {
    let server = server_with_page(&fixture_html()).await;

    let result = screenshot::session(&server, SessionIdArgs { session_id: SID.to_string() })
        .await
        .expect("session_screenshot ok");

    let text = text_content(&result);
    assert!(text.contains("devicePixelRatio="), "got: {text}");
    assert!(text.contains("bytes PNG"), "got: {text}");

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_unknown_session_fails_explicitly() {
    let err = screenshot::session(
        &VoidCrawlServer::new(Arc::new(AppState::new(Arc::new(SessionRegistry::default())))),
        SessionIdArgs { session_id: "missing".into() },
    )
    .await
    .expect_err("unknown session should error");

    assert!(err.message.contains("unknown session_id: missing"), "got: {}", err.message);
}

#[tokio::test]
async fn session_screenshot_closed_session_fails_explicitly() {
    let server = server_with_page(&fixture_html()).await;
    teardown(&server).await;

    let err = screenshot::session(&server, SessionIdArgs { session_id: SID.to_string() })
        .await
        .expect_err("closed session should error");

    assert!(err.message.contains(&format!("unknown session_id: {SID}")), "got: {}", err.message);
}
