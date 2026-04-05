//! High-level wrapper around a `chromiumoxide::Page`.

use std::{collections::HashMap, time::Duration};

use chromiumoxide::{
    Page as CdpPage,
    cdp::browser_protocol::{
        emulation::{SetDeviceMetricsOverrideParams, SetUserAgentOverrideParams},
        input::{
            DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams,
            DispatchMouseEventType, MouseButton,
        },
        network::{
            Cookie, CookieParam, DeleteCookiesParams, EventResponseReceived, Headers, ResourceType,
            SetExtraHttpHeadersParams,
        },
        page::{
            AddScriptToEvaluateOnNewDocumentParams, CaptureScreenshotFormat, EventLifecycleEvent,
            PrintToPdfParams, SetBypassCspParams,
        },
    },
    page::ScreenshotParams,
};
use futures::StreamExt;
use serde_json::Value;
use tokio::time;

use crate::{
    error::{Result, VoidCrawlError},
    stealth::StealthConfig,
};

/// The result of a [`Page::goto_and_wait_for_idle`] call.
///
/// Bundles the final HTML, URL, and HTTP response metadata captured during
/// navigation.  `status_code` is `None` when the page was served from a
/// service worker, disk cache, or the browser failed to capture a network
/// response (e.g. `file://` URLs).
#[derive(Debug, Clone)]
pub struct PageResponse {
    /// Outer HTML of `<html>` after the page reached network idle.
    pub html:        String,
    /// Final URL after any redirects.
    pub url:         String,
    /// HTTP status code of the last response in the navigation chain.
    pub status_code: Option<u16>,
    /// `true` when at least one HTTP redirect occurred before the final URL.
    pub redirected:  bool,
}

/// Thin wrapper over `chromiumoxide::Page` exposing a clean async API.
#[derive(Debug)]
pub struct Page {
    inner: CdpPage,
}

impl Page {
    /// Wrap an existing CDP page.
    pub(crate) fn new(inner: CdpPage) -> Self {
        Self { inner }
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

        // 2. User-agent override (only if stealth mode didn't already handle it)
        if !cfg.use_builtin_stealth {
            if let Some(ua) = &cfg.user_agent {
                let params = SetUserAgentOverrideParams::builder()
                    .user_agent(ua.clone())
                    .accept_language(&cfg.locale)
                    .platform("Win32")
                    .build()
                    .map_err(VoidCrawlError::PageError)?;
                self.inner
                    .execute(params)
                    .await
                    .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
            }
        }

        // 3. Viewport / device metrics
        let metrics = SetDeviceMetricsOverrideParams::new(
            i64::from(cfg.viewport_width),
            i64::from(cfg.viewport_height),
            1.0,
            false,
        );
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
        // Subscribe to BOTH event streams BEFORE navigation so no events slip
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

        // Start navigation (non-blocking CDP command)
        self.inner.goto(url).await.map_err(|e| VoidCrawlError::NavigationFailed(e.to_string()))?;

        let deadline = time::sleep(timeout);
        tokio::pin!(deadline);

        let mut status_code: Option<u16> = None;
        let mut redirect_count: u32 = 0;
        let mut got_almost_idle = false;

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
                            }
                            // A new Document response after networkAlmostIdle
                            // means a new navigation started; reset the flag so
                            // we don't exit on a stale almost-idle signal.
                            got_almost_idle = false;
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
                    return Ok(PageResponse {
                        html,
                        url: final_url,
                        status_code,
                        redirected: redirect_count > 0,
                    });
                }
            }
        }

        let html = self.content().await?;
        let final_url = self.url().await?.unwrap_or_default();
        Ok(PageResponse { html, url: final_url, status_code, redirected: redirect_count > 0 })
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

    /// Polling-based DOM stability check. Kept for backward compatibility
    /// with code that needs a content-length guard.
    #[deprecated(since = "0.2.0", note = "use wait_for_network_idle instead")]
    pub async fn wait_for_stable_dom(
        &self,
        timeout: Duration,
        min_length: usize,
        stable_checks: u32,
    ) -> Result<bool> {
        let deadline = time::Instant::now() + timeout;
        let poll_interval = Duration::from_millis(200);
        let mut previous_size: usize = 0;
        let mut stable_count: u32 = 0;

        while time::Instant::now() < deadline {
            let size: usize = usize::try_from(
                self.evaluate_js("document.body ? document.body.innerHTML.length : 0")
                    .await?
                    .as_u64()
                    .unwrap_or(0),
            )
            .unwrap_or(0);

            if size >= min_length && size == previous_size {
                stable_count += 1;
                if stable_count >= stable_checks {
                    return Ok(true);
                }
            } else {
                stable_count = 0;
                previous_size = size;
            }

            time::sleep(poll_interval).await;
        }

        Ok(false)
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

    // ── Screenshots & PDF ───────────────────────────────────────────────

    /// Capture a full-page PNG screenshot, returned as raw bytes.
    pub async fn screenshot_png(&self) -> Result<Vec<u8>> {
        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .full_page(true)
            .build();
        self.inner
            .screenshot(params)
            .await
            .map_err(|e| VoidCrawlError::ScreenshotError(e.to_string()))
    }

    /// Generate a PDF of the page, returned as raw bytes.
    pub async fn pdf_bytes(&self) -> Result<Vec<u8>> {
        let params = PrintToPdfParams::default();
        self.inner.pdf(params).await.map_err(|e| VoidCrawlError::PdfError(e.to_string()))
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
