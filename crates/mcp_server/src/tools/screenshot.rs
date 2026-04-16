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
    let bytes_result = async {
        let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        tab.page.goto_and_wait_for_idle(&args.url, timeout).await?;
        wait::apply(&tab.page, args.wait_for.as_deref(), timeout).await?;
        tab.page.screenshot_png().await
    }
    .await;
    pool.release(tab).await;
    let bytes = bytes_result.map_err(map_err)?;
    let len = bytes.len();
    let encoded = B64.encode(&bytes);
    Ok(CallToolResult::success(vec![
        Content::text(format!("{len} bytes PNG of {url}", url = args.url)),
        Content::image(encoded, "image/png"),
    ]))
}
