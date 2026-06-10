//! `PyO3` bindings for `void_crawl_core`.
//!
//! Exposes `PyBrowserSession` and `PyPage` as Python classes with async methods
//! that bridge to Python's asyncio via `pyo3-async-runtimes`.

use std::{
    collections::HashMap,
    convert::Infallible,
    fmt,
    path::Path,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use futures::future;
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict, PyList, PyType},
};
use pyo3_async_runtimes::tokio::future_into_py;
use serde_json::Value;
use tokio::sync::Mutex;
use void_crawl_core::{
    AntibotEvidence, AntibotVerdict, BrowserMode, BrowserPool, BrowserSession, CookieParam,
    DEFAULT_MAX_BYTES, DeleteCookiesParams, DispatchKeyEventType, DispatchMouseEventType,
    DownloadCapture, DownloadOutcome, MouseButton, Page, PageResponse, PoolConfig, PooledTab,
    ProfileHandle, ProfileInfo, ScanConfig, ScanReport, StealthConfig, Verdict, acquire_profile,
    list_profiles, scan_bytes, scan_path,
};

// ── Error conversion ────────────────────────────────────────────────────

pyo3::create_exception!(voidcrawl._ext, VoidCrawlError, PyRuntimeError);
pyo3::create_exception!(voidcrawl._ext, ProfileBusy, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, ProfileLeaseExpired, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, ProfileNotFound, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, CaptchaDetected, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, AntibotChallenge, VoidCrawlError);

#[allow(clippy::needless_pass_by_value)] // used as fn pointer in map_err(to_py_err)
fn to_py_err(e: void_crawl_core::VoidCrawlError) -> PyErr {
    match e {
        void_crawl_core::VoidCrawlError::ProfileBusy { .. } => ProfileBusy::new_err(e.to_string()),
        void_crawl_core::VoidCrawlError::ProfileLeaseExpired { .. } => {
            ProfileLeaseExpired::new_err(e.to_string())
        }
        void_crawl_core::VoidCrawlError::ProfileNotFound { .. } => {
            ProfileNotFound::new_err(e.to_string())
        }
        void_crawl_core::VoidCrawlError::CaptchaDetected { .. } => {
            CaptchaDetected::new_err(e.to_string())
        }
        void_crawl_core::VoidCrawlError::AntibotChallenge { .. } => {
            AntibotChallenge::new_err(e.to_string())
        }
        _ => PyRuntimeError::new_err(e.to_string()),
    }
}

/// Wrapper so `Vec<u8>` converts to Python `bytes` instead of `list[int]`.
struct PyBytesResult(Vec<u8>);

/// Converts `ScreenshotOutput` to bytes-or-str for Python.
struct PyScreenshotOutput(void_crawl_core::ScreenshotOutput);

impl<'py> IntoPyObject<'py> for PyScreenshotOutput {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        match self.0 {
            void_crawl_core::ScreenshotOutput::Bytes(b) => Ok(PyBytes::new(py, &b).into_any()),
            void_crawl_core::ScreenshotOutput::Path(p) => {
                Ok(p.display().to_string().into_pyobject(py)?.into_any())
            }
        }
    }
}

/// Wrapper for direct `serde_json::Value` → Python object conversion.
///
/// Avoids the double-serialization of `val.to_string()` → `PyString`.
struct PyJsonValue(Value);

impl<'py> IntoPyObject<'py> for PyBytesResult {
    type Target = PyBytes;
    type Output = Bound<'py, PyBytes>;
    type Error = Infallible;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        Ok(PyBytes::new(py, &self.0))
    }
}

impl<'py> IntoPyObject<'py> for PyJsonValue {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        json_to_py(py, self.0)
    }
}

/// Convert a [`Value`] directly to a Python object.
fn json_to_py(py: Python<'_>, val: Value) -> PyResult<Bound<'_, PyAny>> {
    match val {
        Value::Null => Ok(py.None().into_bound(py)),
        Value::Bool(b) => Ok(b.into_pyobject(py)?.to_owned().into_any()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into_pyobject(py)?.into_any())
            } else if let Some(f) = n.as_f64() {
                Ok(f.into_pyobject(py)?.into_any())
            } else {
                Ok(py.None().into_bound(py))
            }
        }
        Value::String(s) => Ok(s.into_pyobject(py)?.into_any()),
        Value::Array(arr) => {
            let list = PyList::empty(py);
            for item in arr {
                list.append(json_to_py(py, item)?)?;
            }
            Ok(list.into_any())
        }
        Value::Object(map) => {
            let dict = PyDict::new(py);
            for (k, v) in map {
                dict.set_item(k, json_to_py(py, v)?)?;
            }
            Ok(dict.into_any())
        }
    }
}

// ── AntibotVerdict ──────────────────────────────────────────────────────

/// Python-visible signature-based anti-bot / CDN vendor fingerprint.
///
/// Attributes:
///     vendors (list[str]): Canonical vendor tags detected (sorted).
///     challenged (bool): ``True`` when an active wall/challenge fired (vs.
///         mere CDN presence).
///     `challenge_vendor` (str | None): Vendor whose challenge fired.
///     `corpus_version` (str): Signature corpus the verdict was produced
///         against — record alongside captures for replay-grade provenance.
///     evidence (str): Which tier matched — ``"none"`` / ``"headers"`` /
///         ``"body"``.
// Only ever returned to Python (a getter on `PageResponse`), never accepted as
// an argument — skip the `FromPyObject` derive pyo3 0.28 adds for `Clone` types.
#[pyclass(name = "AntibotVerdict", skip_from_py_object)]
#[derive(Debug, Clone)]
pub struct PyAntibotVerdict {
    #[pyo3(get)]
    pub vendors:          Vec<String>,
    #[pyo3(get)]
    pub challenged:       bool,
    #[pyo3(get)]
    pub challenge_vendor: Option<String>,
    #[pyo3(get)]
    pub corpus_version:   String,
    #[pyo3(get)]
    pub evidence:         String,
}

#[pymethods]
impl PyAntibotVerdict {
    fn __repr__(&self) -> String {
        format!(
            "AntibotVerdict(vendors={:?}, challenged={}, challenge_vendor={:?}, evidence={:?})",
            self.vendors, self.challenged, self.challenge_vendor, self.evidence,
        )
    }
}

impl From<AntibotVerdict> for PyAntibotVerdict {
    fn from(v: AntibotVerdict) -> Self {
        let evidence = match v.evidence {
            AntibotEvidence::None => "none",
            AntibotEvidence::Headers => "headers",
            AntibotEvidence::Body => "body",
        };
        Self {
            vendors:          v.vendors,
            challenged:       v.challenged,
            challenge_vendor: v.challenge_vendor,
            corpus_version:   v.corpus_version.to_string(),
            evidence:         evidence.to_string(),
        }
    }
}

// ── PageResponse ────────────────────────────────────────────────────────

/// Python-visible result of `Page.goto()` / `PooledTab.goto()`.
///
/// Attributes:
///     html (str): Full outer HTML after network idle.
///     url (str): Final URL after any redirects.
///     `status_code` (int | None): HTTP status of the last response, or
///         ``None`` when served from cache / service worker.
///     redirected (bool): ``True`` when at least one HTTP redirect occurred.
///     headers (dict[str, str]): Final Document response headers (lowercased
///         names; last value wins on duplicates).
///     antibot (AntibotVerdict | None): Anti-bot / CDN vendor fingerprint, or
///         ``None`` when no network response was captured.
///     endpoints (list[str] | None): Data-plane network endpoints (XHR + Fetch
///         request URLs) — a sorted, deduplicated set of ``scheme://host/path``
///         with query/fragment/userinfo stripped and secret-like path segments
///         redacted at the source. ``None`` unless ``capture_endpoints=True``
///         was passed to ``goto()``; ``[]`` when requested but none were seen.
///     `endpoints_truncated` (bool): ``True`` when the endpoint set hit its cap
///         and further endpoints were dropped.
///     `endpoint_sanitizer_version` (str | None): Which redaction-rule version
///         produced ``endpoints`` (record it alongside the set for replay-grade
///         provenance). ``None`` iff ``endpoints`` is ``None``.
#[pyclass(name = "PageResponse")]
#[derive(Debug)]
pub struct PyPageResponse {
    #[pyo3(get)]
    pub html: String,
    #[pyo3(get)]
    pub url: String,
    #[pyo3(get)]
    pub status_code: Option<u16>,
    #[pyo3(get)]
    pub redirected: bool,
    #[pyo3(get)]
    pub headers: HashMap<String, String>,
    #[pyo3(get)]
    pub antibot: Option<PyAntibotVerdict>,
    #[pyo3(get)]
    pub endpoints: Option<Vec<String>>,
    #[pyo3(get)]
    pub endpoints_truncated: bool,
    #[pyo3(get)]
    pub endpoint_sanitizer_version: Option<String>,
}

#[pymethods]
impl PyPageResponse {
    fn __repr__(&self) -> String {
        format!(
            "PageResponse(url={:?}, status_code={:?}, redirected={}, html_len={}, endpoints={})",
            self.url,
            self.status_code,
            self.redirected,
            self.html.len(),
            self.endpoints.as_ref().map_or_else(|| "None".to_string(), |e| e.len().to_string()),
        )
    }
}

impl From<PageResponse> for PyPageResponse {
    fn from(r: PageResponse) -> Self {
        Self {
            html: r.html,
            url: r.url,
            status_code: r.status_code,
            redirected: r.redirected,
            headers: r.headers.into_iter().collect(),
            antibot: r.antibot.map(PyAntibotVerdict::from),
            endpoints: r.endpoints,
            endpoints_truncated: r.endpoints_truncated,
            endpoint_sanitizer_version: r.endpoint_sanitizer_version.map(str::to_string),
        }
    }
}

// ── DownloadOutcome ─────────────────────────────────────────────────────

/// Python-visible result of `Page.download()` / `PooledTab.download()`.
///
/// Attributes:
///     path (str): Absolute path to the downloaded file inside the dir.
///     bytes (int): Size of the downloaded file in bytes.
///     `content_type` (str | None): The server's ``Content-Type`` (parameters
///         stripped), or ``None`` if it sent none. Pass this to
///         :func:`scan_file` as ``claimed_mime`` to catch disguised payloads.
#[pyclass(name = "DownloadOutcome")]
#[derive(Debug)]
pub struct PyDownloadOutcome {
    #[pyo3(get)]
    pub path:         String,
    #[pyo3(get)]
    pub bytes:        u64,
    #[pyo3(get)]
    pub content_type: Option<String>,
}

#[pymethods]
impl PyDownloadOutcome {
    fn __repr__(&self) -> String {
        format!(
            "DownloadOutcome(path={:?}, bytes={}, content_type={:?})",
            self.path, self.bytes, self.content_type
        )
    }
}

impl From<DownloadOutcome> for PyDownloadOutcome {
    fn from(o: DownloadOutcome) -> Self {
        Self {
            path:         o.path.display().to_string(),
            bytes:        o.bytes,
            content_type: o.content_type,
        }
    }
}

// ── DownloadCapture ─────────────────────────────────────────────────────

/// Opaque handle for an armed action-triggered download. Created by
/// ``Page.arm_download`` / ``PooledTab.arm_download``; pass to the matching
/// ``wait_download`` after performing the triggering action. Consumed once.
#[pyclass(name = "DownloadCapture")]
#[derive(Debug)]
pub struct PyDownloadCapture {
    inner: StdMutex<Option<DownloadCapture>>,
}

impl PyDownloadCapture {
    fn new(capture: DownloadCapture) -> Self {
        Self { inner: StdMutex::new(Some(capture)) }
    }

    /// Take the capture out, erroring if it was already waited on.
    fn take(&self) -> PyResult<DownloadCapture> {
        self.inner
            .lock()
            .map_err(|_| PyRuntimeError::new_err("download capture lock poisoned"))?
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("download capture already consumed"))
    }
}

// ── ScanReport ──────────────────────────────────────────────────────────

/// Python-visible result of :func:`scan_file` / :func:`scan_bytes`.
///
/// Attributes:
///     verdict (str): ``"clean"`` or ``"flagged"``.
///     `is_clean` (bool): ``True`` iff ``verdict == "clean"``.
///     reason (str | None): Why it was flagged (``None`` when clean).
///     `detected_mime` (str | None): MIME inferred from the file's magic bytes.
///     size (int): Size of the scanned buffer in bytes.
#[pyclass(name = "ScanReport")]
#[derive(Debug)]
pub struct PyScanReport {
    #[pyo3(get)]
    pub verdict:       String,
    #[pyo3(get)]
    pub reason:        Option<String>,
    #[pyo3(get)]
    pub detected_mime: Option<String>,
    #[pyo3(get)]
    pub size:          u64,
}

#[pymethods]
impl PyScanReport {
    #[getter]
    fn is_clean(&self) -> bool {
        self.verdict == "clean"
    }

    fn __repr__(&self) -> String {
        format!(
            "ScanReport(verdict={:?}, reason={:?}, detected_mime={:?}, size={})",
            self.verdict, self.reason, self.detected_mime, self.size
        )
    }
}

impl From<ScanReport> for PyScanReport {
    fn from(r: ScanReport) -> Self {
        let (verdict, reason) = match r.verdict {
            Verdict::Clean => ("clean".to_string(), None),
            Verdict::Flagged { reason } => ("flagged".to_string(), Some(reason)),
        };
        Self { verdict, reason, detected_mime: r.detected_mime, size: r.size }
    }
}

// ── CDP Input enum conversions ──────────────────────────────────────────

fn parse_mouse_event_type(s: &str) -> PyResult<DispatchMouseEventType> {
    match s {
        "mousePressed" => Ok(DispatchMouseEventType::MousePressed),
        "mouseReleased" => Ok(DispatchMouseEventType::MouseReleased),
        "mouseMoved" => Ok(DispatchMouseEventType::MouseMoved),
        "mouseWheel" => Ok(DispatchMouseEventType::MouseWheel),
        other => Err(PyValueError::new_err(format!(
            "unknown mouse event type: {other:?} \
             (expected mousePressed, mouseReleased, mouseMoved, or mouseWheel)"
        ))),
    }
}

fn parse_mouse_button(s: &str) -> PyResult<MouseButton> {
    match s {
        "none" => Ok(MouseButton::None),
        "left" => Ok(MouseButton::Left),
        "middle" => Ok(MouseButton::Middle),
        "right" => Ok(MouseButton::Right),
        "back" => Ok(MouseButton::Back),
        "forward" => Ok(MouseButton::Forward),
        other => Err(PyValueError::new_err(format!(
            "unknown mouse button: {other:?} \
             (expected none, left, middle, right, back, or forward)"
        ))),
    }
}

fn parse_key_event_type(s: &str) -> PyResult<DispatchKeyEventType> {
    match s {
        "keyDown" => Ok(DispatchKeyEventType::KeyDown),
        "keyUp" => Ok(DispatchKeyEventType::KeyUp),
        "rawKeyDown" => Ok(DispatchKeyEventType::RawKeyDown),
        "char" => Ok(DispatchKeyEventType::Char),
        other => Err(PyValueError::new_err(format!(
            "unknown key event type: {other:?} \
             (expected keyDown, keyUp, rawKeyDown, or char)"
        ))),
    }
}

// ── Shared launch logic ─────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn do_launch(
    inner: Arc<Mutex<Option<BrowserSession>>>,
    mode: BrowserMode,
    stealth_enabled: bool,
    no_sandbox: bool,
    proxy: Option<String>,
    chrome_executable: Option<String>,
    extra_args: Vec<String>,
    user_data_dir: Option<String>,
) -> PyResult<()> {
    let stealth =
        if stealth_enabled { StealthConfig::chrome_like() } else { StealthConfig::none() };

    let mut builder = BrowserSession::builder().mode(mode).stealth(stealth);

    if no_sandbox {
        builder = builder.no_sandbox();
    }
    if let Some(p) = proxy {
        builder = builder.proxy(p);
    }
    if let Some(exe) = chrome_executable {
        builder = builder.chrome_executable(exe);
    }
    if let Some(dir) = user_data_dir {
        builder = builder.user_data_dir(dir);
    }
    for arg in extra_args {
        builder = builder.arg(arg);
    }

    let session = builder.launch().await.map_err(to_py_err)?;
    let mut guard = inner.lock().await;
    *guard = Some(session);
    Ok(())
}

// ── PyPage ──────────────────────────────────────────────────────────────

/// A browser page / tab.
///
/// All navigation and DOM methods are async — await them from Python.
#[pyclass(name = "Page")]
pub struct PyPage {
    inner: Arc<Mutex<Option<Page>>>,
}

impl fmt::Debug for PyPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PyPage").finish_non_exhaustive()
    }
}

impl PyPage {
    fn new(page: Page) -> Self {
        Self { inner: Arc::new(Mutex::new(Some(page))) }
    }
}

/// Run an async op on the inner page using take-work-replace pattern.
///
/// The Mutex is held only for microseconds (take/replace), NOT during
/// the async CDP operation itself. This eliminates lock contention.
///
/// The page is always restored after the operation completes — even on
/// error — so a failed CDP call never permanently empties the slot.
///
/// **Cancellation safety**: If the Python future is cancelled (e.g. by
/// `asyncio.wait_for` timeout) between the `take()` and `replace()`,
/// the page is permanently lost.  This is inherent to the
/// `future_into_py` model — there is no async `Drop` — and is
/// acceptable because a cancelled CDP operation leaves the page in an
/// indeterminate state anyway.
macro_rules! with_page {
    ($self:expr, $py:expr, |$page:ident| $body:expr) => {{
        let inner = Arc::clone(&$self.inner);
        future_into_py($py, async move {
            let page = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let result = {
                let $page = &page;
                $body.await.map_err(to_py_err)
            };
            inner.lock().await.replace(page);
            result
        })
    }};
}

/// Variant of `with_page!` that allows a custom transformation on the
/// result before returning.  The page is always restored.
macro_rules! with_page_map {
    ($self:expr, $py:expr, |$page:ident| $body:expr, |$res:ident| $map:expr) => {{
        let inner = Arc::clone(&$self.inner);
        future_into_py($py, async move {
            let page = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let result = {
                let $page = &page;
                $body.await.map_err(to_py_err)
            };
            inner.lock().await.replace(page);
            let $res = result?;
            Ok($map)
        })
    }};
}

#[pymethods]
impl PyPage {
    /// Navigate to a URL.
    fn navigate<'py>(&self, py: Python<'py>, url: String) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.navigate(&url))
    }

    /// Navigate and wait for network idle, returning a :class:`PageResponse`.
    ///
    /// Faster than calling `navigate()` then `wait_for_network_idle()`
    /// separately because the event listener is set up before navigation
    /// starts, so early networkIdle events are never missed.
    ///
    /// Returns:
    ///     `PageResponse`: HTML, final URL, HTTP status code, and redirect
    /// flag.
    #[pyo3(signature = (url, timeout=30.0, capture_endpoints=false))]
    fn goto<'py>(
        &self,
        py: Python<'py>,
        url: String,
        timeout: f64,
        capture_endpoints: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(
            self,
            py,
            |page| page.goto_and_wait_for_idle_with_capture(
                &url,
                Duration::from_secs_f64(timeout),
                capture_endpoints
            ),
            |resp| PyPageResponse::from(resp)
        )
    }

    /// Wait for the current navigation to complete.
    fn wait_for_navigation<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.wait_for_navigation())
    }

    /// Get the full HTML content of the page.
    fn content<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.content())
    }

    /// Get the page title.
    fn title<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.title())
    }

    /// Get the current URL.
    fn url<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.url())
    }

    /// Evaluate a JavaScript expression and return the result as a native
    /// Python object.
    ///
    /// JSON objects → dict, arrays → list, strings → str, numbers → int/float,
    /// etc.
    fn evaluate_js<'py>(&self, py: Python<'py>, expression: String) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.evaluate_js(&expression), |val| PyJsonValue(val))
    }

    /// Alias for :meth:`evaluate_js` — short form used by MCP tooling and
    /// internal Yosoi callers.
    fn eval_js<'py>(&self, py: Python<'py>, expression: String) -> PyResult<Bound<'py, PyAny>> {
        self.evaluate_js(py, expression)
    }

    /// Evaluate a JavaScript expression **inside a specific (possibly
    /// cross-origin) iframe**, selected by a substring of its URL.
    ///
    /// The expression runs in that frame's own execution context, so
    /// ``document`` is the frame's document. This is the only way to read or
    /// drive an iframe whose ``contentDocument`` is ``null`` from the parent
    /// under the same-origin policy (e.g. a reCAPTCHA ``bframe`` on a real
    /// third-party site).
    ///
    /// Args:
    ///     frame_url_pattern: Substring of the target frame's URL, e.g.
    ///         ``"recaptcha/api2/bframe"``.
    ///     expression: JavaScript expression; its value is returned as a
    ///         native Python object (JSON objects → dict, arrays → list, …).
    ///
    /// Raises:
    ///     RuntimeError: if no frame matches, or the matched frame has no
    ///         scriptable execution context.
    fn evaluate_js_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        expression: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(
            self,
            py,
            |page| page.evaluate_js_in_frame(&frame_url_pattern, &expression),
            |val| PyJsonValue(val)
        )
    }

    /// Alias for :meth:`evaluate_js_in_frame` — short form matching the
    /// ``eval_js`` / MCP naming.
    fn eval_js_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        expression: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.evaluate_js_in_frame(py, frame_url_pattern, expression)
    }

    /// List the URLs of every frame on the page — handy for discovering the
    /// right `frame_url_pattern` for :meth:`evaluate_js_in_frame`.
    fn frame_urls<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.frame_urls(), |urls| PyJsonValue(
            serde_json::Value::from(urls)
        ))
    }

    /// Take a PNG screenshot, returned as Python bytes.
    fn screenshot_png<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.screenshot_png(), |bytes| PyBytesResult(bytes))
    }

    /// Take a PNG screenshot with optional disk output and/or cropping.
    ///
    /// Args:
    ///     path: If set, writes PNG to this path and returns the path as a
    ///         string. If omitted, returns raw bytes.
    ///     bbox: Optional ``(x, y, width, height)`` in CSS pixels to crop.
    #[pyo3(signature = (path=None, bbox=None))]
    fn screenshot<'py>(
        &self,
        py: Python<'py>,
        path: Option<String>,
        bbox: Option<(u32, u32, u32, u32)>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let mut opts = void_crawl_core::ScreenshotOptions::default();
            if let Some(p) = path {
                opts = opts.with_path(p);
            }
            if let Some((x, y, w, h)) = bbox {
                opts = opts.with_bbox(void_crawl_core::Bbox { x, y, width: w, height: h });
            }
            let result = page.screenshot(opts).await.map_err(to_py_err);
            inner.lock().await.replace(page);
            Ok(PyScreenshotOutput(result?))
        })
    }

    /// Probe DOM for captcha / bot-wall markers. Returns the kind tag
    /// (``"recaptcha"``, ``"hcaptcha"``, ``"turnstile"``,
    /// ``"cloudflare_challenge"``, ``"datadome"``) or ``None``.
    fn detect_captcha<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let result = void_crawl_core::detect_captcha(&page).await.map_err(to_py_err);
            inner.lock().await.replace(page);
            Ok(result?.map(|k| k.as_str().to_string()))
        })
    }

    /// Generate a PDF, returned as Python bytes.
    fn pdf_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.pdf_bytes(), |bytes| PyBytesResult(bytes))
    }

    /// Download the resource at ``url`` into directory ``dir`` through this
    /// page's browser context (cookies / fingerprint preserved), returning a
    /// :class:`DownloadOutcome`.
    ///
    /// The stream aborts past ``max_bytes`` so a hostile server can't exhaust
    /// the tab. ``dir`` should be a fresh directory you treat as quarantine and
    /// pass to :func:`scan_file` before trusting the file. The CDP download
    /// behavior is reset before this returns.
    ///
    /// Args:
    ///     url: Absolute URL of the file to download.
    ///     dir: Directory the file is saved into.
    ///     timeout: Download timeout in seconds (default 120).
    ///     `max_bytes`: Abort past this many bytes (default 100 MiB).
    #[pyo3(signature = (url, dir, timeout=120.0, max_bytes=None))]
    fn download<'py>(
        &self,
        py: Python<'py>,
        url: String,
        dir: String,
        timeout: f64,
        max_bytes: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let max = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
            let result = page
                .download_to_dir(&url, Path::new(&dir), Duration::from_secs_f64(timeout), max)
                .await
                .map_err(to_py_err);
            inner.lock().await.replace(page);
            Ok(PyDownloadOutcome::from(result?))
        })
    }

    /// Arm an **action-triggered** download capture into *dir*, returning a
    /// :class:`DownloadCapture`. Perform the triggering action next (e.g.
    /// :meth:`click_by_role`), then pass the capture to :meth:`wait_download`.
    /// Use for downloads started by a page action — a "Download" button, a
    /// generated/cross-origin URL (Google Drive) — rather than
    /// :meth:`download`, which needs a URL in hand. The convenience wrapper
    /// :func:`voidcrawl.capture_download` brackets these as a context manager.
    #[pyo3(signature = (dir, max_bytes=None))]
    fn arm_download<'py>(
        &self,
        py: Python<'py>,
        dir: String,
        max_bytes: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let max = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
            let result = page.arm_download(Path::new(&dir), max).await.map_err(to_py_err);
            inner.lock().await.replace(page);
            Ok(PyDownloadCapture::new(result?))
        })
    }

    /// Wait for the armed *capture* to land a new download, returning a
    /// :class:`DownloadOutcome`. Resets the page's download behavior. The
    /// capture is consumed — a second wait errors.
    #[pyo3(signature = (capture, timeout=120.0))]
    fn wait_download<'py>(
        &self,
        py: Python<'py>,
        capture: &PyDownloadCapture,
        timeout: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        let cap = capture.take()?;
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let result = cap.wait(&page, Duration::from_secs_f64(timeout)).await.map_err(to_py_err);
            inner.lock().await.replace(page);
            Ok(PyDownloadOutcome::from(result?))
        })
    }

    /// Reset this page's CDP download behavior to Chrome's default. Call to
    /// release an armed-but-unused capture (e.g. on an error path).
    fn reset_download<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            page.reset_download_behavior().await;
            inner.lock().await.replace(page);
            Ok(())
        })
    }

    /// Fetch the browser-computed accessibility (AX) tree.
    ///
    /// Returns a flat list of AX node dicts (`Accessibility.getFullAXTree`):
    /// each has ``role``, computed ``name``, ``properties``, ``childIds`` and
    /// ``backendDOMNodeId``. Call after the page has rendered. ``depth``
    /// bounds descendant traversal (``None`` = full tree).
    #[pyo3(signature = (depth=None))]
    fn get_full_ax_tree<'py>(
        &self,
        py: Python<'py>,
        depth: Option<i64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.get_full_ax_tree(depth), |val| PyJsonValue(val))
    }

    /// Fetch the AX tree as a compact, indented ``role "name"`` outline string
    /// (text-noise and hidden nodes pruned) — the readable counterpart to
    /// ``get_full_ax_tree``.
    #[pyo3(signature = (depth=None))]
    fn ax_tree_outline<'py>(
        &self,
        py: Python<'py>,
        depth: Option<i64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.ax_tree_outline(depth))
    }

    /// Query the AX tree for nodes matching ``role`` and/or accessible
    /// ``name`` (`Accessibility.queryAXTree`). Returns a list of node dicts.
    #[pyo3(signature = (role=None, name=None))]
    fn query_ax_tree<'py>(
        &self,
        py: Python<'py>,
        role: Option<String>,
        name: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(
            self,
            py,
            |page| page.query_ax_tree(role.as_deref(), name.as_deref()),
            |val| PyJsonValue(val)
        )
    }

    /// Click the ``nth`` element (0-based) matching accessibility ``role`` and
    /// accessible ``name`` — the markup-independent analogue of
    /// ``click_element``. Raises if no such node exists.
    #[pyo3(signature = (role, name, nth=0))]
    fn click_by_role<'py>(
        &self,
        py: Python<'py>,
        role: String,
        name: String,
        nth: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.click_by_role(&role, &name, nth))
    }

    /// Override geolocation (and grant the permission). `accuracy` defaults
    /// to 50 metres. `navigator.geolocation` reads require a secure context.
    #[pyo3(signature = (latitude, longitude, accuracy=None))]
    fn set_geolocation<'py>(
        &self,
        py: Python<'py>,
        latitude: f64,
        longitude: f64,
        accuracy: Option<f64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.set_geolocation(latitude, longitude, accuracy))
    }

    /// Override the locale (Intl + Accept-Language), e.g. "en-US", "fr-FR".
    fn set_locale<'py>(&self, py: Python<'py>, locale: String) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.set_locale(&locale))
    }

    /// Override the timezone by IANA id, e.g. `America/New_York`.
    fn set_timezone<'py>(
        &self,
        py: Python<'py>,
        timezone_id: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.set_timezone(&timezone_id))
    }

    /// Query for an element by CSS selector, return its inner HTML or None.
    fn query_selector<'py>(
        &self,
        py: Python<'py>,
        selector: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.query_selector(&selector))
    }

    /// Query for all matching elements, return list of inner HTML strings.
    fn query_selector_all<'py>(
        &self,
        py: Python<'py>,
        selector: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.query_selector_all(&selector))
    }

    /// Click on the first element matching a CSS selector.
    fn click_element<'py>(&self, py: Python<'py>, selector: String) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.click_element(&selector))
    }

    /// Type text into the first element matching a CSS selector.
    fn type_into<'py>(
        &self,
        py: Python<'py>,
        selector: String,
        text: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.type_into(&selector, &text))
    }

    /// Set extra HTTP headers for all subsequent requests.
    fn set_headers<'py>(
        &self,
        py: Python<'py>,
        headers: HashMap<String, String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.set_headers(headers))
    }

    /// Return all cookies matching the current page URL.
    ///
    /// Each cookie is a dict with keys: name, value, domain, path, expires,
    /// size, httpOnly, secure, session, sameSite, priority, etc.
    fn get_cookies<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.get_cookies(), |cookies| {
            let val = serde_json::to_value(&cookies)
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            PyJsonValue(val)
        })
    }

    /// Set a cookie on the current page.
    #[pyo3(signature = (name, value, *, domain=None, path=None, secure=None, http_only=None))]
    #[allow(clippy::too_many_arguments)]
    fn set_cookie<'py>(
        &self,
        py: Python<'py>,
        name: String,
        value: String,
        domain: Option<String>,
        path: Option<String>,
        secure: Option<bool>,
        http_only: Option<bool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut cookie = CookieParam::new(name, value);
        cookie.domain = domain;
        cookie.path = path;
        cookie.secure = secure;
        cookie.http_only = http_only;
        with_page!(self, py, |page| page.set_cookie(cookie))
    }

    /// Delete a cookie by name, optionally scoped to a domain and path.
    #[pyo3(signature = (name, *, domain=None, path=None))]
    fn delete_cookie<'py>(
        &self,
        py: Python<'py>,
        name: String,
        domain: Option<String>,
        path: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut params = DeleteCookiesParams::new(name);
        params.domain = domain;
        params.path = path;
        with_page!(self, py, |page| page.delete_cookies(vec![params]))
    }

    /// Event-driven wait for network idle. No polling.
    ///
    /// Returns the lifecycle event name ("networkIdle" or "networkAlmostIdle")
    /// or None if the timeout was reached.
    #[pyo3(signature = (timeout=30.0))]
    fn wait_for_network_idle<'py>(
        &self,
        py: Python<'py>,
        timeout: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.wait_for_network_idle(Duration::from_secs_f64(timeout)))
    }

    /// Wait until a CSS selector matches via an in-page MutationObserver.
    /// Event-driven — no polling. Returns None on match, raises Timeout
    /// if `timeout` seconds pass without a match.
    #[pyo3(signature = (selector, timeout=30.0))]
    fn wait_for_selector<'py>(
        &self,
        py: Python<'py>,
        selector: String,
        timeout: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page
            .wait_for_selector(&selector, Duration::from_secs_f64(timeout)))
    }

    /// Dispatch a mouse event via the CDP Input.dispatchMouseEvent command.
    #[pyo3(signature = (event_type, x, y, button="left", click_count=1, delta_x=None, delta_y=None, modifiers=None))]
    #[allow(clippy::too_many_arguments)]
    fn dispatch_mouse_event<'py>(
        &self,
        py: Python<'py>,
        event_type: &str,
        x: f64,
        y: f64,
        button: &str,
        click_count: i64,
        delta_x: Option<f64>,
        delta_y: Option<f64>,
        modifiers: Option<i64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let evt = parse_mouse_event_type(event_type)?;
        let btn = parse_mouse_button(button)?;
        with_page!(self, py, |page| page.dispatch_mouse_event(
            evt,
            x,
            y,
            Some(btn),
            Some(click_count),
            delta_x,
            delta_y,
            modifiers,
        ))
    }

    /// Dispatch a key event via the CDP Input.dispatchKeyEvent command.
    #[pyo3(signature = (event_type, key=None, code=None, text=None, modifiers=None))]
    fn dispatch_key_event<'py>(
        &self,
        py: Python<'py>,
        event_type: &str,
        key: Option<String>,
        code: Option<String>,
        text: Option<String>,
        modifiers: Option<i64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let evt = parse_key_event_type(event_type)?;
        with_page!(self, py, |page| page.dispatch_key_event(
            evt,
            key.as_deref(),
            code.as_deref(),
            text.as_deref(),
            modifiers,
        ))
    }

    /// Close this page / tab.
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let mut guard = inner.lock().await;
            if let Some(page) = guard.take() {
                page.close().await.map_err(to_py_err)?;
            }
            Ok(())
        })
    }
}

// ── PyBrowserSession ────────────────────────────────────────────────────

/// Browser session that wraps a Chromium instance via CDP.
///
/// Supports async context manager protocol (`async with`).
///
/// # Example
///
///
///     async with BrowserSession() as browser:
///         page = await browser.new_page("https://example.com")
///         html = await page.content()
#[pyclass(name = "BrowserSession")]
pub struct PyBrowserSession {
    inner:             Arc<Mutex<Option<BrowserSession>>>,
    mode:              BrowserMode,
    stealth_enabled:   bool,
    no_sandbox:        bool,
    proxy:             Option<String>,
    chrome_executable: Option<String>,
    extra_args:        Vec<String>,
    user_data_dir:     Option<String>,
}

impl fmt::Debug for PyBrowserSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PyBrowserSession").field("mode", &self.mode).finish_non_exhaustive()
    }
}

#[pymethods]
impl PyBrowserSession {
    /// Create a new browser session.
    ///
    /// Args:
    ///     headless: Run in headless mode (default True).
    ///     `ws_url`: Connect to existing browser via WebSocket URL.
    ///     stealth: Enable anti-detection (default True).
    ///     `no_sandbox`: Disable Chrome sandbox (default False).
    ///     proxy: Proxy server URL.
    ///     `chrome_executable`: Path to Chrome/Chromium binary.
    ///     `extra_args`: Additional Chrome command-line arguments.
    ///     `user_data_dir`: Persistent Chrome user data directory.
    #[new]
    #[pyo3(signature = (*, headless=true, ws_url=None, stealth=true, no_sandbox=false, proxy=None, chrome_executable=None, extra_args=None, user_data_dir=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        headless: bool,
        ws_url: Option<String>,
        stealth: bool,
        no_sandbox: bool,
        proxy: Option<String>,
        chrome_executable: Option<String>,
        extra_args: Option<Vec<String>>,
        user_data_dir: Option<String>,
    ) -> Self {
        let mode = if let Some(url) = ws_url {
            BrowserMode::RemoteDebug { ws_url: url }
        } else if headless {
            BrowserMode::Headless
        } else {
            BrowserMode::Headful
        };

        Self {
            inner: Arc::new(Mutex::new(None)),
            mode,
            stealth_enabled: stealth,
            no_sandbox,
            proxy,
            chrome_executable,
            extra_args: extra_args.unwrap_or_default(),
            user_data_dir,
        }
    }

    /// Launch (or connect to) the browser. Called automatically by
    /// `__aenter__`.
    fn launch<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        let mode = self.mode.clone();
        let stealth_enabled = self.stealth_enabled;
        let no_sandbox = self.no_sandbox;
        let proxy = self.proxy.clone();
        let chrome_executable = self.chrome_executable.clone();
        let extra_args = self.extra_args.clone();
        let user_data_dir = self.user_data_dir.clone();

        future_into_py(py, async move {
            do_launch(
                inner,
                mode,
                stealth_enabled,
                no_sandbox,
                proxy,
                chrome_executable,
                extra_args,
                user_data_dir,
            )
            .await
        })
    }

    /// Open a new page and navigate to the URL.
    ///
    /// **Cancellation safety**: if the Python future is cancelled (e.g. by
    /// `asyncio.wait_for`) while the tab is opening, the browser session is
    /// permanently lost — subsequent calls will raise "browser not launched".
    /// This matches the `with_page!` contract: a cancelled CDP operation
    /// leaves the browser in an indeterminate state.
    fn new_page<'py>(&self, py: Python<'py>, url: String) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner.lock().await.take().ok_or_else(|| {
                PyRuntimeError::new_err(
                    "browser not launched — use `async with` or call launch() first",
                )
            })?;
            let page_result = session.new_page(&url).await.map_err(to_py_err);
            inner.lock().await.replace(session);
            Ok(PyPage::new(page_result?))
        })
    }

    /// Get browser version string.
    fn version<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("browser not launched"))?;
            let result = session.version().await.map_err(to_py_err);
            inner.lock().await.replace(session);
            result
        })
    }

    /// Close the browser.
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let mut guard = inner.lock().await;
            if let Some(session) = guard.take() {
                session.close().await.map_err(to_py_err)?;
            }
            Ok(())
        })
    }

    // ── async context manager ───────────────────────────────────────────

    fn __aenter__<'py>(slf: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (
            inner,
            mode,
            stealth_enabled,
            no_sandbox,
            proxy,
            chrome_executable,
            extra_args,
            user_data_dir,
        ) = {
            let this = slf.borrow();
            (
                Arc::clone(&this.inner),
                this.mode.clone(),
                this.stealth_enabled,
                this.no_sandbox,
                this.proxy.clone(),
                this.chrome_executable.clone(),
                this.extra_args.clone(),
                this.user_data_dir.clone(),
            )
        };
        let slf_ref = slf.into_any().unbind();

        future_into_py(py, async move {
            do_launch(
                inner,
                mode,
                stealth_enabled,
                no_sandbox,
                proxy,
                chrome_executable,
                extra_args,
                user_data_dir,
            )
            .await?;
            Ok(slf_ref)
        })
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        _exc_type: Option<Bound<'py, PyAny>>,
        _exc_val: Option<Bound<'py, PyAny>>,
        _exc_tb: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let mut guard = inner.lock().await;
            if let Some(session) = guard.take() {
                let _ = session.close().await;
            }
            Ok(false) // don't suppress exceptions
        })
    }

    fn __repr__(&self) -> String {
        let mode = match &self.mode {
            BrowserMode::Headless => "headless",
            BrowserMode::Headful => "headful",
            BrowserMode::RemoteDebug { ws_url } => ws_url,
        };
        format!("BrowserSession(mode={mode})")
    }
}

// ── PyPooledTab ────────────────────────────────────────────────────────

/// A tab checked out from a [`BrowserPool`].
///
/// Exposes the same navigation / DOM methods as [`Page`]. Obtained via the
/// `async with pool.acquire() as tab:` pattern — release back to the pool
/// is handled automatically by the context manager.
#[pyclass(name = "PooledTab")]
pub struct PyPooledTab {
    inner:     Arc<Mutex<Option<PooledTab>>>,
    /// Snapshot of `use_count` at the moment the tab was acquired.
    #[pyo3(get)]
    use_count: u32,
}

impl fmt::Debug for PyPooledTab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledTab").field("use_count", &self.use_count).finish_non_exhaustive()
    }
}

/// Helper macro: run an async op on the page inside the pooled tab.
/// Uses take-work-replace to minimize lock hold time. The tab is always
/// restored after the operation completes.
macro_rules! with_pooled_page {
    ($self:expr, $py:expr, |$page:ident| $body:expr) => {{
        let inner = Arc::clone(&$self.inner);
        future_into_py($py, async move {
            let tab = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("tab has been released"))?;
            let result = {
                let $page = &tab.page;
                $body.await.map_err(to_py_err)
            };
            inner.lock().await.replace(tab);
            result
        })
    }};
}

/// Variant of `with_pooled_page!` with a custom result transformation.
macro_rules! with_pooled_page_map {
    ($self:expr, $py:expr, |$page:ident| $body:expr, |$res:ident| $map:expr) => {{
        let inner = Arc::clone(&$self.inner);
        future_into_py($py, async move {
            let tab = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("tab has been released"))?;
            let result = {
                let $page = &tab.page;
                $body.await.map_err(to_py_err)
            };
            inner.lock().await.replace(tab);
            let $res = result?;
            Ok($map)
        })
    }};
}

#[pymethods]
impl PyPooledTab {
    fn navigate<'py>(&self, py: Python<'py>, url: String) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.navigate(&url))
    }

    /// Navigate and wait for network idle in one shot.
    ///
    /// Faster than calling `navigate()` then `wait_for_network_idle()`
    /// separately because the event listener is set up before navigation
    /// starts.
    #[pyo3(signature = (url, timeout=30.0, capture_endpoints=false))]
    fn goto<'py>(
        &self,
        py: Python<'py>,
        url: String,
        timeout: f64,
        capture_endpoints: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(
            self,
            py,
            |page| page.goto_and_wait_for_idle_with_capture(
                &url,
                Duration::from_secs_f64(timeout),
                capture_endpoints
            ),
            |resp| PyPageResponse::from(resp)
        )
    }

    fn wait_for_navigation<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.wait_for_navigation())
    }

    fn content<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.content())
    }

    fn title<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.title())
    }

    fn url<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.url())
    }

    fn evaluate_js<'py>(&self, py: Python<'py>, expression: String) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(self, py, |page| page.evaluate_js(&expression), |val| PyJsonValue(
            val
        ))
    }

    /// Alias for :meth:`evaluate_js`.
    fn eval_js<'py>(&self, py: Python<'py>, expression: String) -> PyResult<Bound<'py, PyAny>> {
        self.evaluate_js(py, expression)
    }

    /// Evaluate JavaScript inside a specific (possibly cross-origin) iframe,
    /// selected by a substring of its URL. See
    /// :meth:`Page.evaluate_js_in_frame`.
    fn evaluate_js_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        expression: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(
            self,
            py,
            |page| page.evaluate_js_in_frame(&frame_url_pattern, &expression),
            |val| PyJsonValue(val)
        )
    }

    /// Alias for :meth:`evaluate_js_in_frame`.
    fn eval_js_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        expression: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.evaluate_js_in_frame(py, frame_url_pattern, expression)
    }

    /// List the URLs of every frame on the page — handy for discovering the
    /// right `frame_url_pattern` for :meth:`evaluate_js_in_frame`.
    fn frame_urls<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(self, py, |page| page.frame_urls(), |urls| PyJsonValue(
            serde_json::Value::from(urls)
        ))
    }

    fn screenshot_png<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(self, py, |page| page.screenshot_png(), |bytes| PyBytesResult(bytes))
    }

    /// Download the resource at ``url`` into directory ``dir`` over this pooled
    /// tab, returning a :class:`DownloadOutcome`. See :meth:`Page.download`.
    #[pyo3(signature = (url, dir, timeout=120.0, max_bytes=None))]
    fn download<'py>(
        &self,
        py: Python<'py>,
        url: String,
        dir: String,
        timeout: f64,
        max_bytes: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let tab = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("tab has been released"))?;
            let max = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
            let result = tab
                .page
                .download_to_dir(&url, Path::new(&dir), Duration::from_secs_f64(timeout), max)
                .await
                .map_err(to_py_err);
            inner.lock().await.replace(tab);
            Ok(PyDownloadOutcome::from(result?))
        })
    }

    /// Arm an action-triggered download capture into *dir*; see
    /// :meth:`Page.arm_download`.
    #[pyo3(signature = (dir, max_bytes=None))]
    fn arm_download<'py>(
        &self,
        py: Python<'py>,
        dir: String,
        max_bytes: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let tab = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("tab has been released"))?;
            let max = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
            let result = tab.page.arm_download(Path::new(&dir), max).await.map_err(to_py_err);
            inner.lock().await.replace(tab);
            Ok(PyDownloadCapture::new(result?))
        })
    }

    /// Wait for the armed *capture* to land a new download; see
    /// :meth:`Page.wait_download`.
    #[pyo3(signature = (capture, timeout=120.0))]
    fn wait_download<'py>(
        &self,
        py: Python<'py>,
        capture: &PyDownloadCapture,
        timeout: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        let cap = capture.take()?;
        future_into_py(py, async move {
            let tab = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("tab has been released"))?;
            let result =
                cap.wait(&tab.page, Duration::from_secs_f64(timeout)).await.map_err(to_py_err);
            inner.lock().await.replace(tab);
            Ok(PyDownloadOutcome::from(result?))
        })
    }

    /// Reset this tab's CDP download behavior to Chrome's default; see
    /// :meth:`Page.reset_download`.
    fn reset_download<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let tab = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("tab has been released"))?;
            tab.page.reset_download_behavior().await;
            inner.lock().await.replace(tab);
            Ok(())
        })
    }

    /// Fetch the browser-computed accessibility (AX) tree.
    ///
    /// Returns a flat list of AX node dicts (`Accessibility.getFullAXTree`):
    /// each has ``role``, computed ``name``, ``properties``, ``childIds`` and
    /// ``backendDOMNodeId``. Call after the page has rendered. ``depth``
    /// bounds descendant traversal (``None`` = full tree).
    #[pyo3(signature = (depth=None))]
    fn get_full_ax_tree<'py>(
        &self,
        py: Python<'py>,
        depth: Option<i64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(self, py, |page| page.get_full_ax_tree(depth), |val| PyJsonValue(val))
    }

    /// Fetch the AX tree as a compact, indented ``role "name"`` outline string
    /// (text-noise and hidden nodes pruned) — the readable counterpart to
    /// ``get_full_ax_tree``.
    #[pyo3(signature = (depth=None))]
    fn ax_tree_outline<'py>(
        &self,
        py: Python<'py>,
        depth: Option<i64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.ax_tree_outline(depth))
    }

    /// Query the AX tree for nodes matching ``role`` and/or accessible
    /// ``name`` (`Accessibility.queryAXTree`). Returns a list of node dicts.
    #[pyo3(signature = (role=None, name=None))]
    fn query_ax_tree<'py>(
        &self,
        py: Python<'py>,
        role: Option<String>,
        name: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(
            self,
            py,
            |page| page.query_ax_tree(role.as_deref(), name.as_deref()),
            |val| PyJsonValue(val)
        )
    }

    /// Click the ``nth`` element (0-based) matching accessibility ``role`` and
    /// accessible ``name`` — the markup-independent analogue of
    /// ``click_element``. Raises if no such node exists.
    #[pyo3(signature = (role, name, nth=0))]
    fn click_by_role<'py>(
        &self,
        py: Python<'py>,
        role: String,
        name: String,
        nth: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.click_by_role(&role, &name, nth))
    }

    /// Override geolocation (and grant the permission). `accuracy` defaults
    /// to 50 metres. `navigator.geolocation` reads require a secure context.
    #[pyo3(signature = (latitude, longitude, accuracy=None))]
    fn set_geolocation<'py>(
        &self,
        py: Python<'py>,
        latitude: f64,
        longitude: f64,
        accuracy: Option<f64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.set_geolocation(latitude, longitude, accuracy))
    }

    /// Override the locale (Intl + Accept-Language), e.g. "en-US", "fr-FR".
    fn set_locale<'py>(&self, py: Python<'py>, locale: String) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.set_locale(&locale))
    }

    /// Override the timezone by IANA id, e.g. `America/New_York`.
    fn set_timezone<'py>(
        &self,
        py: Python<'py>,
        timezone_id: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.set_timezone(&timezone_id))
    }

    fn query_selector<'py>(
        &self,
        py: Python<'py>,
        selector: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.query_selector(&selector))
    }

    fn query_selector_all<'py>(
        &self,
        py: Python<'py>,
        selector: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.query_selector_all(&selector))
    }

    fn click_element<'py>(&self, py: Python<'py>, selector: String) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.click_element(&selector))
    }

    fn type_into<'py>(
        &self,
        py: Python<'py>,
        selector: String,
        text: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.type_into(&selector, &text))
    }

    fn set_headers<'py>(
        &self,
        py: Python<'py>,
        headers: HashMap<String, String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.set_headers(headers))
    }

    /// Return all cookies matching the current page URL.
    fn get_cookies<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(self, py, |page| page.get_cookies(), |cookies| {
            let val = serde_json::to_value(&cookies)
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            PyJsonValue(val)
        })
    }

    /// Set a cookie on the current page.
    #[pyo3(signature = (name, value, *, domain=None, path=None, secure=None, http_only=None))]
    #[allow(clippy::too_many_arguments)]
    fn set_cookie<'py>(
        &self,
        py: Python<'py>,
        name: String,
        value: String,
        domain: Option<String>,
        path: Option<String>,
        secure: Option<bool>,
        http_only: Option<bool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut cookie = CookieParam::new(name, value);
        cookie.domain = domain;
        cookie.path = path;
        cookie.secure = secure;
        cookie.http_only = http_only;
        with_pooled_page!(self, py, |page| page.set_cookie(cookie))
    }

    /// Delete a cookie by name, optionally scoped to a domain and path.
    #[pyo3(signature = (name, *, domain=None, path=None))]
    fn delete_cookie<'py>(
        &self,
        py: Python<'py>,
        name: String,
        domain: Option<String>,
        path: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut params = DeleteCookiesParams::new(name);
        params.domain = domain;
        params.path = path;
        with_pooled_page!(self, py, |page| page.delete_cookies(vec![params]))
    }

    /// Event-driven wait for network idle. No polling.
    ///
    /// Returns the lifecycle event name ("networkIdle" or "networkAlmostIdle")
    /// or None if the timeout was reached.
    #[pyo3(signature = (timeout=30.0))]
    fn wait_for_network_idle<'py>(
        &self,
        py: Python<'py>,
        timeout: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page
            .wait_for_network_idle(Duration::from_secs_f64(timeout)))
    }

    /// Wait until a CSS selector matches via an in-page MutationObserver.
    /// Event-driven — no polling.
    #[pyo3(signature = (selector, timeout=30.0))]
    fn wait_for_selector<'py>(
        &self,
        py: Python<'py>,
        selector: String,
        timeout: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page
            .wait_for_selector(&selector, Duration::from_secs_f64(timeout)))
    }

    /// Dispatch a mouse event via the CDP Input.dispatchMouseEvent command.
    #[pyo3(signature = (event_type, x, y, button="left", click_count=1, delta_x=None, delta_y=None, modifiers=None))]
    #[allow(clippy::too_many_arguments)]
    fn dispatch_mouse_event<'py>(
        &self,
        py: Python<'py>,
        event_type: &str,
        x: f64,
        y: f64,
        button: &str,
        click_count: i64,
        delta_x: Option<f64>,
        delta_y: Option<f64>,
        modifiers: Option<i64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let evt = parse_mouse_event_type(event_type)?;
        let btn = parse_mouse_button(button)?;
        with_pooled_page!(self, py, |page| page.dispatch_mouse_event(
            evt,
            x,
            y,
            Some(btn),
            Some(click_count),
            delta_x,
            delta_y,
            modifiers,
        ))
    }

    /// Dispatch a key event via the CDP Input.dispatchKeyEvent command.
    #[pyo3(signature = (event_type, key=None, code=None, text=None, modifiers=None))]
    fn dispatch_key_event<'py>(
        &self,
        py: Python<'py>,
        event_type: &str,
        key: Option<String>,
        code: Option<String>,
        text: Option<String>,
        modifiers: Option<i64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let evt = parse_key_event_type(event_type)?;
        with_pooled_page!(self, py, |page| page.dispatch_key_event(
            evt,
            key.as_deref(),
            code.as_deref(),
            text.as_deref(),
            modifiers,
        ))
    }

    fn __repr__(&self) -> String {
        format!("PooledTab(use_count={})", self.use_count)
    }
}

// ── PyAcquireContext ────────────────────────────────────────────────────

/// Lazy context manager returned by [`BrowserPool.acquire()`].
///
/// Does the actual tab checkout in `__aenter__` and releases on `__aexit__`.
///
/// # Example
///
///
///     async with pool.acquire() as tab:
///         await tab.navigate("https://example.com")
///         html = await tab.content()
#[pyclass(name = "_AcquireContext")]
pub struct PyAcquireContext {
    pool:     Arc<BrowserPool>,
    tab_slot: Arc<Mutex<Option<PooledTab>>>,
}

impl fmt::Debug for PyAcquireContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("_AcquireContext").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyAcquireContext {
    fn __aenter__<'py>(slf: &Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (pool, tab_slot) = {
            let this = slf.borrow();
            (Arc::clone(&this.pool), Arc::clone(&this.tab_slot))
        };
        future_into_py(py, async move {
            let tab = pool.acquire().await.map_err(to_py_err)?;
            let use_count = tab.use_count;
            *tab_slot.lock().await = Some(tab);
            Ok(PyPooledTab { inner: tab_slot, use_count })
        })
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        _exc_type: Option<Bound<'py, PyAny>>,
        _exc_val: Option<Bound<'py, PyAny>>,
        _exc_tb: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let pool = Arc::clone(&self.pool);
        let tab_slot = Arc::clone(&self.tab_slot);
        future_into_py(py, async move {
            if let Some(tab) = tab_slot.lock().await.take() {
                pool.release(tab).await;
            }
            Ok(false)
        })
    }
}

// ── PyPoolContext ───────────────────────────────────────────────────────

/// Lazy context manager returned by [`BrowserPool.from_env()`].
///
/// Does the actual pool construction in `__aenter__` and closes on `__aexit__`.
///
/// # Example
///
///
///     async with BrowserPool.from_env() as pool:
///         async with pool.acquire() as tab:
///             await tab.navigate("https://example.com")
#[pyclass(name = "_PoolContext")]
pub struct PyPoolContext {
    pool_slot: Arc<Mutex<Option<Arc<BrowserPool>>>>,
}

impl fmt::Debug for PyPoolContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("_PoolContext").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyPoolContext {
    fn __aenter__<'py>(slf: &Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let pool_slot = Arc::clone(&slf.borrow().pool_slot);
        future_into_py(py, async move {
            let pool = Arc::new(BrowserPool::from_env().await.map_err(to_py_err)?);
            if pool.config().auto_evict {
                Arc::clone(&pool).start_eviction_task();
            }
            *pool_slot.lock().await = Some(Arc::clone(&pool));
            Ok(PyBrowserPool { inner: pool })
        })
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        _exc_type: Option<Bound<'py, PyAny>>,
        _exc_val: Option<Bound<'py, PyAny>>,
        _exc_tb: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let pool_slot = Arc::clone(&self.pool_slot);
        future_into_py(py, async move {
            if let Some(pool) = pool_slot.lock().await.take() {
                let _ = pool.close().await;
            }
            Ok(false)
        })
    }
}

// ── PyBrowserPool ──────────────────────────────────────────────────────

/// Pool of reusable browser tabs across one or more Chrome sessions.
///
/// Supports async context manager protocol (`async with`).
///
/// # Example
///
///
///     async with BrowserPool.from_env() as pool:
///         async with pool.acquire() as tab:
///             await tab.navigate("https://example.com")
///             html = await tab.content()
#[pyclass(name = "BrowserPool")]
pub struct PyBrowserPool {
    inner: Arc<BrowserPool>,
}

impl fmt::Debug for PyBrowserPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PyBrowserPool").field("inner", &self.inner).finish()
    }
}

#[pymethods]
impl PyBrowserPool {
    /// Return a context manager that builds the pool from environment
    /// variables.
    ///
    ///     async with BrowserPool.from_env() as pool:
    ///         ...
    #[classmethod]
    fn from_env(_cls: &Bound<'_, PyType>) -> PyPoolContext {
        PyPoolContext { pool_slot: Arc::new(Mutex::new(None)) }
    }

    /// Pre-open tabs across all sessions.
    fn warmup<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let pool = Arc::clone(&self.inner);
        future_into_py(py, async move { pool.warmup().await.map_err(to_py_err) })
    }

    /// Return a context manager that checks out a tab from the pool.
    ///
    ///     async with pool.acquire() as tab:
    ///         ...
    fn acquire(&self) -> PyAcquireContext {
        PyAcquireContext { pool: Arc::clone(&self.inner), tab_slot: Arc::new(Mutex::new(None)) }
    }

    /// Return a context manager that builds a pool from explicit parameters.
    ///
    /// Called by the Python `BrowserPool(config)` wrapper — not part of the
    /// public Python API.
    #[classmethod]
    #[pyo3(signature = (
        browsers, tabs_per_browser, tab_max_uses, tab_max_idle_secs, acquire_timeout_secs,
        auto_evict, headless, no_sandbox, stealth, ws_urls, proxy, chrome_executable, extra_args,
        user_data_dir
    ))]
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::fn_params_excessive_bools)]
    fn _from_params(
        _cls: &Bound<'_, PyType>,
        browsers: usize,
        tabs_per_browser: usize,
        tab_max_uses: u32,
        tab_max_idle_secs: u64,
        acquire_timeout_secs: u64,
        auto_evict: bool,
        headless: bool,
        no_sandbox: bool,
        stealth: bool,
        ws_urls: Vec<String>,
        proxy: Option<String>,
        chrome_executable: Option<String>,
        extra_args: Vec<String>,
        user_data_dir: Option<String>,
    ) -> PyPoolParamsContext {
        PyPoolParamsContext {
            browsers,
            tabs_per_browser,
            tab_max_uses,
            tab_max_idle_secs,
            acquire_timeout_secs,
            auto_evict,
            headless,
            no_sandbox,
            stealth,
            ws_urls,
            proxy,
            chrome_executable,
            extra_args,
            user_data_dir,
            pool_slot: Arc::new(Mutex::new(None)),
        }
    }

    /// Return a tab to the pool.
    fn release<'py>(
        &self,
        py: Python<'py>,
        tab: &Bound<'py, PyPooledTab>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let pool = Arc::clone(&self.inner);
        let tab_inner = Arc::clone(&tab.borrow().inner);
        future_into_py(py, async move {
            let mut guard = tab_inner.lock().await;
            if let Some(pooled_tab) = guard.take() {
                pool.release(pooled_tab).await;
            }
            Ok(())
        })
    }

    // ── async context manager ───────────────────────────────────────────

    fn __aenter__<'py>(slf: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let slf_ref = slf.into_any().unbind();
        // No warmup — tabs are created lazily on first acquire().
        future_into_py(py, async move { Ok(slf_ref) })
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        _exc_type: Option<Bound<'py, PyAny>>,
        _exc_val: Option<Bound<'py, PyAny>>,
        _exc_tb: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let pool = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let _ = pool.close().await;
            Ok(false)
        })
    }

    fn __repr__(&self) -> String {
        let cfg = self.inner.config();
        format!("BrowserPool(browsers={}, tabs_per_browser={})", cfg.browsers, cfg.tabs_per_browser)
    }
}

// ── PyPoolParamsContext ─────────────────────────────────────────────────

/// Context manager returned by `BrowserPool._from_params()`.
///
/// Launches browser sessions from explicit parameters in `__aenter__` and
/// closes the pool in `__aexit__`. Used internally by the Python
/// `BrowserPool(config)` wrapper.
#[allow(clippy::struct_excessive_bools)]
#[pyclass(name = "_PoolParamsContext")]
pub struct PyPoolParamsContext {
    browsers:             usize,
    tabs_per_browser:     usize,
    tab_max_uses:         u32,
    tab_max_idle_secs:    u64,
    acquire_timeout_secs: u64,
    auto_evict:           bool,
    headless:             bool,
    no_sandbox:           bool,
    stealth:              bool,
    ws_urls:              Vec<String>,
    proxy:                Option<String>,
    chrome_executable:    Option<String>,
    extra_args:           Vec<String>,
    user_data_dir:        Option<String>,
    pool_slot:            Arc<Mutex<Option<Arc<BrowserPool>>>>,
}

impl fmt::Debug for PyPoolParamsContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("_PoolParamsContext").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyPoolParamsContext {
    fn __aenter__<'py>(slf: &Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.borrow();
        let browsers = this.browsers;
        let tabs_per_browser = this.tabs_per_browser;
        let tab_max_uses = this.tab_max_uses;
        let tab_max_idle_secs = this.tab_max_idle_secs;
        let acquire_timeout_secs = this.acquire_timeout_secs;
        let auto_evict = this.auto_evict;
        let headless = this.headless;
        let no_sandbox = this.no_sandbox;
        let stealth_enabled = this.stealth;
        let ws_urls = this.ws_urls.clone();
        let proxy = this.proxy.clone();
        let chrome_executable = this.chrome_executable.clone();
        let extra_args = this.extra_args.clone();
        let user_data_dir = this.user_data_dir.clone();
        let pool_slot = Arc::clone(&this.pool_slot);
        drop(this);

        future_into_py(py, async move {
            let stealth =
                if stealth_enabled { StealthConfig::chrome_like() } else { StealthConfig::none() };

            let sessions: Vec<BrowserSession> = if ws_urls.is_empty() {
                let futs: Vec<_> = (0..browsers)
                    .map(|_| {
                        let mut builder = if headless {
                            BrowserSession::builder().headless()
                        } else {
                            BrowserSession::builder().headful()
                        };
                        builder = builder.stealth(stealth.clone());
                        if no_sandbox {
                            builder = builder.no_sandbox();
                        }
                        if let Some(ref p) = proxy {
                            builder = builder.proxy(p.clone());
                        }
                        if let Some(ref exe) = chrome_executable {
                            builder = builder.chrome_executable(exe.clone());
                        }
                        if let Some(ref dir) = user_data_dir {
                            builder = builder.user_data_dir(dir.clone());
                        }
                        for arg in &extra_args {
                            builder = builder.arg(arg.clone());
                        }
                        builder.launch()
                    })
                    .collect();
                future::join_all(futs)
                    .await
                    .into_iter()
                    .collect::<void_crawl_core::Result<Vec<_>>>()
                    .map_err(to_py_err)?
            } else {
                let futs: Vec<_> = ws_urls
                    .into_iter()
                    .map(|url| {
                        BrowserSession::builder()
                            .remote_debug(url)
                            .stealth(stealth.clone())
                            .launch()
                    })
                    .collect();
                future::join_all(futs)
                    .await
                    .into_iter()
                    .collect::<void_crawl_core::Result<Vec<_>>>()
                    .map_err(to_py_err)?
            };

            let config = PoolConfig {
                browsers: sessions.len(),
                tabs_per_browser,
                tab_max_uses,
                tab_max_idle_secs,
                acquire_timeout_secs,
                auto_evict,
            };
            let pool = Arc::new(BrowserPool::new(config, sessions));
            if auto_evict {
                Arc::clone(&pool).start_eviction_task();
            }
            *pool_slot.lock().await = Some(Arc::clone(&pool));
            Ok(PyBrowserPool { inner: pool })
        })
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        _exc_type: Option<Bound<'py, PyAny>>,
        _exc_val: Option<Bound<'py, PyAny>>,
        _exc_tb: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let pool_slot = Arc::clone(&self.pool_slot);
        future_into_py(py, async move {
            if let Some(pool) = pool_slot.lock().await.take() {
                let _ = pool.close().await;
            }
            Ok(false)
        })
    }
}

// ── Profile bindings ────────────────────────────────────────────────────

/// List Chrome profiles found in the platform's default user data dirs.
///
/// Returns a list of ``(name, path)`` tuples. Only profile directories
/// that contain a ``Preferences`` file are returned.
#[pyfunction]
fn py_list_profiles() -> PyResult<Vec<(String, String)>> {
    let profiles: Vec<ProfileInfo> = list_profiles().map_err(to_py_err)?;
    Ok(profiles.into_iter().map(|p| (p.name, p.path.display().to_string())).collect())
}

/// Acquire exclusive lease on a Chrome profile, launching Chrome.
///
/// Args:
///     name: Profile directory name (e.g. "Default", "Profile 1").
///     `lease_timeout`: Seconds to poll for the lock before giving up.
///     headless: Run Chrome headless (default). Set False for a visible
///         window — e.g. for a one-time manual login.
#[pyfunction]
#[pyo3(signature = (name, lease_timeout=300.0, headless=true))]
fn py_acquire_profile(
    py: Python<'_>,
    name: String,
    lease_timeout: f64,
    headless: bool,
) -> PyResult<Bound<'_, PyAny>> {
    future_into_py(py, async move {
        let handle = acquire_profile(&name, Duration::from_secs_f64(lease_timeout), headless)
            .await
            .map_err(to_py_err)?;
        Ok(PyProfileHandle { inner: Arc::new(Mutex::new(Some(handle))), name })
    })
}

/// Handle on a leased Chrome profile. Use as an async context manager,
/// or call ``release()`` explicitly.
#[pyclass(name = "ProfileHandle")]
pub struct PyProfileHandle {
    inner: Arc<Mutex<Option<ProfileHandle>>>,
    #[pyo3(get)]
    name:  String,
}

impl fmt::Debug for PyProfileHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PyProfileHandle").field("name", &self.name).finish_non_exhaustive()
    }
}

#[pymethods]
impl PyProfileHandle {
    /// Path to the profile directory on disk.
    fn path<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let guard = inner.lock().await;
            let h = guard
                .as_ref()
                .ok_or_else(|| PyRuntimeError::new_err("profile handle already released"))?;
            Ok(h.path().display().to_string())
        })
    }

    /// Open a new tab in the profile's Chrome and navigate to `url`.
    fn new_page<'py>(&self, py: Python<'py>, url: String) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let guard = inner.lock().await;
            let h = guard
                .as_ref()
                .ok_or_else(|| PyRuntimeError::new_err("profile handle already released"))?;
            let session = h.session().map_err(to_py_err)?;
            let page = session.new_page(&url).await.map_err(to_py_err)?;
            Ok(PyPage::new(page))
        })
    }

    /// Release the profile lease: close Chrome, drop the lock.
    fn release<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            if let Some(mut h) = inner.lock().await.take() {
                h.close().await.map_err(to_py_err)?;
            }
            Ok(())
        })
    }

    fn __aenter__<'py>(slf: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        // `async with` awaits the return value, so __aenter__ must
        // produce an awaitable — not the pyclass instance directly.
        // Return a future that resolves to self, matching the pattern
        // the other context-manager pyclasses in this file use.
        let slf_ref = slf.into_any().unbind();
        future_into_py(py, async move { Ok(slf_ref) })
    }

    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        _exc_type: Bound<'py, PyAny>,
        _exc_val: Bound<'py, PyAny>,
        _exc_tb: Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.release(py)
    }
}

// ── Scanner bindings ────────────────────────────────────────────────────

/// Scan a file on disk with the content-safety gate (size cap + magic-byte
/// type check + yara-x signatures). Returns a :class:`ScanReport`.
///
/// Args:
///     path: Path to the file to scan.
///     `max_bytes`: Flag files larger than this (default 100 MiB).
///     `claimed_mime`: The Content-Type the server claimed, if known — pass
///         ``DownloadOutcome.content_type`` so an executable disguised as a
///         document is flagged.
#[pyfunction]
#[pyo3(name = "scan_file", signature = (path, max_bytes=None, claimed_mime=None))]
fn py_scan_file(
    path: &str,
    max_bytes: Option<u64>,
    claimed_mime: Option<String>,
) -> PyResult<PyScanReport> {
    let cfg = ScanConfig { max_bytes: max_bytes.unwrap_or(DEFAULT_MAX_BYTES), claimed_mime };
    let report = scan_path(Path::new(path), &cfg).map_err(to_py_err)?;
    Ok(PyScanReport::from(report))
}

/// Scan an in-memory buffer with the content-safety gate. See
/// :func:`scan_file`.
#[pyfunction]
#[pyo3(name = "scan_bytes", signature = (data, max_bytes=None, claimed_mime=None))]
fn py_scan_bytes(
    data: &[u8],
    max_bytes: Option<u64>,
    claimed_mime: Option<String>,
) -> PyScanReport {
    let cfg = ScanConfig { max_bytes: max_bytes.unwrap_or(DEFAULT_MAX_BYTES), claimed_mime };
    PyScanReport::from(scan_bytes(data, &cfg))
}

// ── Module ──────────────────────────────────────────────────────────────

#[pymodule]
fn _ext(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBrowserSession>()?;
    m.add_class::<PyPage>()?;
    m.add_class::<PyBrowserPool>()?;
    m.add_class::<PyPooledTab>()?;
    m.add_class::<PyAcquireContext>()?;
    m.add_class::<PyPoolContext>()?;
    m.add_class::<PyPoolParamsContext>()?;
    m.add_class::<PyPageResponse>()?;
    m.add_class::<PyAntibotVerdict>()?;
    m.add_class::<PyDownloadOutcome>()?;
    m.add_class::<PyDownloadCapture>()?;
    m.add_class::<PyScanReport>()?;
    m.add_class::<PyProfileHandle>()?;
    m.add_function(wrap_pyfunction!(py_list_profiles, m)?)?;
    m.add_function(wrap_pyfunction!(py_acquire_profile, m)?)?;
    m.add_function(wrap_pyfunction!(py_scan_file, m)?)?;
    m.add_function(wrap_pyfunction!(py_scan_bytes, m)?)?;
    let py = m.py();
    m.add("VoidCrawlError", py.get_type::<VoidCrawlError>())?;
    m.add("ProfileBusy", py.get_type::<ProfileBusy>())?;
    m.add("ProfileLeaseExpired", py.get_type::<ProfileLeaseExpired>())?;
    m.add("ProfileNotFound", py.get_type::<ProfileNotFound>())?;
    m.add("CaptchaDetected", py.get_type::<CaptchaDetected>())?;
    m.add("AntibotChallenge", py.get_type::<AntibotChallenge>())?;
    Ok(())
}
