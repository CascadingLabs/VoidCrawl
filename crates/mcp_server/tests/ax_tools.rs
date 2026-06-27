#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration tests for the `session_ax_tree` and `click_by_role` MCP tools.
//!
//! Drives the tool handler functions against a real session loaded with an
//! inline `data:` fixture. The handlers only touch the session registry (not
//! the browser pool), so we build a dedicated headless session by hand and
//! register it — no pool launch required.
//!
//! Requires Chromium. Run with:
//!
//!     cargo test -p voidcrawl-mcp --test ax_tools -- --test-threads=1

use std::sync::Arc;

use tokio::sync::Mutex;
use void_crawl_core::BrowserSession;
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::{
        actions::{self, AxTreeArgs, ClickByRoleArgs, EvalJsArgs},
        session::{self, SessionIdArgs},
    },
};

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

/// Build a server with one registered session already navigated to `html`.
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

#[tokio::test]
async fn ax_tree_compact_renders_role_name_outline_and_richness() {
    let server = server_with_page("<main><button>Load more</button></main>").await;

    let res = actions::ax_tree(
        &server,
        AxTreeArgs { session_id: SID.to_string(), mode: None, depth: None },
    )
    .await
    .expect("ax_tree ok");

    assert!(res.tree.contains("button \"Load more\""), "compact outline: {}", res.tree);
    assert!(res.tree.contains("main"), "landmark should appear: {}", res.tree);
    assert!(res.nodes.is_empty(), "compact mode must not also dump raw nodes");
    assert!(res.node_count > 0, "node_count populated");
    assert!(res.named_count >= 1, "the button contributes a named node");

    teardown(&server).await;
}

#[tokio::test]
async fn ax_tree_raw_returns_nodes_and_no_outline() {
    let server = server_with_page("<button>Save</button>").await;

    let res = actions::ax_tree(
        &server,
        AxTreeArgs {
            session_id: SID.to_string(),
            mode:       Some("raw".into()),
            depth:      None,
        },
    )
    .await
    .expect("ax_tree ok");

    assert!(res.tree.is_empty(), "raw mode must not render the outline");
    assert!(!res.nodes.is_empty(), "raw mode returns the CDP nodes");
    teardown(&server).await;
}

#[tokio::test]
async fn ax_tree_unknown_session_errors() {
    let server = server_with_page("<button>x</button>").await;

    let err = actions::ax_tree(
        &server,
        AxTreeArgs { session_id: "nope".into(), mode: None, depth: None },
    )
    .await
    .expect_err("unknown session should error");
    assert!(err.message.contains("unknown session_id"), "got: {}", err.message);

    teardown(&server).await;
}

#[tokio::test]
async fn click_by_role_clicks_and_reports_ok() {
    let server = server_with_page("<button onclick=\"window.__hit=true\">Subscribe</button>").await;

    let res = actions::click_by_role(
        &server,
        ClickByRoleArgs {
            session_id: SID.to_string(),
            role:       "button".into(),
            name:       "Subscribe".into(),
            nth:        None,
            humanize:   false,
        },
    )
    .await
    .expect("click_by_role ok");
    assert!(res.ok);

    // Confirm the click actually landed, via the eval_js tool.
    let hit = actions::eval_js(
        &server,
        EvalJsArgs { session_id: SID.to_string(), expression: "window.__hit === true".into() },
    )
    .await
    .expect("eval ok");
    assert_eq!(hit.value, serde_json::Value::Bool(true), "onclick should have fired");

    teardown(&server).await;
}

#[tokio::test]
async fn click_by_role_no_match_errors() {
    let server = server_with_page("<button>Only</button>").await;

    let err = actions::click_by_role(
        &server,
        ClickByRoleArgs {
            session_id: SID.to_string(),
            role:       "button".into(),
            name:       "Missing".into(),
            nth:        None,
            humanize:   false,
        },
    )
    .await
    .expect_err("no match should error");
    assert!(err.message.contains("Missing"), "error should name the target: {}", err.message);

    teardown(&server).await;
}
