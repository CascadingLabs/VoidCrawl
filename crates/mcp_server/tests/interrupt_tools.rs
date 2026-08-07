#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration coverage for explicit MCP policy interrupts.
//!
//! Requires Chromium. Run with:
//!
//!     cargo test -p voidcrawl-mcp --test interrupt_tools -- --test-threads=1

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::{
    sync::Mutex,
    time::{sleep, timeout},
};
use void_crawl_core::BrowserSession;
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::{
        actions::{self, EvalJsArgs},
        interrupt::{self, InterruptIdArgs, SessionInterruptArgs},
        screenshot::{self, SessionScreenshotArgs},
        session::{self, SessionIdArgs},
        snapshot::{self, SessionSnapshotArgs},
    },
};

const SID: &str = "interrupt-session";

async fn server_with_page() -> VoidCrawlServer {
    let session =
        BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch chromium");
    let page = session
        .new_page(
            "data:text/html,<title>Interrupt%20fixture</title><main><p>safe%20inspection</p></main>",
        )
        .await
        .expect("navigate fixture");
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
async fn interrupted_mcp_session_blocks_mutation_but_allows_inspection_and_resume() {
    let server = server_with_page().await;
    let interrupted = interrupt::begin(
        &server,
        SessionInterruptArgs {
            session_id:  SID.to_string(),
            code:        "policy.operator_review".into(),
            summary:     "fixture review".into(),
            ttl_seconds: 30,
        },
    )
    .await
    .expect("interrupt session");

    let content = session::content(&server, SessionIdArgs { session_id: SID.to_string() })
        .await
        .expect("content remains available while interrupted");
    assert!(content.html.contains("safe inspection"));
    let snapshot = snapshot::session(
        &server,
        SessionSnapshotArgs { session_id: SID.to_string(), max_chars: None },
    )
    .await
    .expect("snapshot remains available while interrupted");
    assert_eq!(snapshot.title.as_deref(), Some("Interrupt fixture"));
    screenshot::session(
        &server,
        SessionScreenshotArgs { session_id: SID.to_string(), ..Default::default() },
    )
    .await
    .expect("screenshot remains available while interrupted");

    let err = actions::eval_js(
        &server,
        EvalJsArgs {
            session_id: SID.to_string(),
            expression: "document.body.dataset.mutated = 'yes'".into(),
        },
    )
    .await
    .expect_err("mutating eval must be blocked while interrupted");
    assert!(err.message.contains(&interrupted.interrupt_id));
    let wire = serde_json::to_value(&err).expect("serialize MCP error");
    assert_eq!(wire["data"]["exception"], "SessionInterrupted");
    assert_eq!(wire["data"]["interrupt_id"], interrupted.interrupt_id);

    let resumed = interrupt::resume(
        &server,
        InterruptIdArgs { session_id: SID.to_string(), interrupt_id: interrupted.interrupt_id },
    )
    .await
    .expect("resume same session");
    assert_eq!(resumed.state, "resumed");
    let result = actions::eval_js(
        &server,
        EvalJsArgs {
            session_id: SID.to_string(),
            expression: "document.body.dataset.mutated = 'yes'; document.body.dataset.mutated"
                .into(),
        },
    )
    .await
    .expect("mutation works after resume");
    assert_eq!(result.value, "yes");

    teardown(&server).await;
}

#[tokio::test]
async fn expired_mcp_interrupt_closes_and_removes_its_dedicated_session() {
    let server = server_with_page().await;
    interrupt::begin(
        &server,
        SessionInterruptArgs {
            session_id:  SID.to_string(),
            code:        "policy.operator_review".into(),
            summary:     "fixture expiry".into(),
            ttl_seconds: 1,
        },
    )
    .await
    .expect("interrupt session");

    timeout(Duration::from_secs(3), async {
        loop {
            if server.state().sessions.get(SID).await.is_none() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expired interrupt session should be reaped");
    assert!(server.state().sessions.is_empty().await);
}
