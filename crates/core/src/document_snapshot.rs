//! Bounded rendered-DOM and raw accessibility observations.
//!
//! Raw payload bytes remain provider-native. Compact document/AX outlines are
//! presentation projections and are not stored in these authoritative results.

use std::{
    fmt,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use chromiumoxide::cdp::browser_protocol::accessibility::AxNode;
use serde::Serialize;

use crate::ProtectedUrl;

/// Capture-local navigation epoch for document correlation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DocumentEpoch {
    Known(u64),
    /// The page was adopted from an attached browser and has not subsequently
    /// been navigated by this VoidCrawl handle.
    UnavailableForAttachedPage,
}

/// Document/frame scope of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentFrameScope {
    TopLevel,
    Frame { url: Option<ProtectedUrl> },
}

/// Context needed to relate DOM, AX, layout, and visual observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentScope {
    pub epoch: DocumentEpoch,
    pub frame: DocumentFrameScope,
    pub url:   Option<ProtectedUrl>,
}

/// Complete, truncated, or unavailable snapshot payload state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotState {
    Complete,
    Truncated,
    Unavailable { reason: SnapshotUnavailableReason },
}

/// Why a requested DOM or AX payload was unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotUnavailableReason {
    BrowserDidNotReport,
    FrameUnavailable,
    SerializationFailed,
}

/// Bounded rendered-DOM serialization.
#[derive(Clone, PartialEq, Eq)]
pub struct RenderedDomSnapshot {
    pub scope:                DocumentScope,
    pub generated_at_unix_ms: Option<u64>,
    pub state:                SnapshotState,
    pub retained_bytes:       usize,
    pub complete_bytes:       Option<usize>,
    payload:                  Arc<[u8]>,
}

impl RenderedDomSnapshot {
    pub fn bytes(&self) -> &[u8] {
        &self.payload
    }
}

impl fmt::Debug for RenderedDomSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RenderedDomSnapshot")
            .field("scope", &self.scope)
            .field("generated_at_unix_ms", &self.generated_at_unix_ms)
            .field("state", &self.state)
            .field("retained_bytes", &self.retained_bytes)
            .field("complete_bytes", &self.complete_bytes)
            .finish_non_exhaustive()
    }
}

/// Bounds for one accessibility snapshot.
#[derive(Debug, Clone, Copy)]
pub struct AccessibilitySnapshotOptions {
    pub depth:     Option<i64>,
    pub max_nodes: usize,
    pub max_bytes: usize,
}

impl Default for AccessibilitySnapshotOptions {
    fn default() -> Self {
        Self { depth: None, max_nodes: 10_000, max_bytes: 8 * 1024 * 1024 }
    }
}

/// Bounded raw CDP accessibility-tree serialization.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessibilitySnapshot {
    pub scope:                DocumentScope,
    pub generated_at_unix_ms: Option<u64>,
    pub state:                SnapshotState,
    pub requested_depth:      Option<i64>,
    pub nodes_observed:       usize,
    pub nodes_retained:       usize,
    pub retained_bytes:       usize,
    pub complete_bytes:       Option<usize>,
    payload:                  Arc<[u8]>,
}

impl AccessibilitySnapshot {
    pub fn bytes(&self) -> &[u8] {
        &self.payload
    }
}

impl fmt::Debug for AccessibilitySnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccessibilitySnapshot")
            .field("scope", &self.scope)
            .field("generated_at_unix_ms", &self.generated_at_unix_ms)
            .field("state", &self.state)
            .field("requested_depth", &self.requested_depth)
            .field("nodes_observed", &self.nodes_observed)
            .field("nodes_retained", &self.nodes_retained)
            .field("retained_bytes", &self.retained_bytes)
            .field("complete_bytes", &self.complete_bytes)
            .finish_non_exhaustive()
    }
}

pub(crate) fn rendered_dom(
    html: String,
    scope: DocumentScope,
    max_bytes: usize,
) -> RenderedDomSnapshot {
    let bytes = html.into_bytes();
    let complete_bytes = bytes.len();
    let retained_bytes = complete_bytes.min(max_bytes);
    RenderedDomSnapshot {
        scope,
        generated_at_unix_ms: unix_millis(),
        state: if retained_bytes == complete_bytes {
            SnapshotState::Complete
        } else {
            SnapshotState::Truncated
        },
        retained_bytes,
        complete_bytes: Some(complete_bytes),
        payload: Arc::from(bytes[..retained_bytes].to_vec()),
    }
}

pub(crate) fn accessibility(
    nodes: &[AxNode],
    scope: DocumentScope,
    options: AccessibilitySnapshotOptions,
) -> AccessibilitySnapshot {
    let nodes_observed = nodes.len();
    let Ok(complete) = serde_json::to_vec(nodes) else {
        return unavailable_accessibility(
            scope,
            options.depth,
            SnapshotUnavailableReason::SerializationFailed,
        );
    };
    let complete_bytes = complete.len();
    let mut payload = Vec::with_capacity(options.max_bytes.min(complete_bytes));
    payload.push(b'[');
    let mut nodes_retained = 0usize;
    for node in nodes.iter().take(options.max_nodes) {
        let Ok(encoded) = serde_json::to_vec(node) else {
            return unavailable_accessibility(
                scope,
                options.depth,
                SnapshotUnavailableReason::SerializationFailed,
            );
        };
        let separator = usize::from(nodes_retained > 0);
        if payload.len().saturating_add(separator).saturating_add(encoded.len()).saturating_add(1)
            > options.max_bytes
        {
            break;
        }
        if separator == 1 {
            payload.push(b',');
        }
        payload.extend_from_slice(&encoded);
        nodes_retained += 1;
    }
    payload.push(b']');
    let truncated = nodes_retained < nodes_observed;
    AccessibilitySnapshot {
        scope,
        generated_at_unix_ms: unix_millis(),
        state: if truncated { SnapshotState::Truncated } else { SnapshotState::Complete },
        requested_depth: options.depth,
        nodes_observed,
        nodes_retained,
        retained_bytes: payload.len(),
        complete_bytes: Some(complete_bytes),
        payload: Arc::from(payload),
    }
}

pub(crate) fn unavailable_accessibility(
    scope: DocumentScope,
    requested_depth: Option<i64>,
    reason: SnapshotUnavailableReason,
) -> AccessibilitySnapshot {
    AccessibilitySnapshot {
        scope,
        generated_at_unix_ms: unix_millis(),
        state: SnapshotState::Unavailable { reason },
        requested_depth,
        nodes_observed: 0,
        nodes_retained: 0,
        retained_bytes: 0,
        complete_bytes: None,
        payload: Arc::from([]),
    }
}

fn unix_millis() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_dom_truncation_is_exact() {
        let snapshot = rendered_dom(
            "abcdef".into(),
            DocumentScope {
                epoch: DocumentEpoch::Known(1),
                frame: DocumentFrameScope::TopLevel,
                url:   None,
            },
            3,
        );
        assert_eq!(snapshot.state, SnapshotState::Truncated);
        assert_eq!(snapshot.bytes(), b"abc");
        assert_eq!(snapshot.retained_bytes, 3);
        assert_eq!(snapshot.complete_bytes, Some(6));
    }

    #[test]
    fn unavailable_accessibility_has_no_payload() {
        let snapshot = unavailable_accessibility(
            DocumentScope {
                epoch: DocumentEpoch::UnavailableForAttachedPage,
                frame: DocumentFrameScope::TopLevel,
                url:   None,
            },
            None,
            SnapshotUnavailableReason::BrowserDidNotReport,
        );
        assert!(snapshot.bytes().is_empty());
        assert_eq!(snapshot.complete_bytes, None);
    }
}
