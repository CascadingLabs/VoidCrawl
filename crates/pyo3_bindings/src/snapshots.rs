use std::fmt;

use pyo3::{exceptions::PyValueError, prelude::*, types::PyBytes};
use void_crawl_core::{
    AccessibilitySnapshot, BrowserTargetKind, DocumentEpoch, DocumentFrameScope, LayoutSnapshot,
    RenderedDomSnapshot, SnapshotState, SnapshotUnavailableReason, VisualCaptureRegion,
    VisualFormat, VisualSnapshot,
};

fn state_name(state: SnapshotState) -> &'static str {
    match state {
        SnapshotState::Complete => "complete",
        SnapshotState::Truncated => "truncated",
        SnapshotState::Unavailable { .. } => "unavailable",
    }
}

fn unavailable_reason(state: SnapshotState) -> Option<&'static str> {
    let SnapshotState::Unavailable { reason } = state else {
        return None;
    };
    Some(match reason {
        SnapshotUnavailableReason::BrowserDidNotReport => "browser_did_not_report",
        SnapshotUnavailableReason::FrameUnavailable => "frame_unavailable",
        SnapshotUnavailableReason::SerializationFailed => "serialization_failed",
    })
}

fn epoch(epoch: DocumentEpoch) -> Option<u64> {
    match epoch {
        DocumentEpoch::Known(value) => Some(value),
        DocumentEpoch::UnavailableForAttachedPage => None,
    }
}

pub(crate) fn byte_report_dict(
    py: Python<'_>,
    report: void_crawl_core::BrowserByteReport,
) -> PyResult<Bound<'_, PyAny>> {
    let value = serde_json::to_value(report)
        .map_err(|error| PyValueError::new_err(format!("serialize byte report: {error}")))?;
    crate::json_to_py(py, value)
}

fn frame_scope(scope: &DocumentFrameScope) -> &'static str {
    match scope {
        DocumentFrameScope::TopLevel => "top_level",
        DocumentFrameScope::Frame { .. } => "frame",
    }
}

/// Python-visible bounded rendered-DOM snapshot.
#[pyclass(name = "RenderedDomSnapshot", frozen)]
pub struct PyRenderedDomSnapshot {
    inner: RenderedDomSnapshot,
}

impl From<RenderedDomSnapshot> for PyRenderedDomSnapshot {
    fn from(inner: RenderedDomSnapshot) -> Self {
        Self { inner }
    }
}

impl fmt::Debug for PyRenderedDomSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RenderedDomSnapshot")
            .field("state", &self.inner.state)
            .field("retained_bytes", &self.inner.retained_bytes)
            .finish_non_exhaustive()
    }
}

#[pymethods]
impl PyRenderedDomSnapshot {
    #[getter]
    fn state(&self) -> &'static str {
        state_name(self.inner.state)
    }

    #[getter]
    fn unavailable_reason(&self) -> Option<&'static str> {
        unavailable_reason(self.inner.state)
    }

    #[getter]
    fn epoch(&self) -> Option<u64> {
        epoch(self.inner.scope.epoch)
    }

    #[getter]
    fn frame_scope(&self) -> &'static str {
        frame_scope(&self.inner.scope.frame)
    }

    #[getter]
    fn url(&self) -> Option<String> {
        self.inner.scope.url.as_ref().map(|url| url.as_str().to_string())
    }

    #[getter]
    fn generated_at_unix_ms(&self) -> Option<u64> {
        self.inner.generated_at_unix_ms
    }

    #[getter]
    fn retained_bytes(&self) -> usize {
        self.inner.retained_bytes
    }

    #[getter]
    fn complete_bytes(&self) -> Option<usize> {
        self.inner.complete_bytes
    }

    fn bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.bytes())
    }

    #[getter]
    fn byte_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        byte_report_dict(
            py,
            self.inner.byte_report().map_err(|error| PyValueError::new_err(error.to_string()))?,
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "RenderedDomSnapshot(state={:?}, epoch={:?}, retained_bytes={}, complete_bytes={:?})",
            state_name(self.inner.state),
            epoch(self.inner.scope.epoch),
            self.inner.retained_bytes,
            self.inner.complete_bytes,
        )
    }
}

/// Python-visible bounded raw accessibility snapshot.
#[pyclass(name = "AccessibilitySnapshot", frozen)]
pub struct PyAccessibilitySnapshot {
    inner: AccessibilitySnapshot,
}

impl From<AccessibilitySnapshot> for PyAccessibilitySnapshot {
    fn from(inner: AccessibilitySnapshot) -> Self {
        Self { inner }
    }
}

impl fmt::Debug for PyAccessibilitySnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccessibilitySnapshot")
            .field("state", &self.inner.state)
            .field("nodes_retained", &self.inner.nodes_retained)
            .field("retained_bytes", &self.inner.retained_bytes)
            .finish_non_exhaustive()
    }
}

#[pymethods]
impl PyAccessibilitySnapshot {
    #[getter]
    fn state(&self) -> &'static str {
        state_name(self.inner.state)
    }

    #[getter]
    fn unavailable_reason(&self) -> Option<&'static str> {
        unavailable_reason(self.inner.state)
    }

    #[getter]
    fn epoch(&self) -> Option<u64> {
        epoch(self.inner.scope.epoch)
    }

    #[getter]
    fn frame_scope(&self) -> &'static str {
        frame_scope(&self.inner.scope.frame)
    }

    #[getter]
    fn frame_url(&self) -> Option<String> {
        match &self.inner.scope.frame {
            DocumentFrameScope::Frame { url } => url.as_ref().map(|url| url.as_str().to_string()),
            DocumentFrameScope::TopLevel => None,
        }
    }

    #[getter]
    fn url(&self) -> Option<String> {
        self.inner.scope.url.as_ref().map(|url| url.as_str().to_string())
    }

    #[getter]
    fn generated_at_unix_ms(&self) -> Option<u64> {
        self.inner.generated_at_unix_ms
    }

    #[getter]
    fn requested_depth(&self) -> Option<i64> {
        self.inner.requested_depth
    }

    #[getter]
    fn nodes_observed(&self) -> usize {
        self.inner.nodes_observed
    }

    #[getter]
    fn nodes_retained(&self) -> usize {
        self.inner.nodes_retained
    }

    #[getter]
    fn retained_bytes(&self) -> usize {
        self.inner.retained_bytes
    }

    #[getter]
    fn complete_bytes(&self) -> Option<usize> {
        self.inner.complete_bytes
    }

    fn bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.bytes())
    }

    #[getter]
    fn byte_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        byte_report_dict(
            py,
            self.inner.byte_report().map_err(|error| PyValueError::new_err(error.to_string()))?,
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "AccessibilitySnapshot(state={:?}, epoch={:?}, frame_scope={:?}, nodes={}/{}, retained_bytes={})",
            state_name(self.inner.state),
            epoch(self.inner.scope.epoch),
            frame_scope(&self.inner.scope.frame),
            self.inner.nodes_retained,
            self.inner.nodes_observed,
            self.inner.retained_bytes,
        )
    }
}

/// Python-visible CSS layout metrics snapshot.
#[pyclass(name = "LayoutSnapshot", frozen)]
#[derive(Debug)]
pub struct PyLayoutSnapshot {
    inner: LayoutSnapshot,
}

impl From<LayoutSnapshot> for PyLayoutSnapshot {
    fn from(inner: LayoutSnapshot) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyLayoutSnapshot {
    #[getter]
    fn epoch(&self) -> Option<u64> {
        epoch(self.inner.scope.epoch)
    }

    #[getter]
    fn url(&self) -> Option<String> {
        self.inner.scope.url.as_ref().map(|url| url.as_str().to_string())
    }

    #[getter]
    fn generated_at_unix_ms(&self) -> Option<u64> {
        self.inner.generated_at_unix_ms
    }

    #[getter]
    fn layout_viewport(&self) -> (i64, i64, i64, i64) {
        let value = self.inner.layout_viewport;
        (value.page_x, value.page_y, value.client_width, value.client_height)
    }

    #[getter]
    fn visual_viewport(&self) -> (f64, f64, f64, f64, f64, f64, f64, Option<f64>) {
        let value = self.inner.visual_viewport;
        (
            value.offset_x,
            value.offset_y,
            value.page_x,
            value.page_y,
            value.client_width,
            value.client_height,
            value.scale,
            value.zoom,
        )
    }

    #[getter]
    fn content_size(&self) -> (f64, f64, f64, f64) {
        let value = self.inner.content_size;
        (value.x, value.y, value.width, value.height)
    }

    #[getter]
    fn device_scale_factor(&self) -> Option<f64> {
        self.inner.device_scale_factor
    }

    fn __repr__(&self) -> String {
        format!(
            "LayoutSnapshot(epoch={:?}, viewport={}x{}, content={}x{})",
            epoch(self.inner.scope.epoch),
            self.inner.layout_viewport.client_width,
            self.inner.layout_viewport.client_height,
            self.inner.content_size.width,
            self.inner.content_size.height,
        )
    }
}

fn target_kind_name(kind: BrowserTargetKind) -> &'static str {
    match kind {
        BrowserTargetKind::Css => "css",
        BrowserTargetKind::Xpath => "xpath",
        BrowserTargetKind::Regex => "regex",
        BrowserTargetKind::Jsonld => "jsonld",
        BrowserTargetKind::Attr => "attr",
        BrowserTargetKind::GlobalId => "global_id",
        BrowserTargetKind::Role => "role",
        BrowserTargetKind::Visual => "visual",
    }
}

/// Python-visible PNG plus capture metadata.
#[pyclass(name = "VisualSnapshot", frozen)]
#[derive(Debug)]
pub struct PyVisualSnapshot {
    inner: VisualSnapshot,
}

impl From<VisualSnapshot> for PyVisualSnapshot {
    fn from(inner: VisualSnapshot) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyVisualSnapshot {
    #[getter]
    fn epoch(&self) -> Option<u64> {
        epoch(self.inner.scope.epoch)
    }

    #[getter]
    fn url(&self) -> Option<String> {
        self.inner.scope.url.as_ref().map(|url| url.as_str().to_string())
    }

    #[getter]
    fn generated_at_unix_ms(&self) -> Option<u64> {
        self.inner.generated_at_unix_ms
    }

    #[getter]
    fn format(&self) -> &'static str {
        match self.inner.format {
            VisualFormat::Png => "png",
        }
    }

    #[getter]
    fn region(&self) -> &'static str {
        match self.inner.region {
            VisualCaptureRegion::FullPage => "full_page",
            VisualCaptureRegion::Viewport => "viewport",
            VisualCaptureRegion::BoundingBox { .. } => "bounding_box",
            VisualCaptureRegion::BrowserTarget { .. } => "browser_target",
        }
    }

    #[getter]
    fn bbox(&self) -> Option<(u32, u32, u32, u32)> {
        match self.inner.region {
            VisualCaptureRegion::BoundingBox { bbox } => {
                Some((bbox.x, bbox.y, bbox.width, bbox.height))
            }
            _ => None,
        }
    }

    #[getter]
    fn target_kind(&self) -> Option<&'static str> {
        match self.inner.region {
            VisualCaptureRegion::BrowserTarget { target_kind } => {
                Some(target_kind_name(target_kind))
            }
            _ => None,
        }
    }

    #[getter]
    fn image_size(&self) -> (u32, u32) {
        (self.inner.image_width_pixels, self.inner.image_height_pixels)
    }

    #[getter]
    fn capture_viewport(&self) -> (u32, u32) {
        (
            self.inner.capture_viewport.width_css_pixels,
            self.inner.capture_viewport.height_css_pixels,
        )
    }

    #[getter]
    fn device_scale_factor(&self) -> f64 {
        self.inner.device_scale_factor
    }

    #[getter]
    fn retained_bytes(&self) -> usize {
        self.inner.retained_bytes
    }

    #[getter]
    fn complete(&self) -> bool {
        self.inner.complete
    }

    #[getter]
    fn byte_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        byte_report_dict(
            py,
            self.inner.byte_report().map_err(|error| PyValueError::new_err(error.to_string()))?,
        )
    }

    fn bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.bytes())
    }

    fn __repr__(&self) -> String {
        format!(
            "VisualSnapshot(epoch={:?}, region={:?}, image={}x{}, retained_bytes={})",
            epoch(self.inner.scope.epoch),
            self.region(),
            self.inner.image_width_pixels,
            self.inner.image_height_pixels,
            self.inner.retained_bytes,
        )
    }
}
