#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration tests for the MCP `selector` option on
//! `screenshot`/`session_screenshot` (CAS-252).
//!
//! Requires Chromium. Run with:
//!
//!     cargo test -p voidcrawl-mcp --test selector_bbox_tools --
//! --test-threads=1

use std::{collections::HashMap, sync::Arc};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use rmcp::{ErrorData, model::RawContent};
use tokio::sync::Mutex;
use void_crawl_core::BrowserSession;
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::{
        screenshot::{self, SessionScreenshotArgs},
        selector::{SelectorArg, SelectorKindArg},
        session::{self, SessionIdArgs},
        viewport::BboxArg,
    },
};

const SID: &str = "selector-bbox-session";

fn error_code(error: &ErrorData) -> String {
    let value = serde_json::to_value(error).expect("serialize MCP error");
    value["data"]["code"].as_str().unwrap_or_default().to_string()
}

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

const FIXTURE: &str = r#"
<!doctype html>
<html>
  <head><title>Selector Bbox MCP Fixture</title></head>
  <body style="margin:0">
    <h1 style="position:absolute; left:10px; top:10px; width:200px; height:30px; margin:0;">
      Unique Heading
    </h1>
    <div class="dup" style="position:absolute; left:300px; top:10px; width:40px; height:40px;">A</div>
    <div class="dup" style="position:absolute; left:300px; top:60px; width:40px; height:40px;">B</div>
  </body>
</html>
"#;

fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("IHDR width bytes"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("IHDR height bytes"));
    (width, height)
}

fn selector(kind: SelectorKindArg, value: &str) -> SelectorArg {
    SelectorArg {
        kind,
        value: Some(value.to_string()),
        regex: None,
        name: None,
        nth: None,
        x: None,
        y: None,
    }
}

async fn server_with_page(html: &str) -> VoidCrawlServer {
    let session =
        BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch chromium");
    let page = session.new_page(&data_url(html)).await.expect("navigate fixture");
    let handle = Arc::new(DedicatedSession {
        session:                 Arc::new(session),
        page:                    Mutex::new(page),
        profile_lease:           None,
        last_navigation:         Mutex::new(None),
        challenge:               Mutex::new(None),
        pending_download:        Mutex::new(None),
        pending_network_capture: Mutex::new(None),
        pending_recording:       Mutex::new(None),
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
async fn session_screenshot_selector_crops_the_resolved_css_element() {
    let server = server_with_page(FIXTURE).await;

    let result = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            selector: Some(selector(SelectorKindArg::Css, "h1")),
            ..Default::default()
        },
    )
    .await
    .expect("session_screenshot ok");

    let image = result
        .content
        .iter()
        .find_map(|c| match &c.raw {
            RawContent::Image(i) => Some(i.data.clone()),
            _ => None,
        })
        .expect("image content block");
    let bytes = B64.decode(image).expect("valid base64");
    assert_eq!(png_dimensions(&bytes), (200, 30));

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_selector_and_bbox_together_is_rejected() {
    let server = server_with_page(FIXTURE).await;

    let err = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            selector: Some(selector(SelectorKindArg::Css, "h1")),
            bbox: Some(BboxArg { x: 0, y: 0, width: 10, height: 10 }),
            ..Default::default()
        },
    )
    .await
    .expect_err("should be rejected");

    assert!(err.message.contains("mutually exclusive"), "got: {}", err.message);

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_ambiguous_selector_fails_with_invalid_params() {
    let server = server_with_page(FIXTURE).await;

    let err = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            selector: Some(selector(SelectorKindArg::Css, ".dup")),
            ..Default::default()
        },
    )
    .await
    .expect_err("ambiguous selector should error");

    assert_eq!(err.message, "target selector was ambiguous");
    assert_eq!(error_code(&err), "voidcrawl.target.selector_ambiguous");

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_no_match_selector_fails_with_invalid_params() {
    let server = server_with_page(FIXTURE).await;

    let err = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            selector: Some(selector(SelectorKindArg::Css, ".nope")),
            ..Default::default()
        },
    )
    .await
    .expect_err("no match should error");

    assert_eq!(err.message, "target element was not visible");
    assert_eq!(error_code(&err), "voidcrawl.target.element_not_visible");

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_jsonld_selector_fails_as_non_visual() {
    let server = server_with_page(FIXTURE).await;

    let err = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            selector: Some(selector(SelectorKindArg::Jsonld, "$.name")),
            ..Default::default()
        },
    )
    .await
    .expect_err("jsonld should error");

    assert_eq!(err.message, "target kind does not support visual geometry");
    assert_eq!(error_code(&err), "voidcrawl.target.visual_geometry_unsupported");

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_selector_with_nth_disambiguates() {
    let server = server_with_page(FIXTURE).await;

    let mut sel = selector(SelectorKindArg::Css, ".dup");
    sel.nth = Some(1);
    let result = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            selector: Some(sel),
            ..Default::default()
        },
    )
    .await
    .expect("session_screenshot ok");

    let image = result
        .content
        .iter()
        .find_map(|c| match &c.raw {
            RawContent::Image(i) => Some(i.data.clone()),
            _ => None,
        })
        .expect("image content block");
    let bytes = B64.decode(image).expect("valid base64");
    assert_eq!(png_dimensions(&bytes), (40, 40));

    teardown(&server).await;
}
