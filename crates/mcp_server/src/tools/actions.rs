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
use void_crawl_core::{DispatchMouseEventType, MouseButton, detect_captcha};

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

// ── Helper ──────────────────────────────────────────────────────────────

async fn lookup(server: &VoidCrawlServer, id: &str) -> Result<Arc<DedicatedSession>, ErrorData> {
    server
        .state()
        .sessions
        .get(id)
        .await
        .ok_or_else(|| ErrorData::invalid_params(format!("unknown session_id: {id}"), None))
}
