//! Stateful session tools. Each `session_open` launches a dedicated
//! headless `BrowserSession` with its own temporary profile; callers
//! hold the returned `session_id` across tool calls until
//! `session_close`.

use std::{env, sync::Arc, time::Duration};

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;
use void_crawl_core::{BrowserSession, VoidCrawlError};

use crate::{errors::map_err, server::VoidCrawlServer, sessions::DedicatedSession, tools::wait};

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct SessionOpenArgs {
    /// Run headful (visible) instead of headless. Default is headless.
    /// Set this to true if you want to log into a site manually in the
    /// spawned Chrome window (pair with `user_data_dir` to persist).
    #[serde(default)]
    pub headful:       bool,
    /// Optional proxy URL (e.g. "http://user:pass@host:port").
    #[serde(default)]
    pub proxy:         Option<String>,
    /// Persistent Chrome profile directory. Omit for an ephemeral,
    /// cookieless profile. Provide a path (e.g.
    /// `~/.config/voidcrawl-linkedin`) to mount a profile across
    /// sessions — log in once with `headful=true`, then subsequent
    /// sessions reuse the cookie. Pick a path DEDICATED to voidcrawl;
    /// Chrome locks a profile while running, so pointing at your
    /// daily-driver profile while normal Chrome is open will fail.
    #[serde(default)]
    pub user_data_dir: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionOpenResult {
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct SessionNavigateArgs {
    pub session_id:   String,
    pub url:          String,
    /// "networkidle" (default) or "selector:<css>". Event-driven.
    #[serde(default)]
    pub wait_for:     Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionNavigateResult {
    pub url:         String,
    pub status_code: Option<u16>,
    pub redirected:  bool,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct SessionIdArgs {
    pub session_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionContentResult {
    pub url:   Option<String>,
    pub title: Option<String>,
    pub html:  String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionCloseResult {
    pub closed: bool,
}

pub async fn open(
    server: &VoidCrawlServer,
    args: SessionOpenArgs,
) -> Result<SessionOpenResult, ErrorData> {
    let mut builder = BrowserSession::builder();
    builder = if args.headful { builder.headful() } else { builder.headless() };
    if let Some(proxy) = args.proxy {
        builder = builder.proxy(proxy);
    }
    if let Some(path) = args.user_data_dir {
        builder = builder.user_data_dir(expand_tilde(&path));
    }
    let session = builder.launch().await.map_err(map_err)?;
    let page = session.new_blank_page().await.map_err(map_err)?;
    let id = Uuid::new_v4().to_string();
    let handle =
        Arc::new(DedicatedSession { session: Arc::new(session), page: Mutex::new(page) });
    server.state().sessions.insert(id.clone(), handle).await;
    Ok(SessionOpenResult { session_id: id })
}

pub async fn navigate(
    server: &VoidCrawlServer,
    args: SessionNavigateArgs,
) -> Result<SessionNavigateResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let resp = page.goto_and_wait_for_idle(&args.url, timeout).await.map_err(map_err)?;
    wait::apply(&page, args.wait_for.as_deref(), timeout).await.map_err(map_err)?;
    Ok(SessionNavigateResult {
        url:         resp.url,
        status_code: resp.status_code,
        redirected:  resp.redirected,
    })
}

pub async fn content(
    server: &VoidCrawlServer,
    args: SessionIdArgs,
) -> Result<SessionContentResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let html = page.content().await.map_err(map_err)?;
    let title = page.title().await.ok().flatten();
    let url = page.url().await.ok().flatten();
    Ok(SessionContentResult { url, title, html })
}

pub async fn close(
    server: &VoidCrawlServer,
    args: SessionIdArgs,
) -> Result<SessionCloseResult, ErrorData> {
    let Some(handle) = server.state().sessions.remove(&args.session_id).await else {
        return Ok(SessionCloseResult { closed: false });
    };
    close_handle(handle).await.map_err(map_err)?;
    Ok(SessionCloseResult { closed: true })
}

async fn lookup(server: &VoidCrawlServer, id: &str) -> Result<Arc<DedicatedSession>, ErrorData> {
    server
        .state()
        .sessions
        .get(id)
        .await
        .ok_or_else(|| ErrorData::invalid_params(format!("unknown session_id: {id}"), None))
}

/// Shut down the browser backing a session.
pub async fn close_handle(handle: Arc<DedicatedSession>) -> Result<(), VoidCrawlError> {
    handle.session.close().await
}

/// Expand a leading `~/` or bare `~` using the `HOME` env var. Returns
/// the input unchanged if `~` isn't leading or if `HOME` is unset —
/// callers pass absolute paths, so either behaviour is a no-op in the
/// common case.
fn expand_tilde(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else { return path.to_owned() };
    let Ok(home) = env::var("HOME") else { return path.to_owned() };
    if rest.is_empty() {
        home
    } else if let Some(tail) = rest.strip_prefix('/') {
        format!("{home}/{tail}")
    } else {
        path.to_owned()
    }
}
