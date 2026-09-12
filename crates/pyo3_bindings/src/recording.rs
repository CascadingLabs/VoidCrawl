//! Python bindings for screen recording — the moving-picture counterpart to
//! `screenshot()`.
//!
//! Kwargs mirror `screenshot()`'s wherever the concept carries over
//! (`viewport_*`, `scroll_*`, `bbox`, and browser target fields), so the two
//! read the same at a call site. Two deliberate differences, both forced by
//! the underlying CDP screencast and documented on
//! [`void_crawl_core::recording`]:
//!
//! * There is no `full_page` — a screencast only ever contains the viewport.
//! * Selectors are **plural**: `selectors=[entry, ...]` produces one cropped
//!   region per entry, all cut from a single screencast.

use std::{fmt, path::PathBuf, sync::Arc, time::Duration};

use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict},
};
use pyo3_async_runtimes::tokio::future_into_py;
use tokio::sync::Mutex;
use void_crawl_core::{
    BrowserByteCount, BrowserByteDomain, BrowserByteReport, BrowserTarget, DocumentEpoch, Encoding,
    FrameFormat, MaskSpec, Page, Recording, RecordingHandle, RecordingOptions, ScrollTarget,
};

use crate::{resolve_selector_args, resolve_viewport_args, snapshots::byte_report_dict, to_py_err};

/// One captured frame.
#[pyclass(name = "Frame", module = "voidcrawl._ext", frozen)]
#[derive(Debug)]
pub struct PyFrame {
    /// Position in the sequence, 0-based.
    #[pyo3(get)]
    pub index: usize,
    /// Real elapsed milliseconds from the start of the recording. Frames are
    /// **not** evenly spaced — encode against this, not `index / fps`.
    #[pyo3(get)]
    pub offset_ms: f64,
    data: Vec<u8>,
}

#[pymethods]
impl PyFrame {
    /// The encoded image bytes, in the recording's ``format``.
    #[getter]
    fn data<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.data)
    }

    fn __len__(&self) -> usize {
        self.data.len()
    }

    #[getter]
    fn byte_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let bytes = BrowserByteCount::try_from_usize(self.data.len())
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        byte_report_dict(
            py,
            BrowserByteReport::from_known_extent(
                BrowserByteDomain::RecordingFrame,
                None,
                bytes,
                bytes,
            )
            .map_err(|error| PyValueError::new_err(error.to_string()))?,
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "Frame(index={}, offset_ms={:.1}, bytes={})",
            self.index,
            self.offset_ms,
            self.data.len()
        )
    }
}

/// One recorded region: the full viewport, an explicit ``bbox``, or one
/// selector's resolved rectangle.
#[pyclass(name = "RecordedRegion", module = "voidcrawl._ext", frozen)]
#[derive(Debug)]
pub struct PyRecordedRegion {
    /// Name derived from the selector, or ``"viewport"`` / ``"bbox"``. Also
    /// the on-disk subdirectory when ``write_frames=True``.
    #[pyo3(get)]
    pub label: String,
    /// ``(x, y, width, height)`` in CSS pixels, or ``None`` for the full
    /// frame. Resolved once when recording started, then held fixed.
    #[pyo3(get)]
    pub bbox: Option<(u32, u32, u32, u32)>,
    #[pyo3(get)]
    pub frames: Vec<Py<PyFrame>>,
    /// Paths of encoded artifacts written for this region.
    #[pyo3(get)]
    pub outputs: Vec<String>,
    byte_report: BrowserByteReport,
    output_byte_reports: Vec<BrowserByteReport>,
}

#[pymethods]
impl PyRecordedRegion {
    #[getter]
    fn byte_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        byte_report_dict(py, self.byte_report.clone())
    }

    #[getter]
    fn output_byte_reports<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyAny>>> {
        self.output_byte_reports
            .iter()
            .cloned()
            .map(|report| byte_report_dict(py, report))
            .collect()
    }

    fn __repr__(&self) -> String {
        format!("RecordedRegion(label={:?}, frames={})", self.label, self.frames.len())
    }
}

/// What one mask covered.
///
/// The library covers the rectangles it is given and reports the result; it
/// does not decide what is sensitive, and a recording with masks is not
/// thereby a safe-to-share one. ``unresolved_ticks`` and ``stale_frames`` are
/// there so the caller can make that call themselves.
#[pyclass(name = "MaskReport", module = "voidcrawl._ext", frozen)]
#[derive(Debug)]
pub struct PyMaskReport {
    #[pyo3(get)]
    pub label: String,
    /// ``(x, y, width, height)`` in CSS pixels, as first resolved.
    #[pyo3(get)]
    pub bbox: (u32, u32, u32, u32),
    /// Whether the mask was re-resolved while recording.
    #[pyo3(get)]
    pub tracked: bool,
    /// Ticks where re-resolution failed. The mask kept its last known
    /// rectangle for those, so something stayed covered.
    #[pyo3(get)]
    pub unresolved_ticks: usize,
    /// Frames captured while the most recent re-resolution had failed.
    #[pyo3(get)]
    pub stale_frames: usize,
}

#[pymethods]
impl PyMaskReport {
    fn __repr__(&self) -> String {
        format!(
            "MaskReport(label={:?}, bbox={:?}, tracked={}, stale_frames={})",
            self.label, self.bbox, self.tracked, self.stale_frames
        )
    }
}

/// The result of a recording.
#[pyclass(name = "Recording", module = "voidcrawl._ext", frozen)]
#[derive(Debug)]
pub struct PyRecording {
    #[pyo3(get)]
    pub started_at_unix_ms: Option<u64>,
    #[pyo3(get)]
    pub document_epoch: Option<u64>,
    /// One entry per requested region; a single ``"viewport"`` region when
    /// neither ``bbox`` nor ``selectors`` was given.
    #[pyo3(get)]
    pub regions: Vec<Py<PyRecordedRegion>>,
    /// One entry per requested mask. Empty means nothing was asked to be
    /// covered — not that there was nothing worth covering.
    #[pyo3(get)]
    pub masks: Vec<Py<PyMaskReport>>,
    /// ``"jpeg"`` or ``"png"``.
    #[pyo3(get)]
    pub format: String,
    #[pyo3(get)]
    pub duration_ms: f64,
    #[pyo3(get)]
    pub frames_captured: usize,
    /// Frames Chrome delivered that the fps ceiling or frame cap discarded.
    /// Large next to a small ``frames_captured`` means ``fps`` was binding.
    #[pyo3(get)]
    pub frames_dropped: usize,
    #[pyo3(get)]
    pub frames_dropped_by_rate: usize,
    #[pyo3(get)]
    pub frames_dropped_by_limit: usize,
    #[pyo3(get)]
    pub frame_decode_failures: usize,
    #[pyo3(get)]
    pub frame_ack_failures: usize,
    #[pyo3(get)]
    pub stream_disconnected: bool,
    #[pyo3(get)]
    pub complete: bool,
    #[pyo3(get)]
    pub frame_size_pixels: Option<(u32, u32)>,
    #[pyo3(get)]
    pub capture_viewport_css: Option<(f64, f64)>,
    #[pyo3(get)]
    pub device_pixel_ratio: f64,
    /// Whether this recording pinned its tab to the foreground and held the
    /// browser's capture lock. ``False`` means it ran concurrently with the
    /// rest of the browser — see ``foreground`` in :meth:`Page.record`.
    #[pyo3(get)]
    pub foregrounded: bool,
    byte_report: BrowserByteReport,
}

#[pymethods]
impl PyRecording {
    #[getter]
    fn byte_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        byte_report_dict(py, self.byte_report.clone())
    }

    /// Frames per second actually achieved — at most the requested ``fps``,
    /// and usually below it on a mostly-static page. Report this, not the
    /// requested value.
    fn effective_fps(&self) -> f64 {
        if self.duration_ms <= 0.0 {
            return 0.0;
        }
        #[expect(clippy::cast_precision_loss, reason = "frame counts are small")]
        let frames = self.frames_captured as f64;
        frames / (self.duration_ms / 1000.0)
    }

    fn __repr__(&self) -> String {
        format!(
            "Recording(regions={}, frames={}, dropped={}, {:.1}ms, foregrounded={})",
            self.regions.len(),
            self.frames_captured,
            self.frames_dropped,
            self.duration_ms,
            self.foregrounded
        )
    }
}

/// A recording in flight, returned by :meth:`Page.start_recording`.
///
/// Drive the page normally — clicks, typing, navigation all keep recording —
/// then call :meth:`stop`. Also usable as an async context manager, which
/// stops the recording on exit and exposes the result as ``handle.result``.
#[pyclass(name = "RecordingHandle", module = "voidcrawl._ext")]
pub struct PyRecordingHandle {
    handle: Arc<Mutex<Option<RecordingHandle>>>,
    page: Arc<Page>,
}

impl fmt::Debug for PyRecordingHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordingHandle").finish_non_exhaustive()
    }
}

impl PyRecordingHandle {
    pub(crate) fn new(handle: RecordingHandle, page: Arc<Page>) -> Self {
        Self { handle: Arc::new(Mutex::new(Some(handle))), page }
    }
}

#[pymethods]
impl PyRecordingHandle {
    /// Stop the recording and return the :class:`Recording`.
    ///
    /// Restores the page's viewport and scroll position, releases the
    /// capture lock if one was taken, then crops and encodes. Calling twice
    /// raises :class:`RuntimeError`.
    fn stop<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let slot = Arc::clone(&self.handle);
        let page = Arc::clone(&self.page);
        future_into_py(py, async move {
            let handle = slot
                .lock()
                .await
                .take()
                .ok_or_else(|| PyRuntimeError::new_err("recording is already stopped"))?;
            let recording = handle.stop(&page).await.map_err(to_py_err)?;
            Python::attach(|py| into_py_recording(py, recording))
        })
    }
}

/// Convert a core [`Recording`] into its Python mirror.
pub(crate) fn into_py_recording(py: Python<'_>, rec: Recording) -> PyResult<Py<PyRecording>> {
    let byte_report =
        rec.byte_report().map_err(|error| PyValueError::new_err(error.to_string()))?;
    let started_at_unix_ms = rec.started_at_unix_ms;
    let document_epoch = match rec.document_epoch {
        DocumentEpoch::Known(epoch) => Some(epoch),
        DocumentEpoch::UnavailableForAttachedPage => None,
    };
    let mut regions = Vec::with_capacity(rec.regions.len());
    for region in rec.regions {
        let region_byte_report =
            region.byte_report().map_err(|error| PyValueError::new_err(error.to_string()))?;
        let output_byte_reports = region
            .output_byte_reports()
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let mut frames = Vec::with_capacity(region.frames.len());
        for frame in region.frames {
            frames.push(Py::new(
                py,
                PyFrame {
                    index: frame.index,
                    offset_ms: frame.offset.as_secs_f64() * 1000.0,
                    data: frame.data,
                },
            )?);
        }
        regions.push(Py::new(
            py,
            PyRecordedRegion {
                label: region.label,
                bbox: region.bbox.map(|b| (b.x, b.y, b.width, b.height)),
                frames,
                outputs: region.outputs.into_iter().map(|p| p.display().to_string()).collect(),
                byte_report: region_byte_report,
                output_byte_reports,
            },
        )?);
    }
    let mut masks = Vec::with_capacity(rec.masks.len());
    for mask in rec.masks {
        masks.push(Py::new(
            py,
            PyMaskReport {
                label: mask.label,
                bbox: (mask.bbox.x, mask.bbox.y, mask.bbox.width, mask.bbox.height),
                tracked: mask.tracked,
                unresolved_ticks: mask.unresolved_ticks,
                stale_frames: mask.stale_frames,
            },
        )?);
    }
    Py::new(
        py,
        PyRecording {
            started_at_unix_ms,
            document_epoch,
            regions,
            masks,
            format: match rec.format {
                FrameFormat::Jpeg => "jpeg".to_string(),
                FrameFormat::Png => "png".to_string(),
            },
            duration_ms: rec.duration.as_secs_f64() * 1000.0,
            frames_captured: rec.frames_captured,
            frames_dropped: rec.frames_dropped,
            frames_dropped_by_rate: rec.frames_dropped_by_rate,
            frames_dropped_by_limit: rec.frames_dropped_by_limit,
            frame_decode_failures: rec.frame_decode_failures,
            frame_ack_failures: rec.frame_ack_failures,
            stream_disconnected: rec.stream_disconnected,
            complete: rec.complete,
            frame_size_pixels: rec.frame_size_pixels,
            capture_viewport_css: rec.capture_viewport_css,
            device_pixel_ratio: rec.device_pixel_ratio,
            foregrounded: rec.foregrounded,
            byte_report,
        },
    )
}

/// Parse one `selectors=[...]` entry. Accepts the same field names as a
/// Legacy-compatible browser target dump
/// (`type`/`value`/`regex`/`name`/`nth`/`x`/`y`), so `entry.model_dump()` can
/// be passed straight through.
fn selector_from_dict(item: &Bound<'_, PyAny>) -> PyResult<BrowserTarget> {
    let dict = item.cast::<PyDict>().map_err(|_| {
        PyValueError::new_err("each entry in `selectors` must be a dict of BrowserTarget fields")
    })?;
    // Non-generic on purpose: a generic `get::<T>` would tie the extracted
    // value's borrow to the helper's lifetime parameter, which the temporary
    // `Bound` from `get_item` can't satisfy.
    let get_str = |key: &str| -> PyResult<Option<String>> {
        match dict.get_item(key)? {
            Some(v) if !v.is_none() => Ok(Some(v.extract::<String>()?)),
            _ => Ok(None),
        }
    };
    let get_u32 = |key: &str| -> PyResult<Option<u32>> {
        match dict.get_item(key)? {
            Some(v) if !v.is_none() => Ok(Some(v.extract::<u32>()?)),
            _ => Ok(None),
        }
    };
    let get_f64 = |key: &str| -> PyResult<Option<f64>> {
        match dict.get_item(key)? {
            Some(v) if !v.is_none() => Ok(Some(v.extract::<f64>()?)),
            _ => Ok(None),
        }
    };
    // `type` is the legacy wire field; `selector_type` is accepted too so the
    // plural form lines up with `screenshot()`'s flat kwargs.
    let kind = match get_str("type")? {
        Some(k) => Some(k),
        None => get_str("selector_type")?,
    };
    resolve_selector_args(
        kind.as_deref(),
        get_str("value")?,
        get_str("regex")?,
        get_str("name")?,
        get_u32("nth")?,
        get_f64("x")?,
        get_f64("y")?,
    )
}

/// Parse one `masks=[...]` entry.
///
/// Two accepted shapes, because both read naturally at a call site:
///
/// * a bare selector dict — `{"type": "css", "value": "#password"}` — the
///   common case, tracked by default;
/// * a mask dict — `{"bbox": (x, y, w, h)}` or `{"selector": {...}, "track":
///   False, "label": "pw"}`.
fn mask_from_dict(item: &Bound<'_, PyAny>) -> PyResult<MaskSpec> {
    let dict = item.cast::<PyDict>().map_err(|_| {
        PyValueError::new_err(
            "each entry in `masks` must be a dict: a selector dict, or {'bbox': (x, y, w, h)} / \
             {'selector': {...}}",
        )
    })?;

    // A bare selector dump is identifiable by its `type` field, which a mask
    // dict never has.
    if dict.contains("type")? || dict.contains("selector_type")? {
        return Ok(MaskSpec::selector(selector_from_dict(item)?));
    }

    let bbox = dict.get_item("bbox")?.filter(|v| !v.is_none());
    let selector = dict.get_item("selector")?.filter(|v| !v.is_none());
    let mut spec = match (bbox, selector) {
        (Some(_), Some(_)) => {
            return Err(PyValueError::new_err(
                "a mask takes either `bbox` or `selector`, not both",
            ));
        }
        (None, None) => {
            return Err(PyValueError::new_err("a mask needs either `bbox` or `selector`"));
        }
        (Some(bbox), None) => {
            let (x, y, width, height) = bbox.extract::<(u32, u32, u32, u32)>()?;
            MaskSpec::bbox(void_crawl_core::Bbox { x, y, width, height })
        }
        (None, Some(selector)) => MaskSpec::selector(selector_from_dict(&selector)?),
    };

    if let Some(track) = dict.get_item("track")?.filter(|v| !v.is_none()) {
        spec = spec.with_track(track.extract::<bool>()?);
    }
    if let Some(label) = dict.get_item("label")?.filter(|v| !v.is_none()) {
        spec = spec.with_label(label.extract::<String>()?);
    }
    Ok(spec)
}

fn encoding_from_str(name: &str) -> PyResult<Encoding> {
    match name {
        "gif" => Ok(Encoding::Gif),
        "mp4" => Ok(Encoding::Mp4),
        "webm" => Ok(Encoding::WebM),
        other => Err(PyValueError::new_err(format!(
            "unknown encoding {other:?}; expected one of gif, mp4, webm"
        ))),
    }
}

/// Build a [`RecordingOptions`] from the flat kwargs the bindings accept.
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub(crate) fn build_recording_options(
    dir: Option<String>,
    bbox: Option<(u32, u32, u32, u32)>,
    selectors: Option<Vec<Py<PyAny>>>,
    masks: Option<Vec<Py<PyAny>>>,
    mask_pad: Option<u32>,
    viewport_preset: Option<&str>,
    viewport_width: Option<u32>,
    viewport_height: Option<u32>,
    viewport_device_scale_factor: Option<f64>,
    viewport_mobile: Option<bool>,
    scroll_viewports: Option<f64>,
    scroll_pixels: Option<i64>,
    fps: Option<u8>,
    duration_secs: Option<f64>,
    max_frames: Option<usize>,
    format: Option<&str>,
    quality: Option<u8>,
    write_frames: Option<bool>,
    foreground: Option<bool>,
    encode: Option<Vec<String>>,
    py: Python<'_>,
) -> PyResult<RecordingOptions> {
    let has_selectors = selectors.as_ref().is_some_and(|s| !s.is_empty());
    if bbox.is_some() && has_selectors {
        return Err(PyValueError::new_err("bbox and selectors are mutually exclusive"));
    }

    let mut opts = RecordingOptions::default();
    if let Some(d) = dir {
        opts = opts.with_dir(PathBuf::from(d));
    }
    if let Some((x, y, w, h)) = bbox {
        opts = opts.with_bbox(void_crawl_core::Bbox { x, y, width: w, height: h });
    }
    if let Some(entries) = selectors {
        for entry in entries {
            opts = opts.with_selector(selector_from_dict(entry.bind(py))?);
        }
    }
    if let Some(entries) = masks {
        for entry in entries {
            opts = opts.with_mask(mask_from_dict(entry.bind(py))?);
        }
    }
    if let Some(pad) = mask_pad {
        opts = opts.with_mask_pad(pad);
    }
    if viewport_preset.is_some() || viewport_width.is_some() || viewport_height.is_some() {
        opts = opts.with_viewport(resolve_viewport_args(
            viewport_preset,
            viewport_width,
            viewport_height,
            viewport_device_scale_factor,
            viewport_mobile,
        )?);
    }
    match (scroll_viewports, scroll_pixels) {
        (Some(n), None) => opts = opts.with_scroll(ScrollTarget::Viewports(n)),
        (None, Some(y)) => opts = opts.with_scroll(ScrollTarget::Pixels(y)),
        (Some(_), Some(_)) => {
            return Err(PyValueError::new_err(
                "scroll_viewports and scroll_pixels are mutually exclusive",
            ));
        }
        (None, None) => {}
    }
    if let Some(fps) = fps {
        opts = opts.with_fps(fps);
    }
    if let Some(secs) = duration_secs {
        if !secs.is_finite() || secs <= 0.0 {
            return Err(PyValueError::new_err("duration_secs must be a positive number"));
        }
        opts = opts.with_max_duration(Duration::from_secs_f64(secs));
    }
    if let Some(max) = max_frames {
        opts.max_frames = max;
    }
    if let Some(f) = format {
        opts = opts.with_format(match f {
            "jpeg" | "jpg" => FrameFormat::Jpeg,
            "png" => FrameFormat::Png,
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown format {other:?}; expected 'jpeg' or 'png'"
                )));
            }
        });
    }
    if let Some(q) = quality {
        opts.quality = q;
    }
    if let Some(w) = write_frames {
        opts.write_frames = w;
    }
    if let Some(f) = foreground {
        opts = opts.with_foreground(f);
    }
    if let Some(encodings) = encode {
        for name in encodings {
            opts = opts.with_encoding(encoding_from_str(&name)?);
        }
    }
    Ok(opts)
}
