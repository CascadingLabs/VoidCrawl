//! Explicit policy-interrupt tools for retained stateful sessions.

use std::{sync::Arc, time::Duration};

use tokio::time::sleep;

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use void_crawl_core::{InterruptInfo, InterruptRequest, InterruptState};

use crate::{
    errors::map_err,
    server::VoidCrawlServer,
    sessions::{DedicatedSession, SessionRegistry},
    tools::session::close_handle,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionInterruptArgs {
    pub session_id: String,
    /// Stable caller-owned policy code, e.g. `policy.operator_review`.
    pub code: String,
    /// Redacted operator-facing reason. Never put credentials or cookies here.
    pub summary: String,
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u64,
}

fn default_ttl() -> u64 {
    600
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InterruptIdArgs {
    pub session_id: String,
    pub interrupt_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InterruptResult {
    pub interrupt_id: String,
    pub target_id: String,
    pub code: String,
    pub summary: String,
    pub state: String,
    pub expires_in_ms: u64,
}

impl From<InterruptInfo> for InterruptResult {
    fn from(info: InterruptInfo) -> Self {
        #[allow(clippy::cast_possible_truncation)]
        let expires_in_ms = info.expires_in.as_millis().min(u128::from(u64::MAX)) as u64;
        Self {
            interrupt_id: info.interrupt_id,
            target_id: info.target_id,
            code: info.code,
            summary: info.summary,
            state: info.state.as_str().into(),
            expires_in_ms,
        }
    }
}

pub async fn begin(
    server: &VoidCrawlServer,
    args: SessionInterruptArgs,
) -> Result<InterruptResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    let page = handle.page.lock().await;
    let ttl = Duration::from_secs(args.ttl_seconds);
    let info = handle
        .session
        .interrupt_page(&page, InterruptRequest { code: args.code, summary: args.summary, ttl })
        .await
        .map_err(map_err)?;
    drop(page);
    schedule_expiry_cleanup(
        Arc::clone(&server.state().sessions),
        args.session_id,
        info.interrupt_id.clone(),
        ttl,
    );
    Ok(InterruptResult::from(info))
}

pub async fn status(
    server: &VoidCrawlServer,
    args: InterruptIdArgs,
) -> Result<InterruptResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    handle
        .session
        .interrupt_status(&args.interrupt_id)
        .await
        .map(InterruptResult::from)
        .map_err(map_err)
}

pub async fn resume(
    server: &VoidCrawlServer,
    args: InterruptIdArgs,
) -> Result<InterruptResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    handle
        .session
        .resume_interrupt(&args.interrupt_id)
        .await
        .map(InterruptResult::from)
        .map_err(map_err)
}

pub async fn release(
    server: &VoidCrawlServer,
    args: InterruptIdArgs,
) -> Result<InterruptResult, ErrorData> {
    let handle = lookup(server, &args.session_id).await?;
    handle
        .session
        .release_interrupt(&args.interrupt_id)
        .await
        .map(InterruptResult::from)
        .map_err(map_err)
}

/// Remove and close a session only if its original interrupt reaches expiry.
///
/// A resumed or released interrupt leaves its session intact. Once the timer
/// fires, core transitions an active record to `Expired`, so a concurrent
/// resume cannot race this cleanup into closing a live session.
fn schedule_expiry_cleanup(
    sessions: Arc<SessionRegistry>,
    session_id: String,
    interrupt_id: String,
    ttl: Duration,
) {
    tokio::spawn(async move {
        sleep(ttl).await;
        let Some(handle) = sessions.get(&session_id).await else {
            return;
        };
        let expired = matches!(
            handle.session.interrupt_status(&interrupt_id).await,
            Ok(info) if info.state == InterruptState::Expired
        );
        if !expired {
            return;
        }
        if let Some(handle) = sessions.remove(&session_id).await {
            if let Err(error) = close_handle(handle).await {
                tracing::warn!(%error, %session_id, %interrupt_id, "failed to close expired interrupt session");
            }
        }
    });
}

async fn lookup(server: &VoidCrawlServer, id: &str) -> Result<Arc<DedicatedSession>, ErrorData> {
    server
        .state()
        .sessions
        .get(id)
        .await
        .ok_or_else(|| ErrorData::invalid_params(format!("unknown session_id: {id}"), None))
}
