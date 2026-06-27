//! Stateful session registry.
//!
//! Each `session_open` spawns a dedicated `BrowserSession` with its own
//! temporary user-data-dir, so cookies and storage never leak between
//! subagents. Pooled tabs are only used for stateless `fetch*` calls.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use tempfile::TempDir;
use tokio::sync::{Mutex, RwLock};
use void_crawl_core::{
    AntibotVerdict, BrowserSession, ChallengeSnapshot, DownloadCapture, ManagedProfileLease, Page,
    ResolutionOutcome,
};

pub type SessionId = String;

/// A download armed on a session by `download_arm`, awaiting `download_wait`.
/// Holds the quarantine `TempDir` alive between the two tool calls.
#[derive(Debug)]
pub struct PendingDownload {
    pub capture:    DownloadCapture,
    pub quarantine: TempDir,
    pub output_dir: PathBuf,
    pub max_bytes:  u64,
}

/// Last document navigation metadata kept so a later `capture_challenge` call
/// can combine response-side anti-bot evidence with DOM-side captcha evidence.
#[derive(Debug, Clone)]
pub struct LastNavigation {
    pub url:         String,
    pub status_code: Option<u16>,
    pub antibot:     Option<AntibotVerdict>,
}

/// Challenge event currently owned by a session.
#[derive(Debug, Clone)]
pub struct PendingChallenge {
    pub snapshot: ChallengeSnapshot,
    pub outcome:  Option<ResolutionOutcome>,
}

/// Owned state for one stateful MCP session.
#[derive(Debug)]
pub struct DedicatedSession {
    pub session:          Arc<BrowserSession>,
    pub page:             Mutex<Page>,
    pub profile_lease:    Option<ManagedProfileLease>,
    pub last_navigation:  Mutex<Option<LastNavigation>>,
    pub challenge:        Mutex<Option<PendingChallenge>>,
    /// A download armed via `download_arm`, pending its `download_wait`.
    pub pending_download: Mutex<Option<PendingDownload>>,
}

/// Thread-safe map of live sessions.
#[derive(Debug, Default)]
pub struct SessionRegistry {
    inner: RwLock<HashMap<SessionId, Arc<DedicatedSession>>>,
}

impl SessionRegistry {
    pub async fn insert(&self, id: SessionId, session: Arc<DedicatedSession>) {
        self.inner.write().await.insert(id, session);
    }

    pub async fn get(&self, id: &str) -> Option<Arc<DedicatedSession>> {
        self.inner.read().await.get(id).cloned()
    }

    pub async fn remove(&self, id: &str) -> Option<Arc<DedicatedSession>> {
        self.inner.write().await.remove(id)
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    /// Drain every session. Used on shutdown.
    pub async fn drain(&self) -> Vec<Arc<DedicatedSession>> {
        self.inner.write().await.drain().map(|(_, v)| v).collect()
    }
}
