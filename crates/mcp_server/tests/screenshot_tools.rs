#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration tests for `session_screenshot`.
//!
//! Requires Chromium. Run with:
//!
//!     cargo test -p voidcrawl-mcp --test screenshot_tools -- --test-threads=1

use std::sync::Arc;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use rmcp::model::{CallToolResult, RawContent};
use tokio::sync::Mutex;
use void_crawl_core::BrowserSession;
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::{
        screenshot::{self, SessionScreenshotArgs},
        session::{self, SessionIdArgs},
        viewport::{self, BboxArg, ScrollArg, ViewportArg},
    },
};

const SID: &str = "screenshot-session";

/// Read width/height from a PNG's IHDR chunk (bytes 16..24, big-endian u32
/// each — fixed by spec) rather than pulling in an image-decoding
/// dependency just to assert a capture's pixel dimensions.
fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("IHDR width bytes"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("IHDR height bytes"));
    (width, height)
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

    let result = screenshot::session(
        &server,
        SessionScreenshotArgs { session_id: SID.to_string(), ..Default::default() },
    )
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

    let result = screenshot::session(
        &server,
        SessionScreenshotArgs { session_id: SID.to_string(), ..Default::default() },
    )
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
        SessionScreenshotArgs { session_id: "missing".into(), ..Default::default() },
    )
    .await
    .expect_err("unknown session should error");

    assert!(err.message.contains("unknown session_id: missing"), "got: {}", err.message);
}

#[tokio::test]
async fn session_screenshot_closed_session_fails_explicitly() {
    let server = server_with_page(&fixture_html()).await;
    teardown(&server).await;

    let err = screenshot::session(
        &server,
        SessionScreenshotArgs { session_id: SID.to_string(), ..Default::default() },
    )
    .await
    .expect_err("closed session should error");

    assert!(err.message.contains(&format!("unknown session_id: {SID}")), "got: {}", err.message);
}

#[tokio::test]
async fn session_screenshot_one_shot_viewport_preset_captures_at_css_pixel_size() {
    // A page with no `<meta name="viewport">` renders at Chrome's classic
    // ~980px desktop-layout width under mobile emulation (real DevTools
    // "iPhone SE" does the same on a non-responsive site) — a responsive
    // meta tag is what makes the CSS viewport actually match the device.
    let responsive = "<html><head><meta name=\"viewport\" \
                       content=\"width=device-width, initial-scale=1\"></head>\
                       <body><h1>Responsive</h1></body></html>";
    let server = server_with_page(responsive).await;

    let result = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            viewport: Some(ViewportArg { preset: Some("iPhone SE".into()), ..Default::default() }),
            ..Default::default()
        },
    )
    .await
    .expect("session_screenshot ok");

    let (data, _) = image_content(&result);
    let bytes = B64.decode(data).expect("valid base64");
    // iPhone SE is 375x667 CSS px. CDP's captureScreenshot renders at
    // CSS-pixel size regardless of device_scale_factor — window.innerWidth
    // and layout are correctly emulated (see the core-crate integration
    // test), but the exported PNG isn't upscaled to device pixels. See
    // Page::set_viewport's doc comment.
    assert_eq!(png_dimensions(&bytes), (375, 667));

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_one_shot_viewport_does_not_persist() {
    let server = server_with_page(&fixture_html()).await;

    let before = screenshot::session(
        &server,
        SessionScreenshotArgs { session_id: SID.to_string(), ..Default::default() },
    )
    .await
    .expect("session_screenshot ok");
    let (before_data, _) = image_content(&before);
    let before_dims = png_dimensions(&B64.decode(before_data).expect("valid base64"));

    screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            viewport: Some(ViewportArg { preset: Some("iPhone SE".into()), ..Default::default() }),
            ..Default::default()
        },
    )
    .await
    .expect("session_screenshot ok");

    // A follow-up capture with no viewport override must be back to
    // whatever the session had before — not left on the iPhone SE size.
    let after = screenshot::session(
        &server,
        SessionScreenshotArgs { session_id: SID.to_string(), ..Default::default() },
    )
    .await
    .expect("session_screenshot ok");
    let (after_data, _) = image_content(&after);
    let after_dims = png_dimensions(&B64.decode(after_data).expect("valid base64"));

    assert_eq!(before_dims, after_dims, "one-shot viewport must not persist");

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_bbox_and_scroll_crop_at_scrolled_position() {
    let html = "<body style='margin:0'>\
                <div style='height:3000px;background:red'></div>\
                <div style='height:3000px;background:blue'></div></body>";
    let server = server_with_page(html).await;

    let result = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            viewport:   Some(ViewportArg {
                width: Some(800),
                height: Some(600),
                ..Default::default()
            }),
            bbox:       Some(BboxArg { x: 5, y: 5, width: 100, height: 80 }),
            selector:   None,
            scroll:     Some(ScrollArg { viewports: Some(2.0), ..Default::default() }),
            full_page:  None,
        },
    )
    .await
    .expect("session_screenshot ok");

    let (data, _) = image_content(&result);
    let bytes = B64.decode(data).expect("valid base64");
    assert_eq!(png_dimensions(&bytes), (100, 80));

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_viewport_rejects_preset_and_custom_dims_together() {
    let server = server_with_page(&fixture_html()).await;

    let err = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            viewport: Some(ViewportArg {
                preset: Some("iPhone SE".into()),
                width: Some(400),
                height: Some(800),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .expect_err("preset + custom dims should be rejected");

    assert!(err.message.contains("mutually exclusive"), "got: {}", err.message);

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_unknown_preset_lists_valid_names_hint() {
    let server = server_with_page(&fixture_html()).await;

    let err = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            viewport: Some(ViewportArg { preset: Some("Nokia 3310".into()), ..Default::default() }),
            ..Default::default()
        },
    )
    .await
    .expect_err("unknown preset should be rejected");

    assert!(err.message.contains("list_device_presets"), "got: {}", err.message);

    teardown(&server).await;
}

#[tokio::test]
async fn session_screenshot_full_page_false_captures_only_the_viewport() {
    let tall = "<body style='margin:0'><div style='height:6000px; \
                background:linear-gradient(red,blue)'></div></body>";
    let server = server_with_page(tall).await;

    viewport::session_set(
        &server,
        viewport::SessionSetViewportArgs {
            session_id: SID.to_string(),
            viewport:   ViewportArg { width: Some(800), height: Some(600), ..Default::default() },
        },
    )
    .await
    .expect("session_set_viewport ok");

    let full = screenshot::session(
        &server,
        SessionScreenshotArgs { session_id: SID.to_string(), ..Default::default() },
    )
    .await
    .expect("session_screenshot ok");
    let full_bytes = B64.decode(image_content(&full).0).expect("valid base64");
    let (_, full_h) = png_dimensions(&full_bytes);

    let cropped = screenshot::session(
        &server,
        SessionScreenshotArgs {
            session_id: SID.to_string(),
            full_page: Some(false),
            ..Default::default()
        },
    )
    .await
    .expect("session_screenshot ok");
    let cropped_bytes = B64.decode(image_content(&cropped).0).expect("valid base64");
    let (crop_w, crop_h) = png_dimensions(&cropped_bytes);

    assert!(full_h >= 5900, "full-page height should cover the 6000px page, got {full_h}");
    assert_eq!((crop_w, crop_h), (800, 600), "full_page:false should be exactly the viewport");

    teardown(&server).await;
}
