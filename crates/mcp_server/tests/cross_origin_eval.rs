#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration test for the `eval_js_in_frame` MCP tool.
//!
//! Proves the core promise: a **cross-origin** iframe's DOM is unreachable from
//! the parent via page JS (`contentDocument` is null), but `eval_js_in_frame`
//! reaches into the frame's own execution context over CDP and reads/drives it.
//!
//! The fixture is a `data:` parent embedding a *separate* `data:` child. Each
//! `data:` URL gets its own opaque origin, and opaque origins never match — so
//! the child is genuinely cross-origin to the parent, no network required.
//!
//! Requires Chromium. Run with:
//!
//!     cargo test -p voidcrawl-mcp --test cross_origin_eval -- --test-threads=1

use std::sync::Arc;

use tokio::sync::Mutex;
use void_crawl_core::BrowserSession;
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::{
        actions::{self, EvalJsArgs, EvalJsInFrameArgs},
        session::{self, SessionIdArgs},
    },
};

/// Percent-encode HTML for a `data:text/html,` URL. Applying this twice (child
/// then parent) is correct: the inner `%XX` escapes become `%25XX`, which the
/// browser decodes back to `%XX` when it reads the parent, yielding the
/// original child URL as the iframe `src`.
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

const SID: &str = "test-session";

async fn server_with_page(html: &str) -> VoidCrawlServer {
    let session =
        BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch chromium");
    let page = session.new_page(&data_url(html)).await.expect("navigate fixture");
    let handle = Arc::new(DedicatedSession {
        session:          Arc::new(session),
        page:             Mutex::new(page),
        pending_download: Mutex::new(None),
    });
    let sessions = Arc::new(SessionRegistry::default());
    sessions.insert(SID.to_string(), handle).await;
    VoidCrawlServer::new(Arc::new(AppState::new(sessions)))
}

async fn teardown(server: &VoidCrawlServer) {
    session::close(server, SessionIdArgs { session_id: SID.to_string() }).await.ok();
}

/// A cross-origin parent+child fixture. `CHILDFRAME` is a plain-ASCII marker in
/// the child's URL, used as the `frame_url_pattern`. The child hides
/// `#secret = 42` that the parent must not be able to read.
///
/// The child's data URL is base64-encoded and only materialized inside the
/// parent via JS (`atob` → `iframe.src`). That keeps the literal `CHILDFRAME`
/// out of the *parent's* URL, so the pattern matches the child frame uniquely —
/// otherwise the parent (whose source would embed the child URL verbatim) would
/// also match. This mirrors real usage, where a site URL never contains the
/// `recaptcha/api2/bframe` pattern that only the bframe's own URL carries.
fn fixture() -> String {
    use base64::Engine as _;
    let child = "<p>CHILDFRAME</p><div id=secret>42</div>";
    let child_url = data_url(child);
    let b64 = base64::engine::general_purpose::STANDARD.encode(child_url);
    format!(
        "<h1>parent</h1><iframe id=f></iframe>\
         <script>document.getElementById('f').src = atob('{b64}');</script>"
    )
}

#[tokio::test]
async fn eval_js_in_frame_reads_cross_origin_iframe_the_parent_cannot() {
    let server = server_with_page(&fixture()).await;

    // 1. Sanity: from the parent, the child's contentDocument is null (same-origin
    //    policy). This is the wall the feature exists to cross.
    let from_parent = actions::eval_js(
        &server,
        EvalJsArgs {
            session_id: SID.to_string(),
            expression: "(() => { const f = document.querySelector('iframe'); \
                         try { return f.contentDocument \
                             ? f.contentDocument.getElementById('secret').textContent \
                             : 'CROSS_ORIGIN_NULL'; } \
                         catch (e) { return 'CROSS_ORIGIN_THROW'; } })()"
                .to_string(),
        },
    )
    .await
    .expect("eval_js ok");
    assert!(
        matches!(from_parent.value.as_str(), Some("CROSS_ORIGIN_NULL" | "CROSS_ORIGIN_THROW")),
        "parent must NOT read the cross-origin child, got: {:?}",
        from_parent.value
    );

    // 2. eval_js_in_frame runs inside the child's own context and reads it. The
    //    child iframe is attached by script, so retry briefly while it registers
    //    (data: loads are near-instant, but not synchronous).
    let mut from_frame = None;
    for _ in 0..30 {
        match actions::eval_js_in_frame(
            &server,
            EvalJsInFrameArgs {
                session_id:        SID.to_string(),
                frame_url_pattern: "CHILDFRAME".to_string(),
                expression:        "document.getElementById('secret').textContent".to_string(),
            },
        )
        .await
        {
            Ok(r) if r.value.as_str() == Some("42") => {
                from_frame = Some(r);
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
        }
    }
    let from_frame = from_frame.expect("eval_js_in_frame should read the child's secret");
    assert_eq!(from_frame.value.as_str(), Some("42"), "frame-scoped read should see the secret");

    // 3. It can DRIVE the frame too, not just read: mutate the child's DOM and read
    //    the mutation back through the same frame-scoped context.
    let driven = actions::eval_js_in_frame(
        &server,
        EvalJsInFrameArgs {
            session_id:        SID.to_string(),
            frame_url_pattern: "CHILDFRAME".to_string(),
            expression:        "(() => { document.getElementById('secret').textContent = '99'; \
                                return document.getElementById('secret').textContent; })()"
                .to_string(),
        },
    )
    .await
    .expect("eval_js_in_frame drive ok");
    assert_eq!(driven.value.as_str(), Some("99"), "frame-scoped eval should mutate the child");

    teardown(&server).await;
}

#[tokio::test]
async fn eval_js_in_frame_errors_when_no_frame_matches() {
    let server = server_with_page(&fixture()).await;

    let err = actions::eval_js_in_frame(
        &server,
        EvalJsInFrameArgs {
            session_id:        SID.to_string(),
            frame_url_pattern: "no-such-frame-xyz".to_string(),
            expression:        "1 + 1".to_string(),
        },
    )
    .await
    .expect_err("a non-matching pattern must error, not silently run in the top frame");
    // FrameNotFound maps to invalid_params; the message is the pattern itself.
    assert!(
        err.message.contains("no-such-frame-xyz"),
        "error should name the missing frame, got: {}",
        err.message
    );

    teardown(&server).await;
}

/// Two cross-origin children whose URLs both contain `SHARED`. A substring that
/// matches more than one frame must fail closed (`AmbiguousFrame`), never
/// silently pick one — frame order is unstable and a decoy frame could hijack
/// the eval target.
fn ambiguous_fixture() -> String {
    use base64::Engine as _;
    let enc = |html: &str| base64::engine::general_purpose::STANDARD.encode(data_url(html));
    let a = enc("<p>SHARED-A</p>");
    let b = enc("<p>SHARED-B</p>");
    format!(
        "<iframe id=a></iframe><iframe id=b></iframe><script>\
         document.getElementById('a').src = atob('{a}');\
         document.getElementById('b').src = atob('{b}');</script>"
    )
}

#[tokio::test]
async fn eval_js_in_frame_fails_closed_when_pattern_is_ambiguous() {
    let server = server_with_page(&ambiguous_fixture()).await;

    // Wait for both children to register, then a pattern matching both must error.
    let mut err = None;
    for _ in 0..30 {
        match actions::eval_js_in_frame(
            &server,
            EvalJsInFrameArgs {
                session_id:        SID.to_string(),
                frame_url_pattern: "SHARED".to_string(),
                expression:        "1".to_string(),
            },
        )
        .await
        {
            Ok(_) => panic!("an ambiguous pattern must not resolve to a single frame"),
            Err(e) if e.message.contains("matched 2 frames") => {
                err = Some(e);
                break;
            }
            // Both frames not registered yet — retry.
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
        }
    }
    assert!(
        err.is_some(),
        "ambiguous pattern should surface AmbiguousFrame once both frames exist"
    );

    teardown(&server).await;
}
