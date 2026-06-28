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
        introspect::PoolStatus,
        profile_registry::{
            ProfileCloneArgs, ProfileCreateArgs, ProfileDeleteArgs, ProfileDeleteResult,
            ProfileDescribeArgs, ProfileListArgs, ProfileListResult, ProfilePoolCreateArgs,
            ProfilePoolDescribeArgs, ProfilePoolListArgs, ProfilePoolListResult,
        },
        screenshot::ScreenshotArgs,
        session::{
            SessionCloseResult, SessionContentResult, SessionIdArgs, SessionNavigateArgs,
            SessionNavigateResult, SessionOpenArgs, SessionOpenResult,
        },
        snapshot::{FetchSnapshotArgs, PageSnapshot, SessionSnapshotArgs},
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
        description = "Load a URL in stealth headless Chrome and return a full-page PNG."
    )]
    pub async fn screenshot(
        &self,
        Parameters(args): Parameters<ScreenshotArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        tools::screenshot::run(self, args).await
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
`node_count` the accessibility tree is thin; fall back to `session_snapshot` or `screenshot`. Use \
raw `fetch` / `session_content` only when you truly need markup. To click: `click` (CSS selector) \
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
