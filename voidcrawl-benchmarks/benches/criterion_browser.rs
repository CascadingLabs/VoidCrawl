#![allow(
    clippy::panic,
    clippy::semicolon_if_nothing_returned,
    reason = "benchmark harness failures stop measurement and Criterion closures return handles"
)]

use std::{hint::black_box, time::Duration};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use void_crawl_core::{
    BrowserByteLimit, BrowserPool, BrowserSession, NavigationCaptureOptions, PoolConfig,
    ScreenshotOptions,
};
use voidcrawl_benchmarks::{
    CONCURRENCY_MATRIX, ConcurrencyConfig, LoopbackServer, matrix_pool_config,
    record_concurrency_report, run_concurrency_matrix,
};

/// Criterion requires a non-fallible callback; panics here intentionally fail a
/// harness run.
#[allow(clippy::panic)]
fn browser_workloads(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap_or_else(|error| panic!("runtime: {error}"));
    let server = runtime
        .block_on(LoopbackServer::start())
        .unwrap_or_else(|error| panic!("loopback server: {error}"));
    let url = server.url();
    let bytes_url = server.bytes_url();
    let session = runtime
        .block_on(BrowserSession::builder().no_stealth().launch())
        .unwrap_or_else(|error| panic!("browser: {error}"));
    let page =
        runtime.block_on(session.new_blank_page()).unwrap_or_else(|error| panic!("page: {error}"));
    c.bench_function("cold_browser_launch_close", |b| {
        b.to_async(&runtime).iter(|| async {
            let cold = BrowserSession::builder()
                .no_stealth()
                .launch()
                .await
                .unwrap_or_else(|error| panic!("cold launch: {error}"));
            cold.close().await.unwrap_or_else(|error| panic!("cold close: {error}"));
        })
    });
    c.bench_function("warm_navigation_response", |b| {
        b.to_async(&runtime).iter(|| async {
            page.navigate(&url)
                .await
                .unwrap_or_else(|error| panic!("navigation response: {error}"));
        })
    });
    c.bench_function("warm_navigation_load", |b| {
        b.to_async(&runtime).iter(|| async {
            let (loaded, navigated) = tokio::join!(page.wait_for_navigation(), page.navigate(&url));
            navigated.unwrap_or_else(|error| panic!("navigation: {error}"));
            loaded.unwrap_or_else(|error| panic!("load completion: {error}"));
        })
    });
    c.bench_function("warm_navigation_network_idle", |b| {
        b.to_async(&runtime).iter(|| async {
            page.goto_and_wait_for_idle(&url, Duration::from_secs(10))
                .await
                .unwrap_or_else(|error| panic!("network idle: {error}"));
        })
    });
    runtime
        .block_on(page.navigate(&url))
        .unwrap_or_else(|error| panic!("selector setup navigation: {error}"));
    c.bench_function("warm_selector_wait", |b| {
        b.to_async(&runtime).iter(|| async {
            page.wait_for_selector("#box", Duration::from_secs(10))
                .await
                .unwrap_or_else(|error| panic!("selector: {error}"));
        })
    });
    runtime
        .block_on(page.navigate(&url))
        .unwrap_or_else(|error| panic!("DOM setup navigation: {error}"));
    c.bench_function("warm_dom_content", |b| {
        b.to_async(&runtime).iter(|| async {
            black_box(page.content().await.unwrap_or_else(|error| panic!("DOM: {error}")));
        })
    });
    runtime
        .block_on(page.navigate(&url))
        .unwrap_or_else(|error| panic!("AX setup navigation: {error}"));
    c.bench_function("warm_accessibility_tree", |b| {
        b.to_async(&runtime).iter(|| async {
            black_box(
                page.get_full_ax_tree(None).await.unwrap_or_else(|error| panic!("AX: {error}")),
            );
        })
    });
    runtime
        .block_on(page.navigate(&url))
        .unwrap_or_else(|error| panic!("screenshot setup navigation: {error}"));
    c.bench_function("warm_viewport_screenshot", |b| {
        b.to_async(&runtime).iter(|| async {
            black_box(
                page.screenshot(ScreenshotOptions::default().viewport_only())
                    .await
                    .unwrap_or_else(|error| panic!("screenshot: {error}")),
            );
        })
    });
    c.bench_function("warm_network_byte_control", |b| {
        b.to_async(&runtime).iter(|| async {
            let limit = BrowserByteLimit::try_from(16_u64 * 1024)
                .unwrap_or_else(|error| panic!("byte limit: {error}"));
            let capture = page
                .arm_navigation_capture(
                    NavigationCaptureOptions::default()
                        .with_source_limit(limit)
                        .unwrap_or_else(|error| panic!("capture options: {error}")),
                )
                .await
                .unwrap_or_else(|error| panic!("arm capture: {error}"));
            page.navigate(&bytes_url)
                .await
                .unwrap_or_else(|error| panic!("bytes navigation: {error}"));
            let report =
                capture.finish().await.unwrap_or_else(|error| panic!("finish capture: {error}"));
            let source = report
                .main_document
                .as_ref()
                .unwrap_or_else(|| panic!("main document source was not observed"));
            black_box(source.byte_report().unwrap_or_else(|error| panic!("byte report: {error}")));
        })
    });
    c.bench_function("warm_tab_create_close", |b| {
        b.to_async(&runtime).iter(|| async {
            let tab =
                session.new_blank_page().await.unwrap_or_else(|error| panic!("new tab: {error}"));
            tab.close().await.unwrap_or_else(|error| panic!("close tab: {error}"));
        })
    });
    c.bench_function("warm_isolated_context", |b| {
        b.to_async(&runtime).iter(|| async {
            let context = session
                .new_isolated_context()
                .await
                .unwrap_or_else(|error| panic!("context: {error}"));
            let report = context.dispose().await;
            assert!(report.cleanup_complete, "isolated context disposal failed: {report:?}");
        })
    });
    let pool = BrowserPool::new(
        PoolConfig { browsers: 1, tabs_per_browser: 2, ..PoolConfig::default() },
        vec![session],
    );
    runtime.block_on(pool.warmup()).unwrap_or_else(|error| panic!("pool warmup: {error}"));
    c.bench_function("warm_pool_reuse_tab_reset", |b| {
        b.to_async(&runtime).iter(|| async {
            let tab = pool.acquire().await.unwrap_or_else(|error| panic!("acquire: {error}"));
            tab.page
                .navigate(&url)
                .await
                .unwrap_or_else(|error| panic!("pool navigation: {error}"));
            let report = pool.release_checked(tab).await;
            assert!(report.cleanup_complete, "pool reset failed");
        })
    });
    runtime.block_on(pool.close()).unwrap_or_else(|error| panic!("pool close: {error}"));

    let mut group = c.benchmark_group("warm_concurrency_matrix");
    for config in CONCURRENCY_MATRIX {
        let sessions = (0..config.browsers)
            .map(|_| {
                runtime
                    .block_on(BrowserSession::builder().no_stealth().launch())
                    .unwrap_or_else(|error| panic!("matrix browser: {error}"))
            })
            .collect();
        let matrix_pool = BrowserPool::new(matrix_pool_config(config), sessions);
        runtime
            .block_on(matrix_pool.warmup())
            .unwrap_or_else(|error| panic!("matrix warmup: {error}"));
        group.bench_with_input(
            BenchmarkId::new(
                "browser_tabs_inflight",
                format!("{}x{}x{}", config.browsers, config.tabs_per_browser, config.in_flight),
            ),
            &config,
            |b, config: &ConcurrencyConfig| {
                b.to_async(&runtime).iter(|| async {
                    let report = run_concurrency_matrix(&matrix_pool, &url, config.in_flight).await;
                    assert_eq!(report.failures, 0, "concurrency workload failures: {report:?}");
                    black_box(report);
                })
            },
        );
        // Persist one untimed audit iteration. File I/O must not contaminate
        // Criterion's timed samples.
        let audit = runtime.block_on(run_concurrency_matrix(&matrix_pool, &url, config.in_flight));
        assert_eq!(audit.failures, 0, "concurrency audit failures: {audit:?}");
        record_concurrency_report(config, &audit)
            .unwrap_or_else(|error| panic!("concurrency evidence: {error}"));
        runtime
            .block_on(matrix_pool.close())
            .unwrap_or_else(|error| panic!("matrix close: {error}"));
    }
    group.finish();
}
criterion_group! { name = benches; config = Criterion::default().sample_size(10).warm_up_time(Duration::from_secs(1)).measurement_time(Duration::from_secs(2)); targets = browser_workloads }
criterion_main!(benches);
