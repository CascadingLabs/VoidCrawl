//! High-level wrapper around a `chromiumoxide::Page`.

use std::{
    collections::HashMap,
    fmt, fs,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chromiumoxide::{
    Page as CdpPage,
    cdp::{
        browser_protocol::{
            accessibility::{AxNode, GetFullAxTreeParams, QueryAxTreeParams},
            browser::{PermissionDescriptor, PermissionSetting, SetPermissionParams},
            dom::{GetDocumentParams, ResolveNodeParams},
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
                Cookie, CookieParam, DeleteCookiesParams, EventResponseReceived, Headers,
                ResourceType, SetExtraHttpHeadersParams,
            },
            page::{
                AddScriptToEvaluateOnNewDocumentParams, CaptureScreenshotFormat,
                EventLifecycleEvent, EventScreencastFrame, PrintToPdfParams,
                ScreencastFrameAckParams, SetBypassCspParams, StartScreencastFormat,
                StartScreencastParams, StopScreencastParams, Viewport,
            },
        },
        js_protocol::runtime::CallFunctionOnParams,
    },
    page::ScreenshotParams,
};
use futures::{FutureExt, StreamExt};
use serde_json::Value;
use tokio::{sync::oneshot, task::JoinHandle, time};

use crate::{
    ax::compact_outline,
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

/// Compression format for [`Page::start_screencast`] frames.
#[derive(Debug, Clone, Copy)]
pub enum ScreencastFormat {
    Jpeg,
    Png,
}

/// Options for [`Page::start_screencast`].
///
/// CDP screencast captures the **viewport** at a stable size (unlike a
/// full-page screenshot, whose height varies per page), so every frame is
/// the same dimensions — assembly downstream needs no per-frame normalization.
#[derive(Debug, Clone)]
pub struct ScreencastOptions {
    /// Frame image format. JPEG is smaller; PNG is lossless.
    pub format:          ScreencastFormat,
    /// JPEG compression quality, `0..=100`. Ignored for PNG.
    pub quality:         Option<u32>,
    /// Cap frame width in device pixels (Chrome scales down to fit).
    pub max_width:       Option<u32>,
    /// Cap frame height in device pixels.
    pub max_height:      Option<u32>,
    /// Only deliver every n-th frame, to throttle the stream.
    pub every_nth_frame: Option<u32>,
}

impl Default for ScreencastOptions {
    fn default() -> Self {
        Self {
            format:          ScreencastFormat::Jpeg,
            quality:         Some(80),
            max_width:       None,
            max_height:      None,
            every_nth_frame: None,
        }
    }
}

/// A single frame captured during a [`Screencast`].
#[derive(Debug, Clone)]
pub struct ScreencastFrame {
    /// Decoded image bytes (JPEG or PNG per [`ScreencastOptions::format`]).
    pub data:      Vec<u8>,
    /// CDP frame-swap time in seconds since the Unix epoch, when Chrome
    /// supplied one. Use the deltas between frames to drive playback timing.
    pub timestamp: Option<f64>,
}

/// An in-progress screencast.
///
/// Frames stream into an internal buffer on a background task from the moment
/// [`Page::start_screencast`] returns until [`Screencast::stop`] is called.
/// The capture runs on a **cloned** CDP page handle, so the originating
/// [`Page`] stays free to drive the session (navigate, click, inject overlays)
/// while recording — and concurrent screencasts on different pages never
/// interleave, since each subscribes to its own per-target event stream.
pub struct Screencast {
    page:   CdpPage,
    frames: Arc<Mutex<Vec<ScreencastFrame>>>,
    stop:   Option<oneshot::Sender<()>>,
    drain:  JoinHandle<()>,
}

impl fmt::Debug for Screencast {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let buffered = self.frames.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("Screencast").field("buffered_frames", &buffered).finish_non_exhaustive()
    }
}

impl Screencast {
    /// Stop the screencast and return every frame captured so far, in order.
    ///
    /// Tells Chrome to stop emitting frames, then drains any frames already
    /// queued before tearing down the background task — so the final frames
    /// of the session are never dropped.
    pub async fn stop(mut self) -> Result<Vec<ScreencastFrame>> {
        // Ask Chrome to stop first, so no new frames are produced while we
        // drain. A failure here is non-fatal: we still return what we have.
        let _ = self.page.execute(StopScreencastParams::default()).await;
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        // Let the drain task observe the stop signal and flush queued frames.
        let _ = (&mut self.drain).await;
        let frames = match Arc::try_unwrap(self.frames) {
            Ok(m) => m.into_inner().unwrap_or_else(PoisonError::into_inner),
            Err(arc) => arc.lock().map(|g| g.clone()).unwrap_or_default(),
        };
        Ok(frames)
    }
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

    /// Begin recording the page to a stream of image frames via CDP
    /// `Page.startScreencast`.
    ///
    /// Returns immediately with a [`Screencast`] handle; frames accumulate on
    /// a background task until [`Screencast::stop`] is called, which returns
    /// them in order. Chrome only emits a frame when the page actually
    /// changes, so the result is true video of the session, not fixed-cadence
    /// stills.
    ///
    /// The capture clones the CDP page handle, so this `Page` remains fully
    /// usable while recording — drive the flow and inject overlays as normal.
    pub async fn start_screencast(&self, opts: ScreencastOptions) -> Result<Screencast> {
        let format = match opts.format {
            ScreencastFormat::Jpeg => StartScreencastFormat::Jpeg,
            ScreencastFormat::Png => StartScreencastFormat::Png,
        };
        let params = StartScreencastParams {
            format:          Some(format),
            quality:         opts.quality.map(i64::from),
            max_width:       opts.max_width.map(i64::from),
            max_height:      opts.max_height.map(i64::from),
            every_nth_frame: opts.every_nth_frame.map(i64::from),
        };

        // A headless page is treated as hidden — its compositor produces only
        // the initial frame and never repaints, so the screencast would stall
        // at one frame. Activating the target marks it visible so frames flow
        // for the whole session. Best-effort: a failure here shouldn't abort.
        let _ = self.inner.bring_to_front().await;

        // Subscribe before starting the cast so no early frame is missed.
        let mut events = self
            .inner
            .event_listener::<EventScreencastFrame>()
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        self.inner.execute(params).await.map_err(|e| VoidCrawlError::PageError(e.to_string()))?;

        let frames = Arc::new(Mutex::new(Vec::<ScreencastFrame>::new()));
        let frames_w = Arc::clone(&frames);
        let ack_page = self.inner.clone();
        let (stop_tx, mut stop_rx) = oneshot::channel::<()>();

        let drain = tokio::spawn(async move {
            // Ack each frame as it arrives (Chrome stalls the cast until the
            // previous frame is acknowledged), decode it, and buffer it.
            let handle = |ev: Arc<EventScreencastFrame>,
                          ack_page: &CdpPage,
                          buf: &Arc<Mutex<Vec<ScreencastFrame>>>| {
                let ack_page = ack_page.clone();
                let buf = Arc::clone(buf);
                async move {
                    let _ = ack_page.execute(ScreencastFrameAckParams::new(ev.session_id)).await;
                    let b64: &str = ev.data.as_ref();
                    if let Ok(bytes) = BASE64.decode(b64) {
                        let timestamp = ev.metadata.timestamp.as_ref().map(|t| *t.inner());
                        if let Ok(mut g) = buf.lock() {
                            g.push(ScreencastFrame { data: bytes, timestamp });
                        }
                    }
                }
            };
            loop {
                tokio::select! {
                    biased;
                    maybe = events.next() => match maybe {
                        Some(ev) => handle(ev, &ack_page, &frames_w).await,
                        None => break,
                    },
                    _ = &mut stop_rx => {
                        // Flush frames already delivered before we stopped.
                        while let Some(Some(ev)) = events.next().now_or_never() {
                            handle(ev, &ack_page, &frames_w).await;
                        }
                        break;
                    }
                }
            }
        });

        Ok(Screencast { page: self.inner.clone(), frames, stop: Some(stop_tx), drain })
    }

    /// Generate a PDF of the page, returned as raw bytes.
    pub async fn pdf_bytes(&self) -> Result<Vec<u8>> {
        let params = PrintToPdfParams::default();
        self.inner.pdf(params).await.map_err(|e| VoidCrawlError::PdfError(e.to_string()))
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
    /// [`click_element`]: Self::click_element
    pub async fn click_by_role(&self, role: &str, name: &str, nth: usize) -> Result<()> {
        let nodes = self.query_ax_nodes(Some(role), Some(name)).await?;
        let backends: Vec<_> =
            nodes.iter().filter(|n| !n.ignored).filter_map(|n| n.backend_dom_node_id).collect();
        let backend_id = backends.get(nth).copied().ok_or_else(|| {
            VoidCrawlError::PageError(format!(
                "no AX node with role={role:?} name={name:?} at index {nth} (found {} match(es))",
                backends.len()
            ))
        })?;

        // Bridge AX node → DOM → JS handle, then act on it directly. Using the
        // element's own click() (rather than coordinate dispatch) avoids the
        // box-model math and survives elements that are off-screen until
        // scrolled into view.
        let resolved = self
            .inner
            .execute(ResolveNodeParams { backend_node_id: Some(backend_id), ..Default::default() })
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let object_id = resolved.result.object.object_id.ok_or_else(|| {
            VoidCrawlError::PageError("AX node could not be resolved to a DOM handle".into())
        })?;
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
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "test harness")]
mod tests {
    use super::{client_hints_for_ua, dehead};

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
