//! Top-level MCP service. Owns `AppState` and the `ToolRouter`.
//!
//! Each tool method is a thin adapter that delegates to a free
//! function in `crate::tools::*`; the heavy lifting lives there so
//! this file stays focused on wire-protocol concerns.

use std::sync::Arc;

use rmcp::{
    ErrorData,
    handler::server::{
        ServerHandler,
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use void_crawl_core::{ManagedProfileDescription, ProfilePool, ResolvedProfilePool};

use crate::{
    VERSION,
    errors::map_err,
    state::AppState,
    tools,
    tools::{
        actions::{
            AxTreeArgs, AxTreeResult, CaptureCaptchaResult, ClickArgs, ClickByRoleArgs,
            ClickVisualCoordsArgs, DetectCaptchaResult, EvalJsArgs, EvalJsInFrameArgs,
            EvalJsResult, ExtractArgs, ExtractResult, InjectCaptchaTokenArgs, NetworkCaptureResult,
            OkResult, SessionIdArgs as ActionSessionIdArgs, SolveCaptchaArgs, SolveCaptchaResult,
            TeleportArgs, TitleResult, TypeTextArgs, WaitIdleArgs,
        },
        challenge::{
            CaptureChallengeArgs, CaptureChallengeResult, MarkChallengeArgs, ResolutionResult,
            WaitChallengeArgs, WaitChallengeResult,
        },
        download::{
            DownloadArgs, DownloadArmArgs, DownloadArmResult, DownloadResult, DownloadWaitArgs,
        },
        fetch::{FetchArgs, FetchManyArgs, FetchManyResult, FetchResult},
        interrupt::{InterruptIdArgs, InterruptResult, SessionInterruptArgs},
        introspect::PoolStatus,
        network::{
            CookieLeaseOpenArgs, CookieLeaseOpenResult, CookieLeaseRevokeArgs,
            CookieLeaseRevokeResult, NetworkCaptureArmArgs, NetworkCaptureArmResult,
            NetworkCaptureWaitArgs, NetworkCaptureWaitResult, SessionCookiesArgs,
            SessionCookiesResult,
        },
        profile_registry::{
            ProfileCloneArgs, ProfileCreateArgs, ProfileDeleteArgs, ProfileDeleteResult,
            ProfileDescribeArgs, ProfileListArgs, ProfileListResult, ProfilePoolCreateArgs,
            ProfilePoolDescribeArgs, ProfilePoolListArgs, ProfilePoolListResult,
        },
        recording::{RecordArgs, SessionRecordStartArgs, SessionRecordStopArgs},
        screenshot::{ScreenshotArgs, SessionScreenshotArgs},
        session::{
            SessionCloseResult, SessionContentResult, SessionIdArgs, SessionNavigateArgs,
            SessionNavigateResult, SessionOpenArgs, SessionOpenResult,
        },
        snapshot::{FetchSnapshotArgs, PageSnapshot, SessionSnapshotArgs},
        viewport::{DevicePresetsResult, SessionSetViewportArgs},
    },
};

/// The MCP service struct. Cheap to `Arc`-share.
#[derive(Debug)]
pub struct VoidCrawlServer {
    state:       Arc<AppState>,
    #[allow(dead_code, reason = "read by the `#[tool_handler]` macro expansion")]
    tool_router: ToolRouter<Self>,
}

impl VoidCrawlServer {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state, tool_router: Self::tool_router() }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }
}

#[tool_router]
impl VoidCrawlServer {
    #[tool(
        name = "fetch",
        description = "Fetch a URL with stealth headless Chrome and return HTML + metadata. \
Use for single-shot scrapes; for bulk use fetch_many."
    )]
    pub async fn fetch(
        &self,
        Parameters(args): Parameters<FetchArgs>,
    ) -> Result<Json<FetchResult>, ErrorData> {
        tools::fetch::run(self, args).await.map(Json).map_err(map_err)
    }

    #[tool(
        name = "fetch_many",
        description = "Fetch many URLs in parallel over the shared browser pool. Returns \
one entry per request in input order; per-request errors do not abort the batch. \
Each result carries `waited_ms` (time queued for a tab), and the batch carries a \
`pool` summary {max_tabs, submitted, queued, max_waited_ms, note} — if `queued > 0` \
you oversubscribed the pool; cap batches at `max_tabs` (see pool_status) for full parallelism."
    )]
    pub async fn fetch_many(
        &self,
        Parameters(args): Parameters<FetchManyArgs>,
    ) -> Result<Json<FetchManyResult>, ErrorData> {
        Ok(Json(tools::fetch::run_many(self, args).await))
    }

    #[tool(
        name = "fetch_snapshot",
        description = "Fetch a URL with stealth headless Chrome and return a compact rendered-page \
snapshot: headings, text_blocks, links, controls, forms, metadata, and truncation stats. Use as \
the first pass for large pages; use fetch only when you truly need raw HTML."
    )]
    pub async fn fetch_snapshot(
        &self,
        Parameters(args): Parameters<FetchSnapshotArgs>,
    ) -> Result<Json<PageSnapshot>, ErrorData> {
        tools::snapshot::fetch(self, args).await.map(Json).map_err(map_err)
    }

    #[tool(
        name = "download",
        description = "Download a file (PDF, archive, image, …) through stealth Chrome and scan \
it with a built-in Rust antivirus gate (magic-byte type check + yara-x signatures) BEFORE it is \
trusted. The file is fetched into a quarantine dir and only moved into `output_dir` if it passes \
every check; a flagged file is deleted and the result has `ok=false` with a `reason`. Returns \
{ok, verdict, path?, reason?, detected_mime, size}. Use this instead of `fetch` when you need the \
actual bytes of a downloadable resource rather than rendered HTML. OPT-IN: disabled unless the \
server is run with VOIDCRAWL_ALLOW_DOWNLOADS=1. NOTE: a `clean` verdict means it passed the \
size + content-type + bundled-signature checks, not that it is guaranteed malware-free."
    )]
    pub async fn download(
        &self,
        Parameters(args): Parameters<DownloadArgs>,
    ) -> Result<Json<DownloadResult>, ErrorData> {
        tools::download::run(self, args).await.map(Json).map_err(map_err)
    }

    #[tool(
        name = "download_arm",
        description = "Arm an open session to capture the file produced by the NEXT \
download-triggering action — for downloads started by clicking a button (e.g. Google Drive's \
'Download'), where there's no stable URL to pass to `download`. Flow: session_open → \
session_navigate → download_arm → click_by_role(\"button\",\"Download\") (+ \"Download anyway\" if \
an interstitial appears) → download_wait. OPT-IN: needs VOIDCRAWL_ALLOW_DOWNLOADS=1."
    )]
    pub async fn download_arm(
        &self,
        Parameters(args): Parameters<DownloadArmArgs>,
    ) -> Result<Json<DownloadArmResult>, ErrorData> {
        tools::download::arm(self, args).await.map(Json).map_err(map_err)
    }

    #[tool(
        name = "download_wait",
        description = "Wait for the download armed by `download_arm` to land, scan it with the \
antivirus gate, and (if clean) move it into the output dir. Returns {ok, verdict, path?, reason?, \
detected_mime, size}. Call after the click(s) that trigger the download. NOTE: a `clean` verdict \
means it passed the size + bundled-signature checks; the content-type disguise check does NOT run \
on action downloads (no Content-Type is observed), so `clean` is not a malware-free guarantee."
    )]
    pub async fn download_wait(
        &self,
        Parameters(args): Parameters<DownloadWaitArgs>,
    ) -> Result<Json<DownloadResult>, ErrorData> {
        tools::download::wait(self, args).await.map(Json).map_err(map_err)
    }

    #[tool(
        name = "screenshot",
        description = "Load a URL in stealth headless Chrome and return a PNG. Full page by \
default; pass `full_page: false` to capture only the visible viewport (cheaper — no off-screen \
content), `bbox` to crop an exact CSS-pixel region, `selector` to crop a Yosoi selector's resolved \
rectangle instead (any of css/xpath/regex/jsonld/attr/global_id/role/visual — mutually exclusive \
with `bbox`; a selector that matches nothing, is ambiguous, or is inherently non-visual \
(jsonld/regex) fails with invalid_params rather than silently cropping an arbitrary target), \
`viewport` for a one-shot device/size override (preset name from list_device_presets, or custom \
width+height), and `scroll` to page down before cropping. `viewport`/`scroll` never persist past \
this one call."
    )]
    pub async fn screenshot(
        &self,
        Parameters(args): Parameters<ScreenshotArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        tools::screenshot::run(self, args).await
    }

    #[tool(
        name = "session_screenshot",
        description = "Capture a PNG of the given session's page exactly as it stands right now \
— no navigation, no URL change. The visual counterpart to session_content / session_snapshot for \
authenticated, post-click, paginated, or challenge state that only an open session holds. Response \
includes devicePixelRatio guidance compatible with click_visual_coords. Optional one-shot `viewport` \
(preset or custom size), `full_page: false` (visible viewport only, not the whole scroll), `bbox` \
crop, `selector` (crop a Yosoi selector's resolved rectangle — css/xpath/regex/jsonld/attr/ \
global_id/role/visual — mutually exclusive with `bbox`; fails with invalid_params rather than \
guessing when nothing/ambiguous/non-visual resolves), and `scroll` (page down before cropping) — \
none of these persist past this call; use session_set_viewport for a persistent device/size. \
Unknown or closed \
session_ids fail with invalid_params. Prefer session_ax_tree / session_snapshot for structured \
perception; reach for this when you need to see pixels — layout, visual state, or a thin AX tree."
    )]
    pub async fn session_screenshot(
        &self,
        Parameters(args): Parameters<SessionScreenshotArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        tools::screenshot::session(self, args).await
    }

    #[tool(
        name = "record",
        description = "Load a URL in stealth headless Chrome and record it as a sequence of \
timestamped frames written to disk. The moving-picture counterpart to `screenshot`, with the same \
`viewport` / `scroll` / crop options. Frames are NOT returned inline — a recording is hundreds of \
images — so the response carries the output directory, per-region paths and counts; read a single \
frame from disk if you need to see one. Differences from `screenshot`: no `full_page` (a recording \
only ever contains the viewport — use `viewport` for a bigger area or `scroll` to pick which part \
of a long page), `bbox` is viewport-relative, and `selectors` is a LIST — each entry becomes its \
own cropped region cut from one recording, resolved to a rectangle once at start and then held \
fixed (an element that moves drifts out of its crop). Chrome emits frames when it paints, so `fps` \
is a ceiling, not a floor: a static page yields very few frames and that is expected — check \
`effective_fps`. Optional `encode` (gif/mp4/webm) needs the matching build feature."
    )]
    pub async fn record(
        &self,
        Parameters(args): Parameters<RecordArgs>,
    ) -> Result<Json<tools::recording::RecordResult>, ErrorData> {
        tools::recording::run(self, args).await.map(Json).map_err(map_err)
    }

    #[tool(
        name = "session_record_start",
        description = "Begin recording an open session's page, then drive it normally — clicks, \
typing, and navigation all keep recording — and call session_record_stop to finish. Use this \
instead of `record` whenever the thing worth recording is an interaction rather than a page load. \
Takes the same crop/viewport/scroll/fps options as `record`; `max_duration_secs` (default 30, max \
120) is a hard bound after which the recording stops itself, so a forgotten recording can't hold \
the browser. Only one recording per session at a time. Note that a session's tab shares a browser \
window, so recording holds that browser's capture lock: screenshots on other tabs of the same \
browser wait until it stops."
    )]
    pub async fn session_record_start(
        &self,
        Parameters(args): Parameters<SessionRecordStartArgs>,
    ) -> Result<Json<tools::recording::SessionRecordStartResult>, ErrorData> {
        tools::recording::session_start(self, args).await.map(Json).map_err(map_err)
    }

    #[tool(
        name = "session_record_stop",
        description = "Stop the recording started by session_record_start, write the frames and \
any encoded artifact to disk, and return the output paths plus frame counts. Restores the \
session's viewport and scroll position. Fails with invalid_params when no recording is running on \
that session."
    )]
    pub async fn session_record_stop(
        &self,
        Parameters(args): Parameters<SessionRecordStopArgs>,
    ) -> Result<Json<tools::recording::RecordResult>, ErrorData> {
        tools::recording::session_stop(self, args).await.map(Json).map_err(map_err)
    }

    #[tool(
        name = "session_set_viewport",
        description = "Persistently override a session's CDP viewport: dimensions, device pixel \
ratio, mobile/touch identity, and (for a preset) a matching UA — Chrome DevTools' device toolbar as \
a tool call. Stays in effect across subsequent session_navigate/click/screenshot calls until \
session_clear_viewport or another session_set_viewport. Pass `preset` (see list_device_presets, \
e.g. \"iPhone 16 Pro Max\", \"iPad Pro 11\", \"Desktop 1080p\") or custom `width`+`height` \
(+ optional `device_scale_factor`, `mobile`). For a one-off change scoped to a single capture, use \
the `viewport` option on screenshot/session_screenshot instead — it doesn't persist. NOT available \
on stateless fetch/screenshot: pooled tabs are reused across unrelated callers, so a persistent \
device identity there would leak to the next caller."
    )]
    pub async fn session_set_viewport(
        &self,
        Parameters(args): Parameters<SessionSetViewportArgs>,
    ) -> Result<Json<OkResult>, ErrorData> {
        tools::viewport::session_set(self, args).await.map(Json)
    }

    #[tool(
        name = "session_clear_viewport",
        description = "Clear a session_set_viewport override, returning to the session's \
launch-time default viewport."
    )]
    pub async fn session_clear_viewport(
        &self,
        Parameters(args): Parameters<SessionIdArgs>,
    ) -> Result<Json<OkResult>, ErrorData> {
        tools::viewport::session_clear(self, args).await.map(Json)
    }

    #[tool(
        name = "list_device_presets",
        description = "List named device presets available to `viewport` (on screenshot / \
session_screenshot) and session_set_viewport — phones, tablets, and desktop sizes with their CSS \
pixel dimensions, device pixel ratio, and mobile flag. The DevTools device-toolbar dropdown, as data."
    )]
    pub async fn list_device_presets(&self) -> Result<Json<DevicePresetsResult>, ErrorData> {
        Ok(Json(tools::viewport::list_presets()))
    }

    #[tool(
        name = "profile_list",
        description = "List VoidCrawl-managed Chromium profiles. Returns metadata only; cookies and storage values are never exposed."
    )]
    pub async fn profile_list(
        &self,
        Parameters(args): Parameters<ProfileListArgs>,
    ) -> Result<Json<ProfileListResult>, ErrorData> {
        tools::profile_registry::list(self, args).await.map(Json)
    }

    #[tool(
        name = "profile_create",
        description = "Create a standalone VoidCrawl-managed Chromium profile under the managed profile root."
    )]
    pub async fn profile_create(
        &self,
        Parameters(args): Parameters<ProfileCreateArgs>,
    ) -> Result<Json<ManagedProfileDescription>, ErrorData> {
        tools::profile_registry::create(self, args).await.map(Json)
    }

    #[tool(name = "profile_describe", description = "Describe one managed Chromium profile.")]
    pub async fn profile_describe(
        &self,
        Parameters(args): Parameters<ProfileDescribeArgs>,
    ) -> Result<Json<ManagedProfileDescription>, ErrorData> {
        tools::profile_registry::describe(self, args).await.map(Json)
    }

    #[tool(
        name = "profile_clone",
        description = "Clone a managed profile id or source user-data-dir path into a new VoidCrawl-managed profile."
    )]
    pub async fn profile_clone(
        &self,
        Parameters(args): Parameters<ProfileCloneArgs>,
    ) -> Result<Json<ManagedProfileDescription>, ErrorData> {
        tools::profile_registry::clone(self, args).await.map(Json)
    }

    #[tool(
        name = "profile_delete",
        description = "Delete a managed profile if it is not currently leased."
    )]
    pub async fn profile_delete(
        &self,
        Parameters(args): Parameters<ProfileDeleteArgs>,
    ) -> Result<Json<ProfileDeleteResult>, ErrorData> {
        tools::profile_registry::delete(self, args).await.map(Json)
    }

    #[tool(name = "profile_pool_list", description = "List managed Chromium profile pools.")]
    pub async fn profile_pool_list(
        &self,
        Parameters(args): Parameters<ProfilePoolListArgs>,
    ) -> Result<Json<ProfilePoolListResult>, ErrorData> {
        tools::profile_registry::pool_list(self, args).await.map(Json)
    }

    #[tool(
        name = "profile_pool_create",
        description = "Create or replace a named ordered managed-profile pool. Default max_active is 3."
    )]
    pub async fn profile_pool_create(
        &self,
        Parameters(args): Parameters<ProfilePoolCreateArgs>,
    ) -> Result<Json<ProfilePool>, ErrorData> {
        tools::profile_registry::pool_create(self, args).await.map(Json)
    }

    #[tool(
        name = "profile_pool_describe",
        description = "Describe a named managed-profile pool and its profile metadata."
    )]
    pub async fn profile_pool_describe(
        &self,
        Parameters(args): Parameters<ProfilePoolDescribeArgs>,
    ) -> Result<Json<ResolvedProfilePool>, ErrorData> {
        tools::profile_registry::pool_describe(self, args).await.map(Json)
    }

    #[tool(
        name = "session_open",
        description = "Open a new stateful browser session with a dedicated Chrome instance. \
Returns a session_id used by session_navigate / session_content / session_close. \
Pass `profile_id` or `profile_pool` to lease a VoidCrawl-managed profile, or `user_data_dir` \
for an explicit path; omit all for an ephemeral cookieless profile. Set `headful=true` to bring \
up a visible window (useful for a one-time manual login into the persistent profile)."
    )]
    pub async fn session_open(
        &self,
        Parameters(args): Parameters<SessionOpenArgs>,
    ) -> Result<Json<SessionOpenResult>, ErrorData> {
        tools::session::open(self, args).await.map(Json)
    }

    #[tool(
        name = "session_navigate",
        description = "Navigate the given session to a URL and wait for it to settle. \
wait_for accepts 'networkidle' (default) or 'selector:<css>' (event-driven, no polling)."
    )]
    pub async fn session_navigate(
        &self,
        Parameters(args): Parameters<SessionNavigateArgs>,
    ) -> Result<Json<SessionNavigateResult>, ErrorData> {
        tools::session::navigate(self, args).await.map(Json)
    }

    #[tool(
        name = "session_interrupt",
        description = "Explicitly park this stateful session for operator review. No login or CAPTCHA inference occurs; normal mutations fail until session_interrupt_resume or session_interrupt_release."
    )]
    pub async fn session_interrupt(
        &self,
        Parameters(args): Parameters<SessionInterruptArgs>,
    ) -> Result<Json<InterruptResult>, ErrorData> {
        tools::interrupt::begin(self, args).await.map(Json)
    }

    #[tool(
        name = "session_interrupt_status",
        description = "Return redacted state for an explicit session interrupt."
    )]
    pub async fn session_interrupt_status(
        &self,
        Parameters(args): Parameters<InterruptIdArgs>,
    ) -> Result<Json<InterruptResult>, ErrorData> {
        tools::interrupt::status(self, args).await.map(Json)
    }

    #[tool(
        name = "session_interrupt_resume",
        description = "Reactivate a parked session without replaying the action that caused the interrupt."
    )]
    pub async fn session_interrupt_resume(
        &self,
        Parameters(args): Parameters<InterruptIdArgs>,
    ) -> Result<Json<InterruptResult>, ErrorData> {
        tools::interrupt::resume(self, args).await.map(Json)
    }

    #[tool(
        name = "session_interrupt_release",
        description = "Mark a parked interrupt released without replaying a browser action."
    )]
    pub async fn session_interrupt_release(
        &self,
        Parameters(args): Parameters<InterruptIdArgs>,
    ) -> Result<Json<InterruptResult>, ErrorData> {
        tools::interrupt::release(self, args).await.map(Json)
    }

    #[tool(
        name = "session_content",
        description = "Return the current HTML, title, and URL of the given session's page."
    )]
    pub async fn session_content(
        &self,
        Parameters(args): Parameters<SessionIdArgs>,
    ) -> Result<Json<SessionContentResult>, ErrorData> {
        tools::session::content(self, args).await.map(Json)
    }

    #[tool(
        name = "session_snapshot",
        description = "Return a compact rendered-page snapshot for the current stateful session: \
headings, text_blocks, links, controls, forms, metadata, and truncation stats. Use after clicking, \
pagination, login, or other stateful flows; use session_content only when raw HTML is required."
    )]
    pub async fn session_snapshot(
        &self,
        Parameters(args): Parameters<SessionSnapshotArgs>,
    ) -> Result<Json<PageSnapshot>, ErrorData> {
        tools::snapshot::session(self, args).await.map(Json)
    }

    #[tool(
        name = "session_close",
        description = "Close the given session: shut down its Chrome instance and free resources. \
Always call this when you're done — otherwise the browser stays alive until the server exits."
    )]
    pub async fn session_close(
        &self,
        Parameters(args): Parameters<SessionIdArgs>,
    ) -> Result<Json<SessionCloseResult>, ErrorData> {
        tools::session::close(self, args).await.map(Json)
    }

    #[tool(
        name = "pool_status",
        description = "Report the browser pool configuration plus a live snapshot of \
concurrency: `max_tabs`, `available` (free slots right now), `in_flight`, and \
`sessions_open`. Read `available` before a big fan-out to size the batch."
    )]
    pub async fn pool_status(&self) -> Result<Json<PoolStatus>, ErrorData> {
        tools::introspect::pool_status(self).await.map(Json).map_err(map_err)
    }

    #[tool(
        name = "click",
        description = "Click the first element matching a CSS selector in an open session."
    )]
    pub async fn click(
        &self,
        Parameters(args): Parameters<ClickArgs>,
    ) -> Result<Json<OkResult>, ErrorData> {
        tools::actions::click(self, args).await.map(Json)
    }

    #[tool(
        name = "teleport",
        description = "Override the session's geolocation (and optionally timezone + locale) so \
navigator.geolocation and location-aware sites resolve to the given lat/lon — 'teleport' the \
browser. The geolocation permission is granted automatically. Call after session_open and \
BEFORE navigating; the override persists across navigations. For Google Maps 'near me' queries: \
use a FRESH session per location, and navigate to the search twice (prime + read) — Maps resolves \
location on first load and applies it on the next request."
    )]
    pub async fn teleport(
        &self,
        Parameters(args): Parameters<TeleportArgs>,
    ) -> Result<Json<OkResult>, ErrorData> {
        tools::actions::teleport(self, args).await.map(Json)
    }

    #[tool(
        name = "click_visual_coords",
        description = "Click at pixel coordinates (x, y) in CSS pixels. Use when selector-based \
clicks fail silently (React forms that ignore dispatchEvent clicks). Coords are pre-DPR: \
divide screenshot pixels by devicePixelRatio on HiDPI."
    )]
    pub async fn click_visual_coords(
        &self,
        Parameters(args): Parameters<ClickVisualCoordsArgs>,
    ) -> Result<Json<OkResult>, ErrorData> {
        tools::actions::click_visual_coords(self, args).await.map(Json)
    }

    #[tool(
        name = "type_text",
        description = "Type text into an input. With `selector`, focuses + types. Without, \
dispatches keys to whatever currently has focus (pair with click_visual_coords first)."
    )]
    pub async fn type_text(
        &self,
        Parameters(args): Parameters<TypeTextArgs>,
    ) -> Result<Json<OkResult>, ErrorData> {
        tools::actions::type_text(self, args).await.map(Json)
    }

    #[tool(
        name = "eval_js",
        description = "Evaluate a JS expression in the session's page. Returns the value as JSON."
    )]
    pub async fn eval_js(
        &self,
        Parameters(args): Parameters<EvalJsArgs>,
    ) -> Result<Json<EvalJsResult>, ErrorData> {
        tools::actions::eval_js(self, args).await.map(Json)
    }

    #[tool(
        name = "eval_js_in_frame",
        description = "Evaluate a JS expression inside a specific (possibly cross-origin) iframe, \
                       selected by a substring of its URL. The expression runs in that frame's own \
                       execution context (`document` is the frame's document) — the way to read or \
                       drive an iframe whose `contentDocument` is null from the parent. Returns the \
                       value as JSON."
    )]
    pub async fn eval_js_in_frame(
        &self,
        Parameters(args): Parameters<EvalJsInFrameArgs>,
    ) -> Result<Json<EvalJsResult>, ErrorData> {
        tools::actions::eval_js_in_frame(self, args).await.map(Json)
    }

    #[tool(name = "title", description = "Return the current document title of the session.")]
    pub async fn title(
        &self,
        Parameters(args): Parameters<ActionSessionIdArgs>,
    ) -> Result<Json<TitleResult>, ErrorData> {
        tools::actions::title(self, args).await.map(Json)
    }

    #[tool(
        name = "extract",
        description = "Run document.querySelectorAll(selector) and return each element's text content."
    )]
    pub async fn extract(
        &self,
        Parameters(args): Parameters<ExtractArgs>,
    ) -> Result<Json<ExtractResult>, ErrorData> {
        tools::actions::extract(self, args).await.map(Json)
    }

    #[tool(
        name = "session_ax_tree",
        description = "Return the page's accessibility (AX) tree — the semantic view assistive \
tech sees, with implicit roles resolved, accessible names computed, and hidden nodes pruned. \
Default `mode=compact` gives a pruned, indented role/name outline for reading; `mode=raw` gives \
full CDP nodes. `named_count` vs `node_count` signals AX richness: when low, fall back to HTML, \
screenshot, or CSS selectors. Complements (does not replace) the DOM/visual tools."
    )]
    pub async fn session_ax_tree(
        &self,
        Parameters(args): Parameters<AxTreeArgs>,
    ) -> Result<Json<AxTreeResult>, ErrorData> {
        tools::actions::ax_tree(self, args).await.map(Json)
    }

    #[tool(
        name = "click_by_role",
        description = "Click an element by its accessibility role + accessible name (e.g. \
role=\"button\", name=\"Load more\") instead of a CSS selector. More durable across redesigns, \
but flakier when names are ambiguous, localized, or duplicated — pair with session_ax_tree to \
see available roles/names, and fall back to `click` (CSS) or `click_visual_coords` when it fails."
    )]
    pub async fn click_by_role(
        &self,
        Parameters(args): Parameters<ClickByRoleArgs>,
    ) -> Result<Json<OkResult>, ErrorData> {
        tools::actions::click_by_role(self, args).await.map(Json)
    }

    #[tool(
        name = "wait_for_network_idle",
        description = "Wait for Chrome's network-idle lifecycle event. Event-driven, no polling."
    )]
    pub async fn wait_for_network_idle(
        &self,
        Parameters(args): Parameters<WaitIdleArgs>,
    ) -> Result<Json<OkResult>, ErrorData> {
        tools::actions::wait_for_network_idle(self, args).await.map(Json)
    }

    #[tool(
        name = "network_capture",
        description = "Return the Resource Timing entries (URL, initiator type, transfer size, duration) \
observed since the session's most recent navigation. Backed by performance.getEntriesByType('resource')."
    )]
    pub async fn network_capture(
        &self,
        Parameters(args): Parameters<ActionSessionIdArgs>,
    ) -> Result<Json<NetworkCaptureResult>, ErrorData> {
        tools::actions::network_capture(self, args).await.map(Json)
    }

    #[tool(
        name = "network_capture_arm",
        description = "Arm an open session to capture request headers, response headers, status, \
and (opt-in) body of the NEXT requests matching one or more named URL globs — real CDP Network.* \
events, unlike `network_capture` which reads the Resource Timing API and has no headers/body. \
Flow: session_open → session_navigate → network_capture_arm(patterns=[{name,url_glob}]) → the \
click/navigation that triggers the requests → network_capture_wait. `request_headers` is where \
an Authorization bearer set by page code appears. Credential-bearing header values come back as \
`<redacted>`; raw values need BOTH include_sensitive_headers:true AND the operator setting \
VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE=1, else the call is refused. CAVEATS: `url` is returned raw \
and may itself embed a token (presigned URL, ?access_token=); bodies are not credential-scanned; \
cookies are NOT captured from the wire in either direction (Chrome reports them only via \
*ExtraInfo events, which the vendored CDP client does not deliver) — use session_cookies."
    )]
    pub async fn network_capture_arm(
        &self,
        Parameters(args): Parameters<NetworkCaptureArmArgs>,
    ) -> Result<Json<NetworkCaptureArmResult>, ErrorData> {
        tools::network::arm(self, args).await.map(Json)
    }

    #[tool(
        name = "network_capture_wait",
        description = "Wait for the capture armed by `network_capture_arm` to observe every named \
pattern, and return each match's status, request/response headers, and (if `capture_body` was \
set) base64 body. Call after the action(s) that trigger the requests. `timeout_secs` (default 30) \
is measured from THIS call, so time spent typing/clicking between arm and wait does not consume \
it. A timeout means the armed globs never matched a real request — re-check the pattern against \
the URL the page actually requested."
    )]
    pub async fn network_capture_wait(
        &self,
        Parameters(args): Parameters<NetworkCaptureWaitArgs>,
    ) -> Result<Json<NetworkCaptureWaitResult>, ErrorData> {
        tools::network::wait(self, args).await.map(Json)
    }

    #[tool(
        name = "session_cookies",
        description = "Return every cookie CDP can see for the session's current page — including \
HttpOnly and Secure cookies invisible to document.cookie. OPT-IN: returns raw cookie VALUES, so \
it is refused unless the operator sets VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE=1. INTERIM/RAW: a \
direct wrapper over the core cookie-read API, not the CAS-251 leased cookie-jar design — no \
scoping, no TTL/revoke, no value-free provenance, no eligibility classification. Use for local \
debugging/demos, not as a safe handoff surface."
    )]
    pub async fn session_cookies(
        &self,
        Parameters(args): Parameters<SessionCookiesArgs>,
    ) -> Result<Json<SessionCookiesResult>, ErrorData> {
        tools::network::cookies(self, args).await.map(Json)
    }

    #[tool(
        name = "cookie_lease_open",
        description = "Fork the cookies reachable by one replay origin out of a live session into \
an in-memory, revocable lease, and return the lease id plus a VALUE-FREE provenance record per \
cookie: name, domain, path, issuing origin, top-level site (the CHIPS partition key — the site \
the browser was on when the cookie was set), expiry/session state, HttpOnly/Secure/SameSite, \
source scheme/port, and whether a value is present. Cookie VALUES never cross this boundary, so \
the result is safe to persist in a trace or artifact. The lease is scope-bound (you cannot ask \
for all cookies) and dies with its browser session. This tool reports facts only — it does NOT \
classify replay eligibility (expired / HTTPS-only / scope mismatch / partitioned-context), which \
is the caller's policy decision. Use `session_cookies` only for local debugging with raw values."
    )]
    pub async fn cookie_lease_open(
        &self,
        Parameters(args): Parameters<CookieLeaseOpenArgs>,
    ) -> Result<Json<CookieLeaseOpenResult>, ErrorData> {
        tools::network::cookie_lease_open(self, args).await.map(Json)
    }

    #[tool(
        name = "cookie_lease_revoke",
        description = "Revoke a cookie lease, scrubbing its held values and failing any later use \
closed with a recorded reason. Pass a machine-readable `reason` (e.g. \"auth_failed\") so a \
downstream failure can be explained. Idempotent: revoking an unknown or already-revoked lease \
returns revoked=false rather than erroring. Leases are also revoked automatically when their \
session closes."
    )]
    pub async fn cookie_lease_revoke(
        &self,
        Parameters(args): Parameters<CookieLeaseRevokeArgs>,
    ) -> Result<Json<CookieLeaseRevokeResult>, ErrorData> {
        tools::network::cookie_lease_revoke(self, args).await.map(Json)
    }

    #[tool(
        name = "solve_captcha",
        description = "Click the Turnstile / reCAPTCHA-v2 / hCaptcha checkbox in an open session \
using real CDP mouse events (not JS click — widgets detect that) and wait for the response \
token to appear. Returns the kind detected, the coordinates clicked, the token value (once \
the widget writes it into its hidden input), and a `solved` flag. No-op (solved=true) when \
the page has no captcha. Only handles widgets whose anchor frame is already visible — if \
detect_captcha reports `turnstile` because the runtime loaded but no widget mounted, trigger \
the form submit that mounts the widget first."
    )]
    pub async fn solve_captcha(
        &self,
        Parameters(args): Parameters<SolveCaptchaArgs>,
    ) -> Result<Json<SolveCaptchaResult>, ErrorData> {
        tools::actions::solve_captcha(self, args).await.map(Json)
    }

    #[tool(
        name = "detect_captcha",
        description = "Probe the DOM for captcha / bot-wall markers. Returns the kind tag \
(recaptcha, hcaptcha, turnstile, cloudflare_challenge, datadome) or null."
    )]
    pub async fn detect_captcha(
        &self,
        Parameters(args): Parameters<ActionSessionIdArgs>,
    ) -> Result<Json<DetectCaptchaResult>, ErrorData> {
        tools::actions::detect_captcha_tool(self, args).await.map(Json)
    }

    #[tool(
        name = "capture_captcha",
        description = "Deep structured probe of a captcha challenge. Returns kind, sitekey, \
widget rect + selector, response-field selector, existing token (if already solved), page URL, \
and Turnstile action/cdata attrs. Use this to hand off to a third-party solver API \
(2Captcha / CapSolver / Anti-Captcha) or a human-in-the-loop flow, then call \
`inject_captcha_token` with the resulting token."
    )]
    pub async fn capture_captcha(
        &self,
        Parameters(args): Parameters<ActionSessionIdArgs>,
    ) -> Result<Json<CaptureCaptchaResult>, ErrorData> {
        tools::actions::capture_captcha_tool(self, args).await.map(Json)
    }

    #[tool(
        name = "inject_captcha_token",
        description = "Write a solved captcha token into the page's hidden response field and \
fire input/change events so React-controlled forms pick it up. For Turnstile, invokes any \
registered `data-callback` function. `kind` defaults to whatever is currently detected; pass \
explicitly ('turnstile'/'recaptcha'/'hcaptcha') to skip re-detection."
    )]
    pub async fn inject_captcha_token(
        &self,
        Parameters(args): Parameters<InjectCaptchaTokenArgs>,
    ) -> Result<Json<OkResult>, ErrorData> {
        tools::actions::inject_captcha_token_tool(self, args).await.map(Json)
    }

    #[tool(
        name = "capture_challenge",
        description = "Capture the current session's active anti-bot/captcha challenge as a \
neutral event. Combines the last session_navigate anti-bot verdict with a live DOM captcha probe, \
and returns same-tab attach coordinates {websocket_url,target_id,session_id} plus optional vnc_url \
and novnc_url. V1 flow: open noVNC/VNC, clear the wall manually, then call \
mark_challenge_resolved and wait_for_challenge_resolution. Presence-only CDN signals do not create \
a blocking event."
    )]
    pub async fn capture_challenge(
        &self,
        Parameters(args): Parameters<CaptureChallengeArgs>,
    ) -> Result<Json<CaptureChallengeResult>, ErrorData> {
        tools::challenge::capture(self, args).await.map(Json)
    }

    #[tool(
        name = "mark_challenge_resolved",
        description = "Mark a captured challenge event resolved after the operator or resolver \
clears it in the same tab. Defaults resolver=manual_vnc; later phases can pass yosoi_recipe, \
open_sesame_session_actor, or agent_mcp."
    )]
    pub async fn mark_challenge_resolved(
        &self,
        Parameters(args): Parameters<MarkChallengeArgs>,
    ) -> Result<Json<ResolutionResult>, ErrorData> {
        tools::challenge::mark_resolved(self, args).await.map(Json)
    }

    #[tool(
        name = "mark_challenge_failed",
        description = "Mark a captured challenge event failed. Use when manual VNC/noVNC or an \
automated resolver cannot clear the wall; callers can then rotate identity or fail with evidence."
    )]
    pub async fn mark_challenge_failed(
        &self,
        Parameters(args): Parameters<MarkChallengeArgs>,
    ) -> Result<Json<ResolutionResult>, ErrorData> {
        tools::challenge::mark_failed(self, args).await.map(Json)
    }

    #[tool(
        name = "wait_for_challenge_resolution",
        description = "Wait for mark_challenge_resolved/failed on an active challenge event. When \
resolved, re-probes the DOM by default so callers can confirm the wall is gone before resuming."
    )]
    pub async fn wait_for_challenge_resolution(
        &self,
        Parameters(args): Parameters<WaitChallengeArgs>,
    ) -> Result<Json<WaitChallengeResult>, ErrorData> {
        tools::challenge::wait_for_resolution(self, args).await.map(Json)
    }
}

#[tool_handler]
impl ServerHandler for VoidCrawlServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = {
            let mut imp = Implementation::default();
            imp.name = "voidcrawl-mcp".into();
            imp.version = VERSION.into();
            imp
        };
        // Shipped to EVERY MCP client on connect (Claude, opencode, Codex,
        // Cursor, Cline, Zed, …), so the AX-first workflow + gotchas reach
        // hosts that have no skill-file mechanism. Keep this condensed; the
        // full guide is .claude/skills/voidcrawl/SKILL.md.
        info.instructions = Some(
            "Stealthy headless Chrome over a shared, fingerprint-patched tab pool — a drop-in \
replacement for Playwright / Chromium MCP.\n\n\
WORKFLOW. Stateless perception: `fetch_snapshot` for a compact rendered-page snapshot. Stateless \
raw scrape: `fetch` (one URL) or `fetch_many` (parallel; returns \
{results:[{ok,result,error}]} in input order — per-item errors don't abort the batch, and \
status_code is nested under each item's `result`). Stateful flows (login, pagination, clicking): \
`session_open` → `session_navigate` → `session_snapshot` / actions → … → `session_close`. ALWAYS \
session_close; sessions are cookie-isolated.\n\n\
PERCEIVE → ACT → EXTRACT. To inspect a large rendered page, prefer `fetch_snapshot` first, or \
`session_snapshot` after clicking/pagination/login flows. For role/name interaction targeting, \
call `session_ax_tree` — a compact outline of the accessibility tree. If `named_count` is low vs \
`node_count` the accessibility tree is thin; fall back to `session_snapshot` or `session_screenshot`. \
`session_screenshot` captures the current session's page as-is (no navigation) — reach for it before \
`click_visual_coords` to see authenticated, post-click, paginated, or challenge state that `fetch`-based \
`screenshot` can't reach. Use raw `fetch` / `session_content` only when you truly need markup. To click: `click` (CSS selector) \
or `click_by_role` (accessibility role + accessible name — durable across redesigns); last resort \
`click_visual_coords` for React forms that ignore synthetic clicks. To extract data, run `extract` \
/ `eval_js` with a JS expression and return data, not markup.\n\n\
GOTCHAS. `click_by_role` name matching is EXACT (case + whitespace) — read the exact name from \
`session_ax_tree` first; use `nth` for duplicates. After an in-page (SPA) click, \
`wait_for_network_idle` may run to its full timeout — pass a short `timeout_secs` or use \
`wait_for:\"selector:<css>\"`. On a captcha error, surface it and rotate proxy/profile; don't \
retry the same URL. For operator handoff, use `capture_challenge` to get same-tab attach \
coordinates plus VNC/noVNC links, clear the wall manually, then call \
`mark_challenge_resolved` and `wait_for_challenge_resolution`. Phase-3 automated resolvers \
attach to the same `{websocket_url,target_id}` and must mark resolved or failed."
                .into(),
        );
        info
    }
}
