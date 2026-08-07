#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration tests for `session_record_start` / `session_record_stop`.
//!
//! Requires Chromium. Run with:
//!
//!     cargo test -p voidcrawl-mcp --test recording_tools -- --test-threads=1

use std::{collections::HashMap, fs, path::Path, sync::Arc, time::Duration};

use tokio::{sync::Mutex, time::sleep};
use void_crawl_core::BrowserSession;
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::{
        recording::{self, SessionRecordStartArgs, SessionRecordStopArgs},
        selector::{SelectorArg, SelectorKindArg},
        session::{self, SessionIdArgs},
    },
};

const SID: &str = "recording-session";

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

/// Continuously repainting, so Chrome actually composites frames — a static
/// page legitimately yields almost none.
fn fixture_html() -> String {
    r#"
    <html><body style="margin:0">
      <div id="box" style="width:200px;height:120px;background:#c00"></div>
      <div id="other" style="width:150px;height:90px;background:#0c0"></div>
      <script>
        let t = 0;
        function tick() {
          t += 4;
          document.getElementById('box').style.background = 'hsl(' + (t % 360) + ',80%,50%)';
          requestAnimationFrame(tick);
        }
        tick();
      </script>
    </body></html>
    "#
    .to_string()
}

async fn server_with_page() -> VoidCrawlServer {
    let session =
        BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch chromium");
    let page = session.new_page(&data_url(&fixture_html())).await.expect("navigate fixture");
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

fn start_args(dir: &Path) -> SessionRecordStartArgs {
    SessionRecordStartArgs {
        session_id: SID.to_string(),
        max_duration_secs: Some(20.0),
        output_dir: Some(dir.display().to_string()),
        fps: Some(10),
        ..Default::default()
    }
}

fn css(value: &str) -> SelectorArg {
    SelectorArg {
        kind:  SelectorKindArg::Css,
        value: Some(value.to_string()),
        regex: None,
        name:  None,
        nth:   None,
        x:     None,
        y:     None,
    }
}

#[tokio::test]
async fn records_a_session_and_writes_frames_to_disk() {
    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    let started = recording::session_start(&server, start_args(dir.path()))
        .await
        .expect("session_record_start failed");
    assert!(started.recording);

    sleep(Duration::from_millis(1500)).await;

    let result =
        recording::session_stop(&server, SessionRecordStopArgs { session_id: SID.to_string() })
            .await
            .expect("session_record_stop failed");

    assert!(result.frames_captured > 1, "captured {}", result.frames_captured);
    assert_eq!(result.regions.len(), 1);
    assert_eq!(result.regions[0].label, "viewport");
    assert_eq!(result.regions[0].frame_count, result.frames_captured);
    assert_eq!(result.format, "jpeg");

    // The response promises paths, not inline frames — so the paths must
    // actually resolve to files on disk.
    let frames_dir = result.regions[0].frames_dir.as_ref().expect("frames_dir");
    let written = fs::read_dir(frames_dir).expect("frames dir exists").count();
    assert_eq!(written, result.frames_captured, "every counted frame must be on disk");

    teardown(&server).await;
}

#[tokio::test]
async fn multiple_selectors_produce_one_region_each() {
    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    recording::session_start(
        &server,
        SessionRecordStartArgs {
            selectors: vec![css("#box"), css("#other")],
            ..start_args(dir.path())
        },
    )
    .await
    .expect("start failed");

    sleep(Duration::from_millis(1200)).await;

    let result =
        recording::session_stop(&server, SessionRecordStopArgs { session_id: SID.to_string() })
            .await
            .expect("stop failed");

    let labels: Vec<&str> = result.regions.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, vec!["0_box", "1_other"]);
    assert!(result.regions.iter().all(|r| r.bbox.is_some()));
    assert_ne!(result.regions[0].bbox, result.regions[1].bbox);

    teardown(&server).await;
}

/// Starting twice must refuse rather than silently discard the first
/// recording's frames — same contract as `download_arm`.
#[tokio::test]
async fn a_second_start_is_rejected_while_one_is_running() {
    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    recording::session_start(&server, start_args(dir.path())).await.expect("first start");
    let err = recording::session_start(&server, start_args(dir.path()))
        .await
        .expect_err("second start must be rejected");
    assert!(err.to_string().contains("already running"), "got {err}");

    recording::session_stop(&server, SessionRecordStopArgs { session_id: SID.to_string() })
        .await
        .expect("stop failed");
    teardown(&server).await;
}

#[tokio::test]
async fn stop_without_start_is_an_error() {
    let server = server_with_page().await;

    let err =
        recording::session_stop(&server, SessionRecordStopArgs { session_id: SID.to_string() })
            .await
            .expect_err("stop without start must fail");
    assert!(err.to_string().contains("no recording is running"), "got {err}");

    teardown(&server).await;
}

#[tokio::test]
async fn bbox_and_selectors_together_are_rejected() {
    use voidcrawl_mcp::tools::viewport::BboxArg;

    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    let err = recording::session_start(
        &server,
        SessionRecordStartArgs {
            bbox: Some(BboxArg { x: 0, y: 0, width: 10, height: 10 }),
            selectors: vec![css("#box")],
            ..start_args(dir.path())
        },
    )
    .await
    .expect_err("bbox + selectors must be rejected");
    assert!(err.to_string().contains("mutually exclusive"), "got {err}");

    teardown(&server).await;
}

#[tokio::test]
async fn duration_beyond_the_cap_is_rejected() {
    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    let err = recording::session_start(
        &server,
        SessionRecordStartArgs { max_duration_secs: Some(600.0), ..start_args(dir.path()) },
    )
    .await
    .expect_err("an over-long recording must be rejected");
    assert!(err.to_string().contains("maximum"), "got {err}");

    teardown(&server).await;
}

#[tokio::test]
async fn unknown_session_is_an_error() {
    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    let err = recording::session_start(
        &server,
        SessionRecordStartArgs { session_id: "nope".to_string(), ..start_args(dir.path()) },
    )
    .await
    .expect_err("unknown session must fail");
    assert!(err.to_string().contains("no such session"), "got {err}");

    teardown(&server).await;
}
