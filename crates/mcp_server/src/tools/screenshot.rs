//! Screenshot capture: a stateless variant that navigates a pooled tab, and
//! a stateful variant that captures an existing session's page as-is.

use std::{sync::Arc, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use rmcp::{
    ErrorData,
    model::{CallToolResult, Content},
};
use schemars::JsonSchema;
use serde::Deserialize;
use void_crawl_core::{Page, VoidCrawlError};

use crate::{
    errors::map_err,
    server::VoidCrawlServer,
    sessions::DedicatedSession,
    tools::{session::SessionIdArgs, wait},
};

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ScreenshotArgs {
    /// Absolute URL to capture.
    pub url:          String,
    /// Optional wait strategy: "networkidle" (default) or "selector:<css>".
    #[serde(default)]
    pub wait_for:     Option<String>,
    /// Navigation + wait timeout in seconds (default 30).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

pub async fn run(
    server: &VoidCrawlServer,
    args: ScreenshotArgs,
) -> Result<CallToolResult, ErrorData> {
    let pool = server.state().pool().await.map_err(map_err)?;
    let tab = pool.acquire().await.map_err(map_err)?;
    let result = async {
        let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        tab.page.goto_and_wait_for_idle(&args.url, timeout).await?;
        wait::apply_post_navigate(&tab.page, args.wait_for.as_deref(), timeout).await?;
        capture(&tab.page).await
    }
    .await;
    pool.release(tab).await;
    let (bytes, dpr) = result.map_err(map_err)?;
    Ok(to_call_result(&bytes, dpr, &args.url))
}

/// Capture the current session's page exactly as it stands — no navigation,
/// no URL change. The visual counterpart to `session_content` /
/// `session_snapshot` for authenticated, post-click, paginated, or
/// challenge state that only exists inside an open session.
pub async fn session(
    server: &VoidCrawlServer,
    args: SessionIdArgs,
) -> Result<CallToolResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let (bytes, dpr) = capture(&page).await.map_err(map_err)?;
    let url = page.url().await.ok().flatten().unwrap_or(args.session_id);
    Ok(to_call_result(&bytes, dpr, &url))
}

async fn lookup(server: &VoidCrawlServer, id: &str) -> Result<Arc<DedicatedSession>, ErrorData> {
    server
        .state()
        .sessions
        .get(id)
        .await
        .ok_or_else(|| ErrorData::invalid_params(format!("unknown session_id: {id}"), None))
}

/// Grab a PNG plus the page's device-pixel ratio, so callers can translate
/// screenshot-space coordinates to CSS pixels before handing them to
/// `click_visual_coords`. DPR falls back to 1.0 when the page didn't expose
/// the value (e.g. about:blank on some Chromium builds).
async fn capture(page: &Page) -> Result<(Vec<u8>, f64), VoidCrawlError> {
    let bytes = page.screenshot_png().await?;
    let dpr = page
        .evaluate_js("window.devicePixelRatio")
        .await
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    Ok((bytes, dpr))
}

fn to_call_result(bytes: &[u8], dpr: f64, label: &str) -> CallToolResult {
    let len = bytes.len();
    let encoded = B64.encode(bytes);
    CallToolResult::success(vec![
        Content::text(format!(
            "{len} bytes PNG of {label} (devicePixelRatio={dpr}; divide screenshot pixels by DPR before click_visual_coords)"
        )),
        Content::image(encoded, "image/png"),
    ])
}
