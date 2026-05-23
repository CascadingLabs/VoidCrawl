//! `BrowserSession` — the main entry point for controlling a browser.

use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc, Once,
        atomic::{AtomicBool, Ordering},
    },
};

use chromiumoxide::{
    browser::{Browser, BrowserConfig},
    handler::Handler,
};
use rustls::crypto::ring::default_provider as ring_crypto_provider;
use serde_json::Value;
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{
    error::{Result, VoidCrawlError},
    page::Page,
    stealth::StealthConfig,
};

/// VoidCrawl's default Chrome command-line flags, applied to every launched
/// (non-remote) session.
///
/// Two groups:
/// 1. **Anti-automation hygiene** — re-adds the safe flags we want after
///    `disable_default_args()` (which strips chromiumoxide's
///    `--enable-automation` / `--disable-extensions`, both instant WAF
///    giveaways), plus the zendriver/nodriver flags known to pass real bot
///    walls.
/// 2. **Hardware GPU / WebGL** — new headless disables the GPU and falls back
///    to SwiftShader software WebGL, which `WEBGL_debug_renderer_info` reports
///    as "SwiftShader" — a strong bot signal Cloudflare Turnstile weighs. These
///    force hardware acceleration through ANGLE. (`--disable-gpu-sandbox` lets
///    the GPU process reach the DRI render node; on boxes with a stale Vulkan
///    ICD you may also need to steer `VK_DRIVER_FILES` in the launch
///    environment.) Harmless under headful — a real display already has a GPU.
///
/// Flags are stored **without** the leading `--`: chromiumoxide's
/// `BrowserConfig::arg` prepends `--` itself (it treats the whole string as a
/// switch key and emits `--{key}`), so passing `"--foo"` would yield the
/// inert `----foo`. Caller `extra_args` are normalized the same way (a leading
/// `--` is stripped) — see [`assemble_chrome_args`].
///
/// These are merged *before* caller `extra_args`; a caller value for the same
/// switch replaces the default (see [`assemble_chrome_args`]).
pub(crate) const DEFAULT_CHROME_ARGS: &[&str] = &[
    // ── Anti-automation core ────────────────────────────────────────
    "disable-blink-features=AutomationControlled",
    "disable-infobars",
    "disable-features=IsolateOrigins,site-per-process,TranslateUI",
    // ── Safe defaults from chromiumoxide we keep ────────────────────
    "disable-background-networking",
    "disable-background-timer-throttling",
    "disable-backgrounding-occluded-windows",
    "disable-breakpad",
    "disable-client-side-phishing-detection",
    "disable-component-extensions-with-background-pages",
    "disable-default-apps",
    "disable-dev-shm-usage",
    "disable-hang-monitor",
    "disable-ipc-flooding-protection",
    "disable-popup-blocking",
    "disable-prompt-on-repost",
    "disable-renderer-backgrounding",
    "disable-sync",
    "force-color-profile=srgb",
    "metrics-recording-only",
    "no-first-run",
    "password-store=basic",
    "use-mock-keychain",
    // ── Extra zendriver flags ───────────────────────────────────────
    "no-service-autorun",
    "no-default-browser-check",
    "no-pings",
    "disable-component-update",
    "disable-session-crashed-bubble",
    "disable-search-engine-choice-screen",
    "homepage=about:blank",
    // ── Hardware GPU / WebGL ────────────────────────────────────────
    "enable-gpu",
    "ignore-gpu-blocklist",
    // ANGLE backend selector — the single GPU-backend knob a caller overrides
    // (e.g. `use-angle=swiftshader` / `=gl`). Note: do NOT also pass
    // `enable-features=Vulkan` here; that force-enables Vulkan independently
    // and would silently defeat a caller's `use-angle` override.
    "use-angle=vulkan",
    "disable-gpu-sandbox",
];

/// Normalize a Chrome flag to the form chromiumoxide wants: strip a single
/// leading `--` if present, so both `"--use-angle=gl"` (how a human/Python
/// caller writes it) and `"use-angle=gl"` end up as `use-angle=gl`.
fn normalize_flag(arg: &str) -> &str {
    arg.strip_prefix("--").unwrap_or(arg)
}

/// The switch key of a (normalized) Chrome flag: the part before `=`, or the
/// whole flag if it takes no value. `use-angle=vulkan` → `use-angle`.
fn switch_key(arg: &str) -> &str {
    arg.split_once('=').map_or(arg, |(k, _)| k)
}

/// Assemble the final Chrome flag list from VoidCrawl's [`DEFAULT_CHROME_ARGS`]
/// merged with the caller's `extra_args`. Output is in chromiumoxide form (no
/// leading `--`).
///
/// **Override contract (directional control):** caller args are normalized
/// (leading `--` stripped) and merged by switch key — for each caller arg that
/// shares a key with a default (e.g. `--use-angle=gl` vs the default
/// `use-angle=vulkan`), the caller's value *replaces* the default in place,
/// leaving a single occurrence; novel caller args are appended. We deliberately
/// do **not** emit duplicate switches and hope Chrome picks the right one — its
/// precedence is per-switch and inconsistent (`use-angle` takes the *first*
/// value). Dedup-by-key makes the PyO3/Python override
/// (`BrowserConfig(extra_args=...)`) deterministic.
pub(crate) fn assemble_chrome_args(extra_args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = DEFAULT_CHROME_ARGS.iter().map(|s| (*s).to_string()).collect();
    for arg in extra_args {
        let flag = normalize_flag(arg);
        let key = switch_key(flag);
        if let Some(slot) = out.iter_mut().find(|d| switch_key(d) == key) {
            *slot = flag.to_string(); // caller overrides the default for this switch
        } else {
            out.push(flag.to_string());
        }
    }
    out
}

/// How the browser should be acquired.
#[derive(Debug, Clone, Default)]
pub enum BrowserMode {
    /// Launch a new headless browser.
    #[default]
    Headless,
    /// Launch a new browser with a visible window.
    Headful,
    /// Connect to an already-running Chrome via its WebSocket debugger URL.
    RemoteDebug { ws_url: String },
}

/// Builder for `BrowserSession`.
#[derive(Debug, Clone)]
#[must_use]
pub struct BrowserSessionBuilder {
    mode:              BrowserMode,
    stealth:           StealthConfig,
    extra_args:        Vec<String>,
    chrome_executable: Option<String>,
    proxy:             Option<String>,
    no_sandbox:        bool,
    window_size:       Option<(u32, u32)>,
    /// Pinned `--remote-debugging-port` for launched Chrome. `None` (default)
    /// lets the OS pick a free ephemeral port on loopback — never blocks on
    /// a busy or firewalled address. `Some(n)` forces Chrome to bind that
    /// port; useful when only specific ports are reachable through a
    /// firewall or when mapping through Docker.
    port:              Option<u16>,
    /// Persistent Chrome profile directory. `None` (default) = ephemeral
    /// `TempDir` that is deleted on session drop. `Some(path)` = mount an
    /// existing profile (e.g. one you've logged into `LinkedIn` in) and
    /// leave the directory on disk after the session ends.
    user_data_dir:     Option<PathBuf>,
}

impl Default for BrowserSessionBuilder {
    fn default() -> Self {
        Self {
            mode:              BrowserMode::Headless,
            stealth:           StealthConfig::chrome_like(),
            extra_args:        Vec::new(),
            chrome_executable: None,
            proxy:             None,
            no_sandbox:        false,
            window_size:       None,
            port:              None,
            user_data_dir:     None,
        }
    }
}

impl BrowserSessionBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(mut self, mode: BrowserMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn headless(self) -> Self {
        self.mode(BrowserMode::Headless)
    }

    pub fn headful(self) -> Self {
        self.mode(BrowserMode::Headful)
    }

    pub fn remote_debug(self, ws_url: impl Into<String>) -> Self {
        self.mode(BrowserMode::RemoteDebug { ws_url: ws_url.into() })
    }

    pub fn stealth(mut self, config: StealthConfig) -> Self {
        self.stealth = config;
        self
    }

    pub fn no_stealth(mut self) -> Self {
        self.stealth = StealthConfig::none();
        self
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.extra_args.push(arg.into());
        self
    }

    pub fn chrome_executable(mut self, path: impl Into<String>) -> Self {
        self.chrome_executable = Some(path.into());
        self
    }

    pub fn proxy(mut self, proxy_url: impl Into<String>) -> Self {
        self.proxy = Some(proxy_url.into());
        self
    }

    pub fn no_sandbox(mut self) -> Self {
        self.no_sandbox = true;
        self
    }

    pub fn window_size(mut self, width: u32, height: u32) -> Self {
        self.window_size = Some((width, height));
        self
    }

    /// Pin Chrome's `--remote-debugging-port`.
    ///
    /// Leave unset to let the OS pick a free ephemeral port (the default and
    /// the right choice for almost every launch mode — Chrome listens on
    /// loopback, so port-conflict is the only failure mode it avoids). Set
    /// when a firewall or container only exposes specific ports, or when
    /// another tool needs to know the port up front.
    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Mount a persistent Chrome profile directory. Use this to reuse
    /// an existing login (cookies, local storage, extensions) across
    /// sessions. The directory is NOT deleted when the session closes.
    ///
    /// Pick a directory dedicated to `void_crawl` — Chrome locks a
    /// profile while it's running, so pointing at your daily-driver
    /// profile while your real Chrome is open will fail to launch.
    pub fn user_data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.user_data_dir = Some(path.into());
        self
    }

    /// Override the stealth viewport dimensions.
    ///
    /// This sets the CDP device metrics override that the page reports to
    /// JavaScript (e.g. `window.innerWidth`). It does NOT resize the Chrome
    /// window — use [`window_size`](Self::window_size) for that.
    pub fn viewport(mut self, width: u32, height: u32) -> Self {
        self.stealth.viewport_width = width;
        self.stealth.viewport_height = height;
        self
    }

    /// Build and launch (or connect to) the browser.
    pub async fn launch(self) -> Result<BrowserSession> {
        BrowserSession::connect_or_launch(
            self.mode,
            self.stealth,
            self.extra_args,
            self.chrome_executable,
            self.proxy,
            self.no_sandbox,
            self.window_size,
            self.port,
            self.user_data_dir,
        )
        .await
    }
}

/// A live browser session wrapping `chromiumoxide::Browser`.
///
/// Use [`BrowserSessionBuilder`] or the convenience constructors to create one.
pub struct BrowserSession {
    browser:        Arc<Mutex<Browser>>,
    _handler_task:  JoinHandle<()>,
    handler_alive:  Arc<AtomicBool>,
    stealth:        StealthConfig,
    /// True when this session attached to an already-running Chrome via
    /// `BrowserMode::RemoteDebug`. In that case `close()` must NOT send
    /// `Browser.close` over CDP — doing so terminates the user's Chromium
    /// process, which we didn't spawn and have no business shutting down.
    attached:       bool,
    /// Owns the temporary user data directory for launched browsers.
    /// `None` for remote-debug sessions (no local user data dir).
    /// Dropped after `browser` and `_handler_task`, so Chrome has already
    /// been signalled to close before the directory is deleted.
    _user_data_dir: Option<tempfile::TempDir>,
}

impl fmt::Debug for BrowserSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserSession").field("stealth", &self.stealth).finish_non_exhaustive()
    }
}

impl BrowserSession {
    /// Returns `true` while the CDP handler loop is still running.
    ///
    /// When this returns `false`, the browser process has likely crashed or
    /// the WebSocket connection has been lost — all subsequent CDP calls
    /// will fail.
    pub fn is_alive(&self) -> bool {
        self.handler_alive.load(Ordering::Acquire)
    }

    /// Check that the handler is still running; return `BrowserClosed` if not.
    fn check_alive(&self) -> Result<()> {
        if self.is_alive() { Ok(()) } else { Err(VoidCrawlError::BrowserClosed) }
    }

    /// Create a builder.
    pub fn builder() -> BrowserSessionBuilder {
        BrowserSessionBuilder::new()
    }

    /// Quick headless launch with default stealth.
    pub async fn launch_headless() -> Result<Self> {
        Self::builder().headless().launch().await
    }

    /// Quick headed launch with default stealth.
    pub async fn launch_headful() -> Result<Self> {
        Self::builder().headful().launch().await
    }

    /// Connect to an existing browser.
    pub async fn connect(ws_url: impl Into<String>) -> Result<Self> {
        Self::builder().remote_debug(ws_url).launch().await
    }

    /// Internal factory that handles all three modes.
    #[allow(clippy::too_many_arguments, reason = "builder forwards all options at once")]
    async fn connect_or_launch(
        mode: BrowserMode,
        stealth: StealthConfig,
        extra_args: Vec<String>,
        chrome_executable: Option<String>,
        proxy: Option<String>,
        no_sandbox: bool,
        window_size: Option<(u32, u32)>,
        port: Option<u16>,
        persistent_user_data_dir: Option<PathBuf>,
    ) -> Result<Self> {
        let mut owned_user_data_dir: Option<tempfile::TempDir> = None;

        let (browser, handler) = match &mode {
            BrowserMode::RemoteDebug { ws_url } => {
                let ws = resolve_ws_url(ws_url).await?;
                Browser::connect(&ws)
                    .await
                    .map_err(|e| VoidCrawlError::ConnectionFailed(e.to_string()))?
            }
            BrowserMode::Headless | BrowserMode::Headful => {
                // Disable chromiumoxide's DEFAULT_ARGS which include
                // `--enable-automation` and `--disable-extensions` —
                // both are instant giveaways to WAFs like Akamai.
                let mut builder = BrowserConfig::builder().disable_default_args();

                // Caller-supplied persistent profile vs. ephemeral
                // `TempDir`. The ephemeral path handles SingletonLock
                // conflicts across concurrent browsers automatically;
                // the persistent path is the caller's problem (they
                // chose it, so don't pick their live daily-driver).
                if let Some(ref path) = persistent_user_data_dir {
                    builder = builder.user_data_dir(path);
                } else {
                    let tmp = tempfile::tempdir()
                        .map_err(|e| VoidCrawlError::LaunchFailed(format!("tmpdir: {e}")))?;
                    builder = builder.user_data_dir(tmp.path());
                    owned_user_data_dir = Some(tmp);
                }

                if matches!(mode, BrowserMode::Headful) {
                    builder = builder.with_head();
                } else {
                    // Use the *new* headless mode. chromiumoxide defaults to
                    // `HeadlessMode::True`, which emits the legacy `--headless`
                    // flag — and legacy headless forces SwiftShader software
                    // rendering, so `WEBGL_debug_renderer_info` reports
                    // "SwiftShader", a glaring bot signal that WAFs like
                    // Cloudflare Turnstile weigh heavily. `--headless=new` runs
                    // the full browser stack and can drive a real GPU.
                    builder = builder.new_headless_mode();
                }

                if let Some(ref exe) = chrome_executable {
                    builder = builder.chrome_executable(exe);
                }

                if no_sandbox {
                    builder = builder.no_sandbox();
                }

                if let Some((w, h)) = window_size {
                    builder = builder.window_size(w, h);
                }

                // Pinned debug port, or `0` (OS-assigned) by default — the
                // latter avoids port-conflict failures entirely since the
                // kernel hands out a guaranteed-free ephemeral port.
                if let Some(p) = port {
                    builder = builder.port(p);
                }

                if let Some(ref p) = proxy {
                    builder = builder.arg(format!("--proxy-server={p}"));
                }

                // VoidCrawl's default Chrome flags merged with the caller's
                // `extra_args` (dedup-by-switch-key; caller value replaces the
                // default). Lets the PyO3/Python client override any default
                // deterministically via `BrowserConfig(extra_args=...)`. See
                // `assemble_chrome_args` and its unit tests.
                for a in assemble_chrome_args(&extra_args) {
                    builder = builder.arg(a);
                }

                let config = builder.build().map_err(VoidCrawlError::LaunchFailed)?;

                Browser::launch(config)
                    .await
                    .map_err(|e| VoidCrawlError::LaunchFailed(e.to_string()))?
            }
        };

        let alive = Arc::new(AtomicBool::new(true));
        let handler_task = spawn_handler(handler, Arc::clone(&alive));

        Ok(Self {
            browser: Arc::new(Mutex::new(browser)),
            _handler_task: handler_task,
            handler_alive: alive,
            stealth,
            attached: matches!(mode, BrowserMode::RemoteDebug { .. }),
            _user_data_dir: owned_user_data_dir,
        })
    }

    /// Open a new tab, apply stealth settings, and navigate to `url`.
    ///
    /// Stealth is applied on a blank page *before* navigation so that
    /// `addScriptToEvaluateOnNewDocument` scripts fire during the real
    /// page load — not after it.
    pub async fn new_page(&self, url: &str) -> Result<Page> {
        self.check_alive()?;
        let page = {
            let browser = self.browser.lock().await;
            let cdp_page = browser
                .new_page("about:blank")
                .await
                .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
            Page::new(cdp_page)
        }; // browser lock released before navigation

        page.apply_stealth(&self.stealth).await?;
        page.navigate(url).await?;
        Ok(page)
    }

    /// Open a blank tab with stealth applied (no navigation).
    pub async fn new_blank_page(&self) -> Result<Page> {
        self.check_alive()?;
        let page = {
            let browser = self.browser.lock().await;
            let cdp_page = browser
                .new_page("about:blank")
                .await
                .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
            Page::new(cdp_page)
        };
        page.apply_stealth(&self.stealth).await?;
        Ok(page)
    }

    /// List all open pages.
    pub async fn pages(&self) -> Result<Vec<Page>> {
        self.check_alive()?;
        let browser = self.browser.lock().await;
        let cdp_pages =
            browser.pages().await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(cdp_pages.into_iter().map(Page::new).collect())
    }

    /// Get browser version string.
    pub async fn version(&self) -> Result<String> {
        self.check_alive()?;
        let browser = self.browser.lock().await;
        let info = browser.version().await.map_err(|e| VoidCrawlError::Other(e.to_string()))?;
        Ok(info.product)
    }

    /// Gracefully close the browser.
    ///
    /// For a launched session this signals Chrome to shut down over CDP and
    /// cleans up the temporary user data directory when the session is
    /// dropped. For an attached (`RemoteDebug`) session this is a no-op at
    /// the CDP level — we only launched a WebSocket connection, not the
    /// browser process, so we have no mandate to kill it. The connection
    /// is released when the session is dropped.
    pub async fn close(&self) -> Result<()> {
        if self.attached {
            return Ok(());
        }
        let mut browser = self.browser.lock().await;
        browser.close().await.map_err(|e| VoidCrawlError::Other(e.to_string()))?;
        Ok(())
    }

    /// True when this session was attached to an already-running browser
    /// (the `ws_url` / `RemoteDebug` code path).
    #[must_use]
    pub fn is_attached(&self) -> bool {
        self.attached
    }

    /// Access stealth config.
    pub fn stealth_config(&self) -> &StealthConfig {
        &self.stealth
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Spawn the CDP handler loop on a background tokio task.
///
/// Sets `alive` to `false` when the handler stream ends (browser crash,
/// WebSocket disconnect, or graceful close).
fn spawn_handler(mut handler: Handler, alive: Arc<AtomicBool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        use futures::StreamExt;
        while handler.next().await.is_some() {}
        alive.store(false, Ordering::Release);
    })
}

/// If the user gives us `http://host:port` (Chrome's debug HTTP endpoint),
/// resolve it to the actual `ws://` URL by hitting `/json/version`.
async fn resolve_ws_url(url: &str) -> Result<String> {
    // Already a ws:// URL — use directly
    if url.starts_with("ws://") || url.starts_with("wss://") {
        return Ok(url.to_string());
    }

    // reqwest is built with `rustls-no-provider`, so we must install a rustls
    // CryptoProvider before the first request or reqwest panics "No provider
    // set" (even for this plain-HTTP localhost fetch). Install the `ring`
    // provider exactly once; `install_default` errors if already set, so the
    // `Once` + ignored result is idempotent.
    static CRYPTO_INIT: Once = Once::new();
    CRYPTO_INIT.call_once(|| {
        let _ = ring_crypto_provider().install_default();
    });

    // Treat as an HTTP endpoint, fetch /json/version
    let version_url = format!("{}/json/version", url.trim_end_matches('/'));
    let resp: Value = reqwest::get(&version_url)
        .await
        .map_err(|e| VoidCrawlError::ConnectionFailed(format!("GET {version_url}: {e}")))?
        .json()
        .await
        .map_err(|e| VoidCrawlError::ConnectionFailed(format!("parse {version_url}: {e}")))?;

    resp.get("webSocketDebuggerUrl").and_then(|v| v.as_str()).map(ToString::to_string).ok_or_else(
        || {
            VoidCrawlError::ConnectionFailed(
                "webSocketDebuggerUrl not found in /json/version response".into(),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_CHROME_ARGS, assemble_chrome_args};

    /// Flags are stored WITHOUT a leading `--` (chromiumoxide adds it; a `--`
    /// here would become the inert `----flag`). Guards the double-dash bug.
    #[test]
    fn defaults_have_no_leading_double_dash() {
        for f in DEFAULT_CHROME_ARGS {
            assert!(!f.starts_with("--"), "default flag must not start with --: {f}");
        }
    }

    /// Hardware-GPU defaults are present (so headless doesn't fall back to
    /// SwiftShader), alongside the anti-automation core. Forms are un-prefixed.
    #[test]
    fn defaults_enable_hardware_gpu_and_antiautomation() {
        let args = assemble_chrome_args(&[]);
        for expected in [
            "use-angle=vulkan",
            "enable-gpu",
            "ignore-gpu-blocklist",
            "disable-gpu-sandbox",
            "disable-blink-features=AutomationControlled",
        ] {
            assert!(args.iter().any(|a| a == expected), "missing default flag: {expected}");
        }
    }

    /// Novel caller `extra_args` (no matching default switch) are normalized
    /// and appended.
    #[test]
    fn novel_extra_args_are_normalized_and_appended() {
        let extra = vec!["--proxy-bypass-list=*".to_string(), "lang=fr".to_string()];
        let args = assemble_chrome_args(&extra);
        // Both forms (with/without `--`) land un-prefixed and last.
        assert_eq!(
            &args[args.len() - 2..],
            &["proxy-bypass-list=*".to_string(), "lang=fr".to_string()][..]
        );
        assert_eq!(args.len(), DEFAULT_CHROME_ARGS.len() + extra.len());
    }

    /// The override contract: a caller value for a switch that already has a
    /// default *replaces* it — exactly one occurrence, default value gone — and
    /// the caller's `--` is normalized away. (Critical for `use-angle`, which
    /// Chrome reads first-occurrence-wins, so a duplicate would not override.)
    #[test]
    fn caller_value_replaces_default_same_switch() {
        let args = assemble_chrome_args(&["--use-angle=swiftshader".to_string()]);
        let angle: Vec<&String> = args.iter().filter(|a| a.starts_with("use-angle")).collect();
        assert_eq!(angle.len(), 1, "exactly one use-angle flag");
        assert_eq!(angle[0], "use-angle=swiftshader");
        assert!(!args.iter().any(|a| a == "use-angle=vulkan"), "default value must be gone");
        // Length unchanged: replacement, not addition.
        assert_eq!(args.len(), DEFAULT_CHROME_ARGS.len());
    }

    /// Replacement happens in place, so unrelated defaults are untouched.
    #[test]
    fn override_is_in_place_and_leaves_other_defaults() {
        let args = assemble_chrome_args(&["--use-angle=gl".to_string()]);
        assert!(args.iter().any(|a| a == "enable-gpu"));
        assert!(args.iter().any(|a| a == "disable-blink-features=AutomationControlled"));
    }

    /// No `extra_args` => exactly the defaults, unchanged order.
    #[test]
    fn no_extra_args_is_just_defaults() {
        let args = assemble_chrome_args(&[]);
        let defaults: Vec<String> = DEFAULT_CHROME_ARGS.iter().map(|s| s.to_string()).collect();
        assert_eq!(args, defaults);
    }
}
