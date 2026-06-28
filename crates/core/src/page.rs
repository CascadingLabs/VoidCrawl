//! High-level wrapper around a `chromiumoxide::Page`.

use std::{
    collections::{HashMap, HashSet},
    fs, future,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chromiumoxide::{
    Page as CdpPage,
    cdp::{
        browser_protocol::{
            accessibility::{AxNode, AxValue, GetFullAxTreeParams, QueryAxTreeParams},
            browser::{
                PermissionDescriptor, PermissionSetting, SetDownloadBehaviorBehavior,
                SetDownloadBehaviorParams, SetPermissionParams,
            },
            dom::{BackendNodeId, GetBoxModelParams, GetDocumentParams, ResolveNodeParams},
            emulation::{
                SetDeviceMetricsOverrideParams, SetGeolocationOverrideParams,
                SetLocaleOverrideParams, SetTimezoneOverrideParams, SetUserAgentOverrideParams,
                UserAgentBrandVersion, UserAgentMetadata,
            },
            input::{
                DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams,
                DispatchMouseEventType, MouseButton,
            },
            network::{
                Cookie, CookieParam, DeleteCookiesParams, EventRequestWillBeSent,
                EventResponseReceived, Headers, ResourceType, SetExtraHttpHeadersParams,
            },
            page::{
                AddScriptToEvaluateOnNewDocumentParams, CaptureScreenshotFormat,
                EventLifecycleEvent, FrameId, PrintToPdfParams, SetBypassCspParams, Viewport,
            },
        },
        js_protocol::runtime::{CallFunctionOnParams, EvaluateParams},
    },
    page::ScreenshotParams,
};
use futures::StreamExt;
use serde_json::Value;
use tokio::time;

use crate::{
    antibot::{self, AntibotVerdict},
    ax::compact_outline,
    error::{Result, VoidCrawlError},
    input::{HumanizeOptions, Rng, humanized_path},
    stealth::StealthConfig,
};

/// Wall-clock-derived seed for live humanized pointer paths. Tests seed the
/// generator explicitly for determinism; production just wants variety.
fn runtime_seed() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0x1234_5678_9ABC_DEF0, |d| {
        d.as_secs() ^ u64::from(d.subsec_nanos()).rotate_left(32)
    })
}

/// The result of a [`Page::goto_and_wait_for_idle`] call.
///
/// Bundles the final HTML, URL, and HTTP response metadata captured during
/// navigation.  `status_code` is `None` when the page was served from a
/// service worker, disk cache, or the browser failed to capture a network
/// response (e.g. `file://` URLs).
#[derive(Debug, Clone)]
pub struct PageResponse {
    /// Outer HTML of `<html>` after the page reached network idle.
    pub html: String,
    /// Final URL after any redirects.
    pub url: String,
    /// HTTP status code of the last response in the navigation chain.
    pub status_code: Option<u16>,
    /// `true` when at least one HTTP redirect occurred before the final URL.
    pub redirected: bool,
    /// Response headers of the final Document response (`name`, `value`),
    /// lowercased names, in arrival order. Empty when no network response was
    /// captured (cache/service-worker/`file://`). Feeds anti-bot fingerprinting
    /// and replay-grade provenance (`cf-ray`, `x-cache`, …).
    pub headers: Vec<(String, String)>,
    /// Signature-based anti-bot / CDN vendor fingerprint of the final response,
    /// computed from `status_code` + `headers` + `html`. `None` when no
    /// network response was captured. Non-fatal: presence is a routing hint,
    /// `challenged` means an active wall — see [`crate::antibot`].
    pub antibot: Option<AntibotVerdict>,
    /// Data-plane network endpoints (XHR + Fetch request URLs) observed during
    /// navigation — a sorted, deduplicated set of `scheme://host[:port]/path`
    /// strings with query/fragment/userinfo stripped and secret-like path
    /// segments redacted at the source (a replay-grade archive must never
    /// persist a token; see [`safe_endpoint`] and
    /// `ENDPOINT_SANITIZER_VERSION`). `None` when capture was not requested
    /// (opt-in); `Some(empty)` when requested but the page made no
    /// XHR/fetch calls. The *consumer* templatizes id-bearing path segments
    /// — this stays a generic, faithful observation.
    pub endpoints: Option<Vec<String>>,
    /// `true` when the captured endpoint set hit its cap and further endpoints
    /// were dropped — so a consumer can tell "made few calls" from "we stopped
    /// counting". Always `false` when `endpoints` is `None`.
    pub endpoints_truncated: bool,
    /// The [`ENDPOINT_SANITIZER_VERSION`] the `endpoints` were redacted under,
    /// so a long-term archive can reproduce/audit exactly which rules produced
    /// the set (mirrors `AntibotVerdict::corpus_version`). `None` iff
    /// `endpoints` is `None` (capture was not requested).
    pub endpoint_sanitizer_version: Option<&'static str>,
}

/// Version of the endpoint-sanitization rules ([`safe_endpoint`]). Bump on any
/// change to the redaction patterns so a captured set is reproducible/auditable
/// at replay time — mirrors `antibot::CORPUS_VERSION`.
pub const ENDPOINT_SANITIZER_VERSION: &str = "ep-2026.06.06";

/// Largest distinct-endpoint set kept per navigation; past this, capture stops
/// and `PageResponse::endpoints_truncated` is set. Bounds memory on chatty
/// SPAs.
const MAX_ENDPOINTS: usize = 256;

/// Reduce a raw request URL to a `scheme://host[:port]/path` key with secrets
/// removed, or `None` if it must not be archived at all.
///
/// A replay-grade archive cannot retroactively un-persist a secret, so this
/// strips at the source — BEFORE the string is ever stored — and is
/// **redact-by-default** on the path (deny-unknown, not allow-unknown):
///   * query string + fragment removed (where tokens/PII/cache-busters live),
///   * userinfo (`user:pass@`) removed,
///   * non-`http(s)` schemes and loopback/private/CGNAT/`.local` hosts dropped
///     entirely (an operator-environment leak, not page signal),
///   * a path segment is KEPT only when it is clearly a short, low-entropy
///     template token ([`is_safe_segment`]); ANYTHING else — long blobs
///     (JWT/signed-URL/hash), kv/matrix markers (`;`/`=`/`%`), emails, long
///     digit runs — becomes `:redacted`.
///
/// This is a best-effort *security* filter, not a proof: a short high-entropy
/// secret can still resemble a word. It deliberately does NOT templatize
/// ordinary id segments (`/users/123/` keeps `123`) — that semantic
/// normalization is the *consumer's* fingerprint concern; this function's job
/// is only to keep secrets out while staying a faithful, generic observation.
pub fn safe_endpoint(raw_url: &str) -> Option<String> {
    // Cut everything from the first `?` or `#` — query and fragment never enter.
    let head = raw_url.split(['?', '#']).next().unwrap_or("");

    let (scheme, rest) = head.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }

    // Authority is everything up to the first `/`; the rest is the path.
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{p}")),
        None => (rest, String::new()),
    };
    // Drop userinfo (`user:pass@host`) — embedded credentials — then lowercase
    // the host:port ONCE (the single source of truth for both the local-host
    // guard and the emitted key).
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp).to_ascii_lowercase();
    let host = bare_host(&host_port);
    if host.is_empty() || is_local_host(host) {
        return None;
    }

    let safe_path: String = path
        .split('/')
        .map(|seg| if is_safe_segment(seg) { seg } else { ":redacted" })
        .collect::<Vec<_>>()
        .join("/");

    Some(format!("{scheme}://{host_port}{safe_path}"))
}

/// The bare host from a (already-lowercased) `host[:port]` authority, handling
/// the bracketed IPv6 form `[::1]:9000` → `::1` (a plain `split(':')` would
/// return `"["` and let loopback IPv6 slip past [`is_local_host`]).
fn bare_host(host_port: &str) -> &str {
    if let Some(after) = host_port.strip_prefix('[') {
        return after.split(']').next().unwrap_or("");
    }
    host_port.split(':').next().unwrap_or("")
}

/// Loopback / private / CGNAT / link-local / mDNS hosts — never archive these
/// (they describe the crawl operator's machine/network, not the page). `host`
/// is the bare, lowercased host (no brackets, no port).
fn is_local_host(host: &str) -> bool {
    // IPv6 loopback / unspecified / link-local / unique-local (fc00::/7).
    if host == "::1"
        || host == "::"
        || host.starts_with("fe80:")
        || host.starts_with("fc")
        || host.starts_with("fd")
    {
        return true;
    }
    // mDNS `*.local` (compare the final label, not via ends_with — that trips
    // clippy's file-extension lint and would also match a bare "local").
    let mdns_local = host.rsplit_once('.').is_some_and(|(_, tld)| tld == "local");
    if host == "localhost" || host == "0.0.0.0" || mdns_local {
        return true;
    }
    if host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("169.254.")
    {
        return true;
    }
    // RFC-1918 172.16.0.0/12 and RFC-6598 CGNAT 100.64.0.0/10.
    let second_octet = |s: &str| s.split('.').nth(1).and_then(|o| o.parse::<u8>().ok());
    if host.starts_with("172.") {
        return second_octet(host).is_some_and(|o| (16..=31).contains(&o));
    }
    if host.starts_with("100.") {
        return second_octet(host).is_some_and(|o| (64..=127).contains(&o));
    }
    false
}

/// True when a path segment is clearly a SAFE template token worth keeping —
/// the allow-list half of the redact-by-default policy. Conservative: anything
/// that isn't obviously a short, low-entropy lexical/id token is redacted.
///
/// Keeps: `finance`, `quoteSummary`, `v10`, `users`, `123`, `AAPL` (the
/// consumer templatizes ordinary ids). Redacts: JWTs/signed-URLs/hashes (long
/// or high-entropy), emails / kv / matrix params (`@`/`=`/`;`/`%`), and long
/// digit runs (card/SSN/phone).
fn is_safe_segment(seg: &str) -> bool {
    // Empty (a `//` or trailing `/`) is structure, not content — keep it.
    if seg.is_empty() {
        return true;
    }
    // Any kv / matrix / userinfo / percent-encoding marker → not a plain token.
    if seg.contains(['@', '=', ';', '%', ':']) {
        return false;
    }
    // Only ordinary url-path token characters.
    if !seg.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~')) {
        return false;
    }
    // Long segments are tokens/blobs, not template words (`recommendations` is 15).
    if seg.len() > 15 {
        return false;
    }
    let digits = seg.chars().filter(char::is_ascii_digit).count();
    // 9+ digits → SSN / card / phone range (ordinary numeric ids are shorter).
    if digits >= 9 {
        return false;
    }
    // A 12+ char all-hex blob is a hash/token, never a word.
    if seg.len() >= 12 && seg.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    // A 12+ char segment spanning 3 character classes (lower AND upper AND
    // digit) is an opaque mixed-case token, not a template word — `oAuth2…`-
    // style names are rare in paths and over-redacting them is the safe trade.
    if seg.len() >= 12 {
        let has_lower = seg.chars().any(|c| c.is_ascii_lowercase());
        let has_upper = seg.chars().any(|c| c.is_ascii_uppercase());
        let has_digit = seg.chars().any(|c| c.is_ascii_digit());
        if has_lower && has_upper && has_digit {
            return false;
        }
    }
    true
}

/// Turn the in-loop deduped endpoint set into the final field value: `None`
/// when capture was off, else a SORTED `Vec` (a stable set — arrival order is a
/// session/timing tell, and the consumer set-ifies anyway).
fn finalize_endpoints(seen: &HashSet<String>, capture: bool) -> Option<Vec<String>> {
    if !capture {
        return None;
    }
    let mut v: Vec<String> = seen.iter().cloned().collect();
    v.sort();
    Some(v)
}

/// Flatten CDP's `Network.Response.headers` (a JSON object of name → string
/// value) into ordered `(lowercased-name, value)` pairs. Non-string values are
/// skipped; an unexpected non-object yields an empty list.
fn flatten_headers(value: &serde_json::Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.to_lowercase(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Rectangular crop in CSS pixels for [`ScreenshotOptions::bbox`].
#[derive(Debug, Clone, Copy)]
pub struct Bbox {
    pub x:      u32,
    pub y:      u32,
    pub width:  u32,
    pub height: u32,
}

/// Options for [`Page::screenshot`].
#[derive(Debug, Default, Clone)]
pub struct ScreenshotOptions {
    /// Write PNG to this path instead of returning bytes.
    pub path: Option<PathBuf>,
    /// Crop to this CSS-pixel region. None = full page.
    pub bbox: Option<Bbox>,
}

impl ScreenshotOptions {
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_bbox(mut self, bbox: Bbox) -> Self {
        self.bbox = Some(bbox);
        self
    }
}

/// Return type of [`Page::screenshot`].
#[derive(Debug)]
pub enum ScreenshotOutput {
    /// PNG bytes held in memory (no path supplied).
    Bytes(Vec<u8>),
    /// Path the PNG was written to.
    Path(PathBuf),
}

/// Outcome of [`Page::download_to_dir`]: the file that landed on disk.
#[derive(Debug, Clone)]
pub struct DownloadOutcome {
    /// Absolute path to the downloaded file inside the target directory.
    pub path:         PathBuf,
    /// Size of the downloaded file in bytes.
    pub bytes:        u64,
    /// The `Content-Type` the server sent for the download (parameters
    /// stripped), if any — fed to the scanner to catch disguised payloads.
    /// `None` for action-captured downloads (see [`Page::arm_download`]), where
    /// Chrome streams to disk and the header isn't observed.
    pub content_type: Option<String>,
}

/// A primed capture for an **action-triggered** download — created by
/// [`Page::arm_download`], consumed by [`DownloadCapture::wait`].
///
/// Use this when the download is started by a page action (clicking a
/// "Download" button, a generated/redirected/cross-origin URL) rather than a
/// URL you already hold — e.g. Google Drive. The flow is *arm → act → await*:
///
/// ```no_run
/// # async fn f(page: &void_crawl_core::Page) -> void_crawl_core::Result<()> {
/// # use std::{path::Path, time::Duration};
/// let cap = page.arm_download(Path::new("/tmp/dl"), 100 << 20).await?;
/// page.click_by_role("button", "Download all", 0, false).await?; // the triggering action
/// let file = cap.wait(page, Duration::from_secs(120)).await?;
/// # Ok(()) }
/// ```
///
/// `arm_download` snapshots the directory's existing files, so `wait` only
/// accepts a file that appears *after* arming. Not `Clone` — a capture is
/// consumed exactly once.
#[derive(Debug)]
pub struct DownloadCapture {
    dir:       PathBuf,
    before:    HashSet<PathBuf>,
    max_bytes: u64,
}

impl DownloadCapture {
    /// Wait for a new completed download to settle in the armed directory, then
    /// reset `page`'s download behavior. `page` must be the page that armed
    /// this capture.
    ///
    /// The size cap is enforced *after* the file lands (Chrome streams a native
    /// download straight to disk — it can't be aborted mid-stream the way
    /// [`Page::download_to_dir`] aborts its in-page fetch). An oversized file
    /// is deleted and an error returned.
    pub async fn wait(self, page: &Page, timeout: Duration) -> Result<DownloadOutcome> {
        let result = self.poll(timeout).await;
        page.reset_download_behavior().await;
        result
    }

    /// Poll for the download **without** touching the page, so a caller holding
    /// the page lock elsewhere doesn't hold it for the whole wait. Does NOT
    /// reset download behavior — pair with [`Page::reset_download_behavior`].
    pub async fn poll(&self, timeout: Duration) -> Result<DownloadOutcome> {
        wait_for_new_download(&self.dir, &self.before, self.max_bytes, timeout).await
    }
}

/// Thin wrapper over `chromiumoxide::Page` exposing a clean async API.
#[derive(Debug)]
pub struct Page {
    inner:          CdpPage,
    /// `true` between [`Page::arm_download`] / a `download_to_dir` in flight
    /// and the matching reset. The pool checks this on release to reset an
    /// abandoned download behavior cheaply (no CDP call on the common path).
    download_armed: AtomicBool,
    /// Last virtual cursor position (CSS px), so a humanized move starts from
    /// where the pointer actually is. Defaults to the top-left.
    cursor:         Mutex<(f64, f64)>,
}

impl Page {
    /// Wrap an existing CDP page.
    pub(crate) fn new(inner: CdpPage) -> Self {
        Self { inner, download_armed: AtomicBool::new(false), cursor: Mutex::new((0.0, 0.0)) }
    }

    /// Whether a download is currently armed on this page (set by
    /// `arm_download` / `download_to_dir`, cleared by
    /// `reset_download_behavior`).
    pub fn is_download_armed(&self) -> bool {
        self.download_armed.load(Ordering::Relaxed)
    }

    /// The CDP target id of the underlying page, as a string.
    ///
    /// Stable across same-tab navigations, so another connection (a second
    /// process attached to the same Chrome via `ws_url`) can re-adopt this
    /// exact tab with
    /// [`BrowserSession::attach_page`](crate::BrowserSession::attach_page).
    pub fn target_id(&self) -> String {
        self.inner.target_id().inner().clone()
    }

    /// Apply stealth settings to this page.
    pub(crate) async fn apply_stealth(&self, cfg: &StealthConfig) -> Result<()> {
        // 1. Built-in stealth (patches navigator.webdriver etc.)
        if cfg.use_builtin_stealth {
            if let Some(ua) = &cfg.user_agent {
                self.inner
                    .enable_stealth_mode_with_agent(ua)
                    .await
                    .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
            } else {
                self.inner
                    .enable_stealth_mode()
                    .await
                    .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
            }
        }

        // 2. User-agent override + matching Client Hints.
        //
        // Three cases, in precedence order:
        //   a. Caller supplied an explicit `user_agent` — use it verbatim.
        //   b. No explicit UA, but `use_builtin_stealth` already applied
        //      its own agent via `enable_stealth_mode_with_agent` — skip.
        //   c. Default (cfg.user_agent = None, builtin stealth off): probe
        //      the browser's *real* UA and strip any "Headless" token. We
        //      override even when nothing was stripped, because the override
        //      is also what makes `navigator.platform` and
        //      `navigator.userAgentData` (Client Hints) CONSISTENT with the
        //      UA — a UA that says Linux while `navigator.platform` says
        //      "Win32" or `userAgentData.brands` is empty is itself a strong
        //      bot signal.
        let override_ua = if let Some(ua) = cfg.user_agent.clone() {
            Some(ua)
        } else if cfg.use_builtin_stealth {
            None
        } else {
            probe_user_agent(&self.inner).await?.map(|ua| dehead(&ua))
        };

        if let Some(ua) = override_ua {
            // Derive a coherent navigator.platform + Client-Hints metadata
            // from the UA so all three agree.
            let (nav_platform, metadata) = client_hints_for_ua(&ua);
            let mut builder = SetUserAgentOverrideParams::builder()
                .user_agent(ua)
                .accept_language(&cfg.locale)
                .platform(nav_platform);
            if let Some(metadata) = metadata {
                builder = builder.user_agent_metadata(metadata);
            }
            let params = builder.build().map_err(VoidCrawlError::PageError)?;
            self.inner
                .execute(params)
                .await
                .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        }

        // 3. Viewport / device metrics
        let mut metrics = SetDeviceMetricsOverrideParams::new(
            i64::from(cfg.viewport_width),
            i64::from(cfg.viewport_height),
            1.0,
            false,
        );
        metrics.screen_width = Some(i64::from(cfg.viewport_width));
        metrics.screen_height = Some(i64::from(cfg.viewport_height));
        self.inner.execute(metrics).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;

        // 4. Bypass CSP so our injected JS can run
        if cfg.bypass_csp {
            let csp = SetBypassCspParams::new(true);
            self.inner.execute(csp).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        }

        // 5. Inject custom JS before every navigation
        if let Some(js) = &cfg.inject_js {
            let params = AddScriptToEvaluateOnNewDocumentParams::new(js.clone());
            self.inner
                .execute(params)
                .await
                .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        }

        Ok(())
    }

    // ── Navigation ──────────────────────────────────────────────────────

    /// Navigate to `url` and wait for the CDP response.
    pub async fn navigate(&self, url: &str) -> Result<()> {
        self.inner.goto(url).await.map_err(|e| VoidCrawlError::NavigationFailed(e.to_string()))?;
        Ok(())
    }

    /// Navigate to `url` and wait for network idle, returning a
    /// [`PageResponse`].
    ///
    /// Subscribes to both `Page.lifecycleEvent` and `Network.responseReceived`
    /// **before** navigation starts so that no events are missed.  The
    /// `networkIdle` (or `networkAlmostIdle` fallback) event terminates the
    /// wait; a timeout is also applied.
    ///
    /// Equivalent to Playwright's `page.goto(url, wait_until='networkidle')`.
    pub async fn goto_and_wait_for_idle(
        &self,
        url: &str,
        timeout: Duration,
    ) -> Result<PageResponse> {
        self.goto_and_wait_for_idle_with_capture(url, timeout, false).await
    }

    /// Like [`Page::goto_and_wait_for_idle`], but when `capture_endpoints` is
    /// `true` also records the page's data-plane network endpoint set (XHR +
    /// Fetch request URLs) onto [`PageResponse::endpoints`].
    ///
    /// Capture is **opt-in** so the default fetch path pays no extra cost: the
    /// `Network.requestWillBeSent` listener is only subscribed when requested.
    /// It is passive (listen-only — no request interception, invisible to the
    /// site) and the endpoints are PII-stripped at the source via
    /// [`safe_endpoint`]. The listener is function-local and dropped on return,
    /// so nothing leaks across a pooled tab's recycle.
    #[allow(
        clippy::cognitive_complexity,
        reason = "a single navigate select-loop reads more clearly inline than split across helpers"
    )]
    pub async fn goto_and_wait_for_idle_with_capture(
        &self,
        url: &str,
        timeout: Duration,
        capture_endpoints: bool,
    ) -> Result<PageResponse> {
        // Subscribe to ALL event streams BEFORE navigation so no events slip
        // through the gap between goto() and the listener setup.
        let mut lifecycle = self
            .inner
            .event_listener::<EventLifecycleEvent>()
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;

        let mut network = self
            .inner
            .event_listener::<EventResponseReceived>()
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;

        // Request listener is gated on the opt-in so the wire/decode cost is
        // only paid when a caller wants the endpoint set.
        let mut requests = if capture_endpoints {
            Some(
                self.inner
                    .event_listener::<EventRequestWillBeSent>()
                    .await
                    .map_err(|e| VoidCrawlError::PageError(e.to_string()))?,
            )
        } else {
            None
        };

        // Start navigation (non-blocking CDP command)
        self.inner.goto(url).await.map_err(|e| VoidCrawlError::NavigationFailed(e.to_string()))?;

        let deadline = time::sleep(timeout);
        tokio::pin!(deadline);

        let mut status_code: Option<u16> = None;
        let mut redirect_count: u32 = 0;
        let mut got_almost_idle = false;
        // Headers of the final (non-redirect) Document response. Overwritten if
        // a later navigation supersedes it, mirroring `status_code`.
        let mut headers: Vec<(String, String)> = Vec::new();
        // Deduped data-plane endpoint set (only populated when capturing).
        let mut endpoints: HashSet<String> = HashSet::new();
        let mut endpoints_truncated = false;

        loop {
            tokio::select! {
                biased;
                maybe_lifecycle = lifecycle.next() => {
                    match maybe_lifecycle {
                        Some(event) => match event.name.as_str() {
                            "networkIdle" => break,
                            "networkAlmostIdle" => { got_almost_idle = true; }
                            _ => {}
                        },
                        None => break,
                    }
                }
                maybe_network = network.next() => {
                    if let Some(event) = maybe_network {
                        // Only the Document response carries the page's actual
                        // status code. Sub-resources (images, scripts, XHRs)
                        // are ignored so a 404 favicon doesn't overwrite a 200
                        // document status.
                        if event.r#type == ResourceType::Document {
                            // status is i64 from the CDP spec; real HTTP codes
                            // fit in u16, so the lossy truncation is intentional.
                            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            let code = event.response.status as u16;
                            if (300..400).contains(&code) {
                                // Redirect in the navigation chain.
                                redirect_count += 1;
                            } else if code != 0 {
                                // Chrome emits 0 for cancelled/intercepted
                                // requests — treat as "no network response".
                                status_code = Some(code);
                                headers = flatten_headers(event.response.headers.inner());
                            }
                            // A new Document response after networkAlmostIdle
                            // means a new navigation started; reset the flag so
                            // we don't exit on a stale almost-idle signal.
                            got_almost_idle = false;
                        }
                    }
                }
                // Endpoint capture — only polled when capturing (guard ensures
                // `requests` is Some). Sits BELOW lifecycle so a chatty request
                // stream can never starve the networkIdle break.
                //
                // select! evaluates every branch's future expression even when
                // its `if` guard is false, so the `None` branch must still yield
                // a same-typed future that never resolves — pending() parks it
                // harmlessly (it's unreachable in practice: requests is Some iff
                // capture_endpoints).
                maybe_request = async {
                    match requests.as_mut() {
                        Some(s) => s.next().await,
                        None => future::pending().await,
                    }
                }, if capture_endpoints => {
                    if let Some(event) = maybe_request {
                        if matches!(event.r#type, Some(ResourceType::Xhr | ResourceType::Fetch)) {
                            if let Some(ep) = safe_endpoint(&event.request.url) {
                                // A duplicate (already counted) applies no cap
                                // pressure; only a NEW endpoint past the cap
                                // flips the truncated flag.
                                if !endpoints.contains(&ep) {
                                    if endpoints.len() < MAX_ENDPOINTS {
                                        endpoints.insert(ep);
                                    } else {
                                        endpoints_truncated = true;
                                    }
                                }
                            }
                        }
                    }
                }
                () = &mut deadline => {
                    if got_almost_idle {
                        break;
                    }
                    // Hard timeout with no idle signal
                    let html = self.content().await.unwrap_or_default();
                    let final_url = self.url().await.unwrap_or_default().unwrap_or_default();
                    let antibot = status_code.map(|c| antibot::classify(c, &headers, &html));
                    return Ok(PageResponse {
                        html,
                        url: final_url,
                        status_code,
                        redirected: redirect_count > 0,
                        headers,
                        antibot,
                        endpoints: finalize_endpoints(&endpoints, capture_endpoints),
                        endpoints_truncated,
                        endpoint_sanitizer_version: capture_endpoints.then_some(ENDPOINT_SANITIZER_VERSION),
                    });
                }
            }
        }

        let html = self.content().await?;
        let final_url = self.url().await?.unwrap_or_default();
        let antibot = status_code.map(|c| antibot::classify(c, &headers, &html));
        Ok(PageResponse {
            html,
            url: final_url,
            status_code,
            redirected: redirect_count > 0,
            headers,
            antibot,
            endpoints: finalize_endpoints(&endpoints, capture_endpoints),
            endpoints_truncated,
            endpoint_sanitizer_version: capture_endpoints.then_some(ENDPOINT_SANITIZER_VERSION),
        })
    }

    /// Wait for the in-flight navigation to finish.
    pub async fn wait_for_navigation(&self) -> Result<()> {
        self.inner
            .wait_for_navigation()
            .await
            .map_err(|e| VoidCrawlError::NavigationFailed(e.to_string()))?;
        Ok(())
    }

    /// Event-driven wait for the network to become idle.
    ///
    /// Subscribes to `Page.lifecycleEvent` and waits for one of these
    /// events (in priority order):
    ///
    /// 1. **`networkIdle`** — 0 in-flight requests for 500 ms (best signal)
    /// 2. **`networkAlmostIdle`** — ≤ 2 in-flight requests for 500 ms (fallback
    ///    when analytics / long-polls prevent true idle)
    ///
    /// Returns the name of the lifecycle event that resolved the wait
    /// (`"networkIdle"` or `"networkAlmostIdle"`), or `None` if the
    /// timeout was reached without either event firing.
    ///
    /// This is fully async and event-driven — **no polling**.
    pub async fn wait_for_network_idle(&self, timeout: Duration) -> Result<Option<String>> {
        let mut events = self
            .inner
            .event_listener::<EventLifecycleEvent>()
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;

        let deadline = time::sleep(timeout);
        tokio::pin!(deadline);

        // Track the best event we've seen so far
        let mut got_almost_idle = false;

        loop {
            tokio::select! {
                biased;
                maybe_event = events.next() => {
                    match maybe_event {
                        Some(event) => {
                            match event.name.as_str() {
                                "networkIdle" => return Ok(Some("networkIdle".into())),
                                "networkAlmostIdle" => { got_almost_idle = true; }
                                _ => {} // DOMContentLoaded, load, etc — ignore
                            }
                        }
                        None => break, // stream closed
                    }
                }
                () = &mut deadline => break,
            }
        }

        // Timeout reached — return best fallback
        if got_almost_idle { Ok(Some("networkAlmostIdle".into())) } else { Ok(None) }
    }

    /// Wait until `document.querySelector(selector)` matches an element,
    /// driven by a `MutationObserver` inside the page — no Rust-side polling.
    /// Resolves immediately if the element is already present. Rejects with
    /// `VoidCrawlError::Timeout` after `timeout`.
    pub async fn wait_for_selector(&self, selector: &str, timeout: Duration) -> Result<()> {
        let sel_lit = serde_json::to_string(selector)
            .map_err(|e| VoidCrawlError::Other(format!("selector encode: {e}")))?;
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let js = format!(
            "() => new Promise((resolve, reject) => {{\
              const sel = {sel_lit};\
              if (document.querySelector(sel)) return resolve(true);\
              const root = document.documentElement || document.body;\
              const obs = new MutationObserver(() => {{\
                if (document.querySelector(sel)) {{\
                  obs.disconnect();\
                  clearTimeout(t);\
                  resolve(true);\
                }}\
              }});\
              obs.observe(root, {{ childList: true, subtree: true }});\
              const t = setTimeout(() => {{\
                obs.disconnect();\
                reject(new Error('wait_for_selector timeout: ' + sel));\
              }}, {timeout_ms});\
            }})"
        );
        match self.inner.evaluate_function(js).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("wait_for_selector timeout") {
                    Err(VoidCrawlError::Timeout(format!(
                        "selector {selector:?} did not appear within {timeout_ms}ms"
                    )))
                } else {
                    Err(VoidCrawlError::JsEvalError(msg))
                }
            }
        }
    }

    // ── Content ─────────────────────────────────────────────────────────

    /// Return the full HTML of the page (outer HTML of `<html>`).
    pub async fn content(&self) -> Result<String> {
        self.inner.content().await.map_err(|e| VoidCrawlError::PageError(e.to_string()))
    }

    /// Return the page title.
    pub async fn title(&self) -> Result<Option<String>> {
        self.inner.get_title().await.map_err(|e| VoidCrawlError::PageError(e.to_string()))
    }

    /// Return the current URL.
    pub async fn url(&self) -> Result<Option<String>> {
        self.inner.url().await.map_err(|e| VoidCrawlError::PageError(e.to_string()))
    }

    // ── JavaScript ──────────────────────────────────────────────────────

    /// Evaluate a JS expression and return the result as a JSON value.
    pub async fn evaluate_js(&self, expression: &str) -> Result<Value> {
        let result = self
            .inner
            .evaluate(expression)
            .await
            .map_err(|e| VoidCrawlError::JsEvalError(e.to_string()))?;
        // `into_value()` fails when the JS expression returns null/undefined
        // (the RemoteObject has no `value` field).  Fall back to Value::Null.
        match result.value() {
            Some(v) => Ok(v.clone()),
            None => Ok(Value::Null),
        }
    }

    /// Evaluate a JS expression **inside a specific frame's** execution
    /// context and return the result as a JSON value.
    ///
    /// Unlike [`Page::evaluate_js`] — which always runs in the top document —
    /// this targets the frame whose current URL contains `frame_url_pattern`.
    /// It is the only way to read or drive a **cross-origin** iframe: that
    /// frame's `contentDocument` is `null` from the parent under the
    /// same-origin policy, but CDP can evaluate in the frame's own execution
    /// context, where the origin check is satisfied. `expression` runs as if
    /// it were the frame's own page script (`document` is the frame's
    /// document).
    ///
    /// The match must be unique: more than one frame containing
    /// `frame_url_pattern` returns [`VoidCrawlError::AmbiguousFrame`]; no match
    /// (or a matched frame with no scriptable execution context — e.g. a
    /// `sandbox`ed frame without `allow-scripts`, or one not yet loaded)
    /// returns [`VoidCrawlError::FrameNotFound`].
    ///
    /// **In-process requirement.** The target frame must be in the page's
    /// renderer process for its context to be reachable here. VoidCrawl's
    /// default flags keep ordinary cross-origin frames in-process, but Chrome
    /// *field-trial*-isolates a few origins (notably google.com, hence
    /// reCAPTCHA's bframe) out-of-process regardless; those surface as
    /// `FrameNotFound`. To reach them, launch the session with
    /// `extra_args=["disable-site-isolation-trials"]` (an explicit opt-in,
    /// since it weakens the browser's isolation posture).
    pub async fn evaluate_js_in_frame(
        &self,
        frame_url_pattern: &str,
        expression: &str,
    ) -> Result<Value> {
        let frame_id = self.resolve_frame(frame_url_pattern).await?;
        let context_id = self
            .inner
            .frame_execution_context(frame_id)
            .await
            .map_err(|e| VoidCrawlError::JsEvalError(e.to_string()))?
            .ok_or_else(|| {
                VoidCrawlError::FrameNotFound(format!(
                    "{frame_url_pattern:?}: matched frame has no scriptable execution \
                     context (sandboxed without allow-scripts, or not yet loaded)"
                ))
            })?;
        let params = EvaluateParams::builder()
            .expression(expression)
            .context_id(context_id)
            .return_by_value(true)
            .await_promise(true)
            .build()
            .map_err(VoidCrawlError::JsEvalError)?;
        // `evaluate_expression` (not `evaluate`) so chromiumoxide does not
        // overwrite our explicit `context_id` with the top-document context.
        let result = self
            .inner
            .evaluate_expression(params)
            .await
            .map_err(|e| VoidCrawlError::JsEvalError(e.to_string()))?;
        match result.value() {
            Some(v) => Ok(v.clone()),
            None => Ok(Value::Null),
        }
    }

    /// Resolve the single frame whose URL contains `pattern`.
    ///
    /// chromiumoxide's handler already tracks the frame tree and each frame's
    /// execution context, so this is a cheap lookup with no extra CDP round
    /// trips beyond reading cached frame URLs.
    ///
    /// **Fails closed on ambiguity.** The match must be *unique*: if more than
    /// one frame's URL contains `pattern`, this returns
    /// [`VoidCrawlError::AmbiguousFrame`] rather than silently picking one.
    /// Frame enumeration order is not stable, and a hostile page can embed a
    /// decoy frame whose URL contains a common substring — so guessing would
    /// risk running the caller's JS in the wrong (possibly attacker-scripted)
    /// frame. Use a specific pattern (e.g. `recaptcha/api2/bframe`, not
    /// `recaptcha`); [`Page::frame_urls`] helps you find one.
    async fn resolve_frame(&self, pattern: &str) -> Result<FrameId> {
        let frames =
            self.inner.frames().await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let mut matched: Vec<(FrameId, String)> = Vec::new();
        for frame_id in frames {
            let url = self
                .inner
                .frame_url(frame_id.clone())
                .await
                .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
            if let Some(url) = url {
                if url.contains(pattern) {
                    matched.push((frame_id, url));
                }
            }
        }
        match matched.len() {
            0 => Err(VoidCrawlError::FrameNotFound(pattern.to_string())),
            1 => Ok(matched.swap_remove(0).0),
            n => {
                let urls = matched.iter().map(|(_, u)| u.as_str()).collect::<Vec<_>>().join(", ");
                Err(VoidCrawlError::AmbiguousFrame(format!(
                    "{pattern:?} matched {n} frames ({urls}); use a more specific substring"
                )))
            }
        }
    }

    /// List the URLs of every frame currently tracked on this page, in no
    /// particular order. Useful for discovering the right `frame_url_pattern`
    /// to pass to [`Page::evaluate_js_in_frame`].
    pub async fn frame_urls(&self) -> Result<Vec<String>> {
        let frames =
            self.inner.frames().await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let mut urls = Vec::with_capacity(frames.len());
        for frame_id in frames {
            if let Some(url) = self
                .inner
                .frame_url(frame_id)
                .await
                .map_err(|e| VoidCrawlError::PageError(e.to_string()))?
            {
                urls.push(url);
            }
        }
        Ok(urls)
    }

    // ── Screenshots & PDF ───────────────────────────────────────────────

    /// Capture a full-page PNG screenshot, returned as raw bytes.
    ///
    /// Backward-compatible shim around [`Page::screenshot`] with no
    /// options (full page, no crop, bytes in memory).
    pub async fn screenshot_png(&self) -> Result<Vec<u8>> {
        match self.screenshot(ScreenshotOptions::default()).await? {
            ScreenshotOutput::Bytes(b) => Ok(b),
            ScreenshotOutput::Path(_) => unreachable!("no path supplied"),
        }
    }

    /// Capture a PNG screenshot with optional cropping and/or writing
    /// to disk.
    ///
    /// * No `path` → returns bytes in memory.
    /// * `path` set → writes PNG to disk and returns that path.
    /// * `bbox` crops to a pixel region (CSS pixels, pre-DPR).
    pub async fn screenshot(&self, opts: ScreenshotOptions) -> Result<ScreenshotOutput> {
        let mut builder = ScreenshotParams::builder().format(CaptureScreenshotFormat::Png);
        if let Some(bbox) = opts.bbox {
            builder = builder.clip(Viewport {
                x:      f64::from(bbox.x),
                y:      f64::from(bbox.y),
                width:  f64::from(bbox.width),
                height: f64::from(bbox.height),
                scale:  1.0,
            });
        } else {
            builder = builder.full_page(true);
        }
        let bytes = self
            .inner
            .screenshot(builder.build())
            .await
            .map_err(|e| VoidCrawlError::ScreenshotError(e.to_string()))?;

        if let Some(path) = opts.path {
            fs::write(&path, &bytes).map_err(|e| {
                VoidCrawlError::ScreenshotError(format!("write {}: {e}", path.display()))
            })?;
            Ok(ScreenshotOutput::Path(path))
        } else {
            Ok(ScreenshotOutput::Bytes(bytes))
        }
    }

    /// Generate a PDF of the page, returned as raw bytes.
    pub async fn pdf_bytes(&self) -> Result<Vec<u8>> {
        let params = PrintToPdfParams::default();
        self.inner.pdf(params).await.map_err(|e| VoidCrawlError::PdfError(e.to_string()))
    }

    /// Download the resource at `url` into `dir`, returning the file that
    /// landed.
    ///
    /// The transfer runs inside this page's browser context — cookies, TLS
    /// fingerprint, and stealth patches are all preserved, unlike a
    /// side-channel HTTP GET. CDP
    /// `Browser.setDownloadBehavior(allowAndName)` routes the bytes to `dir`.
    ///
    /// A plain navigation only triggers a download for `Content-Disposition:
    /// attachment` responses — `inline` resources (e.g. a PDF) get rendered by
    /// Chrome's built-in viewer instead. To download *any* content type, the
    /// save is forced from inside the page: navigate to the URL's origin so an
    /// in-page `fetch` is same-origin (and carries cookies), then stream the
    /// response — **aborting past `max_bytes`** so a hostile server can't OOM
    /// the tab — into a blob and click a `download` anchor.
    ///
    /// Completion is detected by **watching the directory** (the file settling
    /// without a `.crdownload` suffix), not by `Browser.downloadProgress`
    /// events, which are unreliable in headless Chrome. The in-page fetch also
    /// reports its `Content-Type` and any error back through a `window` flag,
    /// so a failed fetch returns promptly instead of waiting out the
    /// timeout.
    ///
    /// The CDP download behavior is **always reset** before returning, so a
    /// pooled tab recycled to the next caller never inherits this download's
    /// `allowAndName` mode or output path.
    ///
    /// `dir` should be a fresh, empty directory the caller treats as quarantine
    /// and scans before trusting the file.
    pub async fn download_to_dir(
        &self,
        url: &str,
        dir: &Path,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<DownloadOutcome> {
        let outcome = self.run_download(url, dir, timeout, max_bytes).await;
        // ALWAYS reset: setDownloadBehavior is browser-context-scoped and our
        // download_path points at a quarantine dir the caller is about to
        // delete. Leaving it set would mis-route or break the next user of a
        // recycled pool tab.
        self.reset_download_behavior().await;
        outcome
    }

    /// Arm a capture for an **action-triggered** download into `dir`, returning
    /// a [`DownloadCapture`]. Set CDP download behavior to route files into
    /// `dir`, then snapshot the directory's current contents so the matching
    /// `wait` only accepts a *new* file.
    ///
    /// Use this for the *arm → act → await* flow when a page action (a button
    /// click, a generated/redirected/cross-origin URL) starts the download —
    /// the Google-Drive case — rather than [`Page::download_to_dir`], which
    /// needs a URL in hand. After arming, perform the triggering action with
    /// the normal methods (e.g. [`Page::click_by_role`]), then call
    /// [`DownloadCapture::wait`].
    ///
    /// `dir` should be a fresh directory the caller treats as quarantine and
    /// scans before trusting the file.
    pub async fn arm_download(&self, dir: &Path, max_bytes: u64) -> Result<DownloadCapture> {
        let params = SetDownloadBehaviorParams::builder()
            .behavior(SetDownloadBehaviorBehavior::AllowAndName)
            .download_path(dir.to_string_lossy().into_owned())
            .build()
            .map_err(VoidCrawlError::PageError)?;
        self.inner.execute(params).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        self.download_armed.store(true, Ordering::Relaxed);
        Ok(DownloadCapture { dir: dir.to_path_buf(), before: dir_entries(dir), max_bytes })
    }

    /// Reset CDP download behavior to Chrome's default and clear the armed
    /// flag. Best-effort: failures here must not mask the download result,
    /// so errors are swallowed.
    ///
    /// Does **not** navigate the page — a caller's page state (e.g. an open
    /// session sitting on the download's origin) is left intact.
    pub async fn reset_download_behavior(&self) {
        if let Ok(params) = SetDownloadBehaviorParams::builder()
            .behavior(SetDownloadBehaviorBehavior::Default)
            .build()
        {
            let _ = self.inner.execute(params).await;
        }
        self.download_armed.store(false, Ordering::Relaxed);
    }

    async fn run_download(
        &self,
        url: &str,
        dir: &Path,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<DownloadOutcome> {
        // Snapshot the dir so we only accept a file that appears *after* arming
        // — correctness no longer depends on the caller handing us a fresh dir.
        let before = dir_entries(dir);

        let params = SetDownloadBehaviorParams::builder()
            .behavior(SetDownloadBehaviorBehavior::AllowAndName)
            .download_path(dir.to_string_lossy().into_owned())
            .build()
            .map_err(VoidCrawlError::PageError)?;
        self.inner.execute(params).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        self.download_armed.store(true, Ordering::Relaxed);

        // Land on the target's origin so the in-page fetch below is same-origin
        // (no CORS wall, cookies included). Best-effort: a 4xx/5xx on the origin
        // root is fine, we only need a document in the right security context.
        if let Some(origin) = origin_of(url) {
            let _ = self.inner.goto(&origin).await;
        }

        // Kick off the streaming fetch→blob→anchor-click download. The IIFE
        // returns synchronously (so `evaluate_js` doesn't await a pending value)
        // and stashes progress on `window.__vcDl` for the poll loop to read.
        let url_json = serde_json::to_string(url).unwrap_or_else(|_| "''".to_string());
        let js =
            DOWNLOAD_JS.replace("__URL__", &url_json).replace("__MAX__", &max_bytes.to_string());
        self.evaluate_js(&js).await?;

        const POLL: Duration = Duration::from_millis(200);
        let deadline = time::Instant::now() + timeout;
        let mut settle = SettleTracker::new();
        let mut content_type: Option<String> = None;
        let mut done = false;

        loop {
            // Read in-page progress: surface a fetch error immediately, capture
            // the server's Content-Type, and learn when the blob save fired.
            if let Ok(state) = self.evaluate_js("window.__vcDl || null").await {
                if let Some(ct) = state.get("ct").and_then(|v| v.as_str()) {
                    content_type = Some(strip_mime_params(ct));
                }
                if let Some(err) = state.get("err").and_then(|v| v.as_str()) {
                    return Err(VoidCrawlError::Other(format!("download failed: {err}")));
                }
                if state.get("done").and_then(Value::as_bool) == Some(true) {
                    done = true;
                }
            }

            // Only trust the directory once the in-page driver reports the save
            // fired — the authoritative completion signal, not a heuristic.
            if done {
                if let Some(outcome) = settle.poll(dir, &before, max_bytes)? {
                    return Ok(DownloadOutcome { content_type, ..outcome });
                }
            }

            if time::Instant::now() >= deadline {
                return Err(VoidCrawlError::Timeout(format!(
                    "download did not complete within {}s",
                    timeout.as_secs()
                )));
            }
            time::sleep(POLL).await;
        }
    }

    /// Fetch the browser-computed accessibility (AX) tree for the root frame.
    ///
    /// Wraps CDP `Accessibility.getFullAXTree`. The result is the raw,
    /// browser-computed semantic view assistive tech sees: a **flat JSON
    /// array of nodes** linked by `childIds`/`parentId`, each carrying
    /// `role`, computed accessible `name`, `properties` (state like
    /// `focusable`/`expanded`), and `backendDOMNodeId` (the bridge back to
    /// the DOM). Implicit roles are resolved and `aria-hidden`/`display:none`
    /// nodes are pruned, so this is far more redesign-durable than markup.
    ///
    /// The tree only reflects real content once JavaScript has rendered the
    /// page — call it after navigation has settled.
    ///
    /// `depth` bounds how far descendants are walked; `None` returns the
    /// whole tree. Nodes are returned verbatim from CDP (no reshaping) so
    /// callers can address into them however they like.
    pub async fn get_full_ax_tree(&self, depth: Option<i64>) -> Result<Value> {
        let params = GetFullAxTreeParams { depth, frame_id: None };
        let resp = self
            .inner
            .execute(params)
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        serde_json::to_value(&resp.result.nodes)
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))
    }

    /// Fetch the AX tree and render it as a compact, indented `role "name"`
    /// outline — the readable view, with text-noise and hidden nodes pruned.
    /// See [`crate::ax::compact_outline`] for the raw-nodes → string helper.
    pub async fn ax_tree_outline(&self, depth: Option<i64>) -> Result<String> {
        let tree = self.get_full_ax_tree(depth).await?;
        let nodes = tree.as_array().map_or(&[][..], Vec::as_slice);
        Ok(compact_outline(nodes))
    }

    /// Query the accessibility tree for nodes matching `role` and/or the
    /// computed accessible `name`, rooted at the document.
    ///
    /// Wraps CDP `Accessibility.queryAXTree`. Name matching is exact (the
    /// browser's computed accessible name). Returns the matching nodes as
    /// raw CDP JSON — the AX analogue of `query_selector_all`, but addressing
    /// by semantics rather than markup. Passing neither `role` nor `name`
    /// returns every node under the root.
    pub async fn query_ax_tree(&self, role: Option<&str>, name: Option<&str>) -> Result<Value> {
        let nodes = self.query_ax_nodes(role, name).await?;
        serde_json::to_value(&nodes).map_err(|e| VoidCrawlError::PageError(e.to_string()))
    }

    /// Internal: run `Accessibility.queryAXTree` rooted at the document and
    /// return the typed matches.
    async fn query_ax_nodes(&self, role: Option<&str>, name: Option<&str>) -> Result<Vec<AxNode>> {
        let doc = self
            .inner
            .execute(GetDocumentParams::default())
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let params = QueryAxTreeParams {
            node_id: Some(doc.result.root.node_id),
            accessible_name: name.map(str::to_string),
            role: role.map(str::to_string),
            ..Default::default()
        };
        let resp = self
            .inner
            .execute(params)
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(resp.result.nodes)
    }

    /// Click an element addressed by its accessibility `role` and accessible
    /// `name` — the durable, markup-independent analogue of [`click_element`].
    ///
    /// Resolves via `Accessibility.queryAXTree`, picks the `nth` non-ignored
    /// match (0-based), bridges to the DOM through `backendDOMNodeId`, then
    /// scrolls it into view and clicks it. Errors if no such node exists.
    ///
    /// With `humanize = true`, the element is scrolled into view and then
    /// clicked at its box-model centre with a **trusted compositor** event
    /// along a human-like cursor path (see [`click_xy`]) — rather than the
    /// DOM `this.click()` used by default. Untrusted `.click()` is fine for
    /// ordinary forms but rejected by some challenge widgets.
    ///
    /// [`click_element`]: Self::click_element
    /// [`click_xy`]: Self::click_xy
    pub async fn click_by_role(
        &self,
        role: &str,
        name: &str,
        nth: usize,
        humanize: bool,
    ) -> Result<()> {
        let nodes = self.query_ax_nodes(Some(role), Some(name)).await?;
        let backends: Vec<_> =
            nodes.iter().filter(|n| !n.ignored).filter_map(|n| n.backend_dom_node_id).collect();
        let backend_id = backends.get(nth).copied().ok_or_else(|| {
            VoidCrawlError::PageError(format!(
                "no AX node with role={role:?} name={name:?} at index {nth} (found {} match(es))",
                backends.len()
            ))
        })?;

        // Bridge AX node → DOM → JS handle. Resolve once; both paths scroll it
        // into view first.
        let resolved = self
            .inner
            .execute(ResolveNodeParams { backend_node_id: Some(backend_id), ..Default::default() })
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let object_id = resolved.result.object.object_id.ok_or_else(|| {
            VoidCrawlError::PageError("AX node could not be resolved to a DOM handle".into())
        })?;

        if humanize {
            // Scroll into view, then a trusted compositor click at the box centre.
            let scroll = CallFunctionOnParams::builder()
                .object_id(object_id)
                .function_declaration(
                    "function(){ this.scrollIntoView({block:'center',inline:'center'}); }",
                )
                .await_promise(false)
                .build()
                .map_err(VoidCrawlError::PageError)?;
            self.inner
                .execute(scroll)
                .await
                .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
            let bm = self
                .inner
                .execute(GetBoxModelParams {
                    backend_node_id: Some(backend_id),
                    ..Default::default()
                })
                .await
                .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
            let q = bm.result.model.content.inner();
            if q.len() < 8 {
                return Err(VoidCrawlError::PageError(
                    "element has no box-model content quad".into(),
                ));
            }
            let cx = (q[0] + q[2] + q[4] + q[6]) / 4.0;
            let cy = (q[1] + q[3] + q[5] + q[7]) / 4.0;
            return self.click_xy(cx, cy, true).await;
        }

        // Default: the element's own click() — avoids box-model math and survives
        // elements that are off-screen until scrolled into view.
        let call = CallFunctionOnParams::builder()
            .object_id(object_id)
            .function_declaration(
                "function(){ this.scrollIntoView({block:'center',inline:'center'}); this.click(); }",
            )
            .await_promise(false)
            .build()
            .map_err(VoidCrawlError::PageError)?;
        self.inner.execute(call).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    /// Compact accessibility outline of a specific (possibly cross-origin)
    /// **frame** — the cross-frame analogue of [`ax_tree_outline`].
    ///
    /// Roots `Accessibility.getFullAXTree` at the frame matched by
    /// `frame_url_pattern` (resolved like [`evaluate_js_in_frame`]). The AX
    /// tree is browser-computed and ignores shadow-DOM mode, so this
    /// **pierces closed shadow roots** the page's own JavaScript cannot
    /// read — use it to discover the `role` / accessible-name to pass to
    /// [`click_ax_in_frame`].
    ///
    /// [`ax_tree_outline`]: Self::ax_tree_outline
    /// [`evaluate_js_in_frame`]: Self::evaluate_js_in_frame
    /// [`click_ax_in_frame`]: Self::click_ax_in_frame
    pub async fn ax_outline_in_frame(
        &self,
        frame_url_pattern: &str,
        depth: Option<i64>,
    ) -> Result<String> {
        let frame_id = self.resolve_frame(frame_url_pattern).await?;
        let resp = self
            .inner
            .execute(GetFullAxTreeParams { depth, frame_id: Some(frame_id) })
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let nodes = serde_json::to_value(&resp.result.nodes)
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(compact_outline(nodes.as_array().map_or(&[][..], Vec::as_slice)))
    }

    /// Locate an element by accessibility `role` + accessible `name` **inside a
    /// specific (possibly cross-origin) frame** and click it with a real
    /// **compositor** mouse event. The cross-frame, shadow-piercing analogue of
    /// [`click_by_role`].
    ///
    /// `Accessibility.getFullAXTree` rooted at the resolved frame descends into
    /// that frame's tree **including closed shadow roots** (the AX tree is
    /// browser-computed and ignores shadow mode), so it reaches widgets that
    /// `contentDocument` / page-JS cannot — e.g. Cloudflare Turnstile's
    /// "Verify you are human" checkbox, which lives in a closed shadow root
    /// inside a cross-origin `challenges.cloudflare.com` iframe. The matched
    /// node is clicked at its box-model centre via `Input.dispatchMouseEvent`
    /// (a **trusted** event), *not* a DOM `.click()` — challenge widgets reject
    /// untrusted clicks, and crucially this does **no page-JS shadow
    /// tampering**, so it does not trip Turnstile's closed-shadow check
    /// (ERROR 600010).
    ///
    /// An empty `name` matches any node of that `role`. Picks the `nth`
    /// (0-based) non-ignored match; errors if there is none.
    ///
    /// In-process requirement: as with [`evaluate_js_in_frame`], the frame must
    /// be in the page's renderer process — cross-origin google.com / cloudflare
    /// frames need the session launched with `disable-site-isolation-trials`.
    ///
    /// [`click_by_role`]: Self::click_by_role
    /// [`evaluate_js_in_frame`]: Self::evaluate_js_in_frame
    pub async fn click_ax_in_frame(
        &self,
        frame_url_pattern: &str,
        role: &str,
        name: &str,
        nth: usize,
        humanize: bool,
    ) -> Result<()> {
        let q = self.ax_content_quad_in_frame(frame_url_pattern, role, name, nth).await?;
        let cx = (q[0] + q[2] + q[4] + q[6]) / 4.0;
        let cy = (q[1] + q[3] + q[5] + q[7]) / 4.0;
        // Trusted compositor click (optionally humanized approach).
        self.click_xy(cx, cy, humanize).await
    }

    /// Locate an element by accessibility `role` + `name` **inside a specific
    /// frame** and return its on-page rectangle `[x, y, width, height]` in CSS
    /// pixels — the geometry needed to drive a **humanized** click yourself
    /// (e.g. move the cursor along a curved path with [`dispatch_mouse_event`]
    /// and press at a jittered point inside the box), rather than the single
    /// centre click of [`click_ax_in_frame`].
    ///
    /// Same cross-frame, closed-shadow-piercing resolution as
    /// [`click_ax_in_frame`]; an empty `name` matches any node of that `role`.
    ///
    /// [`dispatch_mouse_event`]: Self::dispatch_mouse_event
    /// [`click_ax_in_frame`]: Self::click_ax_in_frame
    pub async fn ax_box_in_frame(
        &self,
        frame_url_pattern: &str,
        role: &str,
        name: &str,
        nth: usize,
    ) -> Result<Vec<f64>> {
        let q = self.ax_content_quad_in_frame(frame_url_pattern, role, name, nth).await?;
        let xs = [q[0], q[2], q[4], q[6]];
        let ys = [q[1], q[3], q[5], q[7]];
        let left = xs.iter().copied().fold(f64::INFINITY, f64::min);
        let top = ys.iter().copied().fold(f64::INFINITY, f64::min);
        let right = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let bottom = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Ok(vec![left, top, right - left, bottom - top])
    }

    /// Resolve a frame-scoped AX `role`+`name` match to its box-model content
    /// quad `[x1,y1, x2,y2, x3,y3, x4,y4]` in page coordinates.
    async fn ax_content_quad_in_frame(
        &self,
        frame_url_pattern: &str,
        role: &str,
        name: &str,
        nth: usize,
    ) -> Result<Vec<f64>> {
        let backend_id = self.ax_backend_in_frame(frame_url_pattern, role, name, nth).await?;
        let bm = self
            .inner
            .execute(GetBoxModelParams { backend_node_id: Some(backend_id), ..Default::default() })
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let quad = bm.result.model.content.inner().clone();
        if quad.len() < 8 {
            return Err(VoidCrawlError::PageError("AX node has no box-model content quad".into()));
        }
        Ok(quad)
    }

    /// Resolve a frame-scoped AX `role`+`name` match to its `backendDOMNodeId`.
    async fn ax_backend_in_frame(
        &self,
        frame_url_pattern: &str,
        role: &str,
        name: &str,
        nth: usize,
    ) -> Result<BackendNodeId> {
        fn ax_text(v: Option<&AxValue>) -> &str {
            v.and_then(|a| a.value.as_ref()).and_then(Value::as_str).unwrap_or("")
        }
        let frame_id = self.resolve_frame(frame_url_pattern).await?;
        let resp = self
            .inner
            .execute(GetFullAxTreeParams { depth: None, frame_id: Some(frame_id) })
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let matched = resp
            .result
            .nodes
            .iter()
            .filter(|n| {
                !n.ignored
                    && ax_text(n.role.as_ref()) == role
                    && (name.is_empty() || ax_text(n.name.as_ref()) == name)
            })
            .filter_map(|n| n.backend_dom_node_id)
            .nth(nth);
        matched.ok_or_else(|| {
            VoidCrawlError::PageError(format!(
                "no AX node with role={role:?} name={name:?} at index {nth} in frame {frame_url_pattern:?}"
            ))
        })
    }

    // ── Humanized pointer input (CAS-147) ───────────────────────────────

    /// Move the virtual cursor to `(x, y)` via CDP `Input.dispatchMouseEvent`.
    ///
    /// With `humanize = true` the cursor travels a realistic path from its last
    /// position — non-linear (arc) curvature, a minimum-jerk velocity profile,
    /// small tremor, and a brief dwell — as multiple `MouseMoved` events
    /// ([`crate::input`]). With `humanize = false` it jumps in a single event.
    /// **No page-world JS** is injected. The path length/duration scale with
    /// distance and stay bounded for agent workflows.
    pub async fn move_mouse(&self, x: f64, y: f64, humanize: bool) -> Result<()> {
        if humanize {
            let start = *self
                .cursor
                .lock()
                .map_err(|_| VoidCrawlError::Other("cursor lock poisoned".into()))?;
            let mut rng = Rng::seed(runtime_seed());
            let path = humanized_path(start, (x, y), &HumanizeOptions::default(), &mut rng);
            for step in path {
                time::sleep(Duration::from_millis(step.delay_ms)).await;
                self.dispatch_mouse_event(
                    DispatchMouseEventType::MouseMoved,
                    step.x,
                    step.y,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
            }
        } else {
            self.dispatch_mouse_event(
                DispatchMouseEventType::MouseMoved,
                x,
                y,
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
        }
        *self.cursor.lock().map_err(|_| VoidCrawlError::Other("cursor lock poisoned".into()))? =
            (x, y);
        Ok(())
    }

    /// Click at `(x, y)` with a **trusted** compositor event (press → release).
    /// With `humanize = true`, the cursor first travels a human-like path to
    /// the point (see [`move_mouse`]). The analogue of
    /// `click_visual_coords`.
    ///
    /// [`move_mouse`]: Self::move_mouse
    pub async fn click_xy(&self, x: f64, y: f64, humanize: bool) -> Result<()> {
        self.move_mouse(x, y, humanize).await?;
        self.dispatch_mouse_event(
            DispatchMouseEventType::MousePressed,
            x,
            y,
            Some(MouseButton::Left),
            Some(1),
            None,
            None,
            None,
        )
        .await?;
        self.dispatch_mouse_event(
            DispatchMouseEventType::MouseReleased,
            x,
            y,
            Some(MouseButton::Left),
            Some(1),
            None,
            None,
            None,
        )
        .await?;
        Ok(())
    }

    // ── Emulation ───────────────────────────────────────────────────────

    /// Override the page's geolocation. Geo-aware sites (maps, "near me"
    /// search, store locators) will behave as if the browser is at these
    /// coordinates. `accuracy` defaults to 50 metres.
    ///
    /// Note: sites that read `navigator.geolocation` still gate on the
    /// geolocation *permission* (granted here) and require a secure context
    /// (https / localhost), not `data:` URLs. Header/IP-driven geo (e.g.
    /// Google Maps) keys off [`set_locale`] and the request URL more than this.
    ///
    /// [`set_locale`]: Self::set_locale
    pub async fn set_geolocation(
        &self,
        latitude: f64,
        longitude: f64,
        accuracy: Option<f64>,
    ) -> Result<()> {
        // Grant the geolocation permission first, otherwise headless Chrome
        // auto-denies `navigator.geolocation` and the override is never read.
        // Origin omitted → applies to every origin (incl. opaque `data:`).
        let grant = SetPermissionParams {
            permission:         PermissionDescriptor::new("geolocation"),
            setting:            PermissionSetting::Granted,
            origin:             None,
            embedded_origin:    None,
            browser_context_id: None,
        };
        self.inner.execute(grant).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;

        let params = SetGeolocationOverrideParams {
            latitude: Some(latitude),
            longitude: Some(longitude),
            accuracy: Some(accuracy.unwrap_or(50.0)),
            ..Default::default()
        };
        self.inner.execute(params).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    /// Override the JS locale and `Accept-Language` (e.g. `"en-US"`,
    /// `"fr-FR"`). This is the lever that shifts region-aware content like
    /// Google Maps results or localized pricing.
    pub async fn set_locale(&self, locale: &str) -> Result<()> {
        let params = SetLocaleOverrideParams { locale: Some(locale.to_string()) };
        self.inner.execute(params).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    /// Override the timezone by IANA id (e.g. `"America/New_York"`). Affects
    /// `Date`, `Intl`, and any server probes that read the rendered clock.
    pub async fn set_timezone(&self, timezone_id: &str) -> Result<()> {
        let params = SetTimezoneOverrideParams::new(timezone_id.to_string());
        self.inner.execute(params).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    // ── DOM Queries ─────────────────────────────────────────────────────

    /// Run `document.querySelector(selector)` and return the inner HTML.
    /// Returns `None` if no element matches. Void elements (e.g. `<input>`)
    /// return `Some("")`.
    ///
    /// Uses a JS eval rather than `find_element` so that a missing element
    /// returns `Ok(None)` without any CDP error — real errors (closed browser,
    /// network failure, etc.) still propagate as `Err`.
    pub async fn query_selector(&self, selector: &str) -> Result<Option<String>> {
        // `querySelector` returns null for no match — never throws — so the
        // only error path here is a real CDP failure, not a missing element.
        let js = format!(
            "(function(){{ var el = document.querySelector({selector:?}); \
             return el === null ? null : el.innerHTML; }})()"
        );
        let result = self
            .inner
            .evaluate_expression(js)
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;

        // `into_value()` returns Err("No value found") when JS evaluates to
        // null/undefined — that is exactly the "not found" case, not a real
        // error, so map it to Ok(None).
        let val: Value = match result.into_value() {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };

        match val {
            Value::Null => Ok(None),
            Value::String(s) => Ok(Some(s)),
            other => Ok(Some(other.to_string())),
        }
    }

    /// Run `document.querySelectorAll(selector)` and return inner HTML of each.
    /// One entry is returned per matched element; void elements yield `""`.
    pub async fn query_selector_all(&self, selector: &str) -> Result<Vec<String>> {
        // Single JS eval returns all innerHTML at once — avoids N serial CDP
        // round-trips (one per element) that the old find_elements approach needed.
        let js = format!("[...document.querySelectorAll({selector:?})].map(e => e.innerHTML)");
        let val: Value = self
            .inner
            .evaluate_expression(js)
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?
            .into_value()
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;

        match val {
            Value::Array(arr) => Ok(arr
                .into_iter()
                .map(|v| match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    // ── Interaction ─────────────────────────────────────────────────────

    /// Click on the first element matching `selector`.
    pub async fn click_element(&self, selector: &str) -> Result<()> {
        let el = self
            .inner
            .find_element(selector)
            .await
            .map_err(|e| VoidCrawlError::ElementNotFound(e.to_string()))?;
        el.click().await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    /// Type text into the first element matching `selector`.
    ///
    /// Focuses the element first so that key events are directed to it.
    pub async fn type_into(&self, selector: &str, text: &str) -> Result<()> {
        let el = self
            .inner
            .find_element(selector)
            .await
            .map_err(|e| VoidCrawlError::ElementNotFound(e.to_string()))?;
        el.focus().await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        el.type_str(text).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    // ── Headers & Network ───────────────────────────────────────────────

    /// Set extra HTTP headers for all subsequent requests from this page.
    pub async fn set_headers(&self, headers: HashMap<String, String>) -> Result<()> {
        let json_val =
            serde_json::to_value(&headers).map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let params = SetExtraHttpHeadersParams::new(Headers::new(json_val));
        self.inner.execute(params).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    // ── Cookies ─────────────────────────────────────────────────────────

    /// Return all cookies that match the current page URL.
    pub async fn get_cookies(&self) -> Result<Vec<Cookie>> {
        self.inner.get_cookies().await.map_err(|e| VoidCrawlError::PageError(e.to_string()))
    }

    /// Set a single cookie on the current page.
    pub async fn set_cookie(&self, cookie: CookieParam) -> Result<()> {
        self.inner
            .set_cookie(cookie)
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    /// Set multiple cookies at once.
    pub async fn set_cookies(&self, cookies: Vec<CookieParam>) -> Result<()> {
        self.inner
            .set_cookies(cookies)
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    /// Delete cookies by name, optionally scoped by domain and path.
    pub async fn delete_cookies(&self, cookies: Vec<DeleteCookiesParams>) -> Result<()> {
        self.inner
            .delete_cookies(cookies)
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    // ── CDP Input ───────────────────────────────────────────────────────

    /// Dispatch a mouse event via the CDP `Input.dispatchMouseEvent` command.
    ///
    /// This sends a **browser-level** input event — as opposed to a JS
    /// `dispatchEvent(new MouseEvent(...))` — so it is processed by the
    /// compositor and behaves like a real user action (including triggering
    /// hover states, native drag, etc.).
    #[allow(clippy::too_many_arguments)]
    pub async fn dispatch_mouse_event(
        &self,
        event_type: DispatchMouseEventType,
        x: f64,
        y: f64,
        button: Option<MouseButton>,
        click_count: Option<i64>,
        delta_x: Option<f64>,
        delta_y: Option<f64>,
        modifiers: Option<i64>,
    ) -> Result<()> {
        let mut builder = DispatchMouseEventParams::builder().r#type(event_type).x(x).y(y);

        if let Some(b) = button {
            builder = builder.button(b);
        }
        if let Some(c) = click_count {
            builder = builder.click_count(c);
        }
        if let Some(dx) = delta_x {
            builder = builder.delta_x(dx);
        }
        if let Some(dy) = delta_y {
            builder = builder.delta_y(dy);
        }
        if let Some(m) = modifiers {
            builder = builder.modifiers(m);
        }

        let params = builder.build().map_err(VoidCrawlError::PageError)?;
        self.inner.execute(params).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    /// Dispatch a key event via the CDP `Input.dispatchKeyEvent` command.
    ///
    /// Sends a browser-level keyboard event. Use `KeyDown` + `KeyUp` for
    /// modifier keys or special keys, and `Char` for text input.
    pub async fn dispatch_key_event(
        &self,
        event_type: DispatchKeyEventType,
        key: Option<&str>,
        code: Option<&str>,
        text: Option<&str>,
        modifiers: Option<i64>,
    ) -> Result<()> {
        let mut builder = DispatchKeyEventParams::builder().r#type(event_type);

        if let Some(k) = key {
            builder = builder.key(k);
        }
        if let Some(c) = code {
            builder = builder.code(c);
        }
        if let Some(t) = text {
            builder = builder.text(t);
        }
        if let Some(m) = modifiers {
            builder = builder.modifiers(m);
        }

        let params = builder.build().map_err(VoidCrawlError::PageError)?;
        self.inner.execute(params).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    /// Close this page / tab.
    pub async fn close(self) -> Result<()> {
        self.inner.close().await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        Ok(())
    }

    /// Access the underlying chromiumoxide Page for advanced usage.
    pub fn inner(&self) -> &CdpPage {
        &self.inner
    }
}

/// In-page download driver. `__URL__` and `__MAX__` are substituted before
/// evaluation. Streams the response, aborting past `__MAX__` bytes so a hostile
/// server can't OOM the tab, then saves the bytes via a blob `download` anchor
/// (which forces a save even for `Content-Disposition: inline` resources like
/// PDFs that Chrome would otherwise render). Progress is reported on
/// `window.__vcDl = { ct, err, done }` for the Rust poll loop.
const DOWNLOAD_JS: &str = r"(() => {
  window.__vcDl = { ct: null, err: null, done: false };
  (async () => {
    try {
      const MAX = __MAX__;
      const ctrl = new AbortController();
      const resp = await fetch(__URL__, { credentials: 'include', signal: ctrl.signal });
      window.__vcDl.ct = resp.headers.get('content-type');
      const cl = resp.headers.get('content-length');
      if (cl && Number(cl) > MAX) { ctrl.abort(); throw new Error('content-length ' + cl + ' exceeds limit ' + MAX); }
      let blob;
      if (resp.body && resp.body.getReader) {
        const reader = resp.body.getReader();
        const chunks = []; let total = 0;
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          total += value.byteLength;
          if (total > MAX) { ctrl.abort(); throw new Error('exceeded size limit ' + MAX + ' bytes'); }
          chunks.push(value);
        }
        blob = new Blob(chunks);
      } else {
        blob = await resp.blob();
        if (blob.size > MAX) throw new Error('exceeded size limit ' + MAX + ' bytes');
      }
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = (__URL__.split(/[?#]/)[0].split('/').pop()) || 'download';
      (document.body || document.documentElement).appendChild(a);
      a.click();
      window.__vcDl.done = true;
    } catch (e) {
      window.__vcDl.err = String((e && e.message) || e);
    }
  })();
  return true;
})()";

/// Strip parameters from a MIME type: `application/pdf; charset=utf-8` →
/// `application/pdf`.
fn strip_mime_params(mime: &str) -> String {
    mime.split(';').next().unwrap_or(mime).trim().to_ascii_lowercase()
}

/// `scheme://host[:port]` for `url`, or `None` if it isn't an absolute URL.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

/// Snapshot the set of paths currently in `dir` (empty on a read error).
fn dir_entries(dir: &Path) -> HashSet<PathBuf> {
    fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect()
}

/// Finished (non-`.crdownload`, non-empty) files in `dir` that are **not** in
/// `before` — i.e. downloads that appeared after the snapshot.
fn new_complete_files(dir: &Path, before: &HashSet<PathBuf>) -> Vec<(PathBuf, u64)> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if before.contains(&path) || path.extension().is_some_and(|e| e == "crdownload") {
                return None;
            }
            match entry.metadata() {
                Ok(m) if m.is_file() && m.len() > 0 => Some((path, m.len())),
                _ => None,
            }
        })
        .collect()
}

/// Number of identical consecutive size samples required before a file is
/// accepted — ~2 poll intervals of an unchanged size, so a stream that pauses
/// mid-write isn't captured truncated.
const SETTLE_SIGHTINGS: u32 = 3;

/// Tracks the size-stability of the newest new download across polls.
struct SettleTracker {
    prev:   Option<(PathBuf, u64)>,
    stable: u32,
}

impl SettleTracker {
    fn new() -> Self {
        Self { prev: None, stable: 0 }
    }

    /// One poll over `dir`. `Ok(Some(_))` once a single new file's size has
    /// held steady for [`SETTLE_SIGHTINGS`] samples; `Ok(None)` to keep
    /// waiting; `Err` if more than one new file appeared (ambiguous) or the
    /// file is oversized (deleted first).
    fn poll(
        &mut self,
        dir: &Path,
        before: &HashSet<PathBuf>,
        max_bytes: u64,
    ) -> Result<Option<DownloadOutcome>> {
        let files = new_complete_files(dir, before);
        if files.len() > 1 {
            let names = files
                .iter()
                .filter_map(|(p, _)| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(VoidCrawlError::Other(format!(
                "ambiguous download: {} new files appeared ({names}); expected exactly one",
                files.len()
            )));
        }
        let Some((path, size)) = files.into_iter().next() else {
            self.prev = None;
            self.stable = 0;
            return Ok(None);
        };

        if self.prev.as_ref().is_some_and(|(p, s)| *p == path && *s == size) {
            self.stable += 1;
        } else {
            self.prev = Some((path.clone(), size));
            self.stable = 1;
        }
        if self.stable < SETTLE_SIGHTINGS {
            return Ok(None);
        }
        if size > max_bytes {
            let _ = fs::remove_file(&path);
            return Err(VoidCrawlError::Other(format!(
                "download is {size} bytes, over the {max_bytes}-byte limit"
            )));
        }
        Ok(Some(DownloadOutcome { path, bytes: size, content_type: None }))
    }
}

/// Poll `dir` until a **new** completed download settles (see
/// [`SettleTracker::poll`]), or `timeout` elapses.
async fn wait_for_new_download(
    dir: &Path,
    before: &HashSet<PathBuf>,
    max_bytes: u64,
    timeout: Duration,
) -> Result<DownloadOutcome> {
    const POLL: Duration = Duration::from_millis(250);
    let deadline = time::Instant::now() + timeout;
    let mut settle = SettleTracker::new();

    loop {
        if let Some(outcome) = settle.poll(dir, before, max_bytes)? {
            return Ok(outcome);
        }
        if time::Instant::now() >= deadline {
            return Err(VoidCrawlError::Timeout(format!(
                "no download completed within {}s",
                timeout.as_secs()
            )));
        }
        time::sleep(POLL).await;
    }
}

/// Probe the browser's real User-Agent and strip any "Headless"
/// qualifier. Returns `Some(stripped_ua)` when the probe finds
/// `HeadlessChrome` (or similar) and a rewrite is needed; returns
/// `None` otherwise, signalling "no override necessary".
///
/// Headless Chrome advertises itself as `HeadlessChrome/<ver>` — an
/// instant bot signal. By probing the real UA and rewriting only the
/// `Headless` substring, we keep the version accurate (no stale
/// hardcoded UA string) while removing the fingerprint.
async fn probe_user_agent(page: &CdpPage) -> Result<Option<String>> {
    let probe = page
        .evaluate("navigator.userAgent")
        .await
        .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
    match probe.value().cloned() {
        Some(Value::String(ua)) => Ok(Some(ua)),
        _ => Ok(None),
    }
}

/// Strip any "Headless" token from a UA. Headless Chrome advertises
/// `HeadlessChrome/<ver>` — an instant bot signal. Rewriting only the
/// `Headless` substring keeps the version accurate (no stale hardcoded UA).
fn dehead(ua: &str) -> String {
    if ua.contains("HeadlessChrome") {
        ua.replace("HeadlessChrome", "Chrome")
    } else if ua.contains("Headless") {
        ua.replace("Headless", "")
    } else {
        ua.to_string()
    }
}

/// Derive a coherent `navigator.platform` value and Client-Hints
/// [`UserAgentMetadata`] from a UA string, so the UA, `navigator.platform`,
/// and `navigator.userAgentData` all agree. A mismatch between them (e.g. a
/// Linux UA with `navigator.platform == "Win32"`, or empty `brands`) is a
/// strong bot signal. Best-effort: an unrecognized UA gets a generic
/// Linux/x86_64 identity, and a missing Chrome version yields empty brands
/// rather than a wrong one.
fn client_hints_for_ua(ua: &str) -> (String, Option<UserAgentMetadata>) {
    // (navigator.platform, Sec-CH-UA-Platform, platformVersion)
    let (nav_platform, ch_platform, platform_version) = if ua.contains("Windows") {
        ("Win32", "Windows", "15.0.0")
    } else if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        ("MacIntel", "macOS", "14.5.0")
    } else {
        ("Linux x86_64", "Linux", "6.8.0")
    };

    // Chrome version from the UA: "…Chrome/148.0.0.0 …" → major "148", full
    // "148.0.0.0". `None` when absent (non-Chrome UA) → no brands.
    let chrome_ver: Option<&str> =
        ua.split("Chrome/").nth(1).and_then(|s| s.split_whitespace().next());
    let major: Option<&str> = chrome_ver.and_then(|v| v.split('.').next());

    let mut builder = UserAgentMetadata::builder()
        .platform(ch_platform)
        .platform_version(platform_version)
        .architecture("x86")
        .model("")
        .mobile(false)
        .bitness("64")
        .wow64(false);

    if let (Some(major), Some(full)) = (major, chrome_ver) {
        // Low-entropy `brands` (major only) + `fullVersionList` (full), each
        // with a GREASE entry, mirroring what real Chrome emits.
        builder = builder
            .brands([
                UserAgentBrandVersion::new("Chromium", major),
                UserAgentBrandVersion::new("Google Chrome", major),
                UserAgentBrandVersion::new("Not_A Brand", "24"),
            ])
            .full_version_lists([
                UserAgentBrandVersion::new("Chromium", full),
                UserAgentBrandVersion::new("Google Chrome", full),
                UserAgentBrandVersion::new("Not_A Brand", "24.0.0.0"),
            ]);
    }

    // build() only errors if a mandatory field is unset; platform,
    // platform_version, architecture, model, and mobile are all set above, so
    // this is `Some` in practice. `None` (unreachable) simply skips metadata.
    (nav_platform.to_string(), builder.build().ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test harness")]
mod download_tests {
    use std::{fs, path::Path};

    use super::{SETTLE_SIGHTINGS, SettleTracker, dir_entries, new_complete_files};

    fn touch(dir: &Path, name: &str, bytes: usize) {
        fs::write(dir.join(name), vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn new_complete_files_excludes_before_crdownload_and_empty() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "old.bin", 10);
        let before = dir_entries(d.path());
        touch(d.path(), "new.bin", 10);
        touch(d.path(), "partial.crdownload", 10);
        touch(d.path(), "empty.bin", 0);
        let files = new_complete_files(d.path(), &before);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0.file_name().unwrap(), "new.bin");
    }

    #[test]
    fn settle_requires_stable_samples_then_accepts() {
        let d = tempfile::tempdir().unwrap();
        let before = dir_entries(d.path());
        touch(d.path(), "f.bin", 100);
        let mut s = SettleTracker::new();
        for _ in 0..(SETTLE_SIGHTINGS - 1) {
            assert!(s.poll(d.path(), &before, 1_000).unwrap().is_none());
        }
        assert_eq!(s.poll(d.path(), &before, 1_000).unwrap().unwrap().bytes, 100);
    }

    #[test]
    fn settle_resets_when_size_still_changing() {
        let d = tempfile::tempdir().unwrap();
        let before = dir_entries(d.path());
        touch(d.path(), "f.bin", 10);
        let mut s = SettleTracker::new();
        s.poll(d.path(), &before, 1_000).unwrap();
        touch(d.path(), "f.bin", 20); // still growing → counter resets
        assert!(s.poll(d.path(), &before, 1_000).unwrap().is_none());
    }

    #[test]
    fn settle_rejects_and_deletes_oversize() {
        let d = tempfile::tempdir().unwrap();
        let before = dir_entries(d.path());
        touch(d.path(), "big.bin", 50);
        let mut s = SettleTracker::new();
        let mut last = Ok(None);
        for _ in 0..SETTLE_SIGHTINGS {
            last = s.poll(d.path(), &before, 8);
        }
        assert!(last.is_err());
        assert!(!d.path().join("big.bin").exists(), "oversize file should be deleted");
    }

    #[test]
    fn settle_errors_on_multiple_new_files() {
        let d = tempfile::tempdir().unwrap();
        let before = dir_entries(d.path());
        touch(d.path(), "a.bin", 10);
        touch(d.path(), "b.bin", 10);
        let mut s = SettleTracker::new();
        assert!(s.poll(d.path(), &before, 1_000).is_err());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "test harness")]
mod tests {
    use std::collections::HashSet;

    use super::{client_hints_for_ua, dehead, finalize_endpoints, safe_endpoint};

    #[test]
    fn safe_endpoint_strips_query_and_fragment() {
        assert_eq!(
            safe_endpoint("https://api.example.com/v2/search?token=SECRET&q=ada#frag"),
            Some("https://api.example.com/v2/search".to_string())
        );
        // host + scheme lowercased; bare host, no path.
        assert_eq!(
            safe_endpoint("HTTPS://API.Example.COM"),
            Some("https://api.example.com".to_string())
        );
        // non-default port is kept (it's infra signature, not a secret).
        assert_eq!(
            safe_endpoint("https://api.example.com:8443/v1/quote"),
            Some("https://api.example.com:8443/v1/quote".to_string())
        );
    }

    #[test]
    fn safe_endpoint_drops_userinfo_and_nonhttp_and_local() {
        // userinfo (embedded credentials) removed.
        assert_eq!(
            safe_endpoint("https://alice:hunter2@host.com/p"),
            Some("https://host.com/p".to_string())
        );
        // non-http(s) schemes are never archived.
        assert_eq!(safe_endpoint("ws://host.com/socket"), None);
        assert_eq!(safe_endpoint("data:text/html,hi"), None);
        // loopback / private / link-local hosts (operator environment) dropped.
        assert_eq!(safe_endpoint("http://127.0.0.1:9000/api"), None);
        assert_eq!(safe_endpoint("http://localhost/api"), None);
        assert_eq!(safe_endpoint("http://192.168.1.5/api"), None);
        assert_eq!(safe_endpoint("http://172.16.0.9/api"), None);
        // 172.x outside the private 16-31 band is public — kept.
        assert!(safe_endpoint("http://172.32.0.1/api").is_some());
    }

    #[test]
    fn safe_endpoint_redacts_secret_path_segments() {
        // JWT-like high-entropy blob.
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        assert_eq!(
            safe_endpoint(&format!("https://h.com/reset/{jwt}")),
            Some("https://h.com/reset/:redacted".to_string())
        );
        // email PII segment.
        assert_eq!(
            safe_endpoint("https://h.com/u/ada@example.com/profile"),
            Some("https://h.com/u/:redacted/profile".to_string())
        );
        // long digit run (card/SSN/phone range).
        assert_eq!(
            safe_endpoint("https://h.com/pay/4111111111111111"),
            Some("https://h.com/pay/:redacted".to_string())
        );
        // an ordinary short numeric id is NOT redacted — templatizing is the
        // consumer's job, not the crawler's.
        assert_eq!(
            safe_endpoint("https://h.com/users/123/profile"),
            Some("https://h.com/users/123/profile".to_string())
        );
    }

    #[test]
    fn safe_endpoint_redacts_by_default_holes() {
        // Holes a denylist missed; redact-by-default catches them:
        // a >15-char opaque token (a real key would be kept under the old >=32
        // rule; a low-entropy stand-in here keeps the secret-scanner happy).
        assert_eq!(
            safe_endpoint("https://h.com/v1/keys/tokentokentokentoken"),
            Some("https://h.com/v1/keys/:redacted".to_string())
        );
        // a 16-char all-hex token (2-class, slipped the old digit+alpha gate).
        assert_eq!(
            safe_endpoint("https://h.com/t/a1b2c3d4e5f6a7b8"),
            Some("https://h.com/t/:redacted".to_string())
        );
        // matrix-param session id (`;jsessionid=`) — never handled before.
        assert_eq!(
            safe_endpoint("https://h.com/store;jsessionid=ABC123/cart"),
            Some("https://h.com/:redacted/cart".to_string())
        );
        // a 12-15 char mixed-case+digit token (under the length/digit/hex caps)
        // is still an opaque secret → redacted by the 3-character-class rule.
        assert_eq!(
            safe_endpoint("https://h.com/s/aB3xK9mP2qR5w"),
            Some("https://h.com/s/:redacted".to_string())
        );
        // template words + a version segment survive (the endpoint skeleton);
        // path case is preserved (only the host is lowercased).
        assert_eq!(
            safe_endpoint("https://q1.finance.yahoo.com/v10/finance/quoteSummary/AAPL"),
            Some("https://q1.finance.yahoo.com/v10/finance/quoteSummary/AAPL".to_string())
        );
    }

    #[test]
    fn safe_endpoint_handles_ipv6_and_cgnat_local_hosts() {
        // bracketed IPv6 loopback — `split(':')` would yield "[" and leak it.
        assert_eq!(safe_endpoint("http://[::1]:9000/api"), None);
        assert_eq!(safe_endpoint("http://[fe80::1]/api"), None);
        // CGNAT (RFC-6598) and mDNS .local are operator-network, not the page.
        assert_eq!(safe_endpoint("http://100.64.0.7/api"), None);
        assert_eq!(safe_endpoint("http://printer.local/status"), None);
        // a public IPv6 host is kept (bracket form parsed correctly).
        assert!(safe_endpoint("http://[2606:4700::1111]/cdn-cgi").is_some());
    }

    #[test]
    fn finalize_endpoints_none_when_not_capturing_else_sorted() {
        let mut seen = HashSet::new();
        seen.insert("https://b.com/2".to_string());
        seen.insert("https://a.com/1".to_string());
        assert_eq!(finalize_endpoints(&seen, false), None);
        assert_eq!(
            finalize_endpoints(&seen, true),
            Some(vec!["https://a.com/1".to_string(), "https://b.com/2".to_string()])
        );
        // capturing with nothing seen → Some(empty), distinct from None ("not
        // captured").
        assert_eq!(finalize_endpoints(&HashSet::new(), true), Some(vec![]));
    }

    const LINUX_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";
    const WIN_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";
    const MAC_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";

    #[test]
    fn dehead_strips_headless_token() {
        assert_eq!(
            dehead("Mozilla/5.0 HeadlessChrome/148.0.0.0 Safari"),
            "Mozilla/5.0 Chrome/148.0.0.0 Safari"
        );
        // No Headless token → unchanged.
        assert_eq!(dehead(LINUX_UA), LINUX_UA);
    }

    /// navigator.platform + Sec-CH-UA-Platform must match the UA's OS — the
    /// mismatch (Linux UA + "Win32") was the bug.
    #[test]
    fn platform_matches_ua_os() {
        assert_eq!(client_hints_for_ua(LINUX_UA).0, "Linux x86_64");
        assert_eq!(client_hints_for_ua(WIN_UA).0, "Win32");
        assert_eq!(client_hints_for_ua(MAC_UA).0, "MacIntel");

        let md = client_hints_for_ua(LINUX_UA).1.unwrap();
        assert_eq!(md.platform, "Linux");
        assert!(!md.mobile);
        assert_eq!(md.architecture, "x86");
    }

    /// Client-Hints brands are populated and carry the UA's Chrome major
    /// version (empty brands was the other half of the bug).
    #[test]
    fn brands_carry_chrome_major_version() {
        let md = client_hints_for_ua(LINUX_UA).1.unwrap();
        let brands = md.brands.unwrap();
        assert!(brands.iter().any(|b| b.brand == "Google Chrome" && b.version == "148"));
        assert!(brands.iter().any(|b| b.brand == "Chromium" && b.version == "148"));
        // A GREASE entry is present (3 brands total).
        assert_eq!(brands.len(), 3);
        // fullVersionList carries the full version.
        let full = md.full_version_list.unwrap();
        assert!(full.iter().any(|b| b.brand == "Google Chrome" && b.version == "148.0.0.0"));
    }

    /// A non-Chrome UA yields no brands rather than a wrong/fabricated one,
    /// but still gets a coherent platform.
    #[test]
    fn non_chrome_ua_has_no_brands() {
        let firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:121.0) Gecko/20100101 Firefox/121.0";
        let (nav_platform, md) = client_hints_for_ua(firefox);
        assert_eq!(nav_platform, "Linux x86_64");
        assert!(md.unwrap().brands.is_none());
    }
}
