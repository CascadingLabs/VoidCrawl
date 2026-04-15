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
        description = "Open a new stateful browser session with its own isolated profile. \
Returns a session_id used by session_navigate / session_content / session_close. Each \
session is a dedicated Chrome, so cookies and storage never leak between sessions."
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
wait_for accepts 'networkidle' (default), 'selector:<css>', or 'ms:<n>'."
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
        Ok(Json(tools::introspect::pool_status(self).await))
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
