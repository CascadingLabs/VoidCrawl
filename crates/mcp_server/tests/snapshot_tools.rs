#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration tests for compact page snapshot MCP tools.
//!
//! Requires Chromium. Run with:
//!
//!     cargo test -p voidcrawl-mcp --test snapshot_tools -- --test-threads=1

use std::sync::Arc;

use tokio::sync::Mutex;
use void_crawl_core::{BrowserPool, BrowserSession, PoolConfig};
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::{
        fetch::{self, FetchArgs},
        session::{self, SessionIdArgs},
        snapshot::{self, FetchSnapshotArgs, SessionSnapshotArgs},
    },
};

const SID: &str = "snapshot-session";

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
      <head><title>Snapshot Fixture</title></head>
      <body>
        <main>
          <h1>Dashboard</h1>
          <h2>Reports</h2>
          <p>The reports page shows revenue, retention, and conversion summaries.</p>
          <a href="https://example.test/reports">Open reports</a>
          <button aria-label="Refresh reports">Refresh</button>
          <form action="/search" method="post">
            <label for="q">Search</label>
            <input id="q" name="q" placeholder="Query">
            <button type="submit">Run</button>
          </form>
        </main>
      </body>
    </html>
    "#
    .to_string()
}

async fn server_with_pool() -> (VoidCrawlServer, Arc<BrowserPool>) {
    let session =
        BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch chromium");
    let pool = Arc::new(BrowserPool::new(
        PoolConfig { browsers: 1, tabs_per_browser: 1, ..PoolConfig::default() },
        vec![session],
    ));
    let sessions = Arc::new(SessionRegistry::default());
    let server = VoidCrawlServer::new(Arc::new(AppState::with_pool(Arc::clone(&pool), sessions)));
    (server, pool)
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

#[tokio::test]
async fn fetch_snapshot_captures_rendered_page_sections() {
    let (server, pool) = server_with_pool().await;

    let snapshot = snapshot::fetch(
        &server,
        FetchSnapshotArgs {
            url:          data_url(&fixture_html()),
            wait_for:     None,
            timeout_secs: Some(10),
            max_chars:    None,
        },
    )
    .await
    .expect("fetch_snapshot ok");

    assert_eq!(snapshot.title.as_deref(), Some("Snapshot Fixture"));
    assert!(snapshot.headings.iter().any(|h| h.text == "Dashboard"));
    assert!(snapshot.text_blocks.iter().any(|b| b.text.contains("revenue")));
    assert!(snapshot.links.iter().any(|l| l.text == "Open reports"));
    assert!(snapshot.controls.iter().any(|c| c.name.as_deref() == Some("Refresh reports")));
    assert_eq!(snapshot.forms.len(), 1);
    assert!(snapshot.forms[0].controls.iter().any(|c| c.placeholder.as_deref() == Some("Query")));
    assert!(!snapshot.stats.truncated);

    pool.close().await.ok();
}

#[tokio::test]
async fn session_snapshot_captures_current_session_page() {
    let server = server_with_page(&fixture_html()).await;

    let snapshot = snapshot::session(
        &server,
        SessionSnapshotArgs { session_id: SID.to_string(), max_chars: None },
    )
    .await
    .expect("session_snapshot ok");

    assert_eq!(snapshot.title.as_deref(), Some("Snapshot Fixture"));
    assert!(snapshot.headings.iter().any(|h| h.text == "Reports"));
    assert!(snapshot.links.iter().any(|l| l.href.contains("example.test/reports")));
    assert!(snapshot.controls.iter().any(|c| c.tag == "input"));
    assert_eq!(snapshot.redirected, None);

    teardown(&server).await;
}

#[tokio::test]
async fn session_snapshot_truncates_large_pages_with_omission_stats() {
    let repeated = (0..120)
        .map(|i| format!("<p>Repeated content block {i} with enough words to become visible.</p>"))
        .collect::<String>();
    let server = server_with_page(&format!("<main><h1>Large</h1>{repeated}</main>")).await;

    let snapshot = snapshot::session(
        &server,
        SessionSnapshotArgs { session_id: SID.to_string(), max_chars: Some(500) },
    )
    .await
    .expect("session_snapshot ok");

    assert!(snapshot.stats.returned_chars <= 500);
    assert!(snapshot.stats.truncated);
    assert_eq!(snapshot.stats.total.text_blocks, 120);
    assert!(snapshot.stats.omitted.text_blocks > 0);
    assert_eq!(snapshot.text_blocks.len(), snapshot.stats.returned.text_blocks);

    teardown(&server).await;
}

#[tokio::test]
async fn raw_html_tools_remain_unchanged() {
    let html = fixture_html();

    let (server, pool) = server_with_pool().await;
    let fetched = fetch::run(
        &server,
        FetchArgs {
            url:          data_url(&html),
            wait_for:     None,
            extract:      None,
            timeout_secs: Some(10),
        },
    )
    .await
    .expect("fetch ok");
    assert!(fetched.html.contains("<form"));
    assert!(fetched.html.contains("Refresh reports"));
    pool.close().await.ok();

    let server = server_with_page(&html).await;
    let content =
        session::content(&server, SessionIdArgs { session_id: SID.to_string() }).await.unwrap();
    assert!(content.html.contains("<form"));
    assert!(content.html.contains("Refresh reports"));
    teardown(&server).await;
}

#[tokio::test]
async fn session_snapshot_unknown_session_uses_invalid_params_style_error() {
    let server = server_with_page("<main><h1>Known</h1></main>").await;

    let err = snapshot::session(
        &server,
        SessionSnapshotArgs { session_id: "missing".into(), max_chars: None },
    )
    .await
    .expect_err("unknown session should error");
    assert!(err.message.contains("unknown session_id: missing"), "got: {}", err.message);

    teardown(&server).await;
}
