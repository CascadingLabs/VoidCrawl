//! Loopback-only fixtures and workload helpers for VoidCrawl benchmarks.

use std::{
    env, fs,
    io::{self, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use futures::future::join_all;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
use void_crawl_core::{BrowserPool, PoolConfig};

pub const FIXTURE: &str = include_str!("../fixtures/index.html");
pub const FIXTURE_NAME: &str = "index.html";

#[derive(Deserialize)]
struct FixtureManifest {
    generator: String,
    fixtures: Vec<FixtureEntry>,
}
#[derive(Deserialize)]
struct FixtureEntry {
    route: String,
    size_bytes: usize,
    sha256: String,
}

#[must_use]
pub fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(FIXTURE_NAME)
}
#[must_use]
pub fn fixture_hash() -> String {
    format!("{:x}", Sha256::digest(FIXTURE.as_bytes()))
}

pub fn verify_fixture_manifest() -> io::Result<()> {
    let manifest: FixtureManifest = serde_json::from_str(&fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/manifest.json"),
    )?)
    .map_err(io::Error::other)?;
    let valid_html = manifest.fixtures.iter().any(|entry| {
        entry.route == "/index.html"
            && entry.size_bytes == FIXTURE.len()
            && entry.sha256 == fixture_hash()
    });
    let bytes_hash = format!("{:x}", Sha256::digest(vec![b'x'; 1024]));
    let valid_bytes = manifest.fixtures.iter().any(|entry| {
        entry.route == "/bytes?size=1024" && entry.size_bytes == 1024 && entry.sha256 == bytes_hash
    });
    if manifest.generator == "scripts/generate-benchmark-fixtures.py"
        && valid_html
        && valid_bytes
        && fs::read_to_string(fixture_path())? == FIXTURE
    {
        Ok(())
    } else {
        Err(io::Error::other("fixture manifest or compiled fixture does not match"))
    }
}

pub fn require_loopback_url(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| "benchmark URL must be http loopback".to_string())?;
    let host = rest.split('/').next().unwrap_or_default().split(':').next().unwrap_or_default();
    if matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        Ok(())
    } else {
        Err(format!("public or non-loopback benchmark URL rejected: {url}"))
    }
}

#[derive(Debug)]
pub struct LoopbackServer {
    addr: SocketAddr,
    task: JoinHandle<()>,
}
impl LoopbackServer {
    pub async fn start() -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut request = [0_u8; 2048];
                    let _ = stream.read(&mut request).await;
                    let first = String::from_utf8_lossy(&request);
                    let (body, content_type) = if first.starts_with("GET /bytes?") {
                        (vec![b'x'; 1024], "text/plain; charset=utf-8")
                    } else {
                        (FIXTURE.as_bytes().to_vec(), "text/html; charset=utf-8")
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        Ok(Self { addr, task })
    }
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}/{FIXTURE_NAME}", self.addr)
    }
    #[must_use]
    pub fn bytes_url(&self) -> String {
        format!("http://{}/bytes?size=1024", self.addr)
    }
}
impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ConcurrencyConfig {
    pub browsers: usize,
    pub tabs_per_browser: usize,
    pub in_flight: usize,
}
pub const CONCURRENCY_MATRIX: [ConcurrencyConfig; 3] = [
    ConcurrencyConfig { browsers: 1, tabs_per_browser: 1, in_flight: 1 },
    ConcurrencyConfig { browsers: 1, tabs_per_browser: 2, in_flight: 2 },
    ConcurrencyConfig { browsers: 2, tabs_per_browser: 2, in_flight: 4 },
];
#[derive(Debug, Serialize)]
pub struct ConcurrencyReport {
    pub completed: usize,
    pub failures: usize,
    pub semaphore_wait_ms_sum: u128,
    pub elapsed_ns: u128,
    pub throughput_per_second: f64,
}
/// Executes one real pool navigation matrix iteration; setup/warmup belongs to
/// the caller.
#[allow(
    clippy::cast_precision_loss,
    reason = "benchmark throughput intentionally converts bounded operation counts and elapsed time"
)]
pub async fn run_concurrency_matrix(
    pool: &BrowserPool,
    url: &str,
    in_flight: usize,
) -> ConcurrencyReport {
    let started = Instant::now();
    let results = join_all((0..in_flight).map(|_| async {
        let acquired = pool.acquire_timed().await;
        match acquired {
            Ok((tab, wait_ms)) => {
                let navigation = tab.page.navigate(url).await;
                let release = pool.release_checked(tab).await;
                (navigation.is_ok() && release.cleanup_complete, u128::from(wait_ms))
            }
            Err(_) => (false, 0),
        }
    }))
    .await;
    let elapsed_ns = started.elapsed().as_nanos();
    let completed = results.iter().filter(|(ok, _)| *ok).count();
    ConcurrencyReport {
        completed,
        failures: in_flight - completed,
        semaphore_wait_ms_sum: results.iter().map(|(_, wait)| wait).sum(),
        elapsed_ns,
        throughput_per_second: if elapsed_ns == 0 {
            0.0
        } else {
            completed as f64 * 1e9 / elapsed_ns as f64
        },
    }
}
/// Append one measured concurrency iteration to the runner-provided evidence
/// file.
///
/// The Criterion throughput facility is deliberately not used: this record
/// carries the completed operation count, failures, semaphore wait, and
/// measured rate.
pub fn record_concurrency_report(
    config: ConcurrencyConfig,
    report: &ConcurrencyReport,
) -> io::Result<()> {
    let Some(path) = env::var_os("VOIDCRAWL_CONCURRENCY_EVIDENCE") else { return Ok(()) };
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({
            "workload": format!(
                "warm_pool_navigation_matrix/{}x{}x{}",
                config.browsers, config.tabs_per_browser, config.in_flight
            ),
            "browsers": config.browsers,
            "tabs_per_browser": config.tabs_per_browser,
            "in_flight": config.in_flight,
            "completed": report.completed,
            "failures": report.failures,
            "semaphore_wait_ms_sum": report.semaphore_wait_ms_sum,
            "elapsed_ns": report.elapsed_ns,
            "throughput_per_second": report.throughput_per_second,
        }),
    )
    .map_err(io::Error::other)?;
    writeln!(file)
}
pub fn matrix_pool_config(config: ConcurrencyConfig) -> PoolConfig {
    PoolConfig {
        browsers: config.browsers,
        tabs_per_browser: config.tabs_per_browser,
        ..PoolConfig::default()
    }
}

#[must_use]
pub fn fixture_dom_token_count() -> usize {
    FIXTURE.split_ascii_whitespace().filter(|token| token.starts_with('<')).count()
}
pub fn fixture_generator_is_current(root: &Path) -> io::Result<()> {
    let status = Command::new("python3")
        .arg(root.join("scripts/generate-benchmark-fixtures.py"))
        .arg("--check")
        .status()?;
    if status.success() { Ok(()) } else { Err(io::Error::other("fixture generator check failed")) }
}
