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

use crate::{
    errors::map_err,
    state::AppState,
    tools,
    tools::{
        actions::{
            ClickArgs, ClickVisualCoordsArgs, DetectCaptchaResult, EvalJsArgs, EvalJsResult,
            ExtractArgs, ExtractResult, NetworkCaptureResult, OkResult,
            SessionIdArgs as ActionSessionIdArgs, TitleResult, TypeTextArgs, WaitIdleArgs,
        },
        fetch::{FetchArgs, FetchManyArgs, FetchManyResult, FetchResult},
        introspect::PoolStatus,
        screenshot::ScreenshotArgs,
        session::{
            SessionCloseResult, SessionContentResult, SessionIdArgs, SessionNavigateArgs,
            SessionNavigateResult, SessionOpenArgs, SessionOpenResult,
        },
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
one entry per request in input order; per-request errors do not abort the batch."
    )]
    pub async fn fetch_many(
        &self,
        Parameters(args): Parameters<FetchManyArgs>,
    ) -> Result<Json<FetchManyResult>, ErrorData> {
        Ok(Json(tools::fetch::run_many(self, args).await))
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
        name = "session_open",
        description = "Open a new stateful browser session with a dedicated Chrome instance. \
Returns a session_id used by session_navigate / session_content / session_close. \
Pass `user_data_dir` to mount a persistent profile (e.g. one already logged into LinkedIn); \
omit it for an ephemeral cookieless profile. Set `headful=true` to bring up a visible window \
(useful for a one-time manual login into the persistent profile)."
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
        description = "Report the current browser pool configuration and how many dedicated \
sessions are open. Useful for sanity-checking concurrency limits before a fan-out."
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
            imp.version = env!("CARGO_PKG_VERSION").into();
            imp
        };
        info.instructions = Some(
            "Stealthy headless browser automation over a shared Chrome pool. \
Use `fetch` / `fetch_many` for stateless scrapes; `session_open` + `session_navigate` + \
`session_content` + `session_close` for login/pagination flows. Sessions are isolated \
(each gets its own Chrome profile), so subagents never share cookies."
                .into(),
        );
        info
    }
}
