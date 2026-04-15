//! `voidcrawl-mcp` binary — stdio MCP server exposing `void_crawl_core`.

use std::{io::stderr, sync::Arc};

use rmcp::{ServiceExt, transport::io::stdio};
use tracing_subscriber::EnvFilter;
use void_crawl_core::BrowserPool;
use voidcrawl_mcp::{
    AppState, VoidCrawlServer, sessions::SessionRegistry, tools::session::close_handle,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("voidcrawl-mcp starting");

    let pool = Arc::new(BrowserPool::from_env().await?);
    Arc::clone(&pool).start_eviction_task();
    let sessions = Arc::new(SessionRegistry::default());
    let state = Arc::new(AppState::new(Arc::clone(&pool), Arc::clone(&sessions)));

    let server = VoidCrawlServer::new(state);
    let service = server.serve(stdio()).await?;

    tracing::info!("voidcrawl-mcp ready");
    let quit = service.waiting().await?;
    tracing::info!(reason = ?quit, "voidcrawl-mcp shutting down");

    for handle in sessions.drain().await {
        if let Err(e) = close_handle(handle).await {
            tracing::warn!(error = %e, "failed to close dedicated session");
        }
    }
    if let Err(e) = pool.close().await {
        tracing::warn!(error = %e, "failed to close browser pool");
    }

    Ok(())
}
