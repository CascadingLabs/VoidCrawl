//! Integration tests for `void_crawl_core`.
//!
//! These tests require a real Chromium/Chrome binary to be available.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::absolute_paths)]

use std::{collections::HashMap, time::Duration};

use tokio::time;
use void_crawl_core::{
    Bbox, BrowserPool, BrowserSession, PoolConfig, ScreenshotOptions, ScreenshotOutput,
    ScrollTarget, StealthConfig, Viewport, viewport,
};

/// Read width/height from a PNG's IHDR chunk (bytes 16..24, big-endian u32
/// each — fixed by spec, right after the 8-byte signature + 4-byte length +
/// 4-byte "IHDR" tag). Avoids pulling in an image-decoding dependency just
/// to assert a capture's pixel dimensions in tests.
fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("IHDR width bytes"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("IHDR height bytes"));
    (width, height)
}

/// Helper: launch headless with no-sandbox (required for CI / containers).
async fn headless_session() -> BrowserSession {
    BrowserSession::builder()
        .headless()
        .no_sandbox()
        .launch()
        .await
        .expect("failed to launch headless browser")
}

#[tokio::test]
async fn test_launch_and_version() {
    let session = headless_session().await;
    let version = session.version().await.expect("version() failed");
    assert!(
        version.contains("Chrome") || version.contains("Headless"),
        "unexpected version string: {version}"
    );
    session.close().await.expect("close() failed");
}

#[tokio::test]
async fn test_new_page_and_content() {
    let session = headless_session().await;
    let page = session.new_page("https://example.com").await.expect("new_page failed");

    let html = page.content().await.expect("content() failed");
    assert!(html.contains("Example Domain"), "expected example.com content");

    page.close().await.expect("page close failed");
    session.close().await.expect("browser close failed");
}

#[tokio::test]
async fn test_title_and_url() {
    let session = headless_session().await;
    let page = session.new_page("https://example.com").await.expect("new_page failed");

    let title = page.title().await.expect("title() failed");
    assert_eq!(title, Some("Example Domain".to_string()));

    let url = page.url().await.expect("url() failed");
    assert_eq!(url, Some("https://example.com/".to_string()));

    page.close().await.expect("close failed");
    session.close().await.ok();
}

#[tokio::test]
async fn test_evaluate_js() {
    let session = headless_session().await;
    let page = session.new_page("https://example.com").await.expect("new_page failed");

    let result = page.evaluate_js("1 + 1").await.expect("evaluate_js failed");
    assert_eq!(result, serde_json::json!(2));

    let title_js = page.evaluate_js("document.title").await.expect("evaluate_js failed");
    assert_eq!(title_js, serde_json::json!("Example Domain"));

    page.close().await.ok();
    session.close().await.ok();
}

#[tokio::test]
async fn test_query_selector() {
    let session = headless_session().await;
    let page = session.new_page("https://example.com").await.expect("new_page failed");

    let h1 = page.query_selector("h1").await.expect("query_selector failed");
    assert!(h1.is_some(), "expected to find <h1>");
    assert!(h1.unwrap().contains("Example Domain"), "h1 should contain Example Domain");

    let missing = page
        .query_selector(".nonexistent-class")
        .await
        .expect("query_selector failed for missing element");
    assert!(missing.is_none());

    page.close().await.ok();
    session.close().await.ok();
}

#[tokio::test]
async fn test_navigate() {
    let session = headless_session().await;
    let page = session.new_page("https://example.com").await.expect("new_page failed");

    page.navigate("https://www.iana.org/domains/reserved").await.expect("navigate failed");

    let html = page.content().await.expect("content failed");
    assert!(html.to_lowercase().contains("iana"), "expected IANA content after navigation");

    page.close().await.ok();
    session.close().await.ok();
}

#[tokio::test]
async fn test_screenshot_png() {
    let session = headless_session().await;
    let page = session.new_page("https://example.com").await.expect("new_page failed");

    let png = page.screenshot_png().await.expect("screenshot failed");
    // PNG files start with the magic bytes 0x89 0x50 0x4E 0x47
    assert!(png.len() > 100, "screenshot too small");
    assert_eq!(&png[..4], b"\x89PNG", "not a valid PNG");

    page.close().await.ok();
    session.close().await.ok();
}

#[tokio::test]
async fn test_set_headers() {
    let session = headless_session().await;
    let page = session.new_page("about:blank").await.expect("new_page failed");

    let mut headers = HashMap::new();
    headers.insert("X-Custom-Header".to_string(), "test-value".to_string());
    page.set_headers(headers).await.expect("set_headers failed");

    page.close().await.ok();
    session.close().await.ok();
}

#[tokio::test]
async fn test_no_stealth_mode() {
    let session = BrowserSession::builder()
        .headless()
        .no_sandbox()
        .no_stealth()
        .launch()
        .await
        .expect("launch failed");

    let page = session.new_page("https://example.com").await.expect("new_page failed");

    let html = page.content().await.expect("content failed");
    assert!(html.contains("Example Domain"));

    page.close().await.ok();
    session.close().await.ok();
}

#[tokio::test]
async fn test_custom_stealth_config() {
    let stealth = StealthConfig {
        user_agent:          Some("YosoiTestBot/1.0".into()),
        viewport_width:      1280,
        viewport_height:     720,
        locale:              "en-GB,en;q=0.9".into(),
        inject_js:           None,
        use_builtin_stealth: false,
        bypass_csp:          false,
    };

    let session = BrowserSession::builder()
        .headless()
        .no_sandbox()
        .stealth(stealth)
        .launch()
        .await
        .expect("launch failed");

    let page = session.new_page("https://example.com").await.expect("new_page failed");

    let html = page.content().await.expect("content failed");
    assert!(html.contains("Example Domain"));

    page.close().await.ok();
    session.close().await.ok();
}

// ── Pool tests ─────────────────────────────────────────────────────────

/// Helper: create a pool with the given config, launching headless no-sandbox.
async fn test_pool(config: PoolConfig) -> BrowserPool {
    let mut sessions = Vec::with_capacity(config.browsers);
    for _ in 0..config.browsers {
        let session = BrowserSession::builder()
            .headless()
            .no_sandbox()
            .launch()
            .await
            .expect("failed to launch browser for pool");
        sessions.push(session);
    }
    BrowserPool::new(config, sessions)
}

#[tokio::test]
async fn test_pool_basic() {
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 30,
        auto_evict:           false,
    };
    let pool = test_pool(config).await;
    pool.warmup().await.expect("warmup failed");

    // First acquire
    let tab = pool.acquire().await.expect("acquire failed");
    assert_eq!(tab.use_count, 0);
    tab.page.navigate("https://example.com").await.expect("navigate failed");
    let html = tab.page.content().await.expect("content failed");
    assert!(html.contains("Example Domain"));
    pool.release(tab).await; // infallible

    // Second acquire — should get a recycled tab with use_count == 1
    let tab2 = pool.acquire().await.expect("second acquire failed");
    assert_eq!(tab2.use_count, 1);
    pool.release(tab2).await; // infallible

    pool.close().await.expect("pool close failed");
}

#[tokio::test]
async fn test_pool_parallel() {
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     4,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 30,
        auto_evict:           false,
    };
    let pool = test_pool(config).await;
    pool.warmup().await.expect("warmup failed");

    // Acquire all 4 tabs concurrently
    let (t1, t2, t3, t4) =
        tokio::join!(pool.acquire(), pool.acquire(), pool.acquire(), pool.acquire(),);
    let t1 = t1.expect("acquire 1");
    let t2 = t2.expect("acquire 2");
    let t3 = t3.expect("acquire 3");
    let t4 = t4.expect("acquire 4");

    // Navigate all to example.com
    for tab in [&t1, &t2, &t3, &t4] {
        tab.page.navigate("https://example.com").await.expect("navigate failed");
        let html = tab.page.content().await.expect("content failed");
        assert!(html.contains("Example Domain"));
    }

    // Release all
    pool.release(t1).await; // infallible
    pool.release(t2).await; // infallible
    pool.release(t3).await; // infallible
    pool.release(t4).await; // infallible

    pool.close().await.expect("pool close failed");
}

/// Regression test for the per-browser screenshot capture lock
/// (`Page::screenshot` / `BrowserSession::capture_lock`): headless Chrome
/// only reliably composites a frame for the foregrounded tab, so screenshot
/// capture across tabs sharing one browser must serialize the
/// activate+capture instant without deadlocking or starving any tab.
///
/// Oversubscribes 4 tabs with 24 concurrent screenshot tasks across 3 rounds
/// (72 captures total) and asserts every one succeeds inside a hard timeout
/// — a deadlock or a lost wakeup on the capture lock would hang this test
/// rather than fail it cleanly, so the timeout turns that into a normal
/// test failure instead of a stuck CI job.
#[tokio::test]
async fn test_pool_screenshot_stress_no_deadlock() {
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     4,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 30,
        auto_evict:           false,
    };
    let pool = test_pool(config).await;
    pool.warmup().await.expect("warmup failed");

    const ROUNDS: usize = 3;
    const TASKS_PER_ROUND: usize = 24;

    for round in 0..ROUNDS {
        let outcome = time::timeout(Duration::from_secs(60), async {
            let tasks = (0..TASKS_PER_ROUND).map(|i| {
                let pool = &pool;
                async move {
                    let tab = pool.acquire().await.map_err(|e| format!("acquire {i}: {e}"))?;
                    let html = format!(
                        "data:text/html,<h1 style=\"height:{}px\">stress {i}</h1>",
                        200 + i * 10
                    );
                    tab.page.navigate(&html).await.map_err(|e| format!("navigate {i}: {e}"))?;
                    let png = tab
                        .page
                        .screenshot_png()
                        .await
                        .map_err(|e| format!("screenshot {i}: {e}"))?;
                    pool.release(tab).await;
                    if png.is_empty() {
                        return Err(format!("screenshot {i}: empty PNG"));
                    }
                    Ok(())
                }
            });
            futures::future::join_all(tasks).await
        })
        .await;
        let timeout_msg = format!("round {round} deadlocked or exceeded the 60s stress timeout");
        let outcome = outcome.expect(&timeout_msg);

        let failures: Vec<String> = outcome.into_iter().filter_map(Result::err).collect();
        assert!(failures.is_empty(), "round {round} had failures: {failures:?}");
    }

    pool.close().await.expect("pool close failed");
}

#[tokio::test]
async fn test_acquire_timed_reports_queue_wait() {
    // Single tab slot so the second acquire must queue behind the first.
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 30,
        auto_evict:           false,
    };
    let pool = std::sync::Arc::new(test_pool(config).await);
    pool.warmup().await.expect("warmup failed");

    // Uncontended acquire: a slot is free, so the wait is negligible.
    let (tab, waited) = pool.acquire_timed().await.expect("first acquire");
    assert!(waited < 50, "uncontended acquire should not queue, waited {waited}ms");

    // Hold the only permit, then race a second acquire that must block until
    // we release ~150ms later. Its reported wait should reflect that block.
    let pool2 = pool.clone();
    let second = tokio::spawn(async move { pool2.acquire_timed().await });
    time::sleep(Duration::from_millis(150)).await;
    pool.release(tab).await;

    let (tab2, waited2) = second.await.expect("join").expect("second acquire");
    assert!(waited2 >= 100, "queued acquire should report the block, waited {waited2}ms");
    pool.release(tab2).await;

    pool.close().await.expect("pool close failed");
}

#[tokio::test]
async fn test_pool_hard_recycle() {
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         2,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 30,
        auto_evict:           false,
    };
    let pool = test_pool(config).await;
    pool.warmup().await.expect("warmup failed");

    // Use 1
    let tab = pool.acquire().await.expect("acquire 1");
    assert_eq!(tab.use_count, 0);
    pool.release(tab).await; // infallible

    // Use 2
    let tab = pool.acquire().await.expect("acquire 2");
    assert_eq!(tab.use_count, 1);
    pool.release(tab).await; // infallible

    // Use 3 — should trigger hard recycle (use_count was 2, >= tab_max_uses)
    let tab = pool.acquire().await.expect("acquire 3");
    assert_eq!(tab.use_count, 0, "tab should have been hard-recycled");
    pool.release(tab).await; // infallible

    pool.close().await.expect("pool close failed");
}

#[tokio::test]
async fn test_pool_idle_eviction() {
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         50,
        tab_max_idle_secs:    1,
        acquire_timeout_secs: 30,
        auto_evict:           false,
    };
    let pool = test_pool(config).await;
    pool.warmup().await.expect("warmup failed");

    // Acquire, use, release
    let tab = pool.acquire().await.expect("acquire");
    pool.release(tab).await; // infallible

    // Wait for idle timeout
    time::sleep(Duration::from_secs(2)).await;

    // Evict idle tabs — should replace with fresh ones
    pool.evict_idle().await.expect("evict_idle failed");

    // Acquire again — should get a fresh tab (use_count reset)
    let tab = pool.acquire().await.expect("acquire after eviction");
    assert_eq!(tab.use_count, 0, "evicted tab should be fresh");
    pool.release(tab).await; // infallible

    pool.close().await.expect("pool close failed");
}

// ── Viewport tests ────────────────────────────────────────────────────

#[tokio::test]
async fn set_viewport_overrides_dimensions_scale_and_mobile_ua_persistently() {
    let session = headless_session().await;
    let page = session.new_page("https://example.com").await.expect("new_page failed");

    let vp = viewport::preset("iPhone 16 Pro Max").expect("known preset");
    page.set_viewport(vp.clone()).await.expect("set_viewport failed");

    let width = page.evaluate_js("window.innerWidth").await.expect("eval failed");
    let height = page.evaluate_js("window.innerHeight").await.expect("eval failed");
    assert_eq!(width.as_f64(), Some(f64::from(vp.width)));
    assert_eq!(height.as_f64(), Some(f64::from(vp.height)));

    let ua = page.evaluate_js("navigator.userAgent").await.expect("eval failed");
    assert!(ua.as_str().expect("string").contains("iPhone"), "expected iPhone UA, got {ua:?}");

    let max_touch = page.evaluate_js("navigator.maxTouchPoints").await.expect("eval failed");
    assert!(max_touch.as_f64().expect("number") > 0.0, "mobile preset should enable touch");

    assert_eq!(page.current_viewport(), Some(vp));

    page.close().await.ok();
    session.close().await.ok();
}

#[tokio::test]
async fn clear_viewport_removes_the_override() {
    let session = headless_session().await;
    let page = session.new_page("https://example.com").await.expect("new_page failed");

    page.set_viewport(Viewport::custom(500, 400)).await.expect("set_viewport failed");
    assert_eq!(
        page.evaluate_js("window.innerWidth").await.expect("eval failed").as_f64(),
        Some(500.0)
    );

    page.clear_viewport().await.expect("clear_viewport failed");
    assert!(page.current_viewport().is_none(), "clear_viewport should drop the override");

    let width_after_clear =
        page.evaluate_js("window.innerWidth").await.expect("eval failed").as_f64();
    assert_ne!(
        width_after_clear,
        Some(500.0),
        "clearing the override should stop reporting the custom width"
    );

    page.close().await.ok();
    session.close().await.ok();
}

#[tokio::test]
async fn screenshot_one_shot_viewport_restores_after_capture() {
    let session = headless_session().await;
    let page = session.new_page("https://example.com").await.expect("new_page failed");

    page.set_viewport(Viewport::custom(1024, 768)).await.expect("set_viewport failed");

    let mobile = viewport::preset("Pixel 7").expect("known preset");
    let opts = ScreenshotOptions::default().with_viewport(mobile);
    page.screenshot(opts).await.expect("screenshot failed");

    // The one-shot override must be undone, restoring the persistent
    // 1024x768 override set above — not left on the mobile preset, and not
    // cleared to the session default either.
    let width = page.evaluate_js("window.innerWidth").await.expect("eval failed");
    assert_eq!(width.as_f64(), Some(1024.0), "one-shot viewport should restore prior override");
    assert_eq!(page.current_viewport(), Some(Viewport::custom(1024, 768)));

    page.close().await.ok();
    session.close().await.ok();
}

#[tokio::test]
async fn screenshot_scroll_then_bbox_crops_relative_to_scrolled_position() {
    let session = headless_session().await;
    let html = "data:text/html,<body%20style='margin:0'>\
                <div%20style='height:5000px;background:red'></div>\
                <div%20style='height:5000px;background:blue'></div></body>";
    let page = session.new_page(html).await.expect("new_page failed");
    page.set_viewport(Viewport::custom(800, 600)).await.expect("set_viewport failed");

    let opts = ScreenshotOptions::default()
        .with_scroll(ScrollTarget::Viewports(2.0))
        .with_bbox(Bbox { x: 10, y: 20, width: 200, height: 150 });
    let output = page.screenshot(opts).await.expect("screenshot failed");
    let ScreenshotOutput::Bytes(bytes) = output else { panic!("expected in-memory bytes") };
    assert_eq!(png_dimensions(&bytes), (200, 150));

    // Scroll position must be restored after the capture.
    let scroll_y = page.evaluate_js("window.scrollY").await.expect("eval failed");
    assert_eq!(scroll_y.as_f64(), Some(0.0), "scroll position should be restored after capture");

    page.close().await.ok();
    session.close().await.ok();
}

#[tokio::test]
async fn screenshot_viewport_only_is_shorter_than_full_page() {
    let session = headless_session().await;
    let html = "data:text/html,<body%20style='margin:0'>\
                <div%20style='height:6000px;background:linear-gradient(red,blue)'></div></body>";
    let page = session.new_page(html).await.expect("new_page failed");
    page.set_viewport(Viewport::custom(800, 600)).await.expect("set_viewport failed");

    let full = page.screenshot(ScreenshotOptions::default()).await.expect("full-page failed");
    let ScreenshotOutput::Bytes(full_bytes) = full else { panic!("expected bytes") };
    let (full_w, full_h) = png_dimensions(&full_bytes);

    let cropped = page
        .screenshot(ScreenshotOptions::default().viewport_only())
        .await
        .expect("viewport-only failed");
    let ScreenshotOutput::Bytes(cropped_bytes) = cropped else { panic!("expected bytes") };
    let (crop_w, crop_h) = png_dimensions(&cropped_bytes);

    assert_eq!(full_w, 800, "full-page width should still match the viewport");
    assert_eq!(
        crop_w, 800,
        "viewport-only width should match the viewport exactly (explicit clip)"
    );
    assert!(full_h >= 5900, "full-page height should cover the 6000px page, got {full_h}");
    assert_eq!(crop_h, 600, "viewport-only height should be exactly the viewport height");
    assert!(full_h > crop_h * 2, "full-page capture should be much taller than viewport-only");

    page.close().await.ok();
    session.close().await.ok();
}
