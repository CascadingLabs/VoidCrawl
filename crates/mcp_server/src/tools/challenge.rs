//! Challenge escalation tools.
//!
//! These tools own the manual/VNC loop for V1: capture the current wall,
//! expose same-tab attach coordinates, let an operator clear it through VNC or
//! noVNC, then mark the event resolved/failed. Later resolvers (Yosoi,
//! OpenSesame, agents) use the same event and attach contract.

use std::{env, time::Duration};

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, sleep};
use uuid::Uuid;
use void_crawl_core::{
    AntibotVerdict, AttachCoordinates, ChallengeSnapshot, ChallengeStatus, DomCaptchaSnapshot,
    ResolutionOutcome, ResolverType, capture_captcha,
};

use crate::{
    errors::map_err,
    server::VoidCrawlServer,
    sessions::{LastNavigation, PendingChallenge},
    tools::fetch::AntibotInfo,
};

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct CaptureChallengeArgs {
    pub session_id: String,
    /// Optional noVNC URL for the human operator, e.g. http://127.0.0.1:6080.
    #[serde(default)]
    pub novnc_url:  Option<String>,
    /// Optional native VNC URL for lower-latency clients, e.g.
    /// vnc://127.0.0.1:5900.
    #[serde(default)]
    pub vnc_url:    Option<String>,
    /// Include a compact AX tree summary inline for triage. On by default.
    #[serde(default = "default_include_ax")]
    pub include_ax: bool,
}

fn default_include_ax() -> bool {
    true
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChallengeAttachInfo {
    pub websocket_url: Option<String>,
    pub target_id:     Option<String>,
    pub session_id:    Option<String>,
    pub vnc_url:       Option<String>,
    pub novnc_url:     Option<String>,
}

impl From<AttachCoordinates> for ChallengeAttachInfo {
    fn from(coords: AttachCoordinates) -> Self {
        Self {
            websocket_url: coords.websocket_url,
            target_id:     coords.target_id,
            session_id:    coords.session_id,
            vnc_url:       coords.vnc_url,
            novnc_url:     coords.novnc_url,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DomCaptchaInfo {
    pub kind:                    String,
    pub sitekey:                 Option<String>,
    pub widget_selector:         Option<String>,
    pub widget_rect:             Option<WidgetRectInfo>,
    pub widget_rendered:         bool,
    pub response_field_selector: Option<String>,
    pub existing_token_present:  bool,
    pub action:                  Option<String>,
    pub cdata:                   Option<String>,
    pub page_url:                String,
    pub active:                  bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WidgetRectInfo {
    pub x:      f64,
    pub y:      f64,
    pub width:  f64,
    pub height: f64,
}

impl From<DomCaptchaSnapshot> for DomCaptchaInfo {
    fn from(c: DomCaptchaSnapshot) -> Self {
        Self {
            kind:                    c.kind,
            sitekey:                 c.sitekey,
            widget_selector:         c.widget_selector,
            widget_rect:             c.widget_rect.map(|r| WidgetRectInfo {
                x:      r.x,
                y:      r.y,
                width:  r.width,
                height: r.height,
            }),
            widget_rendered:         c.widget_rendered,
            response_field_selector: c.response_field_selector,
            existing_token_present:  c.existing_token_present,
            action:                  c.action,
            cdata:                   c.cdata,
            page_url:                c.page_url,
            active:                  c.active,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChallengeSnapshotInfo {
    pub event_id: String,
    pub url: String,
    pub status_code: Option<u16>,
    pub status: String,
    pub blocking: bool,
    pub antibot: Option<AntibotInfo>,
    pub dom_captcha: Option<DomCaptchaInfo>,
    pub evidence_corpus_versions: Vec<String>,
    pub screenshot_handle: Option<String>,
    pub ax_summary_handle: Option<String>,
    pub ax_summary: Option<String>,
    pub attach_coordinates: ChallengeAttachInfo,
}

impl ChallengeSnapshotInfo {
    fn from_snapshot(snapshot: ChallengeSnapshot, ax_summary: Option<String>) -> Self {
        let blocking = snapshot.is_blocking();
        let status = status_tag(snapshot.status).to_string();
        Self {
            event_id: snapshot.event_id,
            url: snapshot.url,
            status_code: snapshot.status_code,
            status,
            blocking,
            antibot: snapshot.antibot.map(AntibotInfo::from),
            dom_captcha: snapshot.dom_captcha.map(DomCaptchaInfo::from),
            evidence_corpus_versions: snapshot.evidence_corpus_versions,
            screenshot_handle: snapshot.screenshot_handle,
            ax_summary_handle: snapshot.ax_summary_handle,
            ax_summary,
            attach_coordinates: snapshot.attach_coordinates.into(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CaptureChallengeResult {
    pub challenge:     ChallengeSnapshotInfo,
    /// Human-readable next action for V1 manual resolution.
    pub operator_hint: String,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct MarkChallengeArgs {
    pub session_id: String,
    pub event_id:   String,
    /// One of manual_vnc, yosoi_recipe, open_sesame_session_actor, agent_mcp,
    /// rotate_identity, fail. Defaults to manual_vnc for resolved and fail for
    /// failed.
    #[serde(default)]
    pub resolver:   Option<String>,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    #[serde(default)]
    pub note:       Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct WaitChallengeArgs {
    pub session_id:    String,
    pub event_id:      String,
    #[serde(default)]
    pub timeout_secs:  Option<u64>,
    /// Re-probe DOM after a resolved mark. On by default so callers can resume
    /// only when the visible wall is actually gone.
    #[serde(default = "default_reprobe")]
    pub reprobe_after: bool,
}

fn default_reprobe() -> bool {
    true
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ResolutionOutcomeInfo {
    pub event_id:   String,
    pub resolver:   String,
    pub status:     String,
    pub elapsed_ms: Option<u64>,
    pub note:       Option<String>,
}

impl From<ResolutionOutcome> for ResolutionOutcomeInfo {
    fn from(outcome: ResolutionOutcome) -> Self {
        Self {
            event_id:   outcome.event_id,
            resolver:   resolver_tag(outcome.resolver).to_string(),
            status:     status_tag(outcome.status).to_string(),
            elapsed_ms: outcome.elapsed_ms,
            note:       outcome.note,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ResolutionResult {
    pub outcome: ResolutionOutcomeInfo,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WaitChallengeResult {
    pub resolved:              bool,
    pub outcome:               Option<ResolutionOutcomeInfo>,
    pub captcha_still_present: Option<bool>,
}

pub async fn capture(
    server: &VoidCrawlServer,
    args: CaptureChallengeArgs,
) -> Result<CaptureChallengeResult, ErrorData> {
    let handle = server.state().sessions.get(&args.session_id).await.ok_or_else(|| {
        ErrorData::invalid_params(format!("unknown session_id: {}", args.session_id), None)
    })?;
    let page = handle.page.lock().await;
    let last = handle.last_navigation.lock().await.clone();
    let url = page
        .url()
        .await
        .map_err(map_err)?
        .or_else(|| last.as_ref().map(|nav| nav.url.clone()))
        .unwrap_or_default();
    let status_code = last.as_ref().and_then(|nav| nav.status_code);
    let antibot = last.and_then(|nav| nav.antibot).filter(AntibotVerdict::detected);
    let dom_captcha = capture_captcha(&page).await.map_err(map_err)?.map(DomCaptchaSnapshot::from);
    let ax_summary = if args.include_ax { page.ax_tree_outline(Some(3)).await.ok() } else { None };
    let websocket_url = handle.session.websocket_url().await;
    let target_id = page.target_id();
    let attach_coordinates = AttachCoordinates {
        websocket_url: Some(websocket_url),
        target_id:     Some(target_id),
        session_id:    Some(args.session_id.clone()),
        vnc_url:       args.vnc_url.or_else(|| env::var("VOIDCRAWL_VNC_URL").ok()),
        novnc_url:     args.novnc_url.or_else(|| env::var("VOIDCRAWL_NOVNC_URL").ok()),
    };
    let mut versions = Vec::new();
    if let Some(v) = antibot.as_ref() {
        versions.push(format!("antibot:{}", v.corpus_version));
    }
    if dom_captcha.is_some() {
        versions.push("dom_captcha:runtime".to_string());
    }
    let snapshot = ChallengeSnapshot {
        event_id: Uuid::new_v4().to_string(),
        url,
        status_code,
        status: ChallengeStatus::Active,
        antibot,
        dom_captcha,
        evidence_corpus_versions: versions,
        screenshot_handle: None,
        ax_summary_handle: ax_summary.as_ref().map(|_| "inline:ax_summary".to_string()),
        attach_coordinates,
    };
    let blocking = snapshot.is_blocking();
    if blocking {
        *handle.challenge.lock().await =
            Some(PendingChallenge { snapshot: snapshot.clone(), outcome: None });
    }
    let operator_hint = if blocking {
        "Open novnc_url or vnc_url, clear the wall in that browser, then call mark_challenge_resolved and wait_for_challenge_resolution.".to_string()
    } else {
        "No active challenge was detected. Presence-only CDN/anti-bot signals are telemetry and do not require a manual pause.".to_string()
    };
    Ok(CaptureChallengeResult {
        challenge: ChallengeSnapshotInfo::from_snapshot(snapshot, ax_summary),
        operator_hint,
    })
}

pub async fn mark_resolved(
    server: &VoidCrawlServer,
    args: MarkChallengeArgs,
) -> Result<ResolutionResult, ErrorData> {
    mark(server, args, ChallengeStatus::Resolved).await
}

pub async fn mark_failed(
    server: &VoidCrawlServer,
    args: MarkChallengeArgs,
) -> Result<ResolutionResult, ErrorData> {
    mark(server, args, ChallengeStatus::Failed).await
}

async fn mark(
    server: &VoidCrawlServer,
    args: MarkChallengeArgs,
    status: ChallengeStatus,
) -> Result<ResolutionResult, ErrorData> {
    let handle = server.state().sessions.get(&args.session_id).await.ok_or_else(|| {
        ErrorData::invalid_params(format!("unknown session_id: {}", args.session_id), None)
    })?;
    let mut guard = handle.challenge.lock().await;
    let Some(pending) = guard.as_mut() else {
        return Err(ErrorData::invalid_params("no active challenge for session", None));
    };
    if pending.snapshot.event_id != args.event_id {
        return Err(ErrorData::invalid_params(
            format!("event_id mismatch: active event is {}", pending.snapshot.event_id),
            None,
        ));
    }
    pending.snapshot.status = status;
    let default_resolver = if status == ChallengeStatus::Resolved {
        ResolverType::ManualVnc
    } else {
        ResolverType::Fail
    };
    let resolver =
        args.resolver.as_deref().map(parse_resolver).transpose()?.unwrap_or(default_resolver);
    let outcome = ResolutionOutcome {
        event_id: args.event_id,
        resolver,
        status,
        elapsed_ms: args.elapsed_ms,
        note: args.note,
    };
    pending.outcome = Some(outcome.clone());
    Ok(ResolutionResult { outcome: outcome.into() })
}

pub async fn wait_for_resolution(
    server: &VoidCrawlServer,
    args: WaitChallengeArgs,
) -> Result<WaitChallengeResult, ErrorData> {
    let handle = server.state().sessions.get(&args.session_id).await.ok_or_else(|| {
        ErrorData::invalid_params(format!("unknown session_id: {}", args.session_id), None)
    })?;
    let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(300));
    let deadline = Instant::now() + timeout;
    loop {
        let outcome = {
            let guard = handle.challenge.lock().await;
            let Some(pending) = guard.as_ref() else {
                return Err(ErrorData::invalid_params("no active challenge for session", None));
            };
            if pending.snapshot.event_id != args.event_id {
                return Err(ErrorData::invalid_params(
                    format!("event_id mismatch: active event is {}", pending.snapshot.event_id),
                    None,
                ));
            }
            pending.outcome.clone()
        };
        if let Some(outcome) = outcome {
            let captcha_still_present =
                if outcome.status == ChallengeStatus::Resolved && args.reprobe_after {
                    let page = handle.page.lock().await;
                    let info = capture_captcha(&page).await.map_err(map_err)?;
                    info.as_ref().map(void_crawl_core::captcha_is_active)
                } else {
                    None
                };
            return Ok(WaitChallengeResult {
                resolved: outcome.status == ChallengeStatus::Resolved,
                outcome: Some(outcome.into()),
                captcha_still_present,
            });
        }
        if Instant::now() >= deadline {
            return Ok(WaitChallengeResult {
                resolved:              false,
                outcome:               None,
                captcha_still_present: None,
            });
        }
        sleep(Duration::from_millis(250)).await;
    }
}

fn parse_resolver(value: &str) -> Result<ResolverType, ErrorData> {
    match value {
        "manual_vnc" => Ok(ResolverType::ManualVnc),
        "yosoi_recipe" => Ok(ResolverType::YosoiRecipe),
        "open_sesame_session_actor" => Ok(ResolverType::OpenSesameSessionActor),
        "agent_mcp" => Ok(ResolverType::AgentMcp),
        "rotate_identity" => Ok(ResolverType::RotateIdentity),
        "fail" => Ok(ResolverType::Fail),
        other => Err(ErrorData::invalid_params(format!("unknown resolver: {other}"), None)),
    }
}

fn resolver_tag(resolver: ResolverType) -> &'static str {
    match resolver {
        ResolverType::ManualVnc => "manual_vnc",
        ResolverType::YosoiRecipe => "yosoi_recipe",
        ResolverType::OpenSesameSessionActor => "open_sesame_session_actor",
        ResolverType::AgentMcp => "agent_mcp",
        ResolverType::RotateIdentity => "rotate_identity",
        ResolverType::Fail => "fail",
    }
}

fn status_tag(status: ChallengeStatus) -> &'static str {
    match status {
        ChallengeStatus::Active => "active",
        ChallengeStatus::Resolved => "resolved",
        ChallengeStatus::Failed => "failed",
    }
}

#[allow(dead_code, reason = "keeps import visible for rustdoc intra-module links")]
fn _last_navigation_type_anchor(_: Option<LastNavigation>) {}
