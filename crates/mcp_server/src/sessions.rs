//! Stateful session registry.
//!
//! Each `session_open` spawns a dedicated `BrowserSession` with its own
//! temporary user-data-dir, so cookies and storage never leak between
//! subagents. Pooled tabs are only used for stateless `fetch*` calls.

use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, RwLock};
use void_crawl_core::{BrowserSession, Page};

pub type SessionId = String;

/// Owned state for one stateful MCP session.
#[derive(Debug)]
pub struct DedicatedSession {
    pub session: Arc<BrowserSession>,
    pub page:    Mutex<Page>,
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
