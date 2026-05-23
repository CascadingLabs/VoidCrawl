//! Session-scoped interaction primitives: click, type, eval JS, read
//! title, extract text, capture network entries, wait for network idle.
//!
//! Each fn takes an existing session (already opened via `session_open`)
//! and runs one action against its page. These are the Claude-Code-facing
//! primitives — small, composable, no hidden state.

use std::{sync::Arc, time::Duration};

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::{Instant, sleep};
use void_crawl_core::{
    CaptchaInfo, CaptchaKind, DispatchMouseEventType, MouseButton, ax, capture_captcha,
    detect_captcha, inject_captcha_token,
};

use crate::{
    errors::map_err, server::VoidCrawlServer, sessions::DedicatedSession,
    tools::session::DEFAULT_TIMEOUT_SECS,
};

// ── Click ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ClickArgs {
    pub session_id: String,
    /// CSS selector of the element to click.
    pub selector:   String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OkResult {
    pub ok: bool,
}

pub async fn click(server: &VoidCrawlServer, args: ClickArgs) -> Result<OkResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    page.click_element(&args.selector).await.map_err(map_err)?;
    Ok(OkResult { ok: true })
}

// ── Click visual coords ─────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ClickVisualCoordsArgs {
    pub session_id: String,
    /// X coordinate in CSS pixels (pre-DPR).
    pub x:          f64,
    /// Y coordinate in CSS pixels (pre-DPR).
    pub y:          f64,
}

pub async fn click_visual_coords(
    server: &VoidCrawlServer,
    args: ClickVisualCoordsArgs,
) -> Result<OkResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    // mousePressed + mouseReleased at (x, y) with left button. Matches
    // the CDP recipe that React-rendered forms respond to when CSS
    // selector clicks fail silently.
    page.dispatch_mouse_event(
        DispatchMouseEventType::MousePressed,
        args.x,
        args.y,
        Some(MouseButton::Left),
        Some(1),
        None,
        None,
        None,
    )
    .await
    .map_err(map_err)?;
    page.dispatch_mouse_event(
        DispatchMouseEventType::MouseReleased,
        args.x,
        args.y,
        Some(MouseButton::Left),
        Some(1),
        None,
        None,
        None,
    )
    .await
    .map_err(map_err)?;
    Ok(OkResult { ok: true })
}

// ── Type text ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct TypeTextArgs {
    pub session_id: String,
    /// CSS selector of the target input. When omitted, keys are
    /// dispatched to whatever currently has focus.
    #[serde(default)]
    pub selector:   Option<String>,
    pub text:       String,
}

pub async fn type_text(
    server: &VoidCrawlServer,
    args: TypeTextArgs,
) -> Result<OkResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    if let Some(sel) = args.selector {
        page.type_into(&sel, &args.text).await.map_err(map_err)?;
    } else {
        // No selector: dispatch each character as a keypress to the
        // currently-focused element (matches the React recipe where
        // you click first, then type).
        for ch in args.text.chars() {
            let s = ch.to_string();
            page.dispatch_key_event(
                void_crawl_core::DispatchKeyEventType::Char,
                Some(&s),
                None,
                Some(&s),
                None,
            )
            .await
            .map_err(map_err)?;
        }
    }
    Ok(OkResult { ok: true })
}

// ── Eval JS ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct EvalJsArgs {
    pub session_id: String,
    /// A JavaScript expression. Its value is returned as JSON.
    pub expression: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct EvalJsResult {
    pub value: Value,
}

pub async fn eval_js(
    server: &VoidCrawlServer,
    args: EvalJsArgs,
) -> Result<EvalJsResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let value = page.evaluate_js(&args.expression).await.map_err(map_err)?;
    Ok(EvalJsResult { value })
}

// ── Title ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct SessionIdArgs {
    pub session_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TitleResult {
    pub title: Option<String>,
}

pub async fn title(
    server: &VoidCrawlServer,
    args: SessionIdArgs,
) -> Result<TitleResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    Ok(TitleResult { title: page.title().await.ok().flatten() })
}

// ── Extract ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ExtractArgs {
    pub session_id: String,
    /// CSS selector. Uses `document.querySelectorAll` — returns text
    /// content (not inner HTML) for each matching element.
    pub selector:   String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ExtractResult {
    pub texts: Vec<String>,
}

pub async fn extract(
    server: &VoidCrawlServer,
    args: ExtractArgs,
) -> Result<ExtractResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let js = format!(
        "Array.from(document.querySelectorAll({sel:?})).map(e => e.textContent || '')",
        sel = args.selector
    );
    let value = page.evaluate_js(&js).await.map_err(map_err)?;
    let texts = match value {
        Value::Array(arr) => {
            arr.into_iter().map(|v| v.as_str().unwrap_or("").to_string()).collect()
        }
        _ => Vec::new(),
    };
    Ok(ExtractResult { texts })
}

// ── Accessibility tree ────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct AxTreeArgs {
    pub session_id: String,
    /// "compact" (default): a pruned, indented role/name outline meant for an
    /// agent to read. "raw": the full CDP AX nodes for programmatic use.
    #[serde(default)]
    pub mode:       Option<String>,
    /// Maximum descendant depth to traverse; omit for the whole tree.
    #[serde(default)]
    pub depth:      Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AxTreeResult {
    /// Indented `role "name"` outline. Populated in compact mode only.
    pub tree:        String,
    /// Raw CDP AX nodes. Populated in raw mode only.
    pub nodes:       Vec<Value>,
    /// Total AX nodes the browser returned.
    pub node_count:  usize,
    /// Non-ignored nodes carrying a non-empty accessible name. A low ratio of
    /// `named_count` to `node_count` signals a thin/poor AX tree — prefer
    /// falling back to HTML, screenshot, or CSS selectors on such pages.
    pub named_count: usize,
}

pub async fn ax_tree(
    server: &VoidCrawlServer,
    args: AxTreeArgs,
) -> Result<AxTreeResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let value = page.get_full_ax_tree(args.depth).await.map_err(map_err)?;
    let nodes = match value {
        Value::Array(arr) => arr,
        _ => Vec::new(),
    };
    let (node_count, named_count) = ax::richness(&nodes);

    let raw = args.mode.as_deref() == Some("raw");
    let (tree, nodes) =
        if raw { (String::new(), nodes) } else { (ax::compact_outline(&nodes), Vec::new()) };
    Ok(AxTreeResult { tree, nodes, node_count, named_count })
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ClickByRoleArgs {
    pub session_id: String,
    /// Computed accessibility role, e.g. "button", "link", "checkbox".
    pub role:       String,
    /// Computed accessible name (exact match).
    pub name:       String,
    /// 0-based index when several nodes match the same role + name.
    #[serde(default)]
    pub nth:        Option<usize>,
}

pub async fn click_by_role(
    server: &VoidCrawlServer,
    args: ClickByRoleArgs,
) -> Result<OkResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    page.click_by_role(&args.role, &args.name, args.nth.unwrap_or(0)).await.map_err(map_err)?;
    Ok(OkResult { ok: true })
}

// ── Wait for network idle ───────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct WaitIdleArgs {
    pub session_id:   String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

pub async fn wait_for_network_idle(
    server: &VoidCrawlServer,
    args: WaitIdleArgs,
) -> Result<OkResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    page.wait_for_network_idle(timeout).await.map_err(map_err)?;
    Ok(OkResult { ok: true })
}

// ── Network capture ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, JsonSchema)]
pub struct NetworkEntry {
    pub url:            String,
    pub initiator_type: String,
    pub transfer_size:  f64,
    pub duration_ms:    f64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NetworkCaptureResult {
    pub entries: Vec<NetworkEntry>,
}

pub async fn network_capture(
    server: &VoidCrawlServer,
    args: SessionIdArgs,
) -> Result<NetworkCaptureResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    // Pull from the Resource Timing API — same source DevTools uses for
    // the Network panel's "transferred" column.
    const JS: &str = r#"
        performance.getEntriesByType('resource').map(e => ({
            url: e.name,
            initiator_type: e.initiatorType || '',
            transfer_size: e.transferSize || 0,
            duration_ms: e.duration || 0,
        }))
    "#;
    let value = page.evaluate_js(JS).await.map_err(map_err)?;
    let entries = match value {
        Value::Array(arr) => arr
            .into_iter()
            .filter_map(|v| {
                let obj = v.as_object()?;
                Some(NetworkEntry {
                    url:            obj.get("url")?.as_str()?.to_string(),
                    initiator_type: obj.get("initiator_type")?.as_str().unwrap_or("").to_string(),
                    transfer_size:  obj.get("transfer_size").and_then(Value::as_f64).unwrap_or(0.0),
                    duration_ms:    obj.get("duration_ms").and_then(Value::as_f64).unwrap_or(0.0),
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(NetworkCaptureResult { entries })
}

// ── Detect captcha ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, JsonSchema)]
pub struct DetectCaptchaResult {
    pub kind: Option<String>,
}

pub async fn detect_captcha_tool(
    server: &VoidCrawlServer,
    args: SessionIdArgs,
) -> Result<DetectCaptchaResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let kind = detect_captcha(&page).await.map_err(map_err)?;
    Ok(DetectCaptchaResult { kind: kind.map(|k| k.as_str().to_string()) })
}

// ── Capture captcha (full structured) ───────────────────────────────────

#[derive(Debug, Serialize, JsonSchema)]
pub struct WidgetRectJson {
    pub x:      f64,
    pub y:      f64,
    pub width:  f64,
    pub height: f64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CaptureCaptchaResult {
    /// Kind tag (same values as detect_captcha). Null when no captcha.
    pub kind:                    Option<String>,
    /// Site key for third-party solver APIs (2Captcha, CapSolver, etc.).
    pub sitekey:                 Option<String>,
    /// CSS selector of the widget container.
    pub widget_selector:         Option<String>,
    pub widget_rect:             Option<WidgetRectJson>,
    /// True when the widget element is actually in the DOM.
    /// False when only the runtime is loaded (Ahrefs-style lazy mount).
    pub widget_rendered:         bool,
    /// Field to write a solved token into (via `inject_captcha_token`).
    pub response_field_selector: Option<String>,
    /// Token already present — skip solving when set.
    pub existing_token:          Option<String>,
    /// Turnstile action / cdata attributes (pass through to solver).
    pub action:                  Option<String>,
    pub cdata:                   Option<String>,
    /// Current document URL — required by most solver APIs.
    pub page_url:                String,
}

pub async fn capture_captcha_tool(
    server: &VoidCrawlServer,
    args: SessionIdArgs,
) -> Result<CaptureCaptchaResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let info: Option<CaptchaInfo> = capture_captcha(&page).await.map_err(map_err)?;
    Ok(match info {
        None => CaptureCaptchaResult {
            kind:                    None,
            sitekey:                 None,
            widget_selector:         None,
            widget_rect:             None,
            widget_rendered:         false,
            response_field_selector: None,
            existing_token:          None,
            action:                  None,
            cdata:                   None,
            page_url:                String::new(),
        },
        Some(i) => CaptureCaptchaResult {
            kind:                    Some(i.kind.as_str().to_string()),
            sitekey:                 i.sitekey,
            widget_selector:         i.widget_selector,
            widget_rect:             i.widget_rect.map(|r| WidgetRectJson {
                x:      r.x,
                y:      r.y,
                width:  r.width,
                height: r.height,
            }),
            widget_rendered:         i.widget_rendered,
            response_field_selector: i.response_field_selector,
            existing_token:          i.existing_token,
            action:                  i.action,
            cdata:                   i.cdata,
            page_url:                i.page_url,
        },
    })
}

// ── Inject captcha token ────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct InjectCaptchaTokenArgs {
    pub session_id: String,
    /// Token returned by your solver (e.g. 2Captcha's `gRecaptchaResponse`).
    pub token:      String,
    /// Kind tag. Must match the captcha on the page: one of
    /// "turnstile", "recaptcha", "hcaptcha". Defaults to whatever
    /// `capture_captcha` currently detects.
    #[serde(default)]
    pub kind:       Option<String>,
}

pub async fn inject_captcha_token_tool(
    server: &VoidCrawlServer,
    args: InjectCaptchaTokenArgs,
) -> Result<OkResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let kind = match args.kind.as_deref() {
        Some("turnstile") => CaptchaKind::Turnstile,
        Some("recaptcha") => CaptchaKind::Recaptcha,
        Some("hcaptcha") => CaptchaKind::Hcaptcha,
        Some(other) => {
            return Err(ErrorData::invalid_params(
                format!(
                    "unknown captcha kind {other:?} — expected 'turnstile', 'recaptcha', or 'hcaptcha'"
                ),
                None,
            ));
        }
        None => {
            // Auto-detect from the page.
            let info = capture_captcha(&page).await.map_err(map_err)?;
            info.map(|i| i.kind).ok_or_else(|| {
                ErrorData::invalid_params(
                    String::from("no captcha detected on page — pass `kind` explicitly"),
                    None,
                )
            })?
        }
    };
    inject_captcha_token(&page, kind, &args.token).await.map_err(map_err)?;
    Ok(OkResult { ok: true })
}

// ── Solve captcha ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct SolveCaptchaArgs {
    pub session_id:        String,
    /// How long to wait (seconds) for the response token to appear after
    /// clicking the widget. Default 20.
    #[serde(default)]
    pub wait_secs:         Option<u64>,
    /// Click offset inside the widget's bounding rect from the left edge,
    /// in CSS pixels. Default 28 — matches the checkbox position for
    /// Turnstile / reCAPTCHA-v2 / hCaptcha anchor iframes. Override only
    /// when a site customises widget size.
    #[serde(default)]
    pub checkbox_offset_x: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SolveCaptchaResult {
    /// Detected captcha kind (same tags as detect_captcha).
    pub kind:    Option<String>,
    /// Click coordinates dispatched in CSS pixels, if a widget rect was
    /// found.
    pub clicked: Option<(f64, f64)>,
    /// Response token value, if one was observed within wait_secs.
    /// Turnstile: `input[name=cf-turnstile-response]`.
    /// reCAPTCHA: `#g-recaptcha-response`.
    /// hCaptcha: `textarea[name=h-captcha-response]`.
    pub token:   Option<String>,
    /// True when a token was obtained (widget solved) or when the
    /// detector no longer reports a captcha (page passed the wall).
    pub solved:  bool,
}

pub async fn solve_captcha(
    server: &VoidCrawlServer,
    args: SolveCaptchaArgs,
) -> Result<SolveCaptchaResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;

    // 1. Identify what's on the page.
    let kind = detect_captcha(&page).await.map_err(map_err)?;
    let Some(kind) = kind else {
        return Ok(SolveCaptchaResult {
            kind:    None,
            clicked: None,
            token:   None,
            solved:  true,
        });
    };
    let kind_tag = kind.as_str().to_string();

    // 2. Locate the widget's bounding rect. We try candidate selectors specific to
    //    the detected kind, then fall back to generic iframe queries. Returns {x,
    //    y, w, h} of the widget's *on-screen* box in CSS pixels, already offset by
    //    any enclosing iframe origins.
    const RECT_JS: &str = r#"
        (function(kind) {
            function rectOf(el) {
                if (!el) return null;
                const r = el.getBoundingClientRect();
                if (r.width < 4 || r.height < 4) return null;
                return { x: r.left, y: r.top, w: r.width, h: r.height };
            }
            const SELS = {
                turnstile: [
                    '.cf-turnstile iframe',
                    'iframe[src*="challenges.cloudflare.com/turnstile"]',
                    '.cf-turnstile',
                ],
                recaptcha: [
                    'iframe[src*="recaptcha/api2/anchor"]',
                    'iframe[src*="google.com/recaptcha"]',
                    '.g-recaptcha',
                ],
                hcaptcha: [
                    'iframe[src*="hcaptcha.com"][data-hcaptcha-widget-id]',
                    'iframe[src*="hcaptcha.com"]',
                    '.h-captcha',
                ],
            };
            const list = SELS[kind] || [];
            for (const sel of list) {
                const el = document.querySelector(sel);
                const r = rectOf(el);
                if (r) return r;
            }
            return null;
        })(arguments_kind_placeholder)
    "#;
    // The evaluate_js API takes a bare expression; inject the literal.
    let rect_expr = RECT_JS.replace("arguments_kind_placeholder", &format!("{kind_tag:?}"));
    let rect_val = page.evaluate_js(&rect_expr).await.map_err(map_err)?;

    let Some(rect) = rect_val.as_object() else {
        return Ok(SolveCaptchaResult {
            kind:    Some(kind_tag),
            clicked: None,
            token:   None,
            solved:  false,
        });
    };
    let rx = rect.get("x").and_then(Value::as_f64).unwrap_or(0.0);
    let ry = rect.get("y").and_then(Value::as_f64).unwrap_or(0.0);
    let rh = rect.get("h").and_then(Value::as_f64).unwrap_or(0.0);

    // 3. Compute click point — the standard checkbox sits ~28px from the iframe's
    //    left edge, vertically centred. Small jitter keeps the event looking less
    //    mechanical.
    let offset_x = args.checkbox_offset_x.unwrap_or(28.0);
    let jitter_x: f64 = (rx.fract() * 100.0) % 3.0 - 1.5; // deterministic ±1.5px
    let jitter_y: f64 = (ry.fract() * 100.0) % 3.0 - 1.5;
    let cx = rx + offset_x + jitter_x;
    let cy = ry + rh / 2.0 + jitter_y;

    // 4. Move, press, release — distinct MouseMoved first gives the widget's JS a
    //    chance to observe a realistic pointer track.
    page.dispatch_mouse_event(
        void_crawl_core::DispatchMouseEventType::MouseMoved,
        cx,
        cy,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .map_err(map_err)?;
    sleep(Duration::from_millis(60)).await;
    page.dispatch_mouse_event(
        DispatchMouseEventType::MousePressed,
        cx,
        cy,
        Some(MouseButton::Left),
        Some(1),
        None,
        None,
        None,
    )
    .await
    .map_err(map_err)?;
    sleep(Duration::from_millis(50)).await;
    page.dispatch_mouse_event(
        DispatchMouseEventType::MouseReleased,
        cx,
        cy,
        Some(MouseButton::Left),
        Some(1),
        None,
        None,
        None,
    )
    .await
    .map_err(map_err)?;

    // 5. Poll for the response token. Each family writes its solved token into a
    //    known hidden input/textarea — presence of a non-empty value is the
    //    canonical "solved" signal.
    const TOKEN_JS: &str = r#"
        (function() {
            const q = (s) => { const el = document.querySelector(s); return el ? (el.value || el.textContent || '') : ''; };
            const t = q('input[name="cf-turnstile-response"]') || q('textarea[name="cf-turnstile-response"]');
            if (t) return t;
            const r = q('#g-recaptcha-response') || q('textarea[name="g-recaptcha-response"]');
            if (r) return r;
            const h = q('textarea[name="h-captcha-response"]') || q('[name="h-captcha-response"]');
            if (h) return h;
            return '';
        })()
    "#;
    let wait_for = Duration::from_secs(args.wait_secs.unwrap_or(20));
    let deadline = Instant::now() + wait_for;
    let mut token: Option<String> = None;
    let mut solved = false;
    while Instant::now() < deadline {
        let v = page.evaluate_js(TOKEN_JS).await.map_err(map_err)?;
        if let Some(s) = v.as_str()
            && !s.is_empty()
        {
            token = Some(s.to_string());
            solved = true;
            break;
        }
        // Also accept: detector no longer sees a captcha (page passed
        // the interstitial entirely, e.g. Cloudflare managed challenge).
        if detect_captcha(&page).await.map_err(map_err)?.is_none() {
            solved = true;
            break;
        }
        sleep(Duration::from_millis(500)).await;
    }

    Ok(SolveCaptchaResult { kind: Some(kind_tag), clicked: Some((cx, cy)), token, solved })
}

// ── Helper ──────────────────────────────────────────────────────────────

async fn lookup(server: &VoidCrawlServer, id: &str) -> Result<Arc<DedicatedSession>, ErrorData> {
    server
        .state()
        .sessions
        .get(id)
        .await
        .ok_or_else(|| ErrorData::invalid_params(format!("unknown session_id: {id}"), None))
}
