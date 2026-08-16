//! Variable CDP viewport MCP tools: named device presets (phone/tablet/
//! desktop, in the spirit of Chrome DevTools' device toolbar), a persistent
//! per-session override, and the shared arg-resolution types
//! `screenshot`/`session_screenshot` use for their one-shot
//! `viewport`/`bbox`/`scroll` options.

use std::sync::Arc;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use void_crawl_core::{Bbox, ScrollTarget, Viewport, viewport as presets};

use crate::{
    errors::map_err,
    server::VoidCrawlServer,
    sessions::DedicatedSession,
    tools::{actions::OkResult, session::SessionIdArgs},
};

/// A device/viewport override: a named preset OR a custom size. Shared by
/// `screenshot`, `session_screenshot` (nested, one-shot) and
/// `session_set_viewport` (flattened, persistent).
#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct ViewportArg {
    /// Named device preset — see `list_device_presets` for valid names
    /// (e.g. "iPhone 16 Pro Max", "iPad Pro 11", "Desktop 1080p"). Mutually
    /// exclusive with width/height.
    #[serde(default)]
    pub preset:              Option<String>,
    /// Custom viewport width in CSS pixels. Requires `height`; mutually
    /// exclusive with `preset`.
    #[serde(default)]
    pub width:               Option<u32>,
    /// Custom viewport height in CSS pixels. Requires `width`.
    #[serde(default)]
    pub height:              Option<u32>,
    /// Device pixel ratio for a custom viewport (default 1.0). Ignored with
    /// `preset`.
    #[serde(default)]
    pub device_scale_factor: Option<f64>,
    /// Emulate a mobile viewport for a custom size — also enables touch
    /// (default false). Ignored with `preset`.
    #[serde(default)]
    pub mobile:              Option<bool>,
}

impl ViewportArg {
    pub fn resolve(&self) -> Result<Viewport, ErrorData> {
        match (self.preset.as_deref(), self.width, self.height) {
            (Some(name), None, None) => presets::preset(name).ok_or_else(|| {
                ErrorData::invalid_params(
                    format!(
                        "unknown device preset {name:?}; call list_device_presets for valid names"
                    ),
                    None,
                )
            }),
            (None, Some(width), Some(height)) => {
                let mut vp = Viewport::custom(width, height);
                vp.device_scale_factor = self.device_scale_factor.unwrap_or(1.0);
                vp.mobile = self.mobile.unwrap_or(false);
                vp.has_touch = vp.mobile;
                Ok(vp)
            }
            (Some(_), _, _) => Err(ErrorData::invalid_params(
                "viewport: `preset` is mutually exclusive with width/height",
                None,
            )),
            (None, Some(_), None) | (None, None, Some(_)) => Err(ErrorData::invalid_params(
                "viewport: both width and height are required together",
                None,
            )),
            (None, None, None) => Err(ErrorData::invalid_params(
                "viewport: pass either `preset` or `width`+`height`",
                None,
            )),
        }
    }
}

/// A CSS-pixel crop region for `screenshot`/`session_screenshot`.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, Default)]
pub struct BboxArg {
    pub x:      u32,
    pub y:      u32,
    pub width:  u32,
    pub height: u32,
}

impl From<BboxArg> for Bbox {
    fn from(b: BboxArg) -> Self {
        Bbox { x: b.x, y: b.y, width: b.width, height: b.height }
    }
}

/// Where to scroll before cropping with `bbox`. Shared by
/// `screenshot`/`session_screenshot`.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, Default)]
pub struct ScrollArg {
    /// Scroll to N viewport-heights from the top of the page (2.0 =
    /// "scrolled down twice"). Mutually exclusive with `pixels`.
    #[serde(default)]
    pub viewports: Option<f64>,
    /// Scroll to an absolute pixel Y from the top. Mutually exclusive with
    /// `viewports`.
    #[serde(default)]
    pub pixels:    Option<i64>,
}

impl ScrollArg {
    pub fn resolve(&self) -> Result<ScrollTarget, ErrorData> {
        match (self.viewports, self.pixels) {
            (Some(v), None) => Ok(ScrollTarget::Viewports(v)),
            (None, Some(p)) => Ok(ScrollTarget::Pixels(p)),
            (Some(_), Some(_)) => Err(ErrorData::invalid_params(
                "scroll: `viewports` and `pixels` are mutually exclusive",
                None,
            )),
            (None, None) => {
                Err(ErrorData::invalid_params("scroll: pass either `viewports` or `pixels`", None))
            }
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct SessionSetViewportArgs {
    pub session_id: String,
    #[serde(flatten)]
    pub viewport:   ViewportArg,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DevicePreset {
    pub name:                String,
    pub width:               u32,
    pub height:              u32,
    pub device_scale_factor: f64,
    pub mobile:              bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DevicePresetsResult {
    pub presets: Vec<DevicePreset>,
}

/// Persistently override a session's viewport/device identity — stays in
/// effect across subsequent navigate/click/screenshot calls until
/// `session_clear_viewport` or another `session_set_viewport`. For a
/// one-off override scoped to a single capture, use the `viewport` option
/// on `screenshot`/`session_screenshot` instead.
pub async fn session_set(
    server: &VoidCrawlServer,
    args: SessionSetViewportArgs,
) -> Result<OkResult, ErrorData> {
    let viewport = args.viewport.resolve()?;
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    page.set_viewport(viewport).await.map_err(map_err)?;
    Ok(OkResult { ok: true })
}

/// Clear a `session_set_viewport` override, returning to the session's
/// launch-time default viewport.
pub async fn session_clear(
    server: &VoidCrawlServer,
    args: SessionIdArgs,
) -> Result<OkResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    page.clear_viewport().await.map_err(map_err)?;
    Ok(OkResult { ok: true })
}

/// List named device presets (phones, tablets, desktop sizes) available to
/// `viewport`/`session_set_viewport` — the DevTools device-toolbar
/// dropdown, as data.
pub fn list_presets() -> DevicePresetsResult {
    let presets = presets::all_presets()
        .into_iter()
        .map(|(name, vp)| DevicePreset {
            name:                name.to_string(),
            width:               vp.width,
            height:              vp.height,
            device_scale_factor: vp.device_scale_factor,
            mobile:              vp.mobile,
        })
        .collect();
    DevicePresetsResult { presets }
}

async fn lookup(server: &VoidCrawlServer, id: &str) -> Result<Arc<DedicatedSession>, ErrorData> {
    server
        .state()
        .sessions
        .get(id)
        .await
        .ok_or_else(|| ErrorData::invalid_params(format!("unknown session_id: {id}"), None))
}
