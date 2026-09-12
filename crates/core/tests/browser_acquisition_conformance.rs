//! Versioned, loopback-only Browser Acquisition conformance corpus.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{
    collections::HashSet,
    fs,
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

use chromiumoxide::cdp::browser_protocol::page::CrashParams;
use serde::Deserialize;
use tokio::time::sleep;
use void_crawl_core::{
    AccessibilitySnapshotOptions, BrowserSession, BrowserTarget, BrowserTargetKind, MeasuredCount,
    NavigationCaptureOptions, ObservationEventKind, ObservationOptions, ObservationTermination,
    ResponseBodyState, RuntimeDiagnosticKind, ScreenshotOptions, SnapshotState,
    SourceBodyUnavailableReason,
};

const STATIC: &str = include_str!("conformance/static.html");
const DELAYED_DOM: &str = include_str!("conformance/delayed-dom.html");
const RUNTIME_ERROR: &str = include_str!("conformance/runtime-error.html");
const FRAME_PARENT: &str = include_str!("conformance/frame-parent.html");
const FRAME_CHILD: &str = include_str!("conformance/frame-child.html");
const VISUAL_LAYOUT: &str = include_str!("conformance/visual-layout.html");
const SERVICE_WORKER: &str = include_str!("conformance/service-worker.html");
const SERVICE_WORKER_JS: &str = include_str!("conformance/conformance-sw.js");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema:     String,
    version:    u64,
    provenance: Provenance,
    scenarios:  Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    network:               String,
    clock:                 String,
    secrets:               String,
    public_sites_required: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    id:       String,
    fixture:  String,
    expected: Vec<String>,
    coverage: String,
}

const EXPECTED_FACT_VOCABULARY: &[&str] = &[
    "body_unavailable",
    "cache_state_explicit",
    "cleanup_complete",
    "complete",
    "console",
    "coordinate_space",
    "deadline",
    "dimensions",
    "document_cleared",
    "document_epoch",
    "dropped_explicit",
    "event_limit",
    "exact_retained_bytes",
    "exception",
    "failed",
    "final_response",
    "finished",
    "frame_payload_or_unavailable",
    "frame_scope",
    "in_flight_known",
    "interrupted",
    "isolated_browser_context",
    "no_partial_bytes_claim",
    "permit_restored",
    "pre_navigation",
    "provider_disconnected",
    "redirect_hops",
    "rendered_dom_changed",
    "same_document_epoch",
    "service_worker_state_explicit_or_unavailable",
    "shared_browser_profile",
    "state_removed",
    "state_retained",
    "stream_may_remain_open",
    "tab_disposed",
    "truncated",
];

struct FixtureServer {
    url:    String,
    stop:   mpsc::Sender<()>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind conformance server");
        listener.set_nonblocking(true).expect("nonblocking conformance server");
        let address = listener.local_addr().expect("fixture address");
        let (stop, stopped) = mpsc::channel();
        let thread = thread::spawn(move || {
            loop {
                if stopped.try_recv().is_ok() {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 4096];
                        let read = stream.read(&mut request).unwrap_or(0);
                        let request = String::from_utf8_lossy(&request[..read]);
                        let path = request.split_whitespace().nth(1).unwrap_or("/");
                        if path == "/partial-body" {
                            let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nshort",
                        );
                            continue;
                        }
                        if path == "/endless" {
                            let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\nx",
                        );
                            thread::sleep(Duration::from_millis(500));
                            continue;
                        }
                        let (status, content_type, body) = match path {
                            "/static.html" => ("200 OK", "text/html", STATIC),
                            "/delayed-dom.html" => ("200 OK", "text/html", DELAYED_DOM),
                            "/runtime-error.html" => ("200 OK", "text/html", RUNTIME_ERROR),
                            "/frame-parent.html" => ("200 OK", "text/html", FRAME_PARENT),
                            "/frame-child.html" => ("200 OK", "text/html", FRAME_CHILD),
                            "/visual-layout.html" => ("200 OK", "text/html", VISUAL_LAYOUT),
                            "/service-worker.html" => ("200 OK", "text/html", SERVICE_WORKER),
                            "/conformance-sw.js" => {
                                ("200 OK", "application/javascript", SERVICE_WORKER_JS)
                            }
                            _ => ("404 Not Found", "text/plain", "missing fixture"),
                        };
                        let response = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("conformance fixture accept: {error}"),
                }
            }
        });
        Self { url: format!("http://{address}"), stop, thread: Some(thread) }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.url)
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            thread.join().expect("join conformance server");
        }
    }
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance")
}

fn coverage_file(module: &str) -> PathBuf {
    match module {
        "cross_origin_eval" => {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../mcp_server/tests/cross_origin_eval.rs")
        }
        "browser_acquisition_conformance" => {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/browser_acquisition_conformance.rs")
        }
        other => Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/{other}.rs")),
    }
}

#[test]
fn manifest_is_versioned_local_and_references_real_coverage() {
    let manifest: Manifest =
        serde_json::from_str(include_str!("conformance/browser-acquisition-v1.json"))
            .expect("valid conformance manifest");
    assert_eq!(manifest.schema, "voidcrawl.browser-acquisition-conformance");
    assert_eq!(manifest.version, 1);
    assert_eq!(manifest.provenance.network, "loopback-only");
    assert_eq!(manifest.provenance.clock, "relative-or-category-only");
    assert_eq!(manifest.provenance.secrets, "synthetic");
    assert!(!manifest.provenance.public_sites_required);
    assert!(manifest.scenarios.len() >= 18);

    let mut ids = HashSet::new();
    for scenario in manifest.scenarios {
        assert!(ids.insert(scenario.id.clone()), "duplicate scenario {}", scenario.id);
        assert!(!scenario.expected.is_empty(), "{} has no expectations", scenario.id);
        let mut expected = HashSet::new();
        for fact in &scenario.expected {
            assert!(expected.insert(fact), "{} repeats expected fact {fact}", scenario.id);
            assert!(
                EXPECTED_FACT_VOCABULARY.contains(&fact.as_str()),
                "{} uses unknown expected fact {fact}",
                scenario.id,
            );
        }
        assert!(!scenario.coverage.is_empty(), "{} has no coverage", scenario.id);
        if !scenario.fixture.contains(':') {
            assert!(corpus_dir().join(&scenario.fixture).is_file(), "missing {}", scenario.fixture);
        }
        let (module, test) = scenario.coverage.split_once("::").expect("coverage is module::test");
        let source = fs::read_to_string(coverage_file(module)).expect("read coverage source");
        assert!(
            source.contains(&format!("fn {test}(")),
            "coverage target {} is stale",
            scenario.coverage
        );
    }
}

fn assert_contract(scenario_id: &str, expected: &[&str]) {
    let manifest: Manifest =
        serde_json::from_str(include_str!("conformance/browser-acquisition-v1.json"))
            .expect("valid conformance manifest");
    let scenario = manifest
        .scenarios
        .iter()
        .find(|scenario| scenario.id == scenario_id)
        .unwrap_or_else(|| panic!("missing contract scenario {scenario_id}"));
    assert_eq!(scenario.expected, expected, "contract expectations drifted for {scenario_id}");
}

async fn session() -> BrowserSession {
    BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch Chromium")
}

#[tokio::test]
async fn static_delayed_runtime_frame_and_visual_fixtures_are_observable() {
    let fixtures = FixtureServer::start();
    let browser = session().await;
    let page = browser.new_blank_page().await.expect("blank page");

    page.navigate(&fixtures.url("/static.html")).await.expect("static navigation");
    let static_dom = page.rendered_dom_snapshot(1024 * 1024).await.expect("static DOM");
    assert_eq!(static_dom.state, SnapshotState::Complete);
    assert!(static_dom.bytes().windows(b"Static fixture".len()).any(|w| w == b"Static fixture"));

    page.navigate(&fixtures.url("/delayed-dom.html")).await.expect("delayed navigation");
    let before = page.rendered_dom_snapshot(1024 * 1024).await.expect("initial DOM");
    sleep(Duration::from_millis(80)).await;
    let after = page.rendered_dom_snapshot(1024 * 1024).await.expect("settled DOM");
    assert_eq!(before.scope.epoch, after.scope.epoch);
    assert!(after.bytes().windows(b"settled".len()).any(|w| w == b"settled"));

    let scope = page
        .arm_observation(ObservationOptions {
            max_duration: Duration::from_secs(3),
            ..ObservationOptions::default()
        })
        .await
        .expect("arm runtime observation");
    page.navigate(&fixtures.url("/runtime-error.html")).await.expect("runtime fixture");
    sleep(Duration::from_millis(25)).await;
    let report = scope.finish().await.expect("finish runtime observation");
    assert!(report.events.iter().any(|event| event.kind == ObservationEventKind::ConsoleApiCalled));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.kind, RuntimeDiagnosticKind::Console { .. }))
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == RuntimeDiagnosticKind::Exception)
    );

    page.navigate(&fixtures.url("/frame-parent.html")).await.expect("frame fixture");
    let frame = page
        .accessibility_snapshot_in_frame(
            "frame-child.html",
            AccessibilitySnapshotOptions::default(),
        )
        .await
        .expect("frame AX");
    assert_eq!(frame.state, SnapshotState::Complete);

    page.navigate(&fixtures.url("/visual-layout.html")).await.expect("visual fixture");
    let visual = page
        .visual_snapshot(ScreenshotOptions {
            selector: Some(BrowserTarget {
                kind:  BrowserTargetKind::Css,
                value: "#moving".into(),
                regex: None,
                name:  None,
                nth:   None,
                x:     None,
                y:     None,
            }),
            ..ScreenshotOptions::default()
        })
        .await
        .expect("visual snapshot");
    assert_eq!(visual.image_width_pixels, 80);
    assert_eq!(visual.image_height_pixels, 40);
    assert!(visual.retained_bytes > 0);
    assert_contract("static_navigation", &["finished", "complete"]);
    assert_contract("delayed_dom", &["same_document_epoch", "rendered_dom_changed"]);
    assert_contract("synchronous_runtime_error", &["console", "exception", "pre_navigation"]);
    assert_contract("same_process_frame", &["frame_scope"]);
    assert_contract("visual_layout_change", &["dimensions", "coordinate_space", "document_epoch"]);

    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn active_network_can_finish_with_explicit_in_flight_accounting() {
    let fixtures = FixtureServer::start();
    let browser = session().await;
    let page = browser.new_page(&fixtures.url("/static.html")).await.expect("static page");
    let scope = page
        .arm_observation(ObservationOptions {
            collect_console: false,
            collect_exceptions: false,
            max_duration: Duration::from_secs(2),
            ..ObservationOptions::default()
        })
        .await
        .expect("arm network observation");
    page.evaluate_js("void fetch('/endless')").await.expect("start endless request");
    sleep(Duration::from_millis(30)).await;
    let report = scope.finish().await.expect("finish active network observation");
    assert_eq!(report.termination, ObservationTermination::Finished);
    assert!(matches!(
        report.accounting.in_flight_requests,
        MeasuredCount::Known { value } if value >= 1
    ));
    assert!(report.cleanup_complete);
    assert_contract("endless_network", &["finished", "in_flight_known", "cleanup_complete"]);
    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn mid_body_disconnect_is_an_explicit_failed_source_not_partial_bytes() {
    let fixtures = FixtureServer::start();
    let browser = session().await;
    let page = browser.new_blank_page().await.expect("blank page");
    let capture = page
        .arm_navigation_capture(NavigationCaptureOptions {
            max_duration: Duration::from_secs(3),
            ..NavigationCaptureOptions::default()
        })
        .await
        .expect("arm navigation capture");
    let _ = page.navigate(&fixtures.url("/partial-body")).await;
    sleep(Duration::from_millis(25)).await;
    let report = capture.finish().await.expect("partial response report");
    let source = report.main_document.expect("main document fact");
    assert_eq!(source.body_state, ResponseBodyState::Unavailable);
    assert!(matches!(
        source.body_unavailable,
        Some(
            SourceBodyUnavailableReason::RequestFailed
                | SourceBodyUnavailableReason::CdpBodyUnavailable
        )
    ));
    assert!(source.body().is_empty());
    assert!(report.cleanup_complete);
    assert_contract(
        "partial_response_body",
        &["failed", "body_unavailable", "no_partial_bytes_claim"],
    );
    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn loopback_cross_origin_frame_is_payload_or_explicitly_unavailable() {
    let parent = FixtureServer::start();
    let child = FixtureServer::start();
    let browser = BrowserSession::builder()
        .headless()
        .no_sandbox()
        .arg("disable-features=")
        .launch()
        .await
        .expect("launch site-isolated Chromium");
    let page = browser.new_page(&parent.url("/static.html")).await.expect("parent page");
    let child_url = child.url("/frame-child.html");
    let encoded_child = serde_json::to_string(&child_url).expect("encode child URL");
    page.evaluate_js(&format!(
        "(() => {{ const f=document.createElement('iframe'); f.src={encoded_child}; document.body.append(f); }})()"
    ))
    .await
    .expect("append cross-origin frame");
    sleep(Duration::from_millis(150)).await;
    assert!(page.frame_urls().await.expect("frame URLs").contains(&child_url));

    let snapshot = page
        .accessibility_snapshot_in_frame(
            "/frame-child.html",
            AccessibilitySnapshotOptions::default(),
        )
        .await
        .expect("truthful cross-origin frame result");
    assert!(matches!(
        snapshot.state,
        SnapshotState::Complete
            | SnapshotState::Unavailable {
                reason: void_crawl_core::SnapshotUnavailableReason::FrameUnavailable,
            }
    ));
    assert_contract("oopif", &["frame_payload_or_unavailable"]);
    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn renderer_crash_is_bounded_by_deadline_when_stream_stays_open() {
    let browser = session().await;
    let page = browser.new_blank_page().await.expect("blank page");
    let scope = page
        .arm_observation(ObservationOptions {
            collect_network: false,
            max_duration: Duration::from_secs(3),
            ..ObservationOptions::default()
        })
        .await
        .expect("arm before renderer crash");

    let _ = page.inner().execute(CrashParams::default()).await;
    let report = scope.finish().await.expect("renderer crash report");
    assert_eq!(report.termination, ObservationTermination::DeadlineReached);
    assert!(report.cleanup_complete);
    assert_contract("renderer_crash", &["deadline", "cleanup_complete", "stream_may_remain_open"]);
    drop(browser);
}

#[tokio::test]
async fn browser_disconnect_reaches_the_provider_disconnect_terminal() {
    let browser = session().await;
    let page = browser.new_blank_page().await.expect("blank page");
    let scope = page
        .arm_observation(ObservationOptions {
            collect_network: false,
            max_duration: Duration::from_secs(3),
            ..ObservationOptions::default()
        })
        .await
        .expect("arm before browser close");

    browser.close().await.expect("close browser");
    let report = scope.finish().await.expect("browser disconnect report");
    assert_eq!(report.termination, ObservationTermination::ProviderDisconnected);
    assert!(report.cleanup_complete);
    assert_contract("browser_disconnect", &["provider_disconnected", "cleanup_complete"]);
}
