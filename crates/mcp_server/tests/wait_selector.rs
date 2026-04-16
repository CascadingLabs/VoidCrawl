#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration test for the event-driven `wait_for:selector` path.
//!
//! Serves a page whose "target" element is appended to the DOM after a
//! 750ms delay. A naive Rust-side poll would either miss the element
//! (if the poll interval is coarse) or hammer the page (if fine). The
//! in-page `MutationObserver` used by `Page::wait_for_selector` fires
//! the moment the element is inserted.
//!
//! Requires Chromium to be installed. Run with:
//!
//!     cargo test -p voidcrawl_mcp --test wait_selector -- --ignored
//! --test-threads=1

use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::{net::TcpListener, task::JoinHandle};
use void_crawl_core::{BrowserPool, BrowserSession, PoolConfig};
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::SessionRegistry,
    tools::fetch::{self, FetchArgs},
};

const HTML_WITH_DELAYED_TARGET: &str = r#"<!doctype html>
<html><head><title>delayed</title></head>
<body>
  <div id="placeholder">loading</div>
  <script>
    setTimeout(() => {
      const div = document.createElement('div');
      div.id = 'target';
      div.textContent = 'hello';
      document.body.appendChild(div);
    }, 750);
  </script>
</body></html>"#;

async fn serve(_req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let mut resp =
        Response::new(Full::new(Bytes::from_static(HTML_WITH_DELAYED_TARGET.as_bytes())));
    resp.headers_mut()
        .insert("content-type", "text/html; charset=utf-8".parse().expect("static hv"));
    Ok(resp)
}

async fn start_test_server() -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { break };
            let io = TokioIo::new(stream);
            tokio::spawn(async move {
                let _ = http1::Builder::new().serve_connection(io, service_fn(serve)).await;
            });
        }
    });
    (addr, handle)
}

#[tokio::test]
#[ignore = "requires chromium; run with --ignored --test-threads=1"]
async fn wait_for_selector_resolves_on_mutation() {
    let (addr, server_task) = start_test_server().await;

    let session = BrowserSession::launch_headless().await.expect("launch chromium");
    let pool = Arc::new(BrowserPool::new(
        PoolConfig { browsers: 1, tabs_per_browser: 1, ..PoolConfig::default() },
        vec![session],
    ));
    let sessions = Arc::new(SessionRegistry::default());
    let state = Arc::new(AppState::with_pool(Arc::clone(&pool), sessions));
    let server = VoidCrawlServer::new(state);

    let url = format!("http://{addr}/");

    let start = Instant::now();
    let result = fetch::run(
        &server,
        FetchArgs {
            url:          url.clone(),
            wait_for:     Some("selector:#target".into()),
            extract:      Some("document.querySelector('#target')?.textContent".into()),
            timeout_secs: Some(10),
        },
    )
    .await
    .expect("fetch ok");
    let elapsed = start.elapsed();

    assert_eq!(result.extracted.as_ref().and_then(|v| v.as_str()), Some("hello"));
    // The element appears after 750ms. We insist the wait actually
    // waited (> 500ms so we know it didn't short-circuit) but came
    // back promptly (< 3s so we know we didn't busy-poll with a long
    // interval or run to timeout).
    assert!(
        elapsed >= Duration::from_millis(500),
        "fetch returned before target appeared ({}ms)",
        elapsed.as_millis()
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "fetch took too long — MutationObserver path may not be firing ({}ms)",
        elapsed.as_millis()
    );

    pool.close().await.ok();
    server_task.abort();
}
