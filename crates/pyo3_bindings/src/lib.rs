//! `PyO3` bindings for `void_crawl_core`.
//!
//! Exposes `PyBrowserSession` and `PyPage` as Python classes with async methods
//! that bridge to Python's asyncio via `pyo3-async-runtimes`.

use std::{
    collections::HashMap,
    convert::Infallible,
    fmt, mem,
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
use tokio::{sync::Mutex, task::spawn_blocking};
use void_crawl_core::{
    AntibotEvidence, AntibotVerdict, BrowserMode, BrowserPool, BrowserSession, CapturedResponse,
    CookieParam, DEFAULT_MAX_BYTES, DEFAULT_MAX_RESPONSE_BYTES, DEFAULT_MAX_TOTAL_RESPONSE_BYTES,
    DeleteCookiesParams, DispatchKeyEventType, DispatchMouseEventType, DownloadCapture,
    DownloadOutcome, MAX_PROFILE_SPLIT_COPIES, ManagedProfileSnapshot, MouseButton, Page,
    PageResponse, PoolConfig, PooledTab, ProfileHandle, ProfileInfo, ProfileRegistry,
    ResponseCapture, ResponseCaptureLimits, ScanConfig, ScanReport, StealthConfig, Verdict,
    acquire_profile, list_profiles, scan_bytes, scan_path,
};

// ── Error conversion ────────────────────────────────────────────────────

pyo3::create_exception!(voidcrawl._ext, VoidCrawlError, PyRuntimeError);
pyo3::create_exception!(voidcrawl._ext, NavigationError, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, NavigationTimeoutError, NavigationError);
pyo3::create_exception!(voidcrawl._ext, BrowserClosedError, NavigationError);
pyo3::create_exception!(voidcrawl._ext, ResponseTimeoutError, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, ProfileBusy, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, ChromeProfileBusy, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, ProfileLeaseExpired, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, ProfileNotFound, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, CaptchaDetected, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, AntibotChallenge, VoidCrawlError);

#[allow(clippy::needless_pass_by_value)] // used as fn pointer in map_err(to_py_err)
fn to_py_err(e: void_crawl_core::VoidCrawlError) -> PyErr {
    match e {
        void_crawl_core::VoidCrawlError::NavigationTimeout {
            ref url,
            ref wait_phase,
            timeout_secs,
            elapsed_secs,
        } => {
            let err = NavigationTimeoutError::new_err(e.to_string());
            Python::attach(|py| {
                let value = err.value(py);
                let _ = value.setattr("url", url);
                let _ = value.setattr("wait_phase", wait_phase);
                let _ = value.setattr("timeout", timeout_secs);
                let _ = value.setattr("elapsed", elapsed_secs);
            });
            err
        }
        void_crawl_core::VoidCrawlError::NavigationFailed(_) => {
            NavigationError::new_err(e.to_string())
        }
        void_crawl_core::VoidCrawlError::BrowserClosed => {
            BrowserClosedError::new_err(e.to_string())
        }
        void_crawl_core::VoidCrawlError::ResponseTimeout { .. } => {
            ResponseTimeoutError::new_err(e.to_string())
        }
        void_crawl_core::VoidCrawlError::ProfileBusy { ref name, pid, acquired_at } => {
            let err = ProfileBusy::new_err(e.to_string());
            Python::attach(|py| {
                let value = err.value(py);
                let _ = value.setattr("profile", name);
                let _ = value.setattr("owner_pid", pid);
                let _ = value.setattr("acquired_at", acquired_at);
            });
            err
        }
        void_crawl_core::VoidCrawlError::ProfileLeaseExpired { .. } => {
            ProfileLeaseExpired::new_err(e.to_string())
        }
        void_crawl_core::VoidCrawlError::ChromeProfileBusy { .. } => {
            ChromeProfileBusy::new_err(e.to_string())
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

// ── CapturedResponse ────────────────────────────────────────────────────

/// A passively observed network response with an opt-in bounded body.
#[pyclass(name = "CapturedResponse", skip_from_py_object)]
#[derive(Debug, Clone)]
pub struct PyCapturedResponse {
    inner: CapturedResponse,
}

impl From<CapturedResponse> for PyCapturedResponse {
    fn from(inner: CapturedResponse) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyCapturedResponse {
    #[getter]
    fn url(&self) -> &str {
        &self.inner.url
    }

    #[getter]
    fn status(&self) -> u16 {
        self.inner.status
    }

    #[getter]
    fn headers(&self) -> HashMap<String, String> {
        self.inner.headers.iter().cloned().collect()
    }

    #[getter]
    fn mime_type(&self) -> &str {
        &self.inner.mime_type
    }

    #[getter]
    fn resource_type(&self) -> &str {
        &self.inner.resource_type
    }

    #[getter]
    #[allow(clippy::wrong_self_convention)]
    fn from_cache(&self) -> bool {
        self.inner.from_cache
    }

    #[getter]
    #[allow(clippy::wrong_self_convention)]
    fn from_service_worker(&self) -> bool {
        self.inner.from_service_worker
    }

    #[getter]
    fn body_state(&self) -> &'static str {
        self.inner.body_state.as_str()
    }

    #[getter]
    fn body_error(&self) -> Option<&str> {
        self.inner.body_error.as_deref()
    }

    #[getter]
    fn truncated(&self) -> bool {
        self.inner.body_state == void_crawl_core::ResponseBodyState::Truncated
    }

    fn bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let body = captured_body(&self.inner)?;
        future_into_py(py, async move { Ok(PyBytesResult(body)) })
    }

    fn text<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        captured_body(&self.inner)?;
        let text = self.inner.text().map_err(to_py_err)?;
        future_into_py(py, async move { Ok(text) })
    }

    fn json<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        captured_body(&self.inner)?;
        let value = self.inner.json().map_err(to_py_err)?;
        future_into_py(py, async move { Ok(PyJsonValue(value)) })
    }

    fn __repr__(&self) -> String {
        format!(
            "CapturedResponse(url={:?}, status={}, body_state={:?}, body_len={})",
            self.inner.url,
            self.inner.status,
            self.inner.body_state.as_str(),
            self.inner.body().len(),
        )
    }
}

fn validate_response_options(
    timeout: f64,
    max_response_bytes: usize,
    max_total_bytes: usize,
) -> PyResult<()> {
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err(PyValueError::new_err("timeout must be positive and finite"));
    }
    if max_response_bytes == 0 || max_total_bytes == 0 {
        return Err(PyValueError::new_err("response byte limits must be positive"));
    }
    Ok(())
}

fn captured_body(response: &CapturedResponse) -> PyResult<Vec<u8>> {
    if response.body_state == void_crawl_core::ResponseBodyState::Unavailable {
        return Err(PyRuntimeError::new_err(
            response.body_error.clone().unwrap_or_else(|| "response body unavailable".into()),
        ));
    }
    Ok(response.body().to_vec())
}

/// Async expectation context returned by ``Page.expect_response(s)``.
#[pyclass(name = "ResponseExpectation")]
pub struct PyResponseExpectation {
    page:     Arc<Mutex<Option<Arc<Page>>>>,
    patterns: Vec<(String, String)>,
    timeout:  Duration,
    limits:   ResponseCaptureLimits,
    single:   bool,
    capture:  Arc<Mutex<Option<ResponseCapture>>>,
    result:   Arc<Mutex<Option<HashMap<String, CapturedResponse>>>>,
}

impl fmt::Debug for PyResponseExpectation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseExpectation")
            .field("patterns", &self.patterns)
            .finish_non_exhaustive()
    }
}

impl PyResponseExpectation {
    fn new(
        page: Arc<Mutex<Option<Arc<Page>>>>,
        patterns: Vec<(String, String)>,
        timeout: f64,
        max_response_bytes: usize,
        max_total_bytes: usize,
        single: bool,
    ) -> Self {
        Self {
            page,
            patterns,
            timeout: Duration::from_secs_f64(timeout),
            limits: ResponseCaptureLimits { max_response_bytes, max_total_bytes },
            single,
            capture: Arc::new(Mutex::new(None)),
            result: Arc::new(Mutex::new(None)),
        }
    }
}

#[pymethods]
impl PyResponseExpectation {
    fn __aenter__<'py>(slf: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (page_slot, patterns, timeout, limits, capture_slot) = {
            let this = slf.borrow();
            (
                Arc::clone(&this.page),
                this.patterns.clone(),
                this.timeout,
                this.limits,
                Arc::clone(&this.capture),
            )
        };
        let slf_ref = slf.into_any().unbind();
        future_into_py(py, async move {
            let page = page_slot
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let capture =
                page.expect_responses(patterns, timeout, limits).await.map_err(to_py_err)?;
            *capture_slot.lock().await = Some(capture);
            Ok(slf_ref)
        })
    }

    #[pyo3(signature = (exc_type=None, _exc_val=None, _exc_tb=None))]
    #[allow(clippy::needless_pass_by_value)]
    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        exc_type: Option<Bound<'py, PyAny>>,
        _exc_val: Option<Bound<'py, PyAny>>,
        _exc_tb: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let failed = exc_type.is_some();
        let capture_slot = Arc::clone(&self.capture);
        let result_slot = Arc::clone(&self.result);
        future_into_py(py, async move {
            let capture = capture_slot.lock().await.take();
            if failed {
                drop(capture);
                return Ok(false);
            }
            let capture = capture
                .ok_or_else(|| PyRuntimeError::new_err("response expectation was not entered"))?;
            let responses = capture.wait().await.map_err(to_py_err)?;
            *result_slot.lock().await = Some(responses);
            Ok(false)
        })
    }

    #[getter]
    fn value<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let result = Arc::clone(&self.result);
        let single = self.single;
        future_into_py(py, async move {
            let responses =
                result.lock().await.as_ref().cloned().ok_or_else(|| {
                    PyRuntimeError::new_err("response expectation has not completed")
                })?;
            if single {
                let response = responses
                    .get("response")
                    .cloned()
                    .ok_or_else(|| PyRuntimeError::new_err("expected response was not captured"))?;
                Python::attach(|py| {
                    Py::new(py, PyCapturedResponse::from(response)).map(Py::into_any)
                })
            } else {
                Python::attach(|py| {
                    let dict = PyDict::new(py);
                    for (name, response) in responses {
                        dict.set_item(name, Py::new(py, PyCapturedResponse::from(response))?)?;
                    }
                    Ok(dict.unbind().into_any())
                })
            }
        })
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
    inner: Arc<Mutex<Option<Arc<BrowserSession>>>>,
    mode: BrowserMode,
    stealth_enabled: bool,
    no_sandbox: bool,
    proxy: Option<String>,
    chrome_executable: Option<String>,
    extra_args: Vec<String>,
    user_data_dir: Option<String>,
    port: Option<u16>,
) -> PyResult<()> {
    let stealth =
        if stealth_enabled { StealthConfig::chrome_like() } else { StealthConfig::none() };

    let mut builder = BrowserSession::builder().mode(mode).stealth(stealth);

    if let Some(p) = port {
        builder = builder.port(p);
    }
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
    *guard = Some(Arc::new(session));
    Ok(())
}

// ── PyPage ──────────────────────────────────────────────────────────────

/// A browser page / tab.
///
/// All navigation and DOM methods are async — await them from Python.
#[pyclass(name = "Page")]
pub struct PyPage {
    inner: Arc<Mutex<Option<Arc<Page>>>>,
}

impl fmt::Debug for PyPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PyPage").finish_non_exhaustive()
    }
}

impl PyPage {
    fn new(page: Page) -> Self {
        Self { inner: Arc::new(Mutex::new(Some(Arc::new(page)))) }
    }
}

/// Run an async operation using a cloned page handle. The state mutex is
/// never held across a CDP await, and cancellation cannot remove the page.
macro_rules! with_page {
    ($self:expr, $py:expr, |$page:ident| $body:expr) => {{
        let inner = Arc::clone(&$self.inner);
        future_into_py($py, async move {
            let page = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let $page = page.as_ref();
            $body.await.map_err(to_py_err)
        })
    }};
}

/// Variant of `with_page!` that transforms the successful result.
macro_rules! with_page_map {
    ($self:expr, $py:expr, |$page:ident| $body:expr, |$res:ident| $map:expr) => {{
        let inner = Arc::clone(&$self.inner);
        future_into_py($py, async move {
            let page = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let $page = page.as_ref();
            let $res = $body.await.map_err(to_py_err)?;
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

    /// Install JavaScript before each subsequent document executes.
    fn add_init_script<'py>(&self, py: Python<'py>, script: String) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.add_init_script(&script))
    }

    /// Arm one passive response expectation before a triggering action.
    #[pyo3(signature = (pattern, timeout=30.0, max_response_bytes=DEFAULT_MAX_RESPONSE_BYTES, max_total_bytes=DEFAULT_MAX_TOTAL_RESPONSE_BYTES))]
    fn expect_response(
        &self,
        pattern: String,
        timeout: f64,
        max_response_bytes: usize,
        max_total_bytes: usize,
    ) -> PyResult<PyResponseExpectation> {
        validate_response_options(timeout, max_response_bytes, max_total_bytes)?;
        Ok(PyResponseExpectation::new(
            Arc::clone(&self.inner),
            vec![("response".into(), pattern)],
            timeout,
            max_response_bytes,
            max_total_bytes,
            true,
        ))
    }

    /// Arm named passive response expectations before a triggering action.
    #[pyo3(signature = (patterns, timeout=30.0, max_response_bytes=DEFAULT_MAX_RESPONSE_BYTES, max_total_bytes=DEFAULT_MAX_TOTAL_RESPONSE_BYTES))]
    fn expect_responses(
        &self,
        patterns: HashMap<String, String>,
        timeout: f64,
        max_response_bytes: usize,
        max_total_bytes: usize,
    ) -> PyResult<PyResponseExpectation> {
        validate_response_options(timeout, max_response_bytes, max_total_bytes)?;
        if patterns.is_empty() {
            return Err(PyValueError::new_err("patterns must not be empty"));
        }
        Ok(PyResponseExpectation::new(
            Arc::clone(&self.inner),
            patterns.into_iter().collect(),
            timeout,
            max_response_bytes,
            max_total_bytes,
            false,
        ))
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
    #[pyo3(signature = (url, timeout=30.0, capture_endpoints=false, *, wait_until="networkidle"))]
    fn goto<'py>(
        &self,
        py: Python<'py>,
        url: String,
        timeout: f64,
        capture_endpoints: bool,
        wait_until: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        if wait_until != "networkidle" {
            return Err(PyValueError::new_err("wait_until currently supports only 'networkidle'"));
        }
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

    /// The CDP target id of this page (stable across same-tab navigations).
    ///
    /// Pass it to :meth:`BrowserSession.attach_page` from another connection
    /// (attached to the same Chrome via ``ws_url``) to re-adopt this exact tab.
    fn target_id<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            Ok(page.target_id())
        })
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
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let mut opts = void_crawl_core::ScreenshotOptions::default();
            if let Some(p) = path {
                opts = opts.with_path(p);
            }
            if let Some((x, y, w, h)) = bbox {
                opts = opts.with_bbox(void_crawl_core::Bbox { x, y, width: w, height: h });
            }
            let result = page.screenshot(opts).await.map_err(to_py_err)?;
            Ok(PyScreenshotOutput(result))
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
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let result = void_crawl_core::detect_captcha(&page).await.map_err(to_py_err)?;
            Ok(result.map(|k| k.as_str().to_string()))
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
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let max = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
            let result = page
                .download_to_dir(&url, Path::new(&dir), Duration::from_secs_f64(timeout), max)
                .await
                .map_err(to_py_err)?;
            Ok(PyDownloadOutcome::from(result))
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
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let max = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
            let result = page.arm_download(Path::new(&dir), max).await.map_err(to_py_err)?;
            Ok(PyDownloadCapture::new(result))
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
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let result =
                cap.wait(&page, Duration::from_secs_f64(timeout)).await.map_err(to_py_err)?;
            Ok(PyDownloadOutcome::from(result))
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
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            page.reset_download_behavior().await;
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
    #[pyo3(signature = (role, name, nth=0, humanize=false))]
    fn click_by_role<'py>(
        &self,
        py: Python<'py>,
        role: String,
        name: String,
        nth: usize,
        humanize: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.click_by_role(&role, &name, nth, humanize))
    }

    /// Move the virtual cursor to ``(x, y)``. With ``humanize=True`` it travels
    /// a realistic curved, min-jerk, lightly-tremored path (multiple CDP
    /// MouseMoved events) from its last position; otherwise it jumps. No
    /// page-world JS.
    #[pyo3(signature = (x, y, humanize=false))]
    fn move_mouse<'py>(
        &self,
        py: Python<'py>,
        x: f64,
        y: f64,
        humanize: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.move_mouse(x, y, humanize))
    }

    /// Click at ``(x, y)`` with a trusted compositor event (press → release).
    /// With ``humanize=True`` the cursor first travels a human-like path there.
    #[pyo3(signature = (x, y, humanize=false))]
    fn click_xy<'py>(
        &self,
        py: Python<'py>,
        x: f64,
        y: f64,
        humanize: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.click_xy(x, y, humanize))
    }

    /// Click an element by accessibility ``role`` + accessible ``name``
    /// **inside a specific (possibly cross-origin) frame**, with a real
    /// compositor click. The cross-frame, shadow-piercing analogue of
    /// :meth:`click_by_role` — reaches widgets in closed shadow roots inside
    /// cross-origin iframes (e.g. Cloudflare Turnstile's checkbox). With
    /// ``humanize=True`` the cursor travels a human-like path to the checkbox.
    /// Empty ``name`` matches any node of that role. Raises if no such node
    /// exists.
    #[pyo3(signature = (frame_url_pattern, role, name, nth=0, humanize=false))]
    fn click_ax_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        role: String,
        name: String,
        nth: usize,
        humanize: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.click_ax_in_frame(
            &frame_url_pattern,
            &role,
            &name,
            nth,
            humanize
        ))
    }

    /// Locate an element by accessibility ``role`` + ``name`` inside a specific
    /// (possibly cross-origin) frame and return its on-page rectangle
    /// ``[x, y, width, height]`` in CSS pixels — the geometry for driving a
    /// **humanized** click yourself (curved approach via
    /// :meth:`dispatch_mouse_event`, press at a jittered point) instead of
    /// the centre click of :meth:`click_ax_in_frame`. Pierces closed shadow
    /// roots. Empty ``name`` matches any node of that role.
    #[pyo3(signature = (frame_url_pattern, role, name, nth=0))]
    fn ax_box_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        role: String,
        name: String,
        nth: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(
            self,
            py,
            |page| page.ax_box_in_frame(&frame_url_pattern, &role, &name, nth),
            |rect| PyJsonValue(serde_json::Value::from(rect))
        )
    }

    /// Compact accessibility outline of a specific (possibly cross-origin)
    /// frame — pierces closed shadow roots. Use it to discover the role /
    /// accessible name for :meth:`click_ax_in_frame`.
    #[pyo3(signature = (frame_url_pattern, depth=None))]
    fn ax_outline_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        depth: Option<i64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.ax_outline_in_frame(&frame_url_pattern, depth), |s| {
            PyJsonValue(serde_json::Value::from(s))
        })
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
    inner:             Arc<Mutex<Option<Arc<BrowserSession>>>>,
    mode:              BrowserMode,
    stealth_enabled:   bool,
    no_sandbox:        bool,
    proxy:             Option<String>,
    chrome_executable: Option<String>,
    extra_args:        Vec<String>,
    user_data_dir:     Option<String>,
    port:              Option<u16>,
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
    ///     port: Pin Chrome's `--remote-debugging-port` so another process can
    ///         attach to this browser via its `ws_url`. `None` lets the OS pick
    ///         a free ephemeral port.
    #[new]
    #[pyo3(signature = (*, headless=true, ws_url=None, stealth=true, no_sandbox=false, proxy=None, chrome_executable=None, extra_args=None, user_data_dir=None, port=None))]
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
        port: Option<u16>,
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
            port,
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
        let port = self.port;

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
                port,
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
    #[pyo3(signature = (url=None))]
    fn new_page<'py>(&self, py: Python<'py>, url: Option<String>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner.lock().await.as_ref().cloned().ok_or_else(|| {
                PyRuntimeError::new_err(
                    "browser not launched — use `async with` or call launch() first",
                )
            })?;
            let page = match url {
                Some(url) => session.new_page(&url).await,
                None => session.new_blank_page().await,
            }
            .map_err(to_py_err)?;
            Ok(PyPage::new(page))
        })
    }

    /// Adopt an existing tab by its CDP ``target_id`` (see
    /// :meth:`Page.target_id`).
    ///
    /// Unlike :meth:`new_page`, this opens NO new tab and does NOT re-apply
    /// stealth — it wraps the live tab the browser already has. Use it from a
    /// second process attached via ``ws_url`` to drive the exact tab the
    /// primary driver is on (e.g. to solve a captcha in place).
    fn attach_page<'py>(&self, py: Python<'py>, target_id: String) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner.lock().await.as_ref().cloned().ok_or_else(|| {
                PyRuntimeError::new_err(
                    "browser not launched — use `async with` or call launch() first",
                )
            })?;
            let page = session.attach_page(&target_id).await.map_err(to_py_err)?;
            Ok(PyPage::new(page))
        })
    }

    /// The browser's CDP WebSocket endpoint (``ws://…``).
    ///
    /// Hand this to another process (with a tab's ``target_id``) so it can
    /// attach to the *same* Chrome via ``BrowserConfig(ws_url=…)``.
    fn websocket_url<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("browser not launched"))?;
            Ok(session.websocket_url().await)
        })
    }

    /// Get browser version string.
    fn version<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("browser not launched"))?;
            session.version().await.map_err(to_py_err)
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
            port,
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
                this.port,
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
                port,
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
    #[pyo3(signature = (role, name, nth=0, humanize=false))]
    fn click_by_role<'py>(
        &self,
        py: Python<'py>,
        role: String,
        name: String,
        nth: usize,
        humanize: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.click_by_role(&role, &name, nth, humanize))
    }

    /// Move the virtual cursor to ``(x, y)``; ``humanize=True`` for a
    /// human-like curved path (multiple CDP MouseMoved events). No
    /// page-world JS.
    #[pyo3(signature = (x, y, humanize=false))]
    fn move_mouse<'py>(
        &self,
        py: Python<'py>,
        x: f64,
        y: f64,
        humanize: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.move_mouse(x, y, humanize))
    }

    /// Click at ``(x, y)`` with a trusted compositor event; ``humanize=True``
    /// first travels a human-like path there.
    #[pyo3(signature = (x, y, humanize=false))]
    fn click_xy<'py>(
        &self,
        py: Python<'py>,
        x: f64,
        y: f64,
        humanize: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.click_xy(x, y, humanize))
    }

    /// Click an element by accessibility ``role`` + accessible ``name``
    /// **inside a specific (possibly cross-origin) frame**, with a real
    /// compositor click — the cross-frame, shadow-piercing analogue of
    /// :meth:`click_by_role` (e.g. Cloudflare Turnstile's checkbox in a closed
    /// shadow root). Empty ``name`` matches any node of that role.
    #[pyo3(signature = (frame_url_pattern, role, name, nth=0, humanize=false))]
    fn click_ax_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        role: String,
        name: String,
        nth: usize,
        humanize: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.click_ax_in_frame(
            &frame_url_pattern,
            &role,
            &name,
            nth,
            humanize
        ))
    }

    /// Locate an element by accessibility ``role`` + ``name`` inside a specific
    /// frame and return its on-page rectangle ``[x, y, width, height]`` — the
    /// geometry for a humanized click (see :meth:`click_ax_in_frame`). Pierces
    /// closed shadow roots. Empty ``name`` matches any node of that role.
    #[pyo3(signature = (frame_url_pattern, role, name, nth=0))]
    fn ax_box_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        role: String,
        name: String,
        nth: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(
            self,
            py,
            |page| page.ax_box_in_frame(&frame_url_pattern, &role, &name, nth),
            |rect| PyJsonValue(serde_json::Value::from(rect))
        )
    }

    /// Compact accessibility outline of a specific (possibly cross-origin)
    /// frame — pierces closed shadow roots; discover roles/names for
    /// :meth:`click_ax_in_frame`.
    #[pyo3(signature = (frame_url_pattern, depth=None))]
    fn ax_outline_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        depth: Option<i64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(
            self,
            py,
            |page| page.ax_outline_in_frame(&frame_url_pattern, depth),
            |s| PyJsonValue(serde_json::Value::from(s))
        )
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

fn registry_from_root(root: Option<String>) -> ProfileRegistry {
    root.map_or_else(ProfileRegistry::default, ProfileRegistry::new)
}

fn to_json_string<T: serde::Serialize>(value: &T) -> PyResult<String> {
    serde_json::to_string(value).map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

#[pyfunction]
#[pyo3(signature = (root=None))]
fn py_profile_registry_root(root: Option<String>) -> String {
    registry_from_root(root).root().display().to_string()
}

#[pyfunction]
#[pyo3(signature = (root=None))]
fn py_profile_registry_list(root: Option<String>) -> PyResult<String> {
    let result = registry_from_root(root).list_profiles().map_err(to_py_err)?;
    to_json_string(&result)
}

#[pyfunction]
#[pyo3(signature = (id, description=None, labels=None, root=None))]
fn py_profile_registry_create(
    id: &str,
    description: Option<String>,
    labels: Option<Vec<String>>,
    root: Option<String>,
) -> PyResult<String> {
    let result = registry_from_root(root)
        .create_profile(id, description, labels.unwrap_or_default())
        .map_err(to_py_err)?;
    to_json_string(&result)
}

#[pyfunction]
#[pyo3(signature = (id, root=None))]
fn py_profile_registry_describe(id: &str, root: Option<String>) -> PyResult<String> {
    let result = registry_from_root(root).describe_profile(id).map_err(to_py_err)?;
    to_json_string(&result)
}

#[pyfunction]
#[pyo3(signature = (source_id_or_path, id, description=None, labels=None, root=None))]
fn py_profile_registry_clone(
    source_id_or_path: &str,
    id: &str,
    description: Option<String>,
    labels: Option<Vec<String>>,
    root: Option<String>,
) -> PyResult<String> {
    let result = registry_from_root(root)
        .clone_profile(source_id_or_path, id, description, labels.unwrap_or_default())
        .map_err(to_py_err)?;
    to_json_string(&result)
}

#[pyclass(name = "ManagedProfileSnapshot")]
#[derive(Debug)]
pub struct PyManagedProfileSnapshot {
    inner: StdMutex<Option<ManagedProfileSnapshot>>,
}

#[pymethods]
impl PyManagedProfileSnapshot {
    #[getter]
    fn path(&self) -> PyResult<String> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| PyRuntimeError::new_err("profile snapshot lock poisoned"))?;
        let snapshot =
            guard.as_ref().ok_or_else(|| PyRuntimeError::new_err("profile snapshot is closed"))?;
        Ok(snapshot.path().display().to_string())
    }

    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let snapshot = self
            .inner
            .lock()
            .map_err(|_| PyRuntimeError::new_err("profile snapshot lock poisoned"))?
            .take();
        future_into_py(py, async move {
            drop(snapshot);
            Ok(())
        })
    }

    fn __aenter__<'py>(slf: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let slf_ref = slf.into_any().unbind();
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
        self.close(py)
    }
}

#[derive(Debug, Clone, Copy)]
enum ProfileSplitSource {
    Managed,
    Native,
}

#[derive(Debug)]
enum ProfileSplitState {
    Ready,
    Preparing,
    Active(Vec<ManagedProfileSnapshot>),
    Closed,
}

struct ProfileSplitPreparation {
    state: Arc<StdMutex<ProfileSplitState>>,
    armed: bool,
}

impl Drop for ProfileSplitPreparation {
    fn drop(&mut self) {
        if self.armed {
            if let Ok(mut state) = self.state.lock() {
                if matches!(*state, ProfileSplitState::Preparing) {
                    *state = ProfileSplitState::Ready;
                }
            }
        }
    }
}

/// A cleanup scope containing isolated copies of one managed profile.
///
/// Copying begins in ``__aenter__`` on a blocking worker, not on Python's
/// asyncio thread. All copies are made while one authoritative source lease is
/// held, so they share a consistent starting point. Their paths are distinct
/// Chrome ``user_data_dir`` roots: writes diverge after the browsers launch.
#[pyclass(name = "ManagedProfileSplit")]
#[derive(Debug)]
pub struct PyManagedProfileSplit {
    source_id: String,
    root:      Option<String>,
    copies:    usize,
    source:    ProfileSplitSource,
    state:     Arc<StdMutex<ProfileSplitState>>,
}

#[pymethods]
impl PyManagedProfileSplit {
    #[getter]
    fn source_id(&self) -> &str {
        &self.source_id
    }

    #[getter]
    fn paths(&self) -> PyResult<Vec<String>> {
        let state = self
            .state
            .lock()
            .map_err(|_| PyRuntimeError::new_err("profile split lock poisoned"))?;
        let ProfileSplitState::Active(snapshots) = &*state else {
            return Err(PyRuntimeError::new_err(
                "profile split paths are available only inside its async context",
            ));
        };
        Ok(snapshots.iter().map(|snapshot| snapshot.path().display().to_string()).collect())
    }

    fn __len__(&self) -> usize {
        self.copies
    }

    fn __aenter__<'py>(slf: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (source_id, root, copies, source, state) = {
            let this = slf.borrow();
            let mut current = this
                .state
                .lock()
                .map_err(|_| PyRuntimeError::new_err("profile split lock poisoned"))?;
            if !matches!(*current, ProfileSplitState::Ready) {
                return Err(PyRuntimeError::new_err(
                    "profile split context cannot be entered more than once",
                ));
            }
            *current = ProfileSplitState::Preparing;
            (
                this.source_id.clone(),
                this.root.clone(),
                this.copies,
                this.source,
                Arc::clone(&this.state),
            )
        };
        let slf_ref = slf.into_any().unbind();
        future_into_py(py, async move {
            let mut preparation =
                ProfileSplitPreparation { state: Arc::clone(&state), armed: true };
            let snapshots = spawn_blocking(move || match source {
                ProfileSplitSource::Managed => {
                    registry_from_root(root).split_profile(&source_id, copies)
                }
                ProfileSplitSource::Native => {
                    registry_from_root(root).fork_profile(&source_id, copies)
                }
            })
            .await
            .map_err(|error| {
                PyRuntimeError::new_err(format!("profile split worker failed: {error}"))
            })?
            .map_err(to_py_err)?;

            let mut current =
                state.lock().map_err(|_| PyRuntimeError::new_err("profile split lock poisoned"))?;
            if !matches!(*current, ProfileSplitState::Preparing) {
                return Err(PyRuntimeError::new_err(
                    "profile split was closed while copies were being prepared",
                ));
            }
            *current = ProfileSplitState::Active(snapshots);
            preparation.armed = false;
            drop(current);
            Ok(slf_ref)
        })
    }

    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let snapshots = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| PyRuntimeError::new_err("profile split lock poisoned"))?;
            match mem::replace(&mut *state, ProfileSplitState::Closed) {
                ProfileSplitState::Active(snapshots) => snapshots,
                ProfileSplitState::Ready | ProfileSplitState::Closed => Vec::new(),
                ProfileSplitState::Preparing => {
                    return Err(PyRuntimeError::new_err("profile split is still being prepared"));
                }
            }
        };
        future_into_py(py, async move {
            spawn_blocking(move || drop(snapshots)).await.map_err(|error| {
                PyRuntimeError::new_err(format!("profile split cleanup failed: {error}"))
            })?;
            Ok(())
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
        self.close(py)
    }
}

fn profile_split_context(
    source_id: String,
    copies: usize,
    root: Option<String>,
    source: ProfileSplitSource,
) -> PyResult<PyManagedProfileSplit> {
    if !(2..=MAX_PROFILE_SPLIT_COPIES).contains(&copies) {
        return Err(PyValueError::new_err(format!(
            "copies must be between 2 and {MAX_PROFILE_SPLIT_COPIES}"
        )));
    }
    Ok(PyManagedProfileSplit {
        source_id,
        root,
        copies,
        source,
        state: Arc::new(StdMutex::new(ProfileSplitState::Ready)),
    })
}

#[pyfunction]
#[pyo3(signature = (id, copies=2, root=None))]
fn py_profile_registry_split(
    id: String,
    copies: usize,
    root: Option<String>,
) -> PyResult<PyManagedProfileSplit> {
    profile_split_context(id, copies, root, ProfileSplitSource::Managed)
}

#[pyfunction]
#[pyo3(signature = (source="Default".to_string(), copies=2, root=None))]
fn py_profile_registry_fork(
    source: String,
    copies: usize,
    root: Option<String>,
) -> PyResult<PyManagedProfileSplit> {
    profile_split_context(source, copies, root, ProfileSplitSource::Native)
}

#[pyfunction]
#[pyo3(signature = (id, root=None))]
fn py_profile_registry_snapshot(
    id: &str,
    root: Option<String>,
) -> PyResult<PyManagedProfileSnapshot> {
    let snapshot = registry_from_root(root).snapshot_profile(id).map_err(to_py_err)?;
    Ok(PyManagedProfileSnapshot { inner: StdMutex::new(Some(snapshot)) })
}

#[pyfunction]
#[pyo3(signature = (id, root=None))]
fn py_profile_registry_delete(id: &str, root: Option<String>) -> PyResult<bool> {
    registry_from_root(root).delete_profile(id).map_err(to_py_err)
}

#[pyfunction]
#[pyo3(signature = (root=None))]
fn py_profile_pool_list(root: Option<String>) -> PyResult<String> {
    let result = registry_from_root(root).list_pools().map_err(to_py_err)?;
    to_json_string(&result)
}

#[pyfunction]
#[pyo3(signature = (name, profile_ids, max_active=3, root=None))]
fn py_profile_pool_create(
    name: &str,
    profile_ids: Vec<String>,
    max_active: usize,
    root: Option<String>,
) -> PyResult<String> {
    let result =
        registry_from_root(root).create_pool(name, profile_ids, max_active).map_err(to_py_err)?;
    to_json_string(&result)
}

#[pyfunction]
#[pyo3(signature = (name, root=None))]
fn py_profile_pool_describe(name: &str, root: Option<String>) -> PyResult<String> {
    let result = registry_from_root(root).resolve_pool(name).map_err(to_py_err)?;
    to_json_string(&result)
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
    m.add_class::<PyCapturedResponse>()?;
    m.add_class::<PyResponseExpectation>()?;
    m.add_class::<PyAntibotVerdict>()?;
    m.add_class::<PyDownloadOutcome>()?;
    m.add_class::<PyDownloadCapture>()?;
    m.add_class::<PyScanReport>()?;
    m.add_class::<PyProfileHandle>()?;
    m.add_class::<PyManagedProfileSnapshot>()?;
    m.add_class::<PyManagedProfileSplit>()?;
    m.add_function(wrap_pyfunction!(py_list_profiles, m)?)?;
    m.add_function(wrap_pyfunction!(py_acquire_profile, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_registry_root, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_registry_list, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_registry_create, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_registry_describe, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_registry_clone, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_registry_snapshot, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_registry_split, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_registry_fork, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_registry_delete, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_pool_list, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_pool_create, m)?)?;
    m.add_function(wrap_pyfunction!(py_profile_pool_describe, m)?)?;
    m.add_function(wrap_pyfunction!(py_scan_file, m)?)?;
    m.add_function(wrap_pyfunction!(py_scan_bytes, m)?)?;
    let py = m.py();
    m.add("VoidCrawlError", py.get_type::<VoidCrawlError>())?;
    m.add("NavigationError", py.get_type::<NavigationError>())?;
    m.add("NavigationTimeoutError", py.get_type::<NavigationTimeoutError>())?;
    m.add("BrowserClosedError", py.get_type::<BrowserClosedError>())?;
    m.add("ResponseTimeoutError", py.get_type::<ResponseTimeoutError>())?;
    m.add("ProfileBusy", py.get_type::<ProfileBusy>())?;
    m.add("ChromeProfileBusy", py.get_type::<ChromeProfileBusy>())?;
    m.add("ProfileLeaseExpired", py.get_type::<ProfileLeaseExpired>())?;
    m.add("ProfileNotFound", py.get_type::<ProfileNotFound>())?;
    m.add("CaptchaDetected", py.get_type::<CaptchaDetected>())?;
    m.add("AntibotChallenge", py.get_type::<AntibotChallenge>())?;
    Ok(())
}
