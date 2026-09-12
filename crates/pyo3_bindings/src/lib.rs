//! `PyO3` bindings for `void_crawl_core`.
//!
//! Exposes `PyBrowserSession` and `PyPage` as Python classes with async methods
//! that bridge to Python's asyncio via `pyo3-async-runtimes`.

use std::{
    collections::HashMap,
    convert::Infallible,
    fmt, mem,
    path::Path,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures::future;
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict, PyList, PyType},
};
use pyo3_async_runtimes::tokio::future_into_py;

mod recording;
mod snapshots;

use recording::{
    PyFrame, PyMaskReport, PyRecordedRegion, PyRecording, PyRecordingHandle,
    build_recording_options, into_py_recording,
};
use serde_json::Value;
use snapshots::{
    PyAccessibilitySnapshot, PyLayoutSnapshot, PyRenderedDomSnapshot, PyVisualSnapshot,
    byte_report_dict,
};
use tokio::{sync::Mutex, task::spawn_blocking};
use void_crawl_core::{
    AccessibilitySnapshotOptions, AntibotEvidence, AntibotVerdict, BrowserMode, BrowserPool,
    BrowserSession, BrowserStateBinding, BrowserTarget, BrowserTargetKind, CapturedResponse,
    CdpMode, ContextCleanupReport, ContextDisposalState, CookieParam, DEFAULT_MAX_BYTES,
    DEFAULT_MAX_RESPONSE_BYTES, DEFAULT_MAX_TOTAL_RESPONSE_BYTES, DeleteCookiesParams,
    DispatchKeyEventType, DispatchMouseEventType, DownloadCapture, DownloadOutcome, InterruptInfo,
    InterruptRequest, IsolatedBrowserContext, MAX_PROFILE_SPLIT_COPIES, ManagedProfileSnapshot,
    MouseButton, NavigationCapture, NavigationCaptureOptions, NavigationCaptureReport,
    NavigationCaptureTermination, NetworkExtraInfoState, ObservationOptions, ObservationScope,
    Page, PageResponse, PoolConfig, PoolReleaseReport, PoolReleaseStrategy, PooledTab,
    ProfileHandle, ProfileInfo, ProfileRegistry, ResourceOutcome, ResponseCapture,
    ResponseCaptureLimits, ResponseCaptureReport, ResponseCaptureTermination, ScanConfig,
    ScanReport, ScrollTarget, StealthConfig, TabInstrumentationState, Verdict, Viewport,
    acquire_profile, list_profiles, scan_bytes, scan_path, viewport as viewport_mod,
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
pyo3::create_exception!(voidcrawl._ext, SessionInterrupted, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, InterruptExpired, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, InterruptTerminal, VoidCrawlError);
pyo3::create_exception!(voidcrawl._ext, InterruptNotFound, VoidCrawlError);

/// Resolve `(preset, width, height, device_scale_factor, mobile)` kwargs —
/// shared by `PyPage`/`PyPooledTab`'s `set_viewport` and `screenshot`
/// methods — into a `Viewport`. Pass either `preset` (a name from
/// `list_device_presets()`) or `width`+`height`; mixing them is an error.
/// This is the raw substrate: the pydantic `Viewport` model in
/// `voidcrawl/__init__.py` is where a Python caller gets enum-style
/// validation before a call ever reaches here.
pub(crate) fn resolve_viewport_args(
    preset: Option<&str>,
    width: Option<u32>,
    height: Option<u32>,
    device_scale_factor: Option<f64>,
    mobile: Option<bool>,
) -> PyResult<Viewport> {
    match (preset, width, height) {
        (Some(name), None, None) => viewport_mod::preset(name).ok_or_else(|| {
            PyValueError::new_err(format!(
                "unknown device preset {name:?}; call list_device_presets() for valid names"
            ))
        }),
        (None, Some(w), Some(h)) => {
            let mut vp = Viewport::custom(w, h);
            vp.device_scale_factor = device_scale_factor.unwrap_or(1.0);
            vp.mobile = mobile.unwrap_or(false);
            vp.has_touch = vp.mobile;
            Ok(vp)
        }
        (Some(_), _, _) => {
            Err(PyValueError::new_err("preset is mutually exclusive with width/height"))
        }
        (None, Some(_), None) | (None, None, Some(_)) => {
            Err(PyValueError::new_err("width and height must both be set together"))
        }
        (None, None, None) => Err(PyValueError::new_err("pass either preset= or width=+height=")),
    }
}

/// Resolve the raw `selector_*` kwargs accepted by Python screenshots into a
/// VoidCrawl-owned `BrowserTarget`. The flat kwargs remain compatible with
/// historic callers, while higher-level recipe types translate at their own
/// adapter boundary.
pub(crate) fn resolve_selector_args(
    kind: Option<&str>,
    value: Option<String>,
    regex: Option<String>,
    name: Option<String>,
    nth: Option<u32>,
    x: Option<f64>,
    y: Option<f64>,
) -> PyResult<BrowserTarget> {
    let kind = match kind {
        Some("css") => BrowserTargetKind::Css,
        Some("xpath") => BrowserTargetKind::Xpath,
        Some("regex") => BrowserTargetKind::Regex,
        Some("jsonld") => BrowserTargetKind::Jsonld,
        Some("attr") => BrowserTargetKind::Attr,
        Some("global_id") => BrowserTargetKind::GlobalId,
        Some("role") => BrowserTargetKind::Role,
        Some("visual") => BrowserTargetKind::Visual,
        Some(other) => {
            return Err(PyValueError::new_err(format!(
                "unknown selector_type {other:?}; expected one of css, xpath, regex, jsonld, \
                 attr, global_id, role, visual"
            )));
        }
        None => return Err(PyValueError::new_err("selector_type is required")),
    };
    Ok(BrowserTarget { kind, value: value.unwrap_or_default(), regex, name, nth, x, y })
}

/// Build a `ScreenshotOptions` from the raw kwargs `PyPage`/`PyPooledTab`'s
/// `screenshot()` accept. Shared so both bindings resolve `selector`/
/// `viewport`/`scroll`/`full_page` identically.
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn build_screenshot_options(
    path: Option<String>,
    bbox: Option<(u32, u32, u32, u32)>,
    selector_type: Option<&str>,
    selector_value: Option<String>,
    selector_regex: Option<String>,
    selector_name: Option<String>,
    selector_nth: Option<u32>,
    selector_x: Option<f64>,
    selector_y: Option<f64>,
    viewport_preset: Option<&str>,
    viewport_width: Option<u32>,
    viewport_height: Option<u32>,
    viewport_device_scale_factor: Option<f64>,
    viewport_mobile: Option<bool>,
    scroll_viewports: Option<f64>,
    scroll_pixels: Option<i64>,
    full_page: Option<bool>,
) -> PyResult<void_crawl_core::ScreenshotOptions> {
    if bbox.is_some() && selector_type.is_some() {
        return Err(PyValueError::new_err("bbox and selector_type are mutually exclusive"));
    }
    let mut opts = void_crawl_core::ScreenshotOptions::default();
    if let Some(p) = path {
        opts = opts.with_path(p);
    }
    if let Some((x, y, w, h)) = bbox {
        opts = opts.with_bbox(void_crawl_core::Bbox { x, y, width: w, height: h });
    }
    if selector_type.is_some() {
        let entry = resolve_selector_args(
            selector_type,
            selector_value,
            selector_regex,
            selector_name,
            selector_nth,
            selector_x,
            selector_y,
        )?;
        opts = opts.with_selector(entry);
    }
    if viewport_preset.is_some() || viewport_width.is_some() || viewport_height.is_some() {
        let vp = resolve_viewport_args(
            viewport_preset,
            viewport_width,
            viewport_height,
            viewport_device_scale_factor,
            viewport_mobile,
        )?;
        opts = opts.with_viewport(vp);
    }
    match (scroll_viewports, scroll_pixels) {
        (Some(n), None) => opts = opts.with_scroll(ScrollTarget::Viewports(n)),
        (None, Some(y)) => opts = opts.with_scroll(ScrollTarget::Pixels(y)),
        (Some(_), Some(_)) => {
            return Err(PyValueError::new_err(
                "scroll_viewports and scroll_pixels are mutually exclusive",
            ));
        }
        (None, None) => {}
    }
    if full_page == Some(false) {
        opts = opts.viewport_only();
    }
    Ok(opts)
}

fn safe_exception_url(url: &str) -> String {
    let head = url.split(['?', '#']).next().unwrap_or_default();
    let Some((scheme, rest)) = head.split_once("://") else {
        return "<unavailable>".into();
    };
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(host, path)| (host, path));
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    format!("{scheme}://{host}/{}", path.trim_start_matches('/'))
}

#[cfg(test)]
mod error_boundary_tests {
    use super::safe_exception_url;

    #[test]
    fn exception_url_omits_credentials_query_and_fragment() {
        assert_eq!(
            safe_exception_url(
                "https://user:secret@example.test/path?access_token=secret#fragment"
            ),
            "https://example.test/path"
        );
        assert_eq!(safe_exception_url("not a URL"), "<unavailable>");
    }
}

#[allow(clippy::needless_pass_by_value)] // used as fn pointer in map_err(to_py_err)
pub(crate) fn to_py_err(e: void_crawl_core::VoidCrawlError) -> PyErr {
    let summary = e.safe_summary();
    let message = summary.message;
    let py_err = match e {
        void_crawl_core::VoidCrawlError::InvalidInput { operation, reason } => {
            let err = PyValueError::new_err(message);
            Python::attach(|py| {
                let value = err.value(py);
                let _ = value.setattr("operation", operation);
                let _ = value.setattr("reason", reason);
            });
            err
        }
        void_crawl_core::VoidCrawlError::NavigationTimeout {
            ref url,
            ref wait_phase,
            timeout_secs,
            elapsed_secs,
        } => {
            let err = NavigationTimeoutError::new_err(message);
            Python::attach(|py| {
                let value = err.value(py);
                let _ = value.setattr("url", safe_exception_url(url));
                let _ = value.setattr("wait_phase", wait_phase);
                let _ = value.setattr("timeout", timeout_secs);
                let _ = value.setattr("elapsed", elapsed_secs);
            });
            err
        }
        void_crawl_core::VoidCrawlError::NavigationFailed(_) => NavigationError::new_err(message),
        void_crawl_core::VoidCrawlError::BrowserClosed => BrowserClosedError::new_err(message),
        void_crawl_core::VoidCrawlError::ResponseTimeout { .. } => {
            ResponseTimeoutError::new_err(message)
        }
        void_crawl_core::VoidCrawlError::ProfileBusy { ref name, pid, acquired_at } => {
            let err = ProfileBusy::new_err(message);
            Python::attach(|py| {
                let value = err.value(py);
                let _ = value.setattr("profile", name);
                let _ = value.setattr("owner_pid", pid);
                let _ = value.setattr("acquired_at", acquired_at);
            });
            err
        }
        void_crawl_core::VoidCrawlError::ProfileLeaseExpired { .. } => {
            ProfileLeaseExpired::new_err(message)
        }
        void_crawl_core::VoidCrawlError::ChromeProfileBusy { .. } => {
            ChromeProfileBusy::new_err(message)
        }
        void_crawl_core::VoidCrawlError::ProfileNotFound { .. } => {
            ProfileNotFound::new_err(message)
        }
        void_crawl_core::VoidCrawlError::CaptchaDetected { .. } => {
            CaptchaDetected::new_err(message)
        }
        void_crawl_core::VoidCrawlError::AntibotChallenge { .. } => {
            AntibotChallenge::new_err(message)
        }
        void_crawl_core::VoidCrawlError::SessionInterrupted { ref interrupt_id } => {
            let err = SessionInterrupted::new_err(message);
            Python::attach(|py| {
                let _ = err.value(py).setattr("interrupt_id", interrupt_id);
            });
            err
        }
        void_crawl_core::VoidCrawlError::InterruptExpired { .. } => {
            InterruptExpired::new_err(message)
        }
        void_crawl_core::VoidCrawlError::InterruptTerminal { .. } => {
            InterruptTerminal::new_err(message)
        }
        void_crawl_core::VoidCrawlError::InterruptNotFound { .. } => {
            InterruptNotFound::new_err(message)
        }
        _ => VoidCrawlError::new_err(message),
    };
    Python::attach(|py| {
        let value = py_err.value(py);
        let _ = value.setattr("code", summary.code.as_str());
        let _ = value.setattr("category", summary.category.as_str());
    });
    py_err
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
pub(crate) fn json_to_py(py: Python<'_>, val: Value) -> PyResult<Bound<'_, PyAny>> {
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

async fn environment_snapshot_value(page: &Page) -> void_crawl_core::Result<Value> {
    let snapshot = page.environment_snapshot().await?;
    serde_json::to_value(snapshot).map_err(|error| {
        void_crawl_core::VoidCrawlError::Other(format!(
            "serialize browser environment snapshot: {error}"
        ))
    })
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
    pub vendors: Vec<String>,
    #[pyo3(get)]
    pub challenged: bool,
    #[pyo3(get)]
    pub challenge_vendor: Option<String>,
    #[pyo3(get)]
    pub corpus_version: String,
    #[pyo3(get)]
    pub evidence: String,
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
            vendors: v.vendors,
            challenged: v.challenged,
            challenge_vendor: v.challenge_vendor,
            corpus_version: v.corpus_version.to_string(),
            evidence: evidence.to_string(),
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
            safe_exception_url(&self.url),
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

    /// Headers the browser SENT for this request, lowercased.
    ///
    /// This is where a request-side credential appears — an `Authorization`
    /// bearer set by page code. Unlike the MCP tools, these values are NOT
    /// redacted: an in-process caller is the intended holder of them. Do not
    /// log or persist them.
    ///
    /// Empty when Chrome reported none. Browser-managed `Cookie` is not among
    /// them (see `CapturedResponse::request_headers` in the core crate).
    #[getter]
    fn request_headers(&self) -> HashMap<String, String> {
        self.inner.request_headers.iter().cloned().collect()
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

    #[getter]
    fn byte_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        byte_report_dict(
            py,
            self.inner.byte_report().map_err(|error| PyValueError::new_err(error.to_string()))?,
        )
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
            safe_exception_url(&self.inner.url),
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

/// Owner of a response expectation.
///
/// Pooled expectations retain the checkout slot so ``__aexit__`` can hold the
/// lease while waiting and fail closed after release instead of observing a
/// recycled tab.
#[derive(Clone)]
enum ResponseExpectationOwner {
    Page(Arc<Mutex<Option<Arc<Page>>>>),
    PooledTab { tab: Arc<Mutex<Option<PooledTab>>>, active: Arc<AtomicUsize> },
}

/// Releases a pooled expectation count even when Python cancels ``__aexit__``.
struct ActiveResponseExpectationGuard(Arc<AtomicUsize>);

impl Drop for ActiveResponseExpectationGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Async expectation context returned by ``Page.expect_response(s)`` or
/// ``PooledTab.expect_response(s)``.
#[pyclass(name = "ResponseExpectation")]
pub struct PyResponseExpectation {
    owner: ResponseExpectationOwner,
    patterns: Vec<(String, String)>,
    timeout: Duration,
    limits: ResponseCaptureLimits,
    single: bool,
    lifecycle: Arc<Mutex<()>>,
    capture: Arc<Mutex<Option<ResponseCapture>>>,
    result: Arc<Mutex<Option<HashMap<String, CapturedResponse>>>>,
    report: Arc<Mutex<Option<ResponseCaptureReport>>>,
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
        Self::new_for_owner(
            ResponseExpectationOwner::Page(page),
            patterns,
            timeout,
            max_response_bytes,
            max_total_bytes,
            single,
        )
    }

    fn new_pooled(
        tab: Arc<Mutex<Option<PooledTab>>>,
        active: Arc<AtomicUsize>,
        patterns: Vec<(String, String)>,
        timeout: f64,
        max_response_bytes: usize,
        max_total_bytes: usize,
        single: bool,
    ) -> Self {
        Self::new_for_owner(
            ResponseExpectationOwner::PooledTab { tab, active },
            patterns,
            timeout,
            max_response_bytes,
            max_total_bytes,
            single,
        )
    }

    fn new_for_owner(
        owner: ResponseExpectationOwner,
        patterns: Vec<(String, String)>,
        timeout: f64,
        max_response_bytes: usize,
        max_total_bytes: usize,
        single: bool,
    ) -> Self {
        Self {
            owner,
            patterns,
            timeout: Duration::from_secs_f64(timeout),
            limits: ResponseCaptureLimits::new(
                void_crawl_core::BrowserByteLimit::try_from(max_response_bytes)
                    .unwrap_or(void_crawl_core::BrowserByteLimit::one()),
                void_crawl_core::BrowserByteLimit::try_from(max_total_bytes)
                    .unwrap_or(void_crawl_core::BrowserByteLimit::one()),
            ),
            single,
            lifecycle: Arc::new(Mutex::new(())),
            capture: Arc::new(Mutex::new(None)),
            result: Arc::new(Mutex::new(None)),
            report: Arc::new(Mutex::new(None)),
        }
    }
}

#[pymethods]
impl PyResponseExpectation {
    fn __aenter__<'py>(slf: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (owner, patterns, timeout, limits, lifecycle, capture_slot) = {
            let this = slf.borrow();
            (
                this.owner.clone(),
                this.patterns.clone(),
                this.timeout,
                this.limits,
                Arc::clone(&this.lifecycle),
                Arc::clone(&this.capture),
            )
        };
        let slf_ref = slf.into_any().unbind();
        future_into_py(py, async move {
            // Serialize enter/exit on this expectation so its tab and capture
            // locks can never be acquired in opposite order by concurrent misuse.
            let _lifecycle = lifecycle.lock().await;
            let capture = match &owner {
                ResponseExpectationOwner::Page(page_slot) => {
                    let page = page_slot
                        .lock()
                        .await
                        .as_ref()
                        .cloned()
                        .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
                    page.expect_responses(patterns, timeout, limits).await.map_err(to_py_err)?
                }
                ResponseExpectationOwner::PooledTab { tab: tab_slot, active } => {
                    let tab = tab_slot.lock().await;
                    let tab = tab
                        .as_ref()
                        .ok_or_else(|| PyRuntimeError::new_err("tab has been released"))?;
                    let capture = tab
                        .page
                        .expect_responses(patterns, timeout, limits)
                        .await
                        .map_err(to_py_err)?;
                    active.fetch_add(1, Ordering::AcqRel);
                    capture
                }
            };
            let mut slot = capture_slot.lock().await;
            if slot.is_some() {
                if let ResponseExpectationOwner::PooledTab { active, .. } = &owner {
                    active.fetch_sub(1, Ordering::AcqRel);
                }
                return Err(PyRuntimeError::new_err("response expectation was already entered"));
            }
            *slot = Some(capture);
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
        let owner = self.owner.clone();
        let lifecycle = Arc::clone(&self.lifecycle);
        let capture_slot = Arc::clone(&self.capture);
        let result_slot = Arc::clone(&self.result);
        let report_slot = Arc::clone(&self.report);
        let patterns =
            self.patterns.iter().map(|(name, pattern)| format!("{name}={pattern}")).collect();
        let timeout = self.timeout;
        future_into_py(py, async move {
            let _lifecycle = lifecycle.lock().await;
            let capture = capture_slot.lock().await.take();
            if failed {
                let was_entered = capture.is_some();
                drop(capture);
                if let (true, ResponseExpectationOwner::PooledTab { active, .. }) =
                    (was_entered, owner)
                {
                    active.fetch_sub(1, Ordering::AcqRel);
                }
                return Ok(false);
            }
            let Some(capture) = capture else {
                if report_slot.lock().await.is_some() {
                    return Ok(false);
                }
                return Err(PyRuntimeError::new_err("response expectation was not entered"));
            };
            // Once the capture leaves its slot, cancellation drops the Rust future.
            // Keep decrement ownership in an RAII guard so a cancelled Python task
            // cannot permanently poison release of the borrowed tab.
            let _active_guard = match &owner {
                ResponseExpectationOwner::PooledTab { active, .. } => {
                    Some(ActiveResponseExpectationGuard(Arc::clone(active)))
                }
                ResponseExpectationOwner::Page(_) => None,
            };
            let report = match owner {
                ResponseExpectationOwner::Page(_) => {
                    capture.wait_report().await.map_err(to_py_err)?
                }
                ResponseExpectationOwner::PooledTab { tab: tab_slot, .. } => {
                    // Hold the checkout slot while waiting so the pool cannot release,
                    // reset, or lend this tab to another caller mid-expectation.
                    let tab = tab_slot.lock().await;
                    if tab.is_none() {
                        return Err(PyRuntimeError::new_err("tab has been released"));
                    }
                    capture.wait_report().await.map_err(to_py_err)?
                }
            };
            *result_slot.lock().await = Some(report.responses.clone());
            *report_slot.lock().await = Some(report.clone());
            match report.termination {
                ResponseCaptureTermination::Complete => Ok(false),
                ResponseCaptureTermination::DeadlineReached => {
                    Err(to_py_err(void_crawl_core::VoidCrawlError::ResponseTimeout {
                        patterns,
                        timeout_secs: timeout.as_secs_f64(),
                    }))
                }
                ResponseCaptureTermination::Cancelled => Err(to_py_err(
                    void_crawl_core::VoidCrawlError::Other("response capture cancelled".into()),
                )),
                ResponseCaptureTermination::ProviderDisconnected => {
                    Err(to_py_err(void_crawl_core::VoidCrawlError::BrowserClosed))
                }
            }
        })
    }

    /// Wait for a terminal report without turning timeout/disconnect into an
    /// exception. The expectation must first be armed with ``async with``.
    fn wait_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.consume_report(py, false)
    }

    /// Cancel an armed expectation and return the partial terminal report.
    fn cancel_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.consume_report(py, true)
    }

    #[getter]
    fn report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let report = Arc::clone(&self.report);
        future_into_py(py, async move {
            let report =
                report.lock().await.clone().ok_or_else(|| {
                    PyRuntimeError::new_err("response expectation has not completed")
                })?;
            Python::attach(|py| Py::new(py, PyResponseCaptureReport { report }).map(Py::into_any))
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

impl PyResponseExpectation {
    fn consume_report<'py>(&self, py: Python<'py>, cancel: bool) -> PyResult<Bound<'py, PyAny>> {
        let owner = self.owner.clone();
        let lifecycle = Arc::clone(&self.lifecycle);
        let capture_slot = Arc::clone(&self.capture);
        let result_slot = Arc::clone(&self.result);
        let report_slot = Arc::clone(&self.report);
        future_into_py(py, async move {
            let _lifecycle = lifecycle.lock().await;
            let capture = capture_slot.lock().await.take().ok_or_else(|| {
                PyRuntimeError::new_err("response expectation was not entered or already consumed")
            })?;
            let _active_guard = match &owner {
                ResponseExpectationOwner::PooledTab { active, .. } => {
                    Some(ActiveResponseExpectationGuard(Arc::clone(active)))
                }
                ResponseExpectationOwner::Page(_) => None,
            };
            let report = match owner {
                ResponseExpectationOwner::Page(_) if cancel => capture.cancel_report().await,
                ResponseExpectationOwner::Page(_) => capture.wait_report().await,
                ResponseExpectationOwner::PooledTab { tab, .. } if cancel => {
                    let tab = tab.lock().await;
                    if tab.is_none() {
                        return Err(PyRuntimeError::new_err("tab has been released"));
                    }
                    capture.cancel_report().await
                }
                ResponseExpectationOwner::PooledTab { tab, .. } => {
                    let tab = tab.lock().await;
                    if tab.is_none() {
                        return Err(PyRuntimeError::new_err("tab has been released"));
                    }
                    capture.wait_report().await
                }
            }
            .map_err(to_py_err)?;
            *result_slot.lock().await = Some(report.responses.clone());
            *report_slot.lock().await = Some(report.clone());
            Python::attach(|py| Py::new(py, PyResponseCaptureReport { report }).map(Py::into_any))
        })
    }
}

fn response_termination_name(termination: ResponseCaptureTermination) -> &'static str {
    match termination {
        ResponseCaptureTermination::Complete => "complete",
        ResponseCaptureTermination::DeadlineReached => "deadline_reached",
        ResponseCaptureTermination::Cancelled => "cancelled",
        ResponseCaptureTermination::ProviderDisconnected => "provider_disconnected",
    }
}

/// Terminal response-capture result. Unlike the legacy expectation context,
/// terminal states preserve every response captured before the stop.
#[pyclass(name = "ResponseCaptureReport", frozen, skip_from_py_object)]
#[derive(Debug, Clone)]
pub struct PyResponseCaptureReport {
    report: ResponseCaptureReport,
}

#[pymethods]
impl PyResponseCaptureReport {
    #[getter]
    fn termination(&self) -> &'static str {
        response_termination_name(self.report.termination)
    }

    #[getter]
    fn responses<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let dict = PyDict::new(py);
        for (name, response) in &self.report.responses {
            dict.set_item(name, Py::new(py, PyCapturedResponse::from(response.clone()))?)?;
        }
        Ok(dict.into_any())
    }

    #[getter]
    fn byte_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        byte_report_dict(py, self.report.aggregate_bytes.clone())
    }

    fn __repr__(&self) -> String {
        format!(
            "ResponseCaptureReport(termination={:?}, responses={})",
            response_termination_name(self.report.termination),
            self.report.responses.len(),
        )
    }
}

fn navigation_termination_name(termination: NavigationCaptureTermination) -> &'static str {
    match termination {
        NavigationCaptureTermination::Finished => "finished",
        NavigationCaptureTermination::Cancelled => "cancelled",
        NavigationCaptureTermination::DeadlineReached => "deadline_reached",
        NavigationCaptureTermination::EventLimitReached => "event_limit_reached",
        NavigationCaptureTermination::ProviderDisconnected => "provider_disconnected",
    }
}

fn resource_outcome_value(outcome: ResourceOutcome) -> Value {
    match outcome {
        ResourceOutcome::Pending => serde_json::json!({ "status": "pending" }),
        ResourceOutcome::ResponseReceived => {
            serde_json::json!({ "status": "response_received" })
        }
        ResourceOutcome::Redirected => serde_json::json!({ "status": "redirected" }),
        ResourceOutcome::Complete => serde_json::json!({ "status": "complete" }),
        ResourceOutcome::Failed { cancelled, blocked } => serde_json::json!({
            "status": "failed",
            "cancelled": cancelled,
            "blocked": blocked,
        }),
    }
}

/// Python-visible terminal browser navigation capture.
#[pyclass(name = "NavigationCaptureReport", frozen)]
pub struct PyNavigationCaptureReport {
    report: NavigationCaptureReport,
}

impl fmt::Debug for PyNavigationCaptureReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NavigationCaptureReport")
            .field("termination", &self.report.termination)
            .field("resources", &self.report.resources.len())
            .field("redirects", &self.report.redirects.len())
            .field(
                "source_bytes",
                &self.report.main_document.as_ref().map(|source| source.retained_bytes),
            )
            .finish_non_exhaustive()
    }
}

#[pymethods]
impl PyNavigationCaptureReport {
    #[getter]
    fn termination(&self) -> &'static str {
        navigation_termination_name(self.report.termination)
    }

    #[getter]
    fn started_at_unix_ms(&self) -> Option<u64> {
        self.report.started_at_unix_ms
    }

    #[getter]
    fn elapsed_micros(&self) -> u64 {
        self.report.elapsed_micros
    }

    #[getter]
    fn events_admitted(&self) -> u64 {
        self.report.events_admitted
    }

    #[getter]
    fn resources_dropped(&self) -> u64 {
        self.report.resources_dropped
    }

    #[getter]
    fn additional_loss_unknown(&self) -> bool {
        self.report.additional_loss_unknown
    }

    #[getter]
    fn cleanup_complete(&self) -> bool {
        self.report.cleanup_complete
    }

    #[getter]
    fn network_extra_info(&self) -> &'static str {
        match self.report.network_extra_info {
            NetworkExtraInfoState::UnavailableInCurrentClient => "unavailable_in_current_client",
        }
    }

    #[getter]
    fn requested_url(&self) -> Option<String> {
        self.report.requested_url.as_ref().map(|url| url.as_str().to_string())
    }

    #[getter]
    fn final_url(&self) -> Option<String> {
        self.report.final_url.as_ref().map(|url| url.as_str().to_string())
    }

    #[getter]
    fn redirect_count(&self) -> usize {
        self.report.redirects.len()
    }

    #[getter]
    fn resource_count(&self) -> usize {
        self.report.resources.len()
    }

    #[getter]
    fn source_status(&self) -> Option<u16> {
        self.report.main_document.as_ref().and_then(|source| source.status)
    }

    #[getter]
    fn source_body_state(&self) -> Option<&'static str> {
        self.report.main_document.as_ref().map(|source| source.body_state.as_str())
    }

    #[getter]
    fn source_retained_bytes(&self) -> Option<usize> {
        self.report.main_document.as_ref().map(|source| source.retained_bytes)
    }

    #[getter]
    fn source_complete_bytes(&self) -> Option<usize> {
        self.report.main_document.as_ref().and_then(|source| source.complete_bytes)
    }

    #[getter]
    fn source_byte_report<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.report
            .main_document
            .as_ref()
            .map(|source| {
                byte_report_dict(
                    py,
                    source
                        .byte_report()
                        .map_err(|error| PyValueError::new_err(error.to_string()))?,
                )
            })
            .transpose()
    }

    fn source_body<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.report.main_document.as_ref().map(|source| PyBytes::new(py, source.body()))
    }

    fn source_header_names(&self) -> Vec<String> {
        self.report
            .main_document
            .as_ref()
            .map(|source| source.headers.as_slice().iter().map(|(name, _)| name.clone()).collect())
            .unwrap_or_default()
    }

    #[pyo3(signature = (*, include_urls=false))]
    fn resources<'py>(&self, py: Python<'py>, include_urls: bool) -> PyResult<Bound<'py, PyAny>> {
        let resources = self
            .report
            .resources
            .iter()
            .map(|resource| {
                let mut value = serde_json::json!({
                    "id": resource.id.0,
                    "redirect_from": resource.redirect_from.map(|id| id.0),
                    "frame": resource.frame.map(|id| id.0),
                    "loader": resource.loader.map(|id| id.0),
                    "resource_type": resource.resource_type,
                    "status": resource.status,
                    "mime_type": resource.mime_type,
                    "from_cache": resource.from_cache,
                    "from_service_worker": resource.from_service_worker,
                    "encoded_data_length": resource.encoded_data_length,
                    "outcome": resource_outcome_value(resource.outcome),
                });
                if include_urls {
                    value["url"] = Value::String(resource.url.as_str().to_string());
                }
                value
            })
            .collect();
        json_to_py(py, Value::Array(resources))
    }

    fn __repr__(&self) -> String {
        format!(
            "NavigationCaptureReport(termination={:?}, resources={}, redirects={}, source_bytes={:?})",
            navigation_termination_name(self.report.termination),
            self.report.resources.len(),
            self.report.redirects.len(),
            self.report.main_document.as_ref().map(|source| source.retained_bytes),
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum NavigationCaptureAction {
    Finish,
    Cancel,
    Wait,
}

/// Armed main-document source and resource-graph capture.
#[pyclass(name = "NavigationCapture")]
pub struct PyNavigationCapture {
    inner: Arc<Mutex<Option<NavigationCapture>>>,
}

impl fmt::Debug for PyNavigationCapture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("NavigationCapture").finish_non_exhaustive()
    }
}

impl PyNavigationCapture {
    fn new(capture: NavigationCapture) -> Self {
        Self { inner: Arc::new(Mutex::new(Some(capture))) }
    }

    fn consume<'py>(
        &self,
        py: Python<'py>,
        action: NavigationCaptureAction,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let capture =
                inner.lock().await.take().ok_or_else(|| {
                    PyRuntimeError::new_err("navigation capture already consumed")
                })?;
            let report = match action {
                NavigationCaptureAction::Finish => capture.finish().await,
                NavigationCaptureAction::Cancel => capture.cancel().await,
                NavigationCaptureAction::Wait => capture.wait().await,
            }
            .map_err(to_py_err)?;
            Python::attach(|py| Py::new(py, PyNavigationCaptureReport { report }))
        })
    }
}

#[pymethods]
impl PyNavigationCapture {
    fn finish<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.consume(py, NavigationCaptureAction::Finish)
    }

    fn cancel<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.consume(py, NavigationCaptureAction::Cancel)
    }

    fn wait<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.consume(py, NavigationCaptureAction::Wait)
    }
}

#[derive(Debug, Clone, Copy)]
enum ObservationStopAction {
    Finish,
    Cancel,
    Interrupt,
    Wait,
}

/// Armed, bounded CDP lifecycle observation.
#[pyclass(name = "ObservationScope")]
pub struct PyObservationScope {
    inner: Arc<Mutex<Option<ObservationScope>>>,
}

impl fmt::Debug for PyObservationScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ObservationScope").finish_non_exhaustive()
    }
}

impl PyObservationScope {
    fn new(scope: ObservationScope) -> Self {
        Self { inner: Arc::new(Mutex::new(Some(scope))) }
    }

    fn consume<'py>(
        &self,
        py: Python<'py>,
        action: ObservationStopAction,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let scope = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("observation scope already consumed"))?;
            let report = match action {
                ObservationStopAction::Finish => scope.finish().await,
                ObservationStopAction::Cancel => scope.cancel().await,
                ObservationStopAction::Interrupt => scope.interrupt().await,
                ObservationStopAction::Wait => scope.wait().await,
            }
            .map_err(to_py_err)?;
            let mut value = serde_json::to_value(&report).map_err(|error| {
                PyRuntimeError::new_err(format!("serialize observation report: {error}"))
            })?;
            let byte_report = serde_json::to_value(report.byte_report().map_err(|error| {
                PyRuntimeError::new_err(format!("observation byte report: {error}"))
            })?)
            .map_err(|error| PyRuntimeError::new_err(format!("serialize byte report: {error}")))?;
            value
                .as_object_mut()
                .ok_or_else(|| {
                    PyRuntimeError::new_err("observation report must serialize as object")
                })?
                .insert("byte_report".to_string(), byte_report);
            Ok(PyJsonValue(value))
        })
    }
}

#[pymethods]
impl PyObservationScope {
    /// Stop normally and return the terminal report.
    fn finish<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.consume(py, ObservationStopAction::Finish)
    }

    /// Stop as an explicit caller cancellation and return the terminal report.
    fn cancel<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.consume(py, ObservationStopAction::Cancel)
    }

    /// Stop as an explicit interruption and return the terminal report.
    fn interrupt<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.consume(py, ObservationStopAction::Interrupt)
    }

    /// Wait for the deadline, event limit, or provider disconnect.
    fn wait<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.consume(py, ObservationStopAction::Wait)
    }
}

/// Per-tab CDP instrumentation state for routing sensitive vs instrumented
/// work.
#[pyclass(name = "TabInstrumentationState", frozen)]
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Python state snapshot mirrors core routing flags"
)]
pub struct PyTabInstrumentationState {
    #[pyo3(get)]
    pub low_cdp: bool,
    #[pyo3(get)]
    pub network_enabled: bool,
    #[pyo3(get)]
    pub runtime_enabled: bool,
    #[pyo3(get)]
    pub utility_world_enabled: bool,
    #[pyo3(get)]
    pub pre_navigation_stealth: bool,
}

#[pymethods]
impl PyTabInstrumentationState {
    fn __repr__(&self) -> String {
        format!(
            "TabInstrumentationState(low_cdp={}, network_enabled={}, runtime_enabled={}, utility_world_enabled={}, pre_navigation_stealth={})",
            self.low_cdp,
            self.network_enabled,
            self.runtime_enabled,
            self.utility_world_enabled,
            self.pre_navigation_stealth,
        )
    }
}

impl From<TabInstrumentationState> for PyTabInstrumentationState {
    fn from(state: TabInstrumentationState) -> Self {
        Self {
            low_cdp: state.low_cdp,
            network_enabled: state.network_enabled,
            runtime_enabled: state.runtime_enabled,
            utility_world_enabled: state.utility_world_enabled,
            pre_navigation_stealth: state.pre_navigation_stealth,
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
    pub path: String,
    #[pyo3(get)]
    pub bytes: u64,
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
        Self { path: o.path.display().to_string(), bytes: o.bytes, content_type: o.content_type }
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
    pub verdict: String,
    #[pyo3(get)]
    pub reason: Option<String>,
    #[pyo3(get)]
    pub detected_mime: Option<String>,
    #[pyo3(get)]
    pub size: u64,
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

/// Parse the Python-facing `cdp_mode` string. `None` leaves the core default
/// (which honors `VOIDCRAWL_STEALTH_NO_RUNTIME`) in place.
fn parse_cdp_mode(mode: Option<&str>) -> PyResult<Option<CdpMode>> {
    match mode {
        None => Ok(None),
        Some("normal") => Ok(Some(CdpMode::Normal)),
        Some("minimal") => Ok(Some(CdpMode::Minimal)),
        Some(other) => Err(PyValueError::new_err(format!(
            "cdp_mode must be 'normal' or 'minimal', got {other:?}"
        ))),
    }
}

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
    cdp_mode: Option<CdpMode>,
) -> PyResult<()> {
    let stealth =
        if stealth_enabled { StealthConfig::chrome_like() } else { StealthConfig::none() };

    let mut builder = BrowserSession::builder().mode(mode).stealth(stealth);

    if let Some(m) = cdp_mode {
        builder = builder.cdp_mode(m);
    }
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

fn state_binding_name(binding: BrowserStateBinding) -> &'static str {
    match binding {
        BrowserStateBinding::SharedBrowserProfile => "shared_browser_profile",
        BrowserStateBinding::IsolatedBrowserContext => "isolated_browser_context",
        BrowserStateBinding::ManagedProfile => "managed_profile",
        BrowserStateBinding::AttachedBrowser => "attached_browser",
    }
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
        Self::from_arc(Arc::new(page))
    }

    fn from_arc(page: Arc<Page>) -> Self {
        Self { inner: Arc::new(Mutex::new(Some(page))) }
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

    /// Arm bounded main-document source and resource-graph capture.
    #[pyo3(signature = (*, max_events=4096, max_resources=512, max_source_bytes=8_388_608, max_duration=30.0))]
    fn arm_navigation_capture<'py>(
        &self,
        py: Python<'py>,
        max_events: usize,
        max_resources: usize,
        max_source_bytes: usize,
        max_duration: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        if max_events == 0 || max_resources == 0 || max_source_bytes == 0 {
            return Err(PyValueError::new_err(
                "event, resource, and source-byte limits must be greater than zero",
            ));
        }
        let max_duration = Duration::try_from_secs_f64(max_duration).map_err(|_| {
            PyValueError::new_err("max_duration must be finite and greater than zero")
        })?;
        if max_duration.is_zero() {
            return Err(PyValueError::new_err("max_duration must be greater than zero"));
        }
        let options =
            NavigationCaptureOptions { max_events, max_resources, max_source_bytes, max_duration };
        with_page_map!(self, py, |page| page.arm_navigation_capture(options), |capture| {
            PyNavigationCapture::new(capture)
        })
    }

    /// Arm bounded CDP lifecycle markers before navigation or another action.
    #[pyo3(signature = (*, collect_network=true, collect_console=true, collect_exceptions=true, max_events=2048, max_diagnostic_bytes=65_536, max_duration=30.0))]
    #[expect(clippy::too_many_arguments, reason = "Python keyword surface mirrors bounded options")]
    fn arm_observation<'py>(
        &self,
        py: Python<'py>,
        collect_network: bool,
        collect_console: bool,
        collect_exceptions: bool,
        max_events: usize,
        max_diagnostic_bytes: usize,
        max_duration: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        if !(collect_network || collect_console || collect_exceptions) {
            return Err(PyValueError::new_err(
                "at least one observation collector must be enabled",
            ));
        }
        if max_events == 0 || max_diagnostic_bytes == 0 {
            return Err(PyValueError::new_err(
                "max_events and max_diagnostic_bytes must be greater than zero",
            ));
        }
        let max_duration = Duration::try_from_secs_f64(max_duration).map_err(|_| {
            PyValueError::new_err("max_duration must be finite and greater than zero")
        })?;
        if max_duration.is_zero() {
            return Err(PyValueError::new_err("max_duration must be greater than zero"));
        }
        let options = ObservationOptions {
            collect_network,
            collect_console,
            collect_exceptions,
            max_events,
            max_diagnostic_bytes,
            max_duration,
        };
        with_page_map!(self, py, |page| page.arm_observation(options), |scope| {
            PyObservationScope::new(scope)
        })
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

    /// Capture bounded rendered-DOM bytes with document scope metadata.
    #[pyo3(signature = (max_bytes=8_388_608))]
    fn rendered_dom_snapshot<'py>(
        &self,
        py: Python<'py>,
        max_bytes: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.rendered_dom_snapshot(max_bytes), |snapshot| {
            PyRenderedDomSnapshot::from(snapshot)
        })
    }

    /// Capture bounded raw accessibility-tree bytes for the top-level frame.
    #[pyo3(signature = (depth=None, max_nodes=10_000, max_bytes=8_388_608))]
    fn accessibility_snapshot<'py>(
        &self,
        py: Python<'py>,
        depth: Option<i64>,
        max_nodes: usize,
        max_bytes: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let options = AccessibilitySnapshotOptions { depth, max_nodes, max_bytes };
        with_page_map!(self, py, |page| page.accessibility_snapshot(options), |snapshot| {
            PyAccessibilitySnapshot::from(snapshot)
        })
    }

    /// Capture bounded raw accessibility-tree bytes for one matching frame.
    #[pyo3(signature = (frame_url_pattern, depth=None, max_nodes=10_000, max_bytes=8_388_608))]
    fn accessibility_snapshot_in_frame<'py>(
        &self,
        py: Python<'py>,
        frame_url_pattern: String,
        depth: Option<i64>,
        max_nodes: usize,
        max_bytes: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let options = AccessibilitySnapshotOptions { depth, max_nodes, max_bytes };
        with_page_map!(
            self,
            py,
            |page| page.accessibility_snapshot_in_frame(&frame_url_pattern, options),
            |snapshot| PyAccessibilitySnapshot::from(snapshot)
        )
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

    /// Mutable browser-state boundary this page belongs to.
    fn state_binding<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| async { Ok(page.state_binding()) }, |binding| {
            state_binding_name(binding).to_string()
        })
    }

    /// Return this tab's CDP instrumentation state for routing/debugging.
    fn instrumentation_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let state = page.instrumentation_state();
            inner.lock().await.replace(page);
            Ok(PyTabInstrumentationState::from(state))
        })
    }

    /// Observe effective browser environment and active capture primitives.
    fn environment_snapshot<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| environment_snapshot_value(page), |value| PyJsonValue(
            value
        ))
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
    /// internal automation callers.
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

    /// Capture CSS layout/visual viewport and content-size metrics.
    fn layout_snapshot<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.layout_snapshot(), |snapshot| {
            PyLayoutSnapshot::from(snapshot)
        })
    }

    /// Take a PNG screenshot, returned as Python bytes.
    fn screenshot_png<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.screenshot_png(), |bytes| PyBytesResult(bytes))
    }

    /// Take a PNG screenshot with optional disk output, cropping, a one-shot
    /// device/viewport override, scrolling, and/or viewport-only capture.
    ///
    /// Args:
    ///     path: If set, writes PNG to this path and returns the path as a
    ///         string. If omitted, returns raw bytes.
    ///     bbox: Optional ``(x, y, width, height)`` in CSS pixels to crop.
    ///         With ``scroll_viewports``/``scroll_pixels`` set, ``x``/``y``
    ///         are relative to wherever that scroll lands. Mutually
    ///         exclusive with ``selector_type``.
    ///     selector_type: Crop to a browser target's resolved rectangle
    ///         instead of an explicit ``bbox`` — one of ``"css"``,
    ///         ``"xpath"``, ``"regex"``, ``"jsonld"``, ``"attr"``,
    ///         ``"global_id"``, ``"role"``, ``"visual"``. Mutually
    ///         exclusive with ``bbox``. A selector that matches nothing, is
    ///         ambiguous, or is inherently non-visual (``jsonld``/
    ///         ``regex``) raises rather than silently cropping an
    ///         arbitrary target. Prefer building a validated
    ///         ``voidcrawl.viewport``-style pydantic model on the Python
    ///         side and unpacking its fields here (this layer does no
    ///         enum/mutual-exclusivity validation beyond parsing the type).
    ///     selector_value: CSS selector / XPath expression, depending on
    ///         ``selector_type`` (unused for ``role``/``visual``/``jsonld``/
    ///         ``regex``, which use ``name``/``x``/``y``/``regex`` instead).
    ///     selector_regex: Regex pattern (``selector_type="regex"`` only —
    ///         currently always resolves to "empty"; not cropped).
    ///     selector_name: Accessible name (``role``), attribute name
    ///         (``attr`` — metadata only, not part of the DOM query), or
    ///         id-prefix filter (``global_id``).
    ///     selector_nth: 0-based index to disambiguate when a selector
    ///         matches more than one visible target.
    ///     selector_x, selector_y: CSS-pixel point (``selector_type="visual"``
    ///         only) — resolves to an exact 1x1 box.
    ///     viewport_preset: Named device (see :func:`list_device_presets`),
    ///         e.g. ``"iPhone 16 Pro Max"``. Mutually exclusive with
    ///         ``viewport_width``/``viewport_height``. One-shot: restores
    ///         whatever viewport was active before, even on error.
    ///     viewport_width, viewport_height: Custom one-shot viewport size in
    ///         CSS pixels. Both required together.
    ///     viewport_device_scale_factor: DPR for a custom viewport
    ///         (default 1.0). Ignored with ``viewport_preset``.
    ///     viewport_mobile: Emulate a mobile viewport for a custom size —
    ///         also enables touch (default ``False``). Ignored with
    ///         ``viewport_preset``.
    ///     scroll_viewports: Scroll to N viewport-heights from the top
    ///         before capturing (``2.0`` = "scrolled down twice"). Mutually
    ///         exclusive with ``scroll_pixels``. Restored after capture.
    ///     scroll_pixels: Scroll to an absolute pixel Y before capturing.
    ///     full_page: Capture the full scrollable page (default ``True``).
    ///         Pass ``False`` to capture only the visible viewport. Ignored
    ///         when ``bbox``/``selector_type`` is set.
    #[pyo3(signature = (
        path=None, bbox=None,
        selector_type=None, selector_value=None, selector_regex=None, selector_name=None,
        selector_nth=None, selector_x=None, selector_y=None,
        viewport_preset=None, viewport_width=None, viewport_height=None,
        viewport_device_scale_factor=None, viewport_mobile=None,
        scroll_viewports=None, scroll_pixels=None, full_page=None,
    ))]
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    fn screenshot<'py>(
        &self,
        py: Python<'py>,
        path: Option<String>,
        bbox: Option<(u32, u32, u32, u32)>,
        selector_type: Option<String>,
        selector_value: Option<String>,
        selector_regex: Option<String>,
        selector_name: Option<String>,
        selector_nth: Option<u32>,
        selector_x: Option<f64>,
        selector_y: Option<f64>,
        viewport_preset: Option<String>,
        viewport_width: Option<u32>,
        viewport_height: Option<u32>,
        viewport_device_scale_factor: Option<f64>,
        viewport_mobile: Option<bool>,
        scroll_viewports: Option<f64>,
        scroll_pixels: Option<i64>,
        full_page: Option<bool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_screenshot_options(
            path,
            bbox,
            selector_type.as_deref(),
            selector_value,
            selector_regex,
            selector_name,
            selector_nth,
            selector_x,
            selector_y,
            viewport_preset.as_deref(),
            viewport_width,
            viewport_height,
            viewport_device_scale_factor,
            viewport_mobile,
            scroll_viewports,
            scroll_pixels,
            full_page,
        )?;
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let result = page.screenshot(opts).await.map_err(to_py_err)?;
            Ok(PyScreenshotOutput(result))
        })
    }

    /// Capture PNG bytes paired with provider-native visual metadata.
    #[pyo3(signature = (
        bbox=None,
        selector_type=None, selector_value=None, selector_regex=None, selector_name=None,
        selector_nth=None, selector_x=None, selector_y=None,
        viewport_preset=None, viewport_width=None, viewport_height=None,
        viewport_device_scale_factor=None, viewport_mobile=None,
        scroll_viewports=None, scroll_pixels=None, full_page=None,
    ))]
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    fn visual_snapshot<'py>(
        &self,
        py: Python<'py>,
        bbox: Option<(u32, u32, u32, u32)>,
        selector_type: Option<String>,
        selector_value: Option<String>,
        selector_regex: Option<String>,
        selector_name: Option<String>,
        selector_nth: Option<u32>,
        selector_x: Option<f64>,
        selector_y: Option<f64>,
        viewport_preset: Option<String>,
        viewport_width: Option<u32>,
        viewport_height: Option<u32>,
        viewport_device_scale_factor: Option<f64>,
        viewport_mobile: Option<bool>,
        scroll_viewports: Option<f64>,
        scroll_pixels: Option<i64>,
        full_page: Option<bool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_screenshot_options(
            None,
            bbox,
            selector_type.as_deref(),
            selector_value,
            selector_regex,
            selector_name,
            selector_nth,
            selector_x,
            selector_y,
            viewport_preset.as_deref(),
            viewport_width,
            viewport_height,
            viewport_device_scale_factor,
            viewport_mobile,
            scroll_viewports,
            scroll_pixels,
            full_page,
        )?;
        with_page_map!(self, py, |page| page.visual_snapshot(opts), |snapshot| {
            PyVisualSnapshot::from(snapshot)
        })
    }

    /// Record this page for ``duration_secs`` and return a
    /// :class:`Recording`.
    ///
    /// The moving-picture counterpart to :meth:`screenshot`, with the same
    /// ``viewport_*`` / ``scroll_*`` / ``bbox`` kwargs. Two differences,
    /// both forced by CDP's screencast:
    ///
    /// * No ``full_page`` — a screencast only ever contains the viewport. Use
    ///   ``viewport_*`` for a bigger visible area, or ``scroll_*`` to choose
    ///   which part of a long page is on screen.
    /// * ``selectors`` is a **list**: each entry becomes its own cropped region
    ///   in ``recording.regions``, all cut from one screencast. Each is
    ///   resolved to a rectangle once, at start, then held fixed.
    ///
    /// Frames arrive when Chrome paints, not on a clock, so ``fps`` is a
    /// ceiling rather than a guarantee and a static page yields very few
    /// frames. Every frame carries its real ``offset_ms``.
    ///
    /// Args:
    ///     duration_secs: How long to record (default 30).
    ///     selectors: List of ``BrowserTarget``-shaped dicts to crop
    ///         to, one region each. Mutually exclusive with ``bbox``.
    ///     bbox: ``(x, y, width, height)`` in CSS pixels, **viewport
    ///         relative** (unlike :meth:`screenshot`'s page-relative bbox,
    ///         since a screencast frame only contains the viewport).
    ///     masks: Rectangles to paint solid black in every frame, before
    ///         anything is cropped, written, or encoded. Each entry is
    ///         either a selector dict (``{"type": "css", "value":
    ///         "#password"}``) or ``{"bbox": (x, y, w, h)}`` /
    ///         ``{"selector": {...}, "track": False, "label": "pw"}``.
    ///         Orthogonal to ``bbox``/``selectors``: crop to the form and
    ///         mask a field inside it. Unlike a crop region, a selector mask
    ///         is re-resolved while recording, so it keeps covering an
    ///         element that moves; a selector that resolves to nothing fails
    ///         the call rather than leaving a hole. What each mask actually
    ///         did is reported in ``recording.masks``.
    ///
    ///         This is a geometric primitive, not a redaction policy: it
    ///         covers exactly what you name and reports what it covered. It
    ///         does not decide what is sensitive, so a masked recording is
    ///         not thereby a safe-to-share one.
    ///     mask_pad: Outward padding in CSS pixels on every mask, to swallow
    ///         antialiasing at the edges (default 2). Set 0 for the exact
    ///         rectangle.
    ///     fps: Frame-rate ceiling (default 10).
    ///     frame_format: ``"jpeg"`` (default) or ``"png"``.
    ///     quality: JPEG quality 1-100 (default 80).
    ///     output_dir: Directory for encoded artifacts and, with
    ///         ``write_frames=True``, the frames themselves.
    ///     encode: List of ``"gif"`` / ``"mp4"`` / ``"webm"``. Requires
    ///         ``output_dir``, and the matching cargo feature at build time —
    ///         otherwise this raises rather than silently producing
    ///         nothing. The frames are always available regardless.
    ///     foreground: Force whether to pin the tab to the foreground and
    ///         hold the browser-wide capture lock. Leave unset to detect it:
    ///         a tab sharing its window must be foregrounded to paint at
    ///         all, while a tab alone in its window records at full rate
    ///         concurrently with everything else.
    ///     max_frames: In-memory frame cap (default 900).
    #[pyo3(signature = (duration_secs=None, selectors=None, bbox=None, masks=None,
        mask_pad=None, viewport_preset=None,
        viewport_width=None, viewport_height=None, viewport_device_scale_factor=None,
        viewport_mobile=None, scroll_viewports=None, scroll_pixels=None, fps=None,
        max_frames=None, frame_format=None, quality=None, output_dir=None, write_frames=None,
        foreground=None, encode=None))]
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    fn record<'py>(
        &self,
        py: Python<'py>,
        duration_secs: Option<f64>,
        selectors: Option<Vec<Py<PyAny>>>,
        bbox: Option<(u32, u32, u32, u32)>,
        masks: Option<Vec<Py<PyAny>>>,
        mask_pad: Option<u32>,
        viewport_preset: Option<String>,
        viewport_width: Option<u32>,
        viewport_height: Option<u32>,
        viewport_device_scale_factor: Option<f64>,
        viewport_mobile: Option<bool>,
        scroll_viewports: Option<f64>,
        scroll_pixels: Option<i64>,
        fps: Option<u8>,
        max_frames: Option<usize>,
        frame_format: Option<String>,
        quality: Option<u8>,
        output_dir: Option<String>,
        write_frames: Option<bool>,
        foreground: Option<bool>,
        encode: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_recording_options(
            output_dir,
            bbox,
            selectors,
            masks,
            mask_pad,
            viewport_preset.as_deref(),
            viewport_width,
            viewport_height,
            viewport_device_scale_factor,
            viewport_mobile,
            scroll_viewports,
            scroll_pixels,
            fps,
            duration_secs,
            max_frames,
            frame_format.as_deref(),
            quality,
            write_frames,
            foreground,
            encode,
            py,
        )?;
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let recording = page.record(opts).await.map_err(to_py_err)?;
            Python::attach(|py| into_py_recording(py, recording))
        })
    }

    /// Begin recording and return a :class:`RecordingHandle` to stop it.
    ///
    /// Use this instead of :meth:`record` when you need to *drive* the page
    /// while it records — click, type, navigate, then ``await
    /// handle.stop()``. Takes the same kwargs as :meth:`record`, where
    /// ``duration_secs`` becomes a hard upper bound rather than the exact
    /// length.
    #[pyo3(signature = (duration_secs=None, selectors=None, bbox=None, masks=None,
        mask_pad=None, viewport_preset=None,
        viewport_width=None, viewport_height=None, viewport_device_scale_factor=None,
        viewport_mobile=None, scroll_viewports=None, scroll_pixels=None, fps=None,
        max_frames=None, frame_format=None, quality=None, output_dir=None, write_frames=None,
        foreground=None, encode=None))]
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    fn start_recording<'py>(
        &self,
        py: Python<'py>,
        duration_secs: Option<f64>,
        selectors: Option<Vec<Py<PyAny>>>,
        bbox: Option<(u32, u32, u32, u32)>,
        masks: Option<Vec<Py<PyAny>>>,
        mask_pad: Option<u32>,
        viewport_preset: Option<String>,
        viewport_width: Option<u32>,
        viewport_height: Option<u32>,
        viewport_device_scale_factor: Option<f64>,
        viewport_mobile: Option<bool>,
        scroll_viewports: Option<f64>,
        scroll_pixels: Option<i64>,
        fps: Option<u8>,
        max_frames: Option<usize>,
        frame_format: Option<String>,
        quality: Option<u8>,
        output_dir: Option<String>,
        write_frames: Option<bool>,
        foreground: Option<bool>,
        encode: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_recording_options(
            output_dir,
            bbox,
            selectors,
            masks,
            mask_pad,
            viewport_preset.as_deref(),
            viewport_width,
            viewport_height,
            viewport_device_scale_factor,
            viewport_mobile,
            scroll_viewports,
            scroll_pixels,
            fps,
            duration_secs,
            max_frames,
            frame_format.as_deref(),
            quality,
            write_frames,
            foreground,
            encode,
            py,
        )?;
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let page = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let handle = page.start_recording(opts).await.map_err(to_py_err)?;
            Ok(PyRecordingHandle::new(handle, page))
        })
    }

    /// Whether this tab is the only one in its browser window.
    ///
    /// Chrome composites only a window's frontmost tab, so a page sharing
    /// its window can't paint while a sibling is active. A page alone in its
    /// window keeps painting regardless — which is what lets :meth:`record`
    /// run concurrently without holding the browser's capture lock.
    fn alone_in_window<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.alone_in_window(), |v| v)
    }

    /// Persistently override this page's CDP viewport — dimensions, DPR,
    /// mobile/touch identity, and (for a preset) a matching UA. Stays in
    /// effect across subsequent navigate/click/screenshot calls until
    /// :meth:`clear_viewport` or another `set_viewport` call. For a
    /// one-off override scoped to a single capture, pass ``viewport_*``
    /// kwargs to :meth:`screenshot` instead.
    ///
    /// Args:
    ///     preset: Named device (see :func:`list_device_presets`).
    ///         Mutually exclusive with ``width``/``height``.
    ///     width, height: Custom viewport size in CSS pixels.
    ///     device_scale_factor: DPR for a custom viewport (default 1.0).
    ///     mobile: Emulate a mobile viewport for a custom size (default
    ///         ``False``; also enables touch).
    #[pyo3(signature = (preset=None, width=None, height=None, device_scale_factor=None, mobile=None))]
    #[allow(clippy::needless_pass_by_value)]
    fn set_viewport<'py>(
        &self,
        py: Python<'py>,
        preset: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        device_scale_factor: Option<f64>,
        mobile: Option<bool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let viewport =
            resolve_viewport_args(preset.as_deref(), width, height, device_scale_factor, mobile)?;
        with_page_map!(self, py, |page| page.set_viewport(viewport), |_r| ())
    }

    /// Clear a :meth:`set_viewport` override, returning to the session's
    /// launch-time default viewport.
    fn clear_viewport<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.clear_viewport(), |_r| ())
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

// ── Interrupt results ───────────────────────────────────────────────────

/// Redacted state returned after a page is explicitly interrupted, resumed,
/// or released. This contains no CDP endpoint, cookies, or credentials.
#[pyclass(name = "InterruptInfo")]
#[derive(Debug)]
pub struct PyInterruptInfo {
    #[pyo3(get)]
    interrupt_id: String,
    #[pyo3(get)]
    target_id: String,
    #[pyo3(get)]
    code: String,
    #[pyo3(get)]
    summary: String,
    #[pyo3(get)]
    state: String,
    #[pyo3(get)]
    expires_in_ms: u64,
}

impl From<InterruptInfo> for PyInterruptInfo {
    fn from(info: InterruptInfo) -> Self {
        #[allow(clippy::cast_possible_truncation)]
        let expires_in_ms = info.expires_in.as_millis().min(u128::from(u64::MAX)) as u64;
        Self {
            interrupt_id: info.interrupt_id,
            target_id: info.target_id,
            code: info.code,
            summary: info.summary,
            state: info.state.as_str().into(),
            expires_in_ms,
        }
    }
}

// ── Isolated browser context ────────────────────────────────────────────

#[pyclass(name = "ContextCleanupReport", frozen)]
#[derive(Debug)]
pub struct PyContextCleanupReport {
    #[pyo3(get)]
    state_binding: String,
    #[pyo3(get)]
    disposal_state: String,
    #[pyo3(get)]
    cleanup_complete: bool,
}

impl From<ContextCleanupReport> for PyContextCleanupReport {
    fn from(report: ContextCleanupReport) -> Self {
        let disposal_state = match report.disposal_state {
            ContextDisposalState::Disposed => "disposed",
            ContextDisposalState::ProviderDisconnected => "provider_disconnected",
            ContextDisposalState::ProviderRejected => "provider_rejected",
        };
        Self {
            state_binding: state_binding_name(report.state_binding).to_string(),
            disposal_state: disposal_state.to_string(),
            cleanup_complete: report.cleanup_complete,
        }
    }
}

/// Disposable Chromium browser context with a single initial page.
#[pyclass(name = "IsolatedBrowserContext")]
pub struct PyIsolatedBrowserContext {
    inner: Arc<Mutex<Option<IsolatedBrowserContext>>>,
    page: Arc<Page>,
}

impl fmt::Debug for PyIsolatedBrowserContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IsolatedBrowserContext").finish_non_exhaustive()
    }
}

impl PyIsolatedBrowserContext {
    fn new(context: IsolatedBrowserContext) -> Self {
        let page = context.page_handle();
        Self { inner: Arc::new(Mutex::new(Some(context))), page }
    }
}

#[pymethods]
impl PyIsolatedBrowserContext {
    /// Page owned by this context. It becomes unusable after disposal.
    fn page(&self) -> PyPage {
        PyPage::from_arc(Arc::clone(&self.page))
    }

    #[getter]
    fn state_binding(&self) -> &'static str {
        state_binding_name(self.page.state_binding())
    }

    /// Dispose all pages and mutable state in this context.
    fn dispose<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let context =
                inner.lock().await.take().ok_or_else(|| {
                    PyRuntimeError::new_err("isolated context is already disposed")
                })?;
            Ok(PyContextCleanupReport::from(context.dispose().await))
        })
    }

    fn __aenter__<'py>(slf: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let slf_ref = slf.into_any().unbind();
        future_into_py(py, async move { Ok(slf_ref) })
    }

    #[pyo3(signature = (exc_type=None, _exc_val=None, _exc_tb=None))]
    #[expect(clippy::needless_pass_by_value, reason = "PyO3 extracts exception arguments by value")]
    fn __aexit__<'py>(
        &self,
        py: Python<'py>,
        exc_type: Option<Bound<'py, PyAny>>,
        _exc_val: Option<Bound<'py, PyAny>>,
        _exc_tb: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let had_exception = exc_type.is_some();
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let context = inner.lock().await.take();
            if let Some(context) = context {
                let report = context.dispose().await;
                if !report.cleanup_complete && !had_exception {
                    return Err(PyRuntimeError::new_err(format!(
                        "isolated context cleanup failed: {:?}",
                        report.disposal_state
                    )));
                }
            }
            Ok(false)
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
    inner: Arc<Mutex<Option<Arc<BrowserSession>>>>,
    mode: BrowserMode,
    stealth_enabled: bool,
    no_sandbox: bool,
    proxy: Option<String>,
    chrome_executable: Option<String>,
    extra_args: Vec<String>,
    user_data_dir: Option<String>,
    port: Option<u16>,
    cdp_mode: Option<CdpMode>,
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
    ///     `cdp_mode`: `"normal"` (default) or `"minimal"`. `"minimal"` skips
    /// the         eager `Runtime`/`Network`/`Performance`/`Log` domain
    /// enables that         make a CDP browser detectable, which is what
    /// lets a session clear a         Cloudflare Managed Challenge. It is a
    /// trade: response capture,         `wait_for_network_idle`,
    /// cross-origin frame eval, and OOPIF         auto-attach are
    /// unavailable in that mode. `None` keeps the default         (and
    /// still honors `VOIDCRAWL_STEALTH_NO_RUNTIME`).
    #[new]
    #[pyo3(signature = (*, headless=true, ws_url=None, stealth=true, no_sandbox=false, proxy=None, chrome_executable=None, extra_args=None, user_data_dir=None, port=None, cdp_mode=None))]
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
        cdp_mode: Option<&str>,
    ) -> PyResult<Self> {
        let mode = if let Some(url) = ws_url {
            BrowserMode::RemoteDebug { ws_url: url }
        } else if headless {
            BrowserMode::Headless
        } else {
            BrowserMode::Headful
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(None)),
            mode,
            stealth_enabled: stealth,
            no_sandbox,
            proxy,
            chrome_executable,
            extra_args: extra_args.unwrap_or_default(),
            user_data_dir,
            port,
            cdp_mode: parse_cdp_mode(cdp_mode)?,
        })
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
        let cdp_mode = self.cdp_mode;

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
                cdp_mode,
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

    /// Create a fresh disposable Chromium browser context.
    fn new_isolated_context<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner.lock().await.as_ref().cloned().ok_or_else(|| {
                PyRuntimeError::new_err(
                    "browser not launched — use `async with` or call launch() first",
                )
            })?;
            let context = session.new_isolated_context().await.map_err(to_py_err)?;
            Ok(PyIsolatedBrowserContext::new(context))
        })
    }

    /// Mutable-state boundary used by ordinary pages from this session.
    fn state_binding<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner.lock().await.as_ref().cloned().ok_or_else(|| {
                PyRuntimeError::new_err(
                    "browser not launched — use `async with` or call launch() first",
                )
            })?;
            Ok(state_binding_name(session.state_binding()).to_string())
        })
    }

    /// Open a new tab in its **own browser window** and navigate to ``url``.
    ///
    /// Chrome composites only the frontmost tab of a window, so tabs from
    /// :meth:`new_page` — which share one window — can't all paint at once.
    /// A tab alone in its window keeps painting whatever other windows do,
    /// which is what lets :meth:`Page.record` run concurrently instead of
    /// holding the browser's capture lock.
    ///
    /// Costs a real window's worth of resources, so it's opt-in. Note that a
    /// later :meth:`new_page` targets the most recently active window and can
    /// land inside this one — create recording windows last, or check
    /// :meth:`Page.alone_in_window`.
    fn new_page_in_window<'py>(&self, py: Python<'py>, url: String) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner.lock().await.as_ref().cloned().ok_or_else(|| {
                PyRuntimeError::new_err(
                    "browser not launched — use `async with` or call launch() first",
                )
            })?;
            let page = session.new_page_in_window(&url).await.map_err(to_py_err)?;
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

    /// Explicitly park one page for external operator review. This uses the
    /// existing page handle; it neither re-attaches the target nor navigates.
    #[pyo3(signature = (page, code, summary, ttl_seconds=600))]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "PyO3 requires extracting a PyRef argument by value"
    )]
    fn interrupt<'py>(
        &self,
        py: Python<'py>,
        page: PyRef<'py, PyPage>,
        code: String,
        summary: String,
        ttl_seconds: u64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        let page_inner = Arc::clone(&page.inner);
        future_into_py(py, async move {
            let session = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("browser not launched"))?;
            let page = page_inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("page is closed"))?;
            let info = session
                .interrupt_page(
                    page.as_ref(),
                    InterruptRequest { code, summary, ttl: Duration::from_secs(ttl_seconds) },
                )
                .await
                .map_err(to_py_err)?;
            Ok(PyInterruptInfo::from(info))
        })
    }

    /// Reactivate the page associated with an interrupt ID. This never
    /// navigates or replays the action that caused the interruption.
    fn resume<'py>(&self, py: Python<'py>, interrupt_id: String) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("browser not launched"))?;
            session
                .resume_interrupt(&interrupt_id)
                .await
                .map(PyInterruptInfo::from)
                .map_err(to_py_err)
        })
    }

    /// Mark an interrupt released without replaying any browser action.
    fn release<'py>(&self, py: Python<'py>, interrupt_id: String) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        future_into_py(py, async move {
            let session = inner
                .lock()
                .await
                .as_ref()
                .cloned()
                .ok_or_else(|| PyRuntimeError::new_err("browser not launched"))?;
            session
                .release_interrupt(&interrupt_id)
                .await
                .map(PyInterruptInfo::from)
                .map_err(to_py_err)
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
            cdp_mode,
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
                this.cdp_mode,
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
                cdp_mode,
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
    inner: Arc<Mutex<Option<PooledTab>>>,
    active_response_expectations: Arc<AtomicUsize>,
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
        Ok(PyResponseExpectation::new_pooled(
            Arc::clone(&self.inner),
            Arc::clone(&self.active_response_expectations),
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
        Ok(PyResponseExpectation::new_pooled(
            Arc::clone(&self.inner),
            Arc::clone(&self.active_response_expectations),
            patterns.into_iter().collect(),
            timeout,
            max_response_bytes,
            max_total_bytes,
            false,
        ))
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

    /// Mutable browser-state boundary. Pooled tabs are never isolated.
    fn state_binding<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(
            self,
            py,
            |page| async move { Ok::<_, void_crawl_core::VoidCrawlError>(page.state_binding()) },
            |binding| state_binding_name(binding).to_string()
        )
    }

    /// Return this tab's CDP instrumentation state for routing/debugging.
    fn instrumentation_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(
            self,
            py,
            |page| async move { Ok::<_, void_crawl_core::VoidCrawlError>(page.instrumentation_state()) },
            |state| PyTabInstrumentationState::from(state)
        )
    }

    /// Observe effective browser environment and active capture primitives.
    fn environment_snapshot<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(self, py, |page| environment_snapshot_value(page), |value| {
            PyJsonValue(value)
        })
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

    /// Take a PNG screenshot with optional disk output, cropping, a one-shot
    /// device/viewport override, scrolling, and/or viewport-only capture.
    /// See :meth:`Page.screenshot` for the full argument reference.
    #[pyo3(signature = (
        path=None, bbox=None,
        selector_type=None, selector_value=None, selector_regex=None, selector_name=None,
        selector_nth=None, selector_x=None, selector_y=None,
        viewport_preset=None, viewport_width=None, viewport_height=None,
        viewport_device_scale_factor=None, viewport_mobile=None,
        scroll_viewports=None, scroll_pixels=None, full_page=None,
    ))]
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    fn screenshot<'py>(
        &self,
        py: Python<'py>,
        path: Option<String>,
        bbox: Option<(u32, u32, u32, u32)>,
        selector_type: Option<String>,
        selector_value: Option<String>,
        selector_regex: Option<String>,
        selector_name: Option<String>,
        selector_nth: Option<u32>,
        selector_x: Option<f64>,
        selector_y: Option<f64>,
        viewport_preset: Option<String>,
        viewport_width: Option<u32>,
        viewport_height: Option<u32>,
        viewport_device_scale_factor: Option<f64>,
        viewport_mobile: Option<bool>,
        scroll_viewports: Option<f64>,
        scroll_pixels: Option<i64>,
        full_page: Option<bool>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_screenshot_options(
            path,
            bbox,
            selector_type.as_deref(),
            selector_value,
            selector_regex,
            selector_name,
            selector_nth,
            selector_x,
            selector_y,
            viewport_preset.as_deref(),
            viewport_width,
            viewport_height,
            viewport_device_scale_factor,
            viewport_mobile,
            scroll_viewports,
            scroll_pixels,
            full_page,
        )?;
        with_pooled_page_map!(self, py, |page| page.screenshot(opts), |result| PyScreenshotOutput(
            result
        ))
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
    pool: Arc<BrowserPool>,
    tab_slot: Arc<Mutex<Option<PooledTab>>>,
    active_response_expectations: Arc<AtomicUsize>,
}

impl fmt::Debug for PyAcquireContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("_AcquireContext").finish_non_exhaustive()
    }
}

#[pymethods]
impl PyAcquireContext {
    fn __aenter__<'py>(slf: &Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (pool, tab_slot, active_response_expectations) = {
            let this = slf.borrow();
            (
                Arc::clone(&this.pool),
                Arc::clone(&this.tab_slot),
                Arc::clone(&this.active_response_expectations),
            )
        };
        future_into_py(py, async move {
            let tab = pool.acquire().await.map_err(to_py_err)?;
            let use_count = tab.use_count;
            *tab_slot.lock().await = Some(tab);
            Ok(PyPooledTab { inner: tab_slot, active_response_expectations, use_count })
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
        let active_response_expectations = Arc::clone(&self.active_response_expectations);
        future_into_py(py, async move {
            let mut tab_slot = tab_slot.lock().await;
            if active_response_expectations.load(Ordering::Acquire) != 0 {
                return Err(PyRuntimeError::new_err(
                    "cannot release tab with an active response expectation",
                ));
            }
            let tab = tab_slot.take();
            drop(tab_slot);
            if let Some(tab) = tab {
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

#[pyclass(name = "PoolReleaseReport", frozen)]
#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "mirrors explicit core cleanup facts")]
pub struct PyPoolReleaseReport {
    #[pyo3(get)]
    state_binding: String,
    #[pyo3(get)]
    strategy: String,
    #[pyo3(get)]
    cleanup_complete: bool,
    #[pyo3(get)]
    tab_reused: bool,
    #[pyo3(get)]
    document_cleared: bool,
    #[pyo3(get)]
    download_behavior_reset: bool,
    #[pyo3(get)]
    shared_state_retained: bool,
}

impl From<PoolReleaseReport> for PyPoolReleaseReport {
    fn from(report: PoolReleaseReport) -> Self {
        let strategy = match report.strategy {
            PoolReleaseStrategy::BlankDocumentAndReuseSharedState => {
                "blank_document_and_reuse_shared_state"
            }
            PoolReleaseStrategy::DisposeTabAfterResetFailure => "dispose_tab_after_reset_failure",
            PoolReleaseStrategy::DisposeTabAfterPoolClosed => "dispose_tab_after_pool_closed",
        };
        Self {
            state_binding: state_binding_name(report.state_binding).to_string(),
            strategy: strategy.to_string(),
            cleanup_complete: report.cleanup_complete,
            tab_reused: report.tab_reused,
            document_cleared: report.document_cleared,
            download_behavior_reset: report.download_behavior_reset,
            shared_state_retained: report.shared_state_retained,
        }
    }
}

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
        PyAcquireContext {
            pool: Arc::clone(&self.inner),
            tab_slot: Arc::new(Mutex::new(None)),
            active_response_expectations: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Return a context manager that builds a pool from explicit parameters.
    ///
    /// Called by the Python `BrowserPool(config)` wrapper — not part of the
    /// public Python API.
    #[classmethod]
    #[pyo3(signature = (
        browsers, tabs_per_browser, tab_max_uses, tab_max_idle_secs, acquire_timeout_secs,
        auto_evict, headless, no_sandbox, stealth, ws_urls, proxy, chrome_executable, extra_args,
        user_data_dir, cdp_mode=None
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
        cdp_mode: Option<&str>,
    ) -> PyResult<PyPoolParamsContext> {
        let cdp_mode = parse_cdp_mode(cdp_mode)?;
        Ok(PyPoolParamsContext {
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
            cdp_mode,
            pool_slot: Arc::new(Mutex::new(None)),
        })
    }

    /// Return a tab to the pool.
    fn release<'py>(
        &self,
        py: Python<'py>,
        tab: &Bound<'py, PyPooledTab>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let pool = Arc::clone(&self.inner);
        let borrowed = tab.borrow();
        let tab_inner = Arc::clone(&borrowed.inner);
        let active_response_expectations = Arc::clone(&borrowed.active_response_expectations);
        drop(borrowed);
        future_into_py(py, async move {
            let mut guard = tab_inner.lock().await;
            if active_response_expectations.load(Ordering::Acquire) != 0 {
                return Err(PyRuntimeError::new_err(
                    "cannot release tab with an active response expectation",
                ));
            }
            let pooled_tab = guard.take();
            drop(guard);
            let report = match pooled_tab {
                Some(pooled_tab) => {
                    Some(PyPoolReleaseReport::from(pool.release_checked(pooled_tab).await))
                }
                None => None,
            };
            Ok(report)
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
    cdp_mode: Option<CdpMode>,
    pool_slot: Arc<Mutex<Option<Arc<BrowserPool>>>>,
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
        let cdp_mode = this.cdp_mode;
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
                        if let Some(m) = cdp_mode {
                            builder = builder.cdp_mode(m);
                        }
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
                        let mut builder =
                            BrowserSession::builder().remote_debug(url).stealth(stealth.clone());
                        if let Some(m) = cdp_mode {
                            builder = builder.cdp_mode(m);
                        }
                        builder.launch()
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
        if self.armed
            && let Ok(mut state) = self.state.lock()
            && matches!(*state, ProfileSplitState::Preparing)
        {
            *state = ProfileSplitState::Ready;
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
    root: Option<String>,
    copies: usize,
    source: ProfileSplitSource,
    state: Arc<StdMutex<ProfileSplitState>>,
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
    name: String,
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

/// List named device presets (phones, tablets, desktop sizes) available to
/// `Page.set_viewport` / `Page.screenshot(viewport_preset=...)` — Chrome
/// DevTools' device-toolbar dropdown, as data. Returns
/// ``(name, width, height, device_scale_factor, mobile)`` tuples.
#[pyfunction]
#[pyo3(name = "list_device_presets")]
fn py_list_device_presets() -> Vec<(String, u32, u32, f64, bool)> {
    viewport_mod::all_presets()
        .into_iter()
        .map(|(name, vp)| {
            (name.to_string(), vp.width, vp.height, vp.device_scale_factor, vp.mobile)
        })
        .collect()
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
    m.add_class::<PyIsolatedBrowserContext>()?;
    m.add_class::<PyContextCleanupReport>()?;
    m.add_class::<PyPage>()?;
    m.add_class::<PyBrowserPool>()?;
    m.add_class::<PyPoolReleaseReport>()?;
    m.add_class::<PyPooledTab>()?;
    m.add_class::<PyAcquireContext>()?;
    m.add_class::<PyPoolContext>()?;
    m.add_class::<PyPoolParamsContext>()?;
    m.add_class::<PyPageResponse>()?;
    m.add_class::<PyCapturedResponse>()?;
    m.add_class::<PyResponseExpectation>()?;
    m.add_class::<PyResponseCaptureReport>()?;
    m.add_class::<PyObservationScope>()?;
    m.add_class::<PyNavigationCapture>()?;
    m.add_class::<PyNavigationCaptureReport>()?;
    m.add_class::<PyRenderedDomSnapshot>()?;
    m.add_class::<PyAccessibilitySnapshot>()?;
    m.add_class::<PyLayoutSnapshot>()?;
    m.add_class::<PyVisualSnapshot>()?;

    m.add_class::<PyTabInstrumentationState>()?;
    m.add_class::<PyAntibotVerdict>()?;
    m.add_class::<PyDownloadOutcome>()?;
    m.add_class::<PyDownloadCapture>()?;
    m.add_class::<PyScanReport>()?;
    m.add_class::<PyInterruptInfo>()?;
    m.add_class::<PyProfileHandle>()?;
    m.add_class::<PyManagedProfileSnapshot>()?;
    m.add_class::<PyManagedProfileSplit>()?;
    m.add_class::<PyRecording>()?;
    m.add_class::<PyRecordedRegion>()?;
    m.add_class::<PyMaskReport>()?;
    m.add_class::<PyFrame>()?;
    m.add_class::<PyRecordingHandle>()?;
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
    m.add_function(wrap_pyfunction!(py_list_device_presets, m)?)?;
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
    m.add("SessionInterrupted", py.get_type::<SessionInterrupted>())?;
    m.add("InterruptExpired", py.get_type::<InterruptExpired>())?;
    m.add("InterruptTerminal", py.get_type::<InterruptTerminal>())?;
    m.add("InterruptNotFound", py.get_type::<InterruptNotFound>())?;
    Ok(())
}
