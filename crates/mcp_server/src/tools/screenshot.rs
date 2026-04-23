//! Stateless screenshot.

use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use rmcp::{
    ErrorData,
    model::{CallToolResult, Content},
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{errors::map_err, server::VoidCrawlServer, tools::wait};

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
        let bytes = tab.page.screenshot_png().await?;
        // Emit the device-pixel ratio alongside the PNG so agents can
        // translate screenshot-space coordinates to CSS pixels before
        // handing them to `click_visual_coords`. Falls back to 1.0 when
        // the page didn't expose the value (e.g. about:blank on some
        // Chromium builds).
        let dpr = tab
            .page
            .evaluate_js("window.devicePixelRatio")
            .await
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        Ok::<_, void_crawl_core::VoidCrawlError>((bytes, dpr))
    }
    .await;
    pool.release(tab).await;
    let (bytes, dpr) = result.map_err(map_err)?;
    let len = bytes.len();
    let encoded = B64.encode(&bytes);
    Ok(CallToolResult::success(vec![
        Content::text(format!(
            "{len} bytes PNG of {url} (devicePixelRatio={dpr}; divide screenshot pixels by DPR before click_visual_coords)",
            url = args.url
        )),
        Content::image(encoded, "image/png"),
    ]))
}
