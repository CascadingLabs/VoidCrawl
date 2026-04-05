//! `PyO3` bindings for `void_crawl_core`.
//!
//! Exposes `PyBrowserSession` and `PyPage` as Python classes with async methods
//! that bridge to Python's asyncio via `pyo3-async-runtimes`.

use std::{collections::HashMap, convert::Infallible, fmt, sync::Arc, time::Duration};

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
    BrowserMode, BrowserPool, BrowserSession, CookieParam, DeleteCookiesParams,
    DispatchKeyEventType, DispatchMouseEventType, MouseButton, Page, PageResponse, PoolConfig,
    PooledTab, StealthConfig, VoidCrawlError,
};

// ── Error conversion ────────────────────────────────────────────────────

#[allow(clippy::needless_pass_by_value)] // used as fn pointer in map_err(to_py_err)
fn to_py_err(e: VoidCrawlError) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// Wrapper so `Vec<u8>` converts to Python `bytes` instead of `list[int]`.
struct PyBytesResult(Vec<u8>);

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

// ── PageResponse ────────────────────────────────────────────────────────

/// Python-visible result of `Page.goto()` / `PooledTab.goto()`.
///
/// Attributes:
///     html (str): Full outer HTML after network idle.
///     url (str): Final URL after any redirects.
///     `status_code` (int | None): HTTP status of the last response, or
///         ``None`` when served from cache / service worker.
///     redirected (bool): ``True`` when at least one HTTP redirect occurred.
#[pyclass(name = "PageResponse")]
#[derive(Debug)]
pub struct PyPageResponse {
    #[pyo3(get)]
    pub html:        String,
    #[pyo3(get)]
    pub url:         String,
    #[pyo3(get)]
    pub status_code: Option<u16>,
    #[pyo3(get)]
    pub redirected:  bool,
}

#[pymethods]
impl PyPageResponse {
    fn __repr__(&self) -> String {
        format!(
            "PageResponse(url={:?}, status_code={:?}, redirected={}, html_len={})",
            self.url,
            self.status_code,
            self.redirected,
            self.html.len(),
        )
    }
}

impl From<PageResponse> for PyPageResponse {
    fn from(r: PageResponse) -> Self {
        Self {
            html:        r.html,
            url:         r.url,
            status_code: r.status_code,
            redirected:  r.redirected,
        }
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

async fn do_launch(
    inner: Arc<Mutex<Option<BrowserSession>>>,
    mode: BrowserMode,
    stealth_enabled: bool,
    no_sandbox: bool,
    proxy: Option<String>,
    chrome_executable: Option<String>,
    extra_args: Vec<String>,
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
    #[pyo3(signature = (url, timeout=30.0))]
    fn goto<'py>(&self, py: Python<'py>, url: String, timeout: f64) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(
            self,
            py,
            |page| page.goto_and_wait_for_idle(&url, Duration::from_secs_f64(timeout)),
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

    /// Take a PNG screenshot, returned as Python bytes.
    fn screenshot_png<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.screenshot_png(), |bytes| PyBytesResult(bytes))
    }

    /// Generate a PDF, returned as Python bytes.
    fn pdf_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_page_map!(self, py, |page| page.pdf_bytes(), |bytes| PyBytesResult(bytes))
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

    /// Wait until the DOM stabilises and exceeds `min_length` characters.
    ///
    /// Returns True if stabilised within timeout, False otherwise.
    /// Prevents redirect gates / loading stubs from being treated as content.
    #[pyo3(signature = (timeout=10.0, min_length=5000, stable_checks=5))]
    #[allow(deprecated)]
    fn wait_for_stable_dom<'py>(
        &self,
        py: Python<'py>,
        timeout: f64,
        min_length: usize,
        stable_checks: u32,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_page!(self, py, |page| page.wait_for_stable_dom(
            Duration::from_secs_f64(timeout),
            min_length,
            stable_checks,
        ))
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
    #[new]
    #[pyo3(signature = (*, headless=true, ws_url=None, stealth=true, no_sandbox=false, proxy=None, chrome_executable=None, extra_args=None))]
    fn new(
        headless: bool,
        ws_url: Option<String>,
        stealth: bool,
        no_sandbox: bool,
        proxy: Option<String>,
        chrome_executable: Option<String>,
        extra_args: Option<Vec<String>>,
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

        future_into_py(py, async move {
            do_launch(
                inner,
                mode,
                stealth_enabled,
                no_sandbox,
                proxy,
                chrome_executable,
                extra_args,
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
        let (inner, mode, stealth_enabled, no_sandbox, proxy, chrome_executable, extra_args) = {
            let this = slf.borrow();
            (
                Arc::clone(&this.inner),
                this.mode.clone(),
                this.stealth_enabled,
                this.no_sandbox,
                this.proxy.clone(),
                this.chrome_executable.clone(),
                this.extra_args.clone(),
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
    #[pyo3(signature = (url, timeout=30.0))]
    fn goto<'py>(&self, py: Python<'py>, url: String, timeout: f64) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(
            self,
            py,
            |page| page.goto_and_wait_for_idle(&url, Duration::from_secs_f64(timeout)),
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

    fn screenshot_png<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page_map!(self, py, |page| page.screenshot_png(), |bytes| PyBytesResult(bytes))
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

    /// Wait until the DOM stabilises and exceeds `min_length` characters.
    ///
    /// Returns True if stabilised within timeout, False otherwise.
    #[pyo3(signature = (timeout=10.0, min_length=5000, stable_checks=5))]
    #[allow(deprecated)]
    fn wait_for_stable_dom<'py>(
        &self,
        py: Python<'py>,
        timeout: f64,
        min_length: usize,
        stable_checks: u32,
    ) -> PyResult<Bound<'py, PyAny>> {
        with_pooled_page!(self, py, |page| page.wait_for_stable_dom(
            Duration::from_secs_f64(timeout),
            min_length,
            stable_checks,
        ))
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
        auto_evict, headless, no_sandbox, stealth, ws_urls, proxy, chrome_executable, extra_args
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
    Ok(())
}
