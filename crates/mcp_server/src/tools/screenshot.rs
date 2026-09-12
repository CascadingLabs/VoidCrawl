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
use void_crawl_core::{Page, ScreenshotOptions, ScreenshotOutput, VoidCrawlError};

use crate::{
    errors::map_err,
    server::VoidCrawlServer,
    sessions::DedicatedSession,
    tools::{
        selector::SelectorArg,
        viewport::{BboxArg, ScrollArg, ViewportArg},
        wait,
    },
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
    /// Capture as this device/viewport instead of the pool's default —
    /// one-shot, does not persist. Pass `preset` (see `list_device_presets`)
    /// or custom `width`+`height` (+ optional `device_scale_factor`, `mobile`).
    #[serde(default)]
    pub viewport:     Option<ViewportArg>,
    /// Crop to this CSS-pixel region instead of the full page. Mutually
    /// exclusive with `selector`.
    #[serde(default)]
    pub bbox:         Option<BboxArg>,
    /// Crop to a browser target's resolved rectangle (any of its 8 kinds —
    /// css/xpath/regex/jsonld/attr/global_id/role/visual) instead of an
    /// explicit `bbox`. Mutually exclusive with `bbox`. A selector that
    /// matches nothing, is ambiguous, or is inherently non-visual
    /// (`jsonld`/`regex`) fails with `invalid_params` rather than silently
    /// cropping an arbitrary target — see `SelectorArg` for the field
    /// reference per kind.
    #[serde(default)]
    pub selector:     Option<SelectorArg>,
    /// Scroll before capturing. Combine with `bbox`/`selector` to crop a
    /// specific on-screen region after paging down a fixed viewport.
    #[serde(default)]
    pub scroll:       Option<ScrollArg>,
    /// Capture the full scrollable page (default `true`). Set `false` to
    /// capture only what's currently visible in the viewport — cheaper,
    /// and the right choice when you want "what a visitor sees first," not
    /// the whole scroll history. Ignored when `bbox`/`selector` is set.
    #[serde(default)]
    pub full_page:    Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct SessionScreenshotArgs {
    pub session_id: String,
    /// Capture as this device/viewport instead of the session's current
    /// one — one-shot, restores the session's viewport after capture. Pass
    /// `preset` (see `list_device_presets`) or custom `width`+`height`
    /// (+ optional `device_scale_factor`, `mobile`).
    #[serde(default)]
    pub viewport:   Option<ViewportArg>,
    /// Crop to this CSS-pixel region instead of the full page. Mutually
    /// exclusive with `selector`.
    #[serde(default)]
    pub bbox:       Option<BboxArg>,
    /// Crop to a browser target's resolved rectangle instead of an explicit
    /// `bbox`. Mutually exclusive with `bbox`. See `ScreenshotArgs.selector`
    /// / `SelectorArg` for the full field reference per kind.
    #[serde(default)]
    pub selector:   Option<SelectorArg>,
    /// Scroll before capturing. Combine with `bbox`/`selector` to crop a
    /// specific on-screen region after paging down a fixed viewport.
    #[serde(default)]
    pub scroll:     Option<ScrollArg>,
    /// Capture the full scrollable page (default `true`). Set `false` to
    /// capture only what's currently visible in the viewport — cheaper,
    /// and the right choice when you want "what's on screen right now,"
    /// not the whole scroll history. Ignored when `bbox`/`selector` is set.
    #[serde(default)]
    pub full_page:  Option<bool>,
}

#[allow(clippy::too_many_arguments)]
fn build_options(
    viewport: Option<&ViewportArg>,
    bbox: Option<&BboxArg>,
    selector: Option<SelectorArg>,
    scroll: Option<&ScrollArg>,
    full_page: Option<bool>,
) -> Result<ScreenshotOptions, ErrorData> {
    if bbox.is_some() && selector.is_some() {
        return Err(ErrorData::invalid_params(
            "`bbox` and `selector` are mutually exclusive",
            None,
        ));
    }
    let mut opts = ScreenshotOptions::default();
    if let Some(v) = viewport {
        opts = opts.with_viewport(v.resolve()?);
    }
    if let Some(b) = bbox {
        opts = opts.with_bbox((*b).into());
    }
    if let Some(s) = selector {
        opts = opts.with_selector(s.into());
    }
    if let Some(s) = scroll {
        opts = opts.with_scroll(s.resolve()?);
    }
    if full_page == Some(false) {
        opts = opts.viewport_only();
    }
    Ok(opts)
}

pub async fn run(
    server: &VoidCrawlServer,
    args: ScreenshotArgs,
) -> Result<CallToolResult, ErrorData> {
    let opts = build_options(
        args.viewport.as_ref(),
        args.bbox.as_ref(),
        args.selector,
        args.scroll.as_ref(),
        args.full_page,
    )?;
    let pool = server.state().pool().await.map_err(map_err)?;
    let tab = pool.acquire().await.map_err(map_err)?;
    let result = async {
        let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        tab.page.goto_and_wait_for_idle(&args.url, timeout).await?;
        wait::apply_post_navigate(&tab.page, args.wait_for.as_deref(), timeout).await?;
        capture(&tab.page, opts).await
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
    args: SessionScreenshotArgs,
) -> Result<CallToolResult, ErrorData> {
    let opts = build_options(
        args.viewport.as_ref(),
        args.bbox.as_ref(),
        args.selector,
        args.scroll.as_ref(),
        args.full_page,
    )?;
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let (bytes, dpr) = capture(&page, opts).await.map_err(map_err)?;
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
async fn capture(page: &Page, opts: ScreenshotOptions) -> Result<(Vec<u8>, f64), VoidCrawlError> {
    let output = page.screenshot(opts).await?;
    let bytes = match output {
        ScreenshotOutput::Bytes(b) => b,
        ScreenshotOutput::Path(_) => unreachable!("MCP screenshot tools never set a path"),
    };
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
