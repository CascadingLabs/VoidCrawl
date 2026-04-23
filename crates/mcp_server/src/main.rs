//! `voidcrawl-mcp` binary — stdio MCP server exposing `void_crawl_core`.

use std::{env, io::stderr, sync::Arc, time::Duration};

use rmcp::{ServiceExt, transport::io::stdio};
use tracing_subscriber::EnvFilter;
use void_crawl_core::{ProfileHandle, acquire_profile};
use voidcrawl_mcp::{
    AppState, VoidCrawlServer, sessions::SessionRegistry, tools::session::close_handle,
};

/// Parse `--profile NAME` / `--profile=NAME` from argv, falling back
/// to the `VOIDCRAWL_PROFILE` env var. Returns `None` if neither is
/// set — the server then runs in its regular pool-from-env mode.
fn resolve_profile_arg() -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--profile" {
            return args.next();
        }
        if let Some(rest) = arg.strip_prefix("--profile=") {
            return Some(rest.to_string());
        }
    }
    env::var("VOIDCRAWL_PROFILE").ok().filter(|s| !s.is_empty())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let profile_name = resolve_profile_arg();
    let profile_handle: Option<ProfileHandle> = if let Some(name) = profile_name.as_deref() {
        tracing::info!(profile = name, "acquiring Chrome profile");
        // 30s poll window. Fail loud — if the profile is busy or not
        // found, there's nothing for the server to do without it.
        // Server-mode default: headless, matching the rest of the MCP
        // pool. Users who want a visible profile window can run a
        // standalone Python script with `with_profile(..., headless=False)`.
        let handle = acquire_profile(name, Duration::from_secs(30), true).await?;
        tracing::info!(profile = name, path = %handle.path().display(), "profile acquired");
        Some(handle)
    } else {
        None
    };

    tracing::info!("voidcrawl-mcp starting");

    let sessions = Arc::new(SessionRegistry::default());
    let state = Arc::new(AppState::new(Arc::clone(&sessions)));

    let server = VoidCrawlServer::new(Arc::clone(&state));
    let service = server.serve(stdio()).await?;

    tracing::info!("voidcrawl-mcp ready");
    let quit = service.waiting().await?;
    tracing::info!(reason = ?quit, "voidcrawl-mcp shutting down");

    for handle in sessions.drain().await {
        if let Err(e) = close_handle(handle).await {
            tracing::warn!(error = %e, "failed to close dedicated session");
        }
    }
    if let Some(pool) = state.pool_if_initialized() {
        if let Err(e) = pool.close().await {
            tracing::warn!(error = %e, "failed to close browser pool");
        }
    }
    if let Some(mut handle) = profile_handle {
        if let Err(e) = handle.close().await {
            tracing::warn!(error = %e, "failed to release profile");
        }
    }

    Ok(())
}
