#![allow(clippy::panic, clippy::unwrap_used, reason = "benchmark contract test harness")]

use std::{fs, path::PathBuf};
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}
#[test]
fn fixtures_are_manifested_and_generator_is_current() {
    voidcrawl_benchmarks::verify_fixture_manifest()
        .unwrap_or_else(|error| panic!("fixture manifest: {error}"));
    voidcrawl_benchmarks::fixture_generator_is_current(&root())
        .unwrap_or_else(|error| panic!("fixture generator: {error}"));
}
#[test]
fn public_and_non_http_urls_are_rejected() {
    assert!(voidcrawl_benchmarks::require_loopback_url("http://127.0.0.1:1234/index.html").is_ok());
    for url in ["https://example.com", "http://192.0.2.1/x", "data:text/html,x", "file:///tmp/x"] {
        assert!(voidcrawl_benchmarks::require_loopback_url(url).is_err(), "{url}");
    }
}
#[test]
fn warm_byte_capture_and_matrix_are_executable_paths() {
    let root = root();
    let criterion =
        fs::read_to_string(root.join("voidcrawl-benchmarks/benches/criterion_browser.rs")).unwrap();
    let support = fs::read_to_string(root.join("voidcrawl-benchmarks/src/lib.rs")).unwrap();
    for required in [
        "BrowserByteLimit::try_from",
        "arm_navigation_capture",
        "capture.finish()",
        "byte_report()",
        "black_box(",
        "CONCURRENCY_MATRIX",
        "run_concurrency_matrix",
        "acquire_timed",
        "release_checked",
        "record_concurrency_report",
        "assert_eq!(report.failures, 0",
        "semaphore_wait_ms_sum",
        "throughput_per_second",
        "cleanup_complete, \"isolated context disposal failed",
        "1, tabs_per_browser: 1, in_flight: 1",
        "1, tabs_per_browser: 2, in_flight: 2",
        "2, tabs_per_browser: 2, in_flight: 4",
    ] {
        assert!(
            criterion.contains(required) || support.contains(required),
            "missing executable path {required}"
        );
    }
    assert!(
        !criterion.contains("Throughput::Elements"),
        "fixed Criterion throughput is misleading"
    );
    assert!(criterion.contains("warm_navigation_load"));
    assert!(criterion.contains("tokio::join!(page.wait_for_navigation(), page.navigate(&url))"));
    assert!(
        !criterion.contains("domcontentloaded") && !criterion.contains("ready_state_probe"),
        "navigation completion workload must not claim an unobserved lifecycle event"
    );
    let benchmark = criterion.find("group.bench_with_input").unwrap();
    let evidence = criterion.rfind("record_concurrency_report").unwrap();
    assert!(evidence > benchmark, "concurrency evidence must be written outside timed samples");
}
#[test]
fn profiler_and_dashboard_normalize_real_metrics() {
    let root = root();
    let profile =
        fs::read_to_string(root.join("voidcrawl-benchmarks/src/bin/profile_browser.rs")).unwrap();
    let dashboard = fs::read_to_string(root.join("scripts/summarize-benchmark-change.py")).unwrap();
    for required in [
        "tokio::spawn",
        "SAMPLE_INTERVAL",
        "cmdline",
        "--type=renderer",
        "--type=gpu-process",
        "--type=utility",
        "aggregate",
        "close_ms",
        "chrome_count_after_close",
        "descendants(id())",
        "navigation_load",
    ] {
        assert!(profile.contains(required), "missing profiler path {required}");
    }
    for required in [
        "criterion_mean",
        "sample_nearest_rank_p{percentile}",
        "workload",
        "peak_rss_kib",
        "f\"perf_{event}\"",
        "cycles",
        "instructions",
        "unavailable",
        "invalid",
        "numeric(",
    ] {
        assert!(dashboard.contains(required), "missing dashboard normalization {required}");
    }
}
#[test]
fn runner_isolates_artifacts_and_publishes_immutable_runs_atomically() {
    let runner = fs::read_to_string(root().join("scripts/run-cas-317.sh")).unwrap();
    for required in [
        "manifest.json",
        "raw_chromiumoxide_cdp=unavailable",
        "raw_chromiumoxide_reason=",
        "perf=unavailable_or_permission_denied",
        "mktemp",
        "CARGO_TARGET_DIR=\"$stage/target\"",
        "VOIDCRAWL_CONCURRENCY_EVIDENCE=\"$stage/concurrency.jsonl\"",
        "mv \"$stage\" \"$run_dir\"",
        "mv -Tf \"$class_root/.latest.$run_id\" \"$class_root/latest\"",
        "rm -rf \"$stage/target\"",
        "runs/$run_id",
    ] {
        assert!(runner.contains(required), "missing {required}");
    }
}

#[test]
fn dashboard_retains_all_supported_measurement_classes() {
    let dashboard =
        fs::read_to_string(root().join("scripts/summarize-benchmark-change.py")).unwrap();
    for required in [
        "kind == \"allocations\"",
        "max_live",
        "allocated",
        "kind == \"heap\"",
        "massif_peak_heap_extra_stack_bytes",
    ] {
        assert!(dashboard.contains(required), "missing normalization path {required}");
    }
}
