//! Fresh-process L2 workloads with concurrent, bounded-rate Chrome-tree
//! sampling.
#![allow(clippy::disallowed_macros, reason = "profile binary emits one JSON result")]

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs,
    process::id,
    sync::Arc,
    time::{Duration, Instant},
};

use serde::Serialize;
use tokio::{
    sync::{Mutex, watch},
    time::sleep,
};
use void_crawl_core::{
    BrowserByteLimit, BrowserSession, NavigationCaptureOptions, ScreenshotOptions,
};
use voidcrawl_benchmarks::LoopbackServer;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(25);
#[derive(Clone, Serialize)]
struct Proc {
    pid: u32,
    rss_kib: Option<u64>,
    pss_kib: Option<u64>,
    cpu_ticks: Option<u64>,
    fds: Option<usize>,
    tasks: Option<usize>,
    role: String,
}
#[derive(Serialize)]
struct Aggregate {
    peak_rss_kib: u64,
    peak_pss_kib: u64,
    cpu_ticks_sum: u64,
    fds_peak: usize,
    tasks_peak: usize,
    role_counts_peak: BTreeMap<String, usize>,
}
#[derive(Serialize)]
struct Cleanup {
    close_ms: u128,
    process_cleanup_ms: u128,
    timed_out: bool,
    chrome_count_before_close: usize,
    chrome_count_after_close: usize,
}
#[derive(Serialize)]
struct Sample {
    workload: String,
    elapsed_ms: u128,
    interval_ms: u64,
    samples: Vec<Vec<Proc>>,
    aggregate: Aggregate,
    cleanup: Cleanup,
    unavailable: Vec<String>,
}
fn descendants(root: u32) -> BTreeSet<u32> {
    let mut all = BTreeSet::from([root]);
    loop {
        let old = all.len();
        if let Ok(entries) = fs::read_dir("/proc") {
            for e in entries.flatten() {
                let Ok(pid) = e.file_name().to_string_lossy().parse() else { continue };
                let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
                if stat
                    .rsplit_once(") ")
                    .and_then(|(_, s)| s.split_whitespace().nth(1))
                    .and_then(|v| v.parse().ok())
                    .is_some_and(|ppid| all.contains(&ppid))
                {
                    all.insert(pid);
                }
            }
        }
        if old == all.len() {
            return all;
        }
    }
}
fn proc_sample(pid: u32) -> Proc {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let fields: Vec<_> =
        stat.rsplit_once(") ").map_or_else(Vec::new, |(_, s)| s.split_whitespace().collect());
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    let value = |key| {
        status.lines().find_map(|line| {
            line.strip_prefix(key)
                .and_then(|x| x.split_whitespace().next())
                .and_then(|x| x.parse().ok())
        })
    };
    let pss = fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok().and_then(|s| {
        s.lines().find_map(|l| {
            l.strip_prefix("Pss:")
                .and_then(|x| x.split_whitespace().next())
                .and_then(|x| x.parse().ok())
        })
    });
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let cmdline = String::from_utf8_lossy(&cmdline).replace('\0', " ");
    let role = if cmdline.contains("--type=renderer") {
        "renderer"
    } else if cmdline.contains("--type=gpu-process") {
        "gpu-process"
    } else if cmdline.contains("--type=utility") {
        "utility"
    } else if cmdline.contains("--type=zygote") {
        "zygote"
    } else if cmdline.contains("crashpad_handler") {
        "crashpad"
    } else if cmdline.contains("chrome") || cmdline.contains("chromium") {
        "browser"
    } else {
        "controller"
    }
    .to_owned();
    Proc {
        pid,
        rss_kib: value("VmRSS:"),
        pss_kib: pss,
        cpu_ticks: fields
            .get(11)
            .and_then(|v| v.parse::<u64>().ok())
            .zip(fields.get(12).and_then(|v| v.parse::<u64>().ok()))
            .map(|(a, b)| a + b),
        fds: fs::read_dir(format!("/proc/{pid}/fd")).ok().map(Iterator::count),
        tasks: fs::read_dir(format!("/proc/{pid}/task")).ok().map(Iterator::count),
        role,
    }
}
fn aggregate(samples: &[Vec<Proc>]) -> Aggregate {
    let mut result = Aggregate {
        peak_rss_kib: 0,
        peak_pss_kib: 0,
        cpu_ticks_sum: 0,
        fds_peak: 0,
        tasks_peak: 0,
        role_counts_peak: BTreeMap::default(),
    };
    for sample in samples {
        let mut roles = BTreeMap::new();
        let mut rss_kib = 0_u64;
        let mut pss_kib = 0_u64;
        let mut cpu_ticks = 0_u64;
        let mut fds = 0_usize;
        let mut tasks = 0_usize;
        for proc in sample {
            rss_kib = rss_kib.saturating_add(proc.rss_kib.unwrap_or(0));
            pss_kib = pss_kib.saturating_add(proc.pss_kib.unwrap_or(0));
            cpu_ticks = cpu_ticks.saturating_add(proc.cpu_ticks.unwrap_or(0));
            fds = fds.saturating_add(proc.fds.unwrap_or(0));
            tasks = tasks.saturating_add(proc.tasks.unwrap_or(0));
            *roles.entry(proc.role.clone()).or_insert(0) += 1;
        }
        result.peak_rss_kib = result.peak_rss_kib.max(rss_kib);
        result.peak_pss_kib = result.peak_pss_kib.max(pss_kib);
        result.cpu_ticks_sum = result.cpu_ticks_sum.max(cpu_ticks);
        result.fds_peak = result.fds_peak.max(fds);
        result.tasks_peak = result.tasks_peak.max(tasks);
        for (role, count) in roles {
            result
                .role_counts_peak
                .entry(role)
                .and_modify(|peak| *peak = (*peak).max(count))
                .or_insert(count);
        }
    }
    result
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let workload = env::args()
        .nth(1)
        .unwrap_or_else(|| "navigation_dom_ax_screenshot_network_byte_control".into());
    let server = LoopbackServer::start().await?;
    let url = server.url();
    let bytes_url = server.bytes_url();
    let started = Instant::now();
    let samples: Arc<Mutex<Vec<Vec<Proc>>>> = Arc::new(Mutex::new(Vec::new()));
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let sampler_samples = Arc::clone(&samples);
    let sampler = tokio::spawn(async move {
        loop {
            sampler_samples
                .lock()
                .await
                .push(descendants(id()).into_iter().map(proc_sample).collect());
            tokio::select! { () = sleep(SAMPLE_INTERVAL) => {}, changed = stop_rx.changed() => if changed.is_err() || *stop_rx.borrow() { break } }
        }
    });
    let mut session_builder = BrowserSession::builder().no_stealth();
    if env::var("VOIDCRAWL_BENCH_NO_SANDBOX").as_deref() == Ok("1") {
        session_builder = session_builder.no_sandbox();
    }
    let session = session_builder.launch().await?;
    let page = session.new_blank_page().await?;
    match workload.as_str() {
        "navigation_response" => page.navigate(&url).await?,
        "navigation_load" => {
            let (loaded, navigated) = tokio::join!(page.wait_for_navigation(), page.navigate(&url));
            navigated?;
            loaded?;
        }
        "navigation_network_idle" => {
            let _ = page.goto_and_wait_for_idle(&url, Duration::from_secs(10)).await?;
        }
        "network_byte_control" => {
            let limit = BrowserByteLimit::try_from(16_u64 * 1024)?;
            let capture = page
                .arm_navigation_capture(
                    NavigationCaptureOptions::default().with_source_limit(limit)?,
                )
                .await?;
            page.navigate(&bytes_url).await?;
            let report = capture.finish().await?;
            let source = report.main_document.as_ref().ok_or("main document source missing")?;
            let _ = source.byte_report()?;
        }
        "selector" => {
            page.navigate(&url).await?;
            page.wait_for_selector("#box", Duration::from_secs(10)).await?;
        }
        "dom" => {
            page.navigate(&url).await?;
            let _ = page.content().await?;
        }
        "ax" => {
            page.navigate(&url).await?;
            let _ = page.get_full_ax_tree(None).await?;
        }
        "screenshot" => {
            page.navigate(&url).await?;
            let _ = page.screenshot(ScreenshotOptions::default().viewport_only()).await?;
        }
        "tab_create_close_reset" => {
            let tab = session.new_blank_page().await?;
            tab.close().await?;
        }
        "controller_stop" => {
            let controller = tokio::spawn(async {
                sleep(Duration::from_secs(30)).await;
            });
            controller.abort();
            let _ = controller.await;
        }
        _ => {
            let _ = page.goto_and_wait_for_idle(&url, Duration::from_secs(10)).await?;
            let _ = page.content().await?;
            let _ = page.get_full_ax_tree(None).await?;
            let _ = page.screenshot(ScreenshotOptions::default().viewport_only()).await?;
        }
    }
    // Observe descendants at the close boundary rather than reusing a sampler
    // snapshot that may be up to one interval stale.
    let before = descendants(id())
        .into_iter()
        .map(proc_sample)
        .filter(|process| process.role != "controller")
        .count();
    let close_started = Instant::now();
    session.close().await?;
    let close_ms = close_started.elapsed().as_millis();
    let process_cleanup_started = Instant::now();
    let cleanup_deadline = Duration::from_secs(2);
    let after = loop {
        let remaining = descendants(id())
            .into_iter()
            .map(proc_sample)
            .filter(|process| process.role != "controller")
            .count();
        if remaining == 0 || process_cleanup_started.elapsed() >= cleanup_deadline {
            break remaining;
        }
        sleep(SAMPLE_INTERVAL).await;
    };
    let process_cleanup_ms = process_cleanup_started.elapsed().as_millis();
    let _ = stop_tx.send(true);
    let _ = sampler.await;
    let samples = Arc::try_unwrap(samples).map_err(|_| "sampler references remained")?.into_inner();
    println!(
        "{}",
        serde_json::to_string(&Sample {
            workload,
            elapsed_ms: started.elapsed().as_millis(),
            interval_ms: u64::try_from(SAMPLE_INTERVAL.as_millis()).unwrap_or(u64::MAX),
            aggregate: aggregate(&samples),
            samples,
            cleanup: Cleanup {
                close_ms,
                process_cleanup_ms,
                timed_out: after != 0,
                chrome_count_before_close: before,
                chrome_count_after_close: after
            },
            unavailable: if cfg!(target_os = "linux") {
                Vec::new()
            } else {
                vec!["/proc RSS/PSS/FD/tasks unavailable".into()]
            }
        })?
    );
    Ok(())
}
