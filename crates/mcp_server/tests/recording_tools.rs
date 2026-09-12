#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! Integration tests for `session_record_start` / `session_record_stop`.
//!
//! Requires Chromium. Run with:
//!
//!     cargo test -p voidcrawl-mcp --test recording_tools -- --test-threads=1

use std::{collections::HashMap, fs, path::Path, sync::Arc, time::Duration};

use tokio::{
    sync::Mutex,
    time::{sleep, timeout},
};
use void_crawl_core::{BrowserSession, VoidCrawlError};
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::{
        recording::{self, SessionRecordStartArgs, SessionRecordStopArgs},
        screenshot::{self, SessionScreenshotArgs},
        selector::{SelectorArg, SelectorKindArg},
        session::{self, SessionIdArgs},
    },
};

const SID: &str = "recording-session";

fn recording_error_detail(error: &VoidCrawlError) -> &str {
    match error {
        VoidCrawlError::RecordingError(detail) => detail,
        other => panic!("expected RecordingError, got {other:?}"),
    }
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
        session: Arc::new(session),
        page: Mutex::new(page),
        profile_lease: None,
        last_navigation: Mutex::new(None),
        challenge: Mutex::new(None),
        pending_download: Mutex::new(None),
        pending_network_capture: Mutex::new(None),
        pending_recording: Mutex::new(None),
        cookie_leases: Mutex::new(HashMap::new()),
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
        kind: SelectorKindArg::Css,
        value: Some(value.to_string()),
        regex: None,
        name: None,
        nth: None,
        x: None,
        y: None,
    }
}

fn mask_selector(value: &str) -> recording::MaskArg {
    recording::MaskArg { selector: Some(css(value)), ..Default::default() }
}

/// The lowest-numbered frame a region wrote, decoded to RGBA.
///
/// Frames are read back off disk rather than out of the response on purpose:
/// the artifact is what gets shared, so the artifact is what the mask tests
/// assert on.
fn first_frame(frames_dir: &str) -> image::RgbaImage {
    let mut paths: Vec<_> = fs::read_dir(frames_dir)
        .expect("frames dir exists")
        .map(|e| e.expect("dir entry").path())
        .collect();
    paths.sort();
    let path = paths.first().expect("at least one frame was written");
    image::open(path).expect("decode frame").to_rgba8()
}

#[tokio::test]
async fn an_expired_recording_releases_the_capture_lock() {
    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    recording::session_start(
        &server,
        SessionRecordStartArgs { max_duration_secs: Some(0.5), ..start_args(dir.path()) },
    )
    .await
    .expect("start failed");

    sleep(Duration::from_millis(800)).await;
    timeout(
        Duration::from_secs(3),
        screenshot::session(
            &server,
            SessionScreenshotArgs {
                session_id: SID.to_string(),
                full_page: Some(false),
                ..Default::default()
            },
        ),
    )
    .await
    .expect("expired recording still holds the capture lock")
    .expect("screenshot failed after recording expired");

    recording::session_stop(&server, SessionRecordStopArgs { session_id: SID.to_string() })
        .await
        .expect("stop failed after automatic expiry");
    teardown(&server).await;
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
    let aggregate_retained = result.byte_report["accounting"]["retained"]
        .as_u64()
        .expect("aggregate retained byte count");
    let region_retained = result.regions[0].byte_report["accounting"]["retained"]
        .as_u64()
        .expect("region retained byte count");
    assert!(aggregate_retained > 0);
    assert_eq!(aggregate_retained, region_retained);
    assert!(result.regions[0].output_byte_reports.is_empty());
    assert_eq!(result.format, "jpeg");
    assert!(result.started_at_unix_ms.is_some());
    assert!(result.document_epoch.is_some());
    assert!(result.complete);
    assert!(result.frame_size_pixels.is_some());
    assert!(result.capture_viewport_css.is_some());
    assert_eq!(
        result.frames_dropped,
        result.frames_dropped_by_rate
            + result.frames_dropped_by_limit
            + result.frame_decode_failures
    );

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
    assert!(recording_error_detail(&err).contains("already running"), "got {err:?}");

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
    assert!(recording_error_detail(&err).contains("no recording is running"), "got {err:?}");

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
    assert!(recording_error_detail(&err).contains("mutually exclusive"), "got {err:?}");

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
    assert!(recording_error_detail(&err).contains("maximum"), "got {err:?}");

    teardown(&server).await;
}

/// The acceptance test for CAS-259: not "the argument was accepted" but "the
/// pixels on disk are black".
#[tokio::test]
async fn a_selector_mask_blacks_out_that_element_in_the_written_frames() {
    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    recording::session_start(
        &server,
        SessionRecordStartArgs {
            masks: vec![mask_selector("#box")],
            format: Some("png".to_string()),
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

    assert_eq!(result.masks.len(), 1);
    let [mx, my, mw, mh] = result.masks[0].bbox;
    assert!(mw > 0 && mh > 0, "mask resolved to {:?}", result.masks[0].bbox);
    assert!(result.masks[0].tracked, "a selector mask tracks by default");

    let frame = first_frame(result.regions[0].frames_dir.as_ref().expect("frames_dir"));

    // Inside the mask: black. #box animates through bright hsl colors, so
    // black there can only have come from the mask.
    let inside = frame.get_pixel(mx + mw / 2, my + mh / 2).0;
    assert_eq!(inside, [0, 0, 0, 255], "masked region is not black: {inside:?}");

    // #other sits directly below #box and is a solid green that must survive:
    // masking one element must not black out the frame.
    let below = frame.get_pixel(20, my + mh + 20).0;
    assert_ne!(below, [0, 0, 0, 255], "an unmasked element was covered too");

    teardown(&server).await;
}

#[tokio::test]
async fn a_fixed_bbox_mask_needs_no_dom_and_is_reported_untracked() {
    use voidcrawl_mcp::tools::viewport::BboxArg;

    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    recording::session_start(
        &server,
        SessionRecordStartArgs {
            masks: vec![recording::MaskArg {
                bbox: Some(BboxArg { x: 0, y: 0, width: 60, height: 60 }),
                label: Some("corner".to_string()),
                ..Default::default()
            }],
            format: Some("png".to_string()),
            ..start_args(dir.path())
        },
    )
    .await
    .expect("start failed");

    sleep(Duration::from_secs(1)).await;

    let result =
        recording::session_stop(&server, SessionRecordStopArgs { session_id: SID.to_string() })
            .await
            .expect("stop failed");

    assert_eq!(result.masks.len(), 1);
    assert_eq!(result.masks[0].label, "corner");
    assert!(!result.masks[0].tracked, "a literal rectangle has nothing to re-resolve");
    assert_eq!(result.masks[0].unresolved_ticks, 0);

    let frame = first_frame(result.regions[0].frames_dir.as_ref().expect("frames_dir"));
    assert_eq!(frame.get_pixel(30, 30).0, [0, 0, 0, 255]);

    teardown(&server).await;
}

/// A mask that can't be resolved is a hole in an artifact the caller believes
/// is covered, so it must fail the call rather than be skipped.
#[tokio::test]
async fn a_mask_matching_nothing_fails_before_any_frame_is_captured() {
    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    let err = recording::session_start(
        &server,
        SessionRecordStartArgs {
            masks: vec![mask_selector("#does-not-exist")],
            ..start_args(dir.path())
        },
    )
    .await
    .expect_err("an unresolvable mask must be rejected");
    assert_eq!(err.code().as_str(), "voidcrawl.target.element_not_visible");
    assert_eq!(err.to_string(), "target element was not visible");

    // And nothing was left running to leak the capture lock.
    recording::session_stop(&server, SessionRecordStopArgs { session_id: SID.to_string() })
        .await
        .expect_err("no recording should have started");

    teardown(&server).await;
}

#[tokio::test]
async fn a_mask_with_both_bbox_and_selector_is_rejected() {
    use voidcrawl_mcp::tools::viewport::BboxArg;

    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    let err = recording::session_start(
        &server,
        SessionRecordStartArgs {
            masks: vec![recording::MaskArg {
                bbox: Some(BboxArg { x: 0, y: 0, width: 10, height: 10 }),
                selector: Some(css("#box")),
                ..Default::default()
            }],
            ..start_args(dir.path())
        },
    )
    .await
    .expect_err("an ambiguous mask must be rejected");
    assert!(recording_error_detail(&err).contains("not both"), "got {err:?}");

    teardown(&server).await;
}

#[tokio::test]
async fn an_empty_mask_is_rejected_rather_than_silently_covering_nothing() {
    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    let err = recording::session_start(
        &server,
        SessionRecordStartArgs {
            masks: vec![recording::MaskArg::default()],
            ..start_args(dir.path())
        },
    )
    .await
    .expect_err("a mask with neither field must be rejected");
    assert!(recording_error_detail(&err).contains("needs either"), "got {err:?}");

    teardown(&server).await;
}

/// Masking and cropping are orthogonal: a caller crops to the form and masks
/// a field inside it.
#[tokio::test]
async fn a_mask_applies_inside_a_cropped_region() {
    let server = server_with_page().await;
    let dir = tempfile::tempdir().expect("tempdir");

    recording::session_start(
        &server,
        SessionRecordStartArgs {
            selectors: vec![css("#box")],
            masks: vec![mask_selector("#box")],
            format: Some("png".to_string()),
            ..start_args(dir.path())
        },
    )
    .await
    .expect("start failed");

    sleep(Duration::from_secs(1)).await;

    let result =
        recording::session_stop(&server, SessionRecordStopArgs { session_id: SID.to_string() })
            .await
            .expect("stop failed");

    let frame = first_frame(result.regions[0].frames_dir.as_ref().expect("frames_dir"));
    let (w, h) = (frame.width(), frame.height());
    assert_eq!(
        frame.get_pixel(w / 2, h / 2).0,
        [0, 0, 0, 255],
        "the crop was cut from a masked frame"
    );

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
