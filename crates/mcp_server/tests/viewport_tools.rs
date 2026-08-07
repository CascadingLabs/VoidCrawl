#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration tests for `session_set_viewport`, `session_clear_viewport`,
//! and `list_device_presets`.
//!
//! Requires Chromium. Run with:
//!
//!     cargo test -p voidcrawl-mcp --test viewport_tools -- --test-threads=1

use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;
use void_crawl_core::BrowserSession;
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::{
        session::{self, SessionIdArgs},
        viewport::{self, SessionSetViewportArgs, ViewportArg},
    },
};

const SID: &str = "viewport-session";

async fn server_with_page() -> VoidCrawlServer {
    let session =
        BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch chromium");
    let page = session.new_page("https://example.com").await.expect("navigate fixture");
    let handle = Arc::new(DedicatedSession {
        session:                 Arc::new(session),
        page:                    Mutex::new(page),
        profile_lease:           None,
        last_navigation:         Mutex::new(None),
        challenge:               Mutex::new(None),
        pending_download:        Mutex::new(None),
        pending_network_capture: Mutex::new(None),
        cookie_leases:           Mutex::new(HashMap::new()),
    });
    let sessions = Arc::new(SessionRegistry::default());
    sessions.insert(SID.to_string(), handle).await;
    VoidCrawlServer::new(Arc::new(AppState::new(sessions)))
}

async fn teardown(server: &VoidCrawlServer) {
    session::close(server, SessionIdArgs { session_id: SID.to_string() }).await.ok();
}

#[tokio::test]
async fn session_set_viewport_persists_across_calls() {
    let server = server_with_page().await;

    viewport::session_set(
        &server,
        SessionSetViewportArgs {
            session_id: SID.to_string(),
            viewport:   ViewportArg { preset: Some("iPad Mini".into()), ..Default::default() },
        },
    )
    .await
    .expect("session_set_viewport ok");

    let handle = server.state().sessions.get(SID).await.expect("session exists");
    let page = handle.page.lock().await;
    let width = page.evaluate_js("window.innerWidth").await.expect("eval failed");
    let height = page.evaluate_js("window.innerHeight").await.expect("eval failed");
    assert_eq!(width.as_f64(), Some(768.0));
    assert_eq!(height.as_f64(), Some(1024.0));
    drop(page);

    teardown(&server).await;
}

#[tokio::test]
async fn session_clear_viewport_removes_the_override() {
    let server = server_with_page().await;

    viewport::session_set(
        &server,
        SessionSetViewportArgs {
            session_id: SID.to_string(),
            viewport:   ViewportArg { width: Some(500), height: Some(400), ..Default::default() },
        },
    )
    .await
    .expect("session_set_viewport ok");

    viewport::session_clear(&server, SessionIdArgs { session_id: SID.to_string() })
        .await
        .expect("session_clear_viewport ok");

    let handle = server.state().sessions.get(SID).await.expect("session exists");
    let page = handle.page.lock().await;
    assert!(page.current_viewport().is_none(), "clear should drop the override");
    let width = page.evaluate_js("window.innerWidth").await.expect("eval failed");
    assert_ne!(width.as_f64(), Some(500.0));
    drop(page);

    teardown(&server).await;
}

#[tokio::test]
async fn session_set_viewport_unknown_session_fails_explicitly() {
    let server = server_with_page().await;
    teardown(&server).await;

    let err = viewport::session_set(
        &server,
        SessionSetViewportArgs {
            session_id: SID.to_string(),
            viewport:   ViewportArg { preset: Some("iPhone 16".into()), ..Default::default() },
        },
    )
    .await
    .expect_err("closed session should error");

    assert!(err.message.contains(&format!("unknown session_id: {SID}")), "got: {}", err.message);
}

#[tokio::test]
async fn list_device_presets_returns_known_names_with_dimensions() {
    let result = viewport::list_presets();

    assert!(!result.presets.is_empty());
    let iphone = result
        .presets
        .iter()
        .find(|p| p.name == "iPhone 16 Pro Max")
        .expect("iPhone 16 Pro Max preset present");
    assert_eq!((iphone.width, iphone.height), (430, 932));
    assert!(iphone.mobile);

    let desktop = result
        .presets
        .iter()
        .find(|p| p.name == "Desktop 1080p")
        .expect("Desktop 1080p preset present");
    assert_eq!((desktop.width, desktop.height), (1920, 1080));
    assert!(!desktop.mobile);
}
