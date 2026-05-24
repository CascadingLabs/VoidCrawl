//! `voidcrawl-mcp` binary — stdio MCP server exposing `void_crawl_core`.

use std::{
    env,
    io::stderr,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use rmcp::{ServiceExt, transport::io::stdio};
use tracing_subscriber::EnvFilter;
use void_crawl_core::{acquire_profile, chrome_user_data_dirs};
use voidcrawl_mcp::{
    AppState, VoidCrawlServer, sessions::SessionRegistry, state::PinnedProfile,
    tools::session::close_handle,
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

/// `--headful` flag / `VOIDCRAWL_HEADFUL=1` env. Only meaningful when
/// paired with `--profile`: makes the pinned-profile Chrome visible.
/// Useful when the target runs an anti-bot pre-check (Turnstile,
/// Cloudflare managed challenge) that fingerprints headless Chrome —
/// warm profile + visible window is the fallback when stealth alone
/// isn't enough.
fn resolve_headful_flag() -> bool {
    if env::args().any(|a| a == "--headful") {
        return true;
    }
    matches!(env::var("VOIDCRAWL_HEADFUL").as_deref(), Ok("1" | "true"))
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
    let headful = resolve_headful_flag();
    let headless = !headful;
    let sessions = Arc::new(SessionRegistry::default());

    let state = if let Some(name) = profile_name.as_deref() {
        tracing::info!(profile = name, headful, "acquiring Chrome profile");
        let mut handle = acquire_profile(name, Duration::from_secs(30), headless).await?;
        let session = handle.take_session().ok_or_else(|| {
            anyhow::anyhow!("profile handle returned without a session — should be unreachable")
        })?;
        // Chrome's user-data-dir is the PARENT of the profile folder.
        // Pick the first platform dir that contains our profile.
        let user_data_root = chrome_user_data_dirs()
            .into_iter()
            .find(|b| b.join(name).is_dir())
            .unwrap_or_else(|| handle.path().to_path_buf());
        tracing::info!(
            profile = name,
            path = %handle.path().display(),
            user_data_root = %user_data_root.display(),
            "profile acquired — pool will inherit its Chrome"
        );
        let pinned = PinnedProfile {
            handle,
            session: StdMutex::new(Some(session)),
            name: name.to_string(),
            user_data_root,
        };
        Arc::new(AppState::with_pinned_profile(Arc::clone(&sessions), pinned))
    } else {
        Arc::new(AppState::new(Arc::clone(&sessions)))
    };

    tracing::info!("voidcrawl-mcp starting");

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
    // Dropping `state` releases the `Arc<PinnedProfile>` → the
    // `ProfileHandle` inside → its `fs2` advisory lock guard. Chrome
    // itself was already shut down via `pool.close()` above (the pool
    // owns the session that was extracted from the handle).
    if let Some(ref pinned) = state.pinned {
        tracing::info!(profile = %pinned.name, "releasing pinned profile");
    }
    drop(state);

    Ok(())
}
