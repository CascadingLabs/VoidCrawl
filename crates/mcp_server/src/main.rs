//! `voidcrawl-mcp` binary — stdio MCP server exposing `void_crawl_core`.

use std::{
    env,
    io::stderr,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use clap::{Parser, Subcommand};
use rmcp::{ServiceExt, transport::io::stdio};
use tracing_subscriber::EnvFilter;
use void_crawl_core::{acquire_profile, chrome_user_data_dirs};
use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    install::{self, InstallArgs},
    sessions::SessionRegistry,
    state::PinnedProfile,
    tools::session::close_handle,
};

/// Stdio MCP server for stealth headless Chrome.
///
/// With no subcommand it runs the server over stdio — how MCP hosts launch
/// it. `install`/`uninstall` wire it into Claude Code, Codex, and opencode.
#[derive(Parser, Debug)]
#[command(name = "voidcrawl-mcp", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Pin the server to a warm Chrome profile (else $VOIDCRAWL_PROFILE).
    #[arg(long)]
    profile: Option<String>,

    /// Run the pinned-profile Chrome visible (else $VOIDCRAWL_HEADFUL=1).
    /// Only meaningful with `--profile`: lets a warm, visible window clear
    /// anti-bot pre-checks (Turnstile, Cloudflare) that fingerprint headless.
    #[arg(long)]
    headful: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Wire voidcrawl into Claude Code, Codex, and opencode.
    Install(InstallArgs),
    /// Remove that wiring.
    Uninstall(InstallArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Some(Command::Install(args)) => return install::run(false, args),
        Some(Command::Uninstall(args)) => return install::run(true, args),
        None => {}
    }

    // No subcommand → run the stdio MCP server (the default mode hosts launch).
    tracing_subscriber::fmt()
        .with_writer(stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let profile_name = cli
        .profile
        .clone()
        .or_else(|| env::var("VOIDCRAWL_PROFILE").ok().filter(|s| !s.is_empty()));
    let headful =
        cli.headful || matches!(env::var("VOIDCRAWL_HEADFUL").as_deref(), Ok("1" | "true"));
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
