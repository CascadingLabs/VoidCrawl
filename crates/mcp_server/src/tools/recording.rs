//! Screen recording: a stateless variant that navigates a pooled tab and
//! records for a fixed duration, plus a start/stop pair that records an open
//! session while the caller drives it.
//!
//! These deliberately do **not** return frames inline. A screenshot is one
//! image; a recording is hundreds, and streaming them through an MCP response
//! would swamp the caller's context for no benefit. Frames and any encoded
//! artifact are written to disk and the response carries paths plus counts —
//! so an agent can hand the path to a human, feed it to a video tool, or read
//! back a single frame with an ordinary file read.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use void_crawl_core::{
    Encoding, FrameFormat, Page, Recording, RecordingOptions, SelectorEntry, VoidCrawlError,
};

use crate::{
    server::VoidCrawlServer,
    sessions::PendingRecording,
    tools::{
        selector::SelectorArg,
        viewport::{BboxArg, ScrollArg, ViewportArg},
        wait,
    },
};

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Default recording length. Short on purpose: a recording on a pooled tab
/// holds that browser's capture lock, and an agent that wants more can ask.
pub const DEFAULT_DURATION_SECS: f64 = 5.0;

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct RecordArgs {
    /// Absolute URL to record.
    pub url:           String,
    /// How long to record, in seconds (default 5, max 120).
    #[serde(default)]
    pub duration_secs: Option<f64>,
    /// Optional wait strategy applied before recording starts:
    /// "networkidle" (default) or "selector:<css>".
    #[serde(default)]
    pub wait_for:      Option<String>,
    /// Navigation + wait timeout in seconds (default 30).
    #[serde(default)]
    pub timeout_secs:  Option<u64>,
    /// Directory for the frames and any encoded artifact. Defaults to a new
    /// directory under the system temp dir; the exact path is returned.
    #[serde(default)]
    pub output_dir:    Option<String>,
    /// Record as this device/viewport instead of the pool's default —
    /// one-shot, does not persist.
    #[serde(default)]
    pub viewport:      Option<ViewportArg>,
    /// Crop every frame to this CSS-pixel region. **Viewport-relative**,
    /// unlike `screenshot`'s page-relative bbox: a recording frame only ever
    /// contains the viewport. Mutually exclusive with `selectors`.
    #[serde(default)]
    pub bbox:          Option<BboxArg>,
    /// Crop to each of these Yosoi selectors' rectangles, producing one
    /// region per selector from a single recording. Each is resolved once at
    /// start and then held fixed, so an element that moves mid-recording
    /// drifts out of its crop. A selector matching nothing, ambiguous, or
    /// non-visual (jsonld/regex) fails the call before recording begins.
    /// Mutually exclusive with `bbox`.
    #[serde(default)]
    pub selectors:     Vec<SelectorArg>,
    /// Scroll before recording. Since a recording only captures the viewport,
    /// this is how you choose which part of a long page gets recorded.
    #[serde(default)]
    pub scroll:        Option<ScrollArg>,
    /// Frame-rate ceiling (default 10). Not a floor: Chrome emits frames when
    /// it paints, so a static page yields very few regardless.
    #[serde(default)]
    pub fps:           Option<u8>,
    /// Frame format: "jpeg" (default) or "png".
    #[serde(default)]
    pub format:        Option<String>,
    /// JPEG quality 1-100 (default 80). Ignored for png.
    #[serde(default)]
    pub quality:       Option<u8>,
    /// Also encode each region to a single file: any of "gif", "mp4",
    /// "webm". Requires the matching build feature (and, for mp4/webm, an
    /// ffmpeg binary); when unavailable this reports an error rather than
    /// silently skipping, and the frames remain on disk either way.
    #[serde(default)]
    pub encode:        Vec<String>,
    /// Write the individual frames to disk (default true). Set false when
    /// only an encoded artifact is wanted.
    #[serde(default)]
    pub write_frames:  Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct SessionRecordStartArgs {
    pub session_id:        String,
    /// Hard upper bound in seconds (default 30, max 120). The recording stops
    /// itself at this point even if `session_record_stop` is never called, so
    /// an abandoned recording can't hold the browser open.
    #[serde(default)]
    pub max_duration_secs: Option<f64>,
    #[serde(default)]
    pub output_dir:        Option<String>,
    #[serde(default)]
    pub viewport:          Option<ViewportArg>,
    #[serde(default)]
    pub bbox:              Option<BboxArg>,
    #[serde(default)]
    pub selectors:         Vec<SelectorArg>,
    #[serde(default)]
    pub scroll:            Option<ScrollArg>,
    #[serde(default)]
    pub fps:               Option<u8>,
    #[serde(default)]
    pub format:            Option<String>,
    #[serde(default)]
    pub quality:           Option<u8>,
    #[serde(default)]
    pub encode:            Vec<String>,
    #[serde(default)]
    pub write_frames:      Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct SessionRecordStopArgs {
    pub session_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RegionResult {
    /// "viewport", "bbox", or a name derived from the selector.
    pub label:       String,
    /// [x, y, width, height] in CSS pixels, or null for the whole frame.
    pub bbox:        Option<[u32; 4]>,
    pub frame_count: usize,
    /// Directory holding this region's numbered frames, when frames were
    /// written.
    pub frames_dir:  Option<String>,
    /// Encoded artifacts written for this region.
    pub outputs:     Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RecordResult {
    pub output_dir:         String,
    pub regions:            Vec<RegionResult>,
    pub format:             String,
    pub duration_ms:        f64,
    pub frames_captured:    usize,
    /// Frames Chrome delivered that the fps ceiling discarded. Large next to
    /// a small `frames_captured` means `fps` was the binding constraint.
    pub frames_dropped:     usize,
    /// Frames per second actually achieved. Well below the requested `fps` on
    /// a mostly-static page — that's expected, not a fault.
    pub effective_fps:      f64,
    pub device_pixel_ratio: f64,
    /// Whether the recording had to hold the browser's capture lock. True for
    /// a pooled tab (it shares a window with its siblings, and a backgrounded
    /// tab in a shared window stops painting entirely).
    pub foregrounded:       bool,
    /// Set when frames were captured but encoding them failed — e.g. no
    /// ffmpeg on PATH. The frames on disk are still usable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encode_error:       Option<String>,
}

/// Cap on how long any one recording may run, whatever the caller asks for.
const MAX_DURATION_SECS: f64 = 120.0;

fn resolve_duration(secs: Option<f64>, default: f64) -> Result<Duration, VoidCrawlError> {
    let secs = secs.unwrap_or(default);
    if !secs.is_finite() || secs <= 0.0 {
        return Err(VoidCrawlError::RecordingError(
            "duration must be a positive number of seconds".into(),
        ));
    }
    if secs > MAX_DURATION_SECS {
        return Err(VoidCrawlError::RecordingError(format!(
            "duration {secs}s exceeds the {MAX_DURATION_SECS}s maximum"
        )));
    }
    Ok(Duration::from_secs_f64(secs))
}

fn resolve_format(name: Option<&str>) -> Result<FrameFormat, VoidCrawlError> {
    match name {
        None | Some("jpeg" | "jpg") => Ok(FrameFormat::Jpeg),
        Some("png") => Ok(FrameFormat::Png),
        Some(other) => Err(VoidCrawlError::RecordingError(format!(
            "unknown format {other:?}; expected 'jpeg' or 'png'"
        ))),
    }
}

fn resolve_encodings(names: &[String]) -> Result<Vec<Encoding>, VoidCrawlError> {
    names
        .iter()
        .map(|n| match n.as_str() {
            "gif" => Ok(Encoding::Gif),
            "mp4" => Ok(Encoding::Mp4),
            "webm" => Ok(Encoding::WebM),
            other => Err(VoidCrawlError::RecordingError(format!(
                "unknown encoding {other:?}; expected one of gif, mp4, webm"
            ))),
        })
        .collect()
}

/// A fresh directory for one recording's artifacts.
fn resolve_output_dir(explicit: Option<&str>) -> Result<PathBuf, VoidCrawlError> {
    let dir = match explicit {
        Some(d) => PathBuf::from(d),
        None => env::temp_dir().join("voidcrawl-recordings").join(uuid::Uuid::new_v4().to_string()),
    };
    fs::create_dir_all(&dir)
        .map_err(|e| VoidCrawlError::RecordingError(format!("create {}: {e}", dir.display())))?;
    Ok(dir)
}

#[allow(clippy::too_many_arguments)]
fn build_options(
    output_dir: PathBuf,
    viewport: Option<&ViewportArg>,
    bbox: Option<&BboxArg>,
    selectors: Vec<SelectorArg>,
    scroll: Option<&ScrollArg>,
    fps: Option<u8>,
    duration: Duration,
    format: Option<&str>,
    quality: Option<u8>,
    encode: &[String],
    write_frames: Option<bool>,
) -> Result<RecordingOptions, ErrorData> {
    if bbox.is_some() && !selectors.is_empty() {
        return Err(ErrorData::invalid_params(
            "`bbox` and `selectors` are mutually exclusive",
            None,
        ));
    }

    let mut opts = RecordingOptions::default().with_dir(output_dir).with_max_duration(duration);
    opts.write_frames = write_frames.unwrap_or(true);
    opts.format = resolve_format(format).map_err(|e| to_invalid_params(&e))?;
    opts.encode = resolve_encodings(encode).map_err(|e| to_invalid_params(&e))?;

    if let Some(v) = viewport {
        opts = opts.with_viewport(v.resolve()?);
    }
    if let Some(b) = bbox {
        opts = opts.with_bbox((*b).into());
    }
    for selector in selectors {
        let entry: SelectorEntry = selector.into();
        opts = opts.with_selector(entry);
    }
    if let Some(s) = scroll {
        opts = opts.with_scroll(s.resolve()?);
    }
    if let Some(fps) = fps {
        if fps == 0 {
            return Err(ErrorData::invalid_params("`fps` must be at least 1", None));
        }
        opts = opts.with_fps(fps);
    }
    if let Some(q) = quality {
        opts.quality = q;
    }
    Ok(opts)
}

fn to_invalid_params(e: &VoidCrawlError) -> ErrorData {
    ErrorData::invalid_params(e.to_string(), None)
}

/// Navigate a pooled tab and record it for a fixed duration.
pub async fn run(
    server: &VoidCrawlServer,
    args: RecordArgs,
) -> Result<RecordResult, VoidCrawlError> {
    let duration = resolve_duration(args.duration_secs, DEFAULT_DURATION_SECS)?;
    let output_dir = resolve_output_dir(args.output_dir.as_deref())?;
    let opts = build_options(
        output_dir.clone(),
        args.viewport.as_ref(),
        args.bbox.as_ref(),
        args.selectors,
        args.scroll.as_ref(),
        args.fps,
        duration,
        args.format.as_deref(),
        args.quality,
        &args.encode,
        args.write_frames,
    )
    .map_err(|e| VoidCrawlError::RecordingError(e.message.to_string()))?;

    let pool = server.state().pool().await?;
    let tab = pool.acquire().await?;
    let result = async {
        let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        tab.page.goto_and_wait_for_idle(&args.url, timeout).await?;
        wait::apply_post_navigate(&tab.page, args.wait_for.as_deref(), timeout).await?;
        record_capturing_encode_errors(&tab.page, opts).await
    }
    .await;
    pool.release(tab).await;

    let (recording, encode_error) = result?;
    Ok(to_result(&recording, &output_dir, encode_error))
}

/// Begin recording an open session's page; the caller drives it and then
/// calls `session_record_stop`.
pub async fn session_start(
    server: &VoidCrawlServer,
    args: SessionRecordStartArgs,
) -> Result<SessionRecordStartResult, VoidCrawlError> {
    let duration = resolve_duration(args.max_duration_secs, 30.0)?;
    let output_dir = resolve_output_dir(args.output_dir.as_deref())?;
    let opts = build_options(
        output_dir.clone(),
        args.viewport.as_ref(),
        args.bbox.as_ref(),
        args.selectors,
        args.scroll.as_ref(),
        args.fps,
        duration,
        args.format.as_deref(),
        args.quality,
        &args.encode,
        args.write_frames,
    )
    .map_err(|e| VoidCrawlError::RecordingError(e.message.to_string()))?;

    let session =
        server.state().sessions.get(&args.session_id).await.ok_or_else(|| {
            VoidCrawlError::Other(format!("no such session: {}", args.session_id))
        })?;

    // Reject a second start rather than silently dropping the first
    // recording's frames — same contract as `download_arm`.
    if session.pending_recording.lock().await.is_some() {
        return Err(VoidCrawlError::RecordingError(
            "a recording is already running on this session; call session_record_stop first".into(),
        ));
    }

    let handle = {
        let page = session.page.lock().await;
        page.start_recording(opts).await?
    };
    *session.pending_recording.lock().await =
        Some(PendingRecording { handle, output_dir: output_dir.clone() });

    Ok(SessionRecordStartResult {
        recording:  true,
        output_dir: output_dir.display().to_string(),
        message:    "recording — drive the session as usual, then call session_record_stop".into(),
    })
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionRecordStartResult {
    pub recording:  bool,
    /// Where the frames and artifacts will land.
    pub output_dir: String,
    pub message:    String,
}

/// Stop the recording started by `session_record_start` and write it out.
pub async fn session_stop(
    server: &VoidCrawlServer,
    args: SessionRecordStopArgs,
) -> Result<RecordResult, VoidCrawlError> {
    let session =
        server.state().sessions.get(&args.session_id).await.ok_or_else(|| {
            VoidCrawlError::Other(format!("no such session: {}", args.session_id))
        })?;

    let pending = session.pending_recording.lock().await.take().ok_or_else(|| {
        VoidCrawlError::RecordingError(
            "no recording is running on this session; call session_record_start first".into(),
        )
    })?;
    let PendingRecording { handle, output_dir } = pending;

    let page = session.page.lock().await;
    let recording = handle.stop(&page).await?;
    Ok(to_result(&recording, &output_dir, None))
}

/// Run a recording, downgrading an encode failure to a reported warning so a
/// missing ffmpeg doesn't throw away frames that were captured successfully.
async fn record_capturing_encode_errors(
    page: &Page,
    opts: RecordingOptions,
) -> Result<(Recording, Option<String>), VoidCrawlError> {
    let encode = opts.encode.clone();
    let mut fallback = opts.clone();
    match page.record(opts).await {
        Ok(rec) => Ok((rec, None)),
        Err(VoidCrawlError::RecordingEncodeError(msg)) if !encode.is_empty() => {
            // Retry once without encoding so the caller still gets frames.
            fallback.encode.clear();
            let rec = page.record(fallback).await?;
            Ok((rec, Some(msg)))
        }
        Err(e) => Err(e),
    }
}

fn to_result(rec: &Recording, output_dir: &Path, encode_error: Option<String>) -> RecordResult {
    let regions = rec
        .regions
        .iter()
        .map(|r| RegionResult {
            label:       r.label.clone(),
            bbox:        r.bbox.map(|b| [b.x, b.y, b.width, b.height]),
            frame_count: r.frames.len(),
            frames_dir:  Some(output_dir.join(&r.label).display().to_string()),
            outputs:     r.outputs.iter().map(|p| p.display().to_string()).collect(),
        })
        .collect();

    RecordResult {
        output_dir: output_dir.display().to_string(),
        regions,
        format: match rec.format {
            FrameFormat::Jpeg => "jpeg".into(),
            FrameFormat::Png => "png".into(),
        },
        duration_ms: rec.duration.as_secs_f64() * 1000.0,
        frames_captured: rec.frames_captured,
        frames_dropped: rec.frames_dropped,
        effective_fps: rec.effective_fps(),
        device_pixel_ratio: rec.device_pixel_ratio,
        foregrounded: rec.foregrounded,
        encode_error,
    }
}
