//! Black-box contract tests for CAS-314 observation scopes.
//!
//! These tests use only a loopback HTTP fixture. Run serially:
//!
//!     cargo test -p void_crawl_core --test observation_scope --
//! --test-threads=1
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use tokio::time::{sleep, timeout};
use void_crawl_core::{
    BrowserPool, BrowserSession, MeasuredCount, MeasurementUnavailableReason, ObservationEventKind,
    ObservationOptions, ObservationReport, ObservationTermination, PoolConfig,
    RuntimeDiagnosticKind,
};

struct LocalFixture {
    address: SocketAddr,
    stop:    Arc<AtomicBool>,
    worker:  Option<thread::JoinHandle<()>>,
}

impl LocalFixture {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fixture");
        listener.set_nonblocking(true).expect("make fixture listener nonblocking");
        let address = listener.local_addr().expect("fixture address");
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                        let mut request = [0_u8; 2_048];
                        let _ = stream.read(&mut request);
                        let body = "<!doctype html><title>CAS-314</title><script>console.log('first');throw new Error('first');</script>";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.flush();
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self { address, stop, worker: Some(worker) }
    }

    fn url(&self) -> String {
        format!("http://{}/initial-document", self.address)
    }
}

impl Drop for LocalFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("join loopback fixture");
        }
    }
}

async fn session() -> BrowserSession {
    BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch local Chromium")
}

fn console_only(max_events: usize, max_duration: Duration) -> ObservationOptions {
    ObservationOptions {
        collect_network: false,
        collect_console: true,
        collect_exceptions: false,
        max_events,
        max_diagnostic_bytes: 64 * 1024,
        max_duration,
    }
}

fn assert_event_accounting(report: &ObservationReport, limit: usize) {
    let count = u64::try_from(report.events.len()).expect("small fixture event count");
    assert_eq!(report.accounting.events.admitted, MeasuredCount::Known { value: count });
    assert_eq!(report.accounting.events.retained, MeasuredCount::Known { value: count });
    assert_eq!(
        report.accounting.events.dropped,
        MeasuredCount::Unavailable { reason: MeasurementUnavailableReason::ProviderDidNotReport }
    );
    assert!(report.events.len() <= limit, "retained events exceeded configured bound");
}

#[tokio::test]
async fn observers_are_armed_before_navigation_and_capture_initial_signals() {
    let fixture = LocalFixture::start();
    let browser = session().await;
    let page = browser.new_blank_page().await.expect("new blank page");
    let options = ObservationOptions {
        max_events: 64,
        max_duration: Duration::from_secs(5),
        ..ObservationOptions::default()
    };

    let scope = page.arm_observation(options).await.expect("arm before navigation");
    page.navigate(&fixture.url()).await.expect("navigate to loopback fixture");
    page.evaluate_js("document.readyState").await.expect("wait for initial script execution");
    sleep(Duration::from_millis(25)).await;
    let report = scope.finish().await.expect("finish observation");

    assert_eq!(report.termination, ObservationTermination::Finished);
    assert!(report.cleanup_complete);
    assert!(report.events.windows(2).all(|pair| {
        pair[0].sequence < pair[1].sequence && pair[0].offset_micros <= pair[1].offset_micros
    }));
    assert_event_accounting(&report, options.max_events);
    for expected in [
        ObservationEventKind::DocumentRequestStarted,
        ObservationEventKind::ConsoleApiCalled,
        ObservationEventKind::RuntimeExceptionThrown,
    ] {
        assert!(
            report.events.iter().any(|event| event.kind == expected),
            "initial event was missed: {expected:?}"
        );
    }

    assert!(report.diagnostics.iter().any(|diagnostic| {
        matches!(diagnostic.kind, RuntimeDiagnosticKind::Console { .. })
            && diagnostic.text().text().is_some_and(|text| text.contains("first"))
    }));
    assert!(report.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind == RuntimeDiagnosticKind::Exception
            && diagnostic.text().text().is_some_and(|text| text.contains("first"))
    }));
    let serialized = serde_json::to_string(&report).expect("serialize safe report");
    assert!(!serialized.contains("first"));
    assert!(!serialized.contains("\"text\""));

    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn diagnostic_payload_limit_is_explicit_and_safe_by_default() {
    let browser = session().await;
    let page = browser.new_blank_page().await.expect("new blank page");
    let options =
        ObservationOptions { max_diagnostic_bytes: 5, ..console_only(8, Duration::from_secs(5)) };
    let scope = page.arm_observation(options).await.expect("arm diagnostic scope");
    page.evaluate_js("console.log('supersecret')").await.expect("emit diagnostic");
    sleep(Duration::from_millis(10)).await;
    let report = scope.finish().await.expect("finish diagnostic scope");
    let diagnostic = report.diagnostics.first().expect("captured diagnostic");

    assert_eq!(diagnostic.retained_bytes, 5);
    assert!(diagnostic.complete_bytes > diagnostic.retained_bytes);
    assert!(diagnostic.truncated);
    assert_eq!(diagnostic.text().bytes(), b"super");
    assert_eq!(report.diagnostic_bytes_retained, 5);
    assert!(report.diagnostic_bytes_dropped > 0);
    assert!(!serde_json::to_string(&report).expect("serialize report").contains("super"));

    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn event_limit_deadline_and_interrupt_are_explicit() {
    let browser = session().await;
    let page = browser.new_blank_page().await.expect("new blank page");

    let limited_options = console_only(2, Duration::from_secs(5));
    let limited = page.arm_observation(limited_options).await.expect("arm limited scope");
    page.evaluate_js("console.log('1');console.log('2');console.log('3')")
        .await
        .expect("emit console burst");
    let limited = limited.wait().await.expect("wait for event limit");
    assert_eq!(limited.termination, ObservationTermination::EventLimitReached);
    assert_event_accounting(&limited, limited_options.max_events);

    let deadline_options = console_only(8, Duration::from_millis(30));
    let deadline = page.arm_observation(deadline_options).await.expect("arm deadline scope");
    let deadline = deadline.wait().await.expect("wait for deadline");
    assert_eq!(deadline.termination, ObservationTermination::DeadlineReached);
    assert_event_accounting(&deadline, deadline_options.max_events);

    let interrupt_options = console_only(8, Duration::from_secs(5));
    let interrupted =
        page.arm_observation(interrupt_options).await.expect("arm interruptible scope");
    let interrupted = interrupted.interrupt().await.expect("interrupt scope");
    assert_eq!(interrupted.termination, ObservationTermination::Interrupted);
    assert_event_accounting(&interrupted, interrupt_options.max_events);

    let cancel_options = console_only(8, Duration::from_secs(5));
    let cancelled = page.arm_observation(cancel_options).await.expect("arm cancellable scope");
    let cancelled = cancelled.cancel().await.expect("cancel scope");
    assert_eq!(cancelled.termination, ObservationTermination::Cancelled);
    assert_event_accounting(&cancelled, cancel_options.max_events);

    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn page_close_reports_provider_disconnect_and_cleans_up() {
    let browser = session().await;
    let page = browser.new_blank_page().await.expect("new blank page");
    let options = console_only(8, Duration::from_secs(2));
    let scope = page.arm_observation(options).await.expect("arm scope");

    page.close().await.expect("close observed page");
    let report = scope.wait().await.expect("wait for page-close termination");

    assert_eq!(report.termination, ObservationTermination::ProviderDisconnected);
    assert!(report.cleanup_complete);
    assert_event_accounting(&report, options.max_events);
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn dropped_scope_does_not_leak_a_pool_permit() {
    let browser = session().await;
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 1,
        auto_evict:           false,
    };
    let pool = BrowserPool::new(config, vec![browser]);
    let tab = pool.acquire().await.expect("acquire only pool slot");
    let scope = tab
        .page
        .arm_observation(console_only(8, Duration::from_secs(5)))
        .await
        .expect("arm pooled-page scope");
    drop(scope);
    pool.release(tab).await;

    let tab = timeout(Duration::from_secs(1), pool.acquire())
        .await
        .expect("pool acquire did not deadlock")
        .expect("pool slot remained available");
    pool.release(tab).await;
    pool.close().await.expect("close pool");
}

#[tokio::test]
async fn dropped_scope_does_not_poison_later_observation_or_cleanup() {
    let browser = session().await;
    let page = browser.new_blank_page().await.expect("new blank page");

    let abandoned = page
        .arm_observation(console_only(8, Duration::from_secs(5)))
        .await
        .expect("arm scope to drop");
    drop(abandoned);

    let next = page
        .arm_observation(console_only(8, Duration::from_secs(5)))
        .await
        .expect("arm scope after dropped predecessor");
    page.evaluate_js("console.log('after-drop')").await.expect("emit subsequent console event");
    sleep(Duration::from_millis(10)).await;
    let report = next.finish().await.expect("finish subsequent scope");
    assert!(report.events.iter().any(|event| event.kind == ObservationEventKind::ConsoleApiCalled));

    page.close().await.expect("page remains closable");
    browser.close().await.expect("browser remains closable");
}
