//! Screen recording: capture a page as a sequence of timestamped frames,
//! optionally cropped to one or more regions, optionally encoded to an
//! animation or video.
//!
//! This is the moving-picture counterpart to [`Page::screenshot`], and the
//! options deliberately mirror [`ScreenshotOptions`](crate::ScreenshotOptions)
//! — same `viewport`, same `scroll`, same `bbox`, the same Yosoi selector
//! crop — so a caller who can screenshot a region can record it by changing
//! one call.
//!
//! # What the engine is, and what that costs
//!
//! Frames come from CDP's `Page.startScreencast` / `Page.screencastFrame`.
//! Three consequences follow from that choice, and callers should know all
//! three before reaching for this module:
//!
//! 1. **Viewport only.** A screencast frame is what's composited on screen.
//!    There is no `full_page: true` equivalent — the CDP surface simply doesn't
//!    offer one — so [`RecordingOptions`] has no `full_page` field. Use
//!    `viewport` to make the visible area bigger, or `scroll` to choose which
//!    part of a long page is on screen when recording starts.
//! 2. **Frames arrive on paint, not on a clock.** Chrome emits a frame when it
//!    swaps one, so a static page yields almost nothing and a busy page yields
//!    bursts. [`RecordingOptions::fps`] is therefore a *ceiling* applied
//!    client-side, never a guarantee. Every [`Frame`] carries its real
//!    [`Frame::offset`] from recording start, so a downstream encoder can
//!    resample honestly instead of pretending the rate was uniform.
//! 3. **It takes the browser's capture lock only when it has to.**
//!
//!    Chrome composites the frontmost tab *of a window*. Tabs opened by
//!    [`BrowserSession::new_page`](crate::BrowserSession::new_page) share one
//!    window, so the moment a sibling tab captures — which foregrounds it —
//!    the recorded tab stops painting and the screencast goes quiet. Measured
//!    on a continuously animating page over 3s:
//!
//!    | tab placement | foregrounded | frames |
//!    |---|---|---|
//!    | shared window | no  | 1 |
//!    | shared window | yes | 27 |
//!    | **own window** | **no** | **28** |
//!
//!    The `--disable-backgrounding-occluded-windows` /
//!    `--disable-renderer-backgrounding` launch flags do not rescue the
//!    shared-window case; occlusion *within* a window is a different
//!    mechanism.
//!
//!    So [`RecordingOptions::foreground`] defaults to `None`, meaning
//!    *detect*: [`Page::alone_in_window`] decides. A pooled, shared-window tab
//!    is foregrounded and holds the lock (correct, but serializes capture on
//!    that browser); a tab created by
//!    [`BrowserSession::new_page_in_window`](crate::BrowserSession::new_page_in_window)
//!    does neither and records at full rate concurrently with everything else.
//!    Neither case requires the caller to know any of this.
//!
//!    One sharp edge worth knowing: a plain `new_page` opens its tab in the
//!    *most recently active* window, so creating a page after
//!    `new_page_in_window` can drop it into that window and re-introduce the
//!    contention. Create the recording window last, or check
//!    [`Page::alone_in_window`].
//!
//!    Either way [`RecordingOptions::max_duration`] bounds the damage and
//!    defaults to 30s — an abandoned recording should neither hold the lock
//!    nor stream frames forever.
//!
//! # Regions
//!
//! Unlike a screenshot, which crops to at most one rectangle, a recording
//! can carry several: pass many [`RecordingOptions::selectors`] and each one
//! becomes its own [`RecordedRegion`] with its own cropped frame sequence,
//! all cut from the *same* underlying screencast. Recording three components
//! of a page costs one screencast, not three.
//!
//! Each selector is resolved to a rectangle **once, when recording starts**,
//! and that rectangle is then fixed for the whole recording. An element that
//! moves or resizes mid-recording will drift out of its crop; that is the
//! documented behavior, chosen to match `screenshot(selector: ...)`
//! semantics and to keep the per-frame cost at zero DOM round-trips.
//!
//! # Output
//!
//! [`Recording`] always carries the frames. Encoding to GIF or to a video
//! container is opt-in and lives behind cargo features (`encode-gif`,
//! `encode-ffmpeg`), because the frame sequence is the substrate and the
//! container is a policy choice — see [`Encoding`].

use std::{
    fmt, fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use chromiumoxide::{
    Page as CdpPage,
    cdp::browser_protocol::page::{
        EventScreencastFrame, ScreencastFrameAckParams, StartScreencastFormat,
        StartScreencastParams, StopScreencastParams,
    },
};
use futures::{Stream, StreamExt};
use image::{
    DynamicImage, GenericImageView, ImageFormat, codecs::jpeg::JpegEncoder, imageops,
    load_from_memory_with_format,
};
use serde::Serialize;
use tokio::{
    sync::{OwnedMutexGuard, oneshot},
    task::{JoinHandle, spawn_blocking},
    time::sleep,
};

use crate::{
    error::{Result, VoidCrawlError},
    page::{Bbox, Page},
    selector::{SelectorEntry, SelectorResolution},
    viewport::{ScrollTarget, Viewport},
};

/// Default frame-rate ceiling: enough to read a UI interaction back, cheap
/// enough that a 30s recording stays in the low hundreds of frames.
pub const DEFAULT_FPS: u8 = 10;
/// Default hard stop. Also the pool-safety bound: a recording on a
/// shared-window tab holds the browser's capture lock, so an abandoned one
/// must not run forever.
pub const DEFAULT_MAX_DURATION: Duration = Duration::from_secs(30);
/// Default in-memory frame cap, independent of duration and fps. At the
/// defaults this is never reached; it bounds a pathological burst.
pub const DEFAULT_MAX_FRAMES: usize = 900;
/// Default JPEG quality for screencast frames.
pub const DEFAULT_QUALITY: u8 = 80;

/// Wire format Chrome encodes each screencast frame in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FrameFormat {
    /// Lossy, far smaller — the right default for motion.
    Jpeg,
    /// Lossless, much larger. Worth it for pixel-exact diffing of frames.
    Png,
}

impl FrameFormat {
    fn as_cdp(self) -> StartScreencastFormat {
        match self {
            Self::Jpeg => StartScreencastFormat::Jpeg,
            Self::Png => StartScreencastFormat::Png,
        }
    }

    fn as_image(self) -> ImageFormat {
        match self {
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Png => ImageFormat::Png,
        }
    }

    /// File extension for a frame written to disk, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
        }
    }
}

/// An optional post-processing step that turns the frame sequence into a
/// single playable artifact.
///
/// Both variants are feature-gated: a recording's *frames* are always
/// available, but this crate does not carry a codec stack by default.
/// Requesting an encoding whose feature is off is an
/// [`VoidCrawlError::RecordingEncodeError`], not a silent no-op — the frames
/// are still returned on the [`Recording`], so nothing is lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Encoding {
    /// Animated GIF, encoded in-process (cargo feature `encode-gif`).
    /// Self-contained and permissively licensed; large files, 256 colors.
    Gif,
    /// H.264 MP4 via an `ffmpeg` binary on PATH (cargo feature
    /// `encode-ffmpeg`). Small files, real video, external runtime
    /// dependency.
    Mp4,
    /// VP9 WebM via an `ffmpeg` binary on PATH (cargo feature
    /// `encode-ffmpeg`).
    WebM,
}

impl Encoding {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Gif => "gif",
            Self::Mp4 => "mp4",
            Self::WebM => "webm",
        }
    }
}

/// Options for [`Page::record`] and [`Page::start_recording`].
///
/// Mirrors [`ScreenshotOptions`](crate::ScreenshotOptions) field-for-field
/// where the concepts carry over. The differences, all forced by the
/// screencast engine, are called out in the module docs: no `full_page`,
/// `bbox` is viewport-relative, and `selectors` is plural.
#[derive(Debug, Clone)]
pub struct RecordingOptions {
    /// Directory for artifacts — encoded outputs, and frames when
    /// `write_frames` is set. `None` keeps everything in memory.
    pub dir:          Option<PathBuf>,
    /// Crop every frame to this region. **Viewport-relative**, unlike
    /// [`ScreenshotOptions::bbox`](crate::ScreenshotOptions::bbox), which is
    /// page-relative: a screencast frame only ever contains the viewport, so
    /// page coordinates outside it have nothing to crop from. Use `scroll`
    /// to bring the region on screen first. Mutually exclusive with
    /// `selectors`.
    pub bbox:         Option<Bbox>,
    /// Crop to each of these selectors' resolved rectangles, producing one
    /// [`RecordedRegion`] per selector from a single screencast. Each is
    /// resolved once at start and then held fixed. Mutually exclusive with
    /// `bbox`. A selector that matches nothing, is ambiguous, or is
    /// inherently non-visual (`jsonld`/`regex`) fails the whole recording
    /// before any frame is captured, rather than silently yielding an empty
    /// region.
    pub selectors:    Vec<SelectorEntry>,
    /// Record as this device/viewport, then restore whatever was active
    /// before — even on error. See [`Page::set_viewport`].
    pub viewport:     Option<Viewport>,
    /// Scroll here before recording starts, and restore the original scroll
    /// position when it stops. Since the screencast is viewport-only, this
    /// is how you choose *which part* of a long page gets recorded.
    pub scroll:       Option<ScrollTarget>,
    /// Frame-rate **ceiling**, applied client-side by dropping frames that
    /// arrive sooner than `1/fps` after the last kept one. Not a floor: see
    /// the module docs on paint-driven delivery. Must be >= 1.
    pub fps:          u8,
    /// Hard stop. [`Page::record`] returns after exactly this long; a
    /// [`RecordingHandle`] stops itself at this point even if nothing calls
    /// [`RecordingHandle::stop`], so an abandoned recording can't hold the
    /// browser's capture lock forever.
    pub max_duration: Duration,
    /// In-memory frame cap. Frames past it are counted in
    /// [`Recording::frames_dropped`] rather than growing the heap without
    /// bound.
    pub max_frames:   usize,
    /// Wire format for each frame.
    pub format:       FrameFormat,
    /// JPEG quality, 0-100. Ignored for [`FrameFormat::Png`].
    pub quality:      u8,
    /// Also write every frame to `dir` as `{region}/{index}.{ext}`.
    pub write_frames: bool,
    /// Whether to pin this tab to the foreground — and hold the browser-wide
    /// capture lock — for the whole recording.
    ///
    /// `None` (the default) decides automatically, via
    /// [`Page::alone_in_window`]:
    ///
    /// * **Tab shares its window** (what
    ///   [`BrowserSession::new_page`](crate::BrowserSession::new_page) and the
    ///   pool produce) → foreground and lock. Chrome composites only a window's
    ///   frontmost tab, so without this the recording would collect 1 frame in
    ///   3s the moment a sibling captured. Sibling captures block until the
    ///   recording stops.
    /// * **Tab is alone in its window** (see
    ///   [`BrowserSession::new_page_in_window`](crate::BrowserSession::new_page_in_window))
    ///   → neither. Another window taking focus can't occlude it, so it records
    ///   at full rate while the rest of the browser stays free.
    ///
    /// `Some(true)` / `Some(false)` force the choice. Forcing `false` on a
    /// shared-window tab is the one genuinely broken combination — it yields
    /// a near-empty recording — so prefer leaving this `None` and putting the
    /// page in its own window when concurrency matters.
    ///
    /// The auto check costs one `Target.getTargets` and one
    /// `Browser.getWindowForTarget` per page target, once per recording.
    pub foreground:   Option<bool>,
    /// Post-processing encodings to produce, each written to `dir`. Requires
    /// `dir` to be set.
    pub encode:       Vec<Encoding>,
}

impl Default for RecordingOptions {
    fn default() -> Self {
        Self {
            dir:          None,
            bbox:         None,
            selectors:    Vec::new(),
            viewport:     None,
            scroll:       None,
            fps:          DEFAULT_FPS,
            max_duration: DEFAULT_MAX_DURATION,
            max_frames:   DEFAULT_MAX_FRAMES,
            format:       FrameFormat::Jpeg,
            quality:      DEFAULT_QUALITY,
            write_frames: false,
            foreground:   None,
            encode:       Vec::new(),
        }
    }
}

impl RecordingOptions {
    pub fn with_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = Some(dir.into());
        self
    }

    pub fn with_bbox(mut self, bbox: Bbox) -> Self {
        self.bbox = Some(bbox);
        self
    }

    /// Add one more region to record. Call repeatedly for several regions.
    pub fn with_selector(mut self, selector: SelectorEntry) -> Self {
        self.selectors.push(selector);
        self
    }

    pub fn with_selectors(mut self, selectors: impl IntoIterator<Item = SelectorEntry>) -> Self {
        self.selectors.extend(selectors);
        self
    }

    pub fn with_viewport(mut self, viewport: Viewport) -> Self {
        self.viewport = Some(viewport);
        self
    }

    pub fn with_scroll(mut self, scroll: ScrollTarget) -> Self {
        self.scroll = Some(scroll);
        self
    }

    pub fn with_fps(mut self, fps: u8) -> Self {
        self.fps = fps;
        self
    }

    pub fn with_max_duration(mut self, max_duration: Duration) -> Self {
        self.max_duration = max_duration;
        self
    }

    pub fn with_format(mut self, format: FrameFormat) -> Self {
        self.format = format;
        self
    }

    /// Force the foreground/capture-lock decision instead of letting it be
    /// detected. See [`RecordingOptions::foreground`].
    pub fn with_foreground(mut self, foreground: bool) -> Self {
        self.foreground = Some(foreground);
        self
    }

    pub fn with_encoding(mut self, encoding: Encoding) -> Self {
        self.encode.push(encoding);
        self
    }

    /// Reject option combinations that can't mean anything, before any
    /// browser state is touched.
    fn validate(&self) -> Result<()> {
        if self.bbox.is_some() && !self.selectors.is_empty() {
            return Err(VoidCrawlError::RecordingError(
                "RecordingOptions: `bbox` and `selectors` are mutually exclusive".into(),
            ));
        }
        if self.fps == 0 {
            return Err(VoidCrawlError::RecordingError(
                "RecordingOptions: `fps` must be at least 1".into(),
            ));
        }
        if self.max_duration.is_zero() {
            return Err(VoidCrawlError::RecordingError(
                "RecordingOptions: `max_duration` must be greater than zero".into(),
            ));
        }
        if self.max_frames == 0 {
            return Err(VoidCrawlError::RecordingError(
                "RecordingOptions: `max_frames` must be at least 1".into(),
            ));
        }
        if !self.encode.is_empty() && self.dir.is_none() {
            return Err(VoidCrawlError::RecordingError(
                "RecordingOptions: `encode` requires `dir` to be set".into(),
            ));
        }
        if self.write_frames && self.dir.is_none() {
            return Err(VoidCrawlError::RecordingError(
                "RecordingOptions: `write_frames` requires `dir` to be set".into(),
            ));
        }
        Ok(())
    }
}

/// One captured frame.
#[derive(Clone, Serialize)]
pub struct Frame {
    /// Position in the sequence, 0-based, after fps throttling.
    pub index:  usize,
    /// Real elapsed time from the start of the recording. Frames are *not*
    /// evenly spaced — encode against this, not against `index / fps`.
    pub offset: Duration,
    /// Encoded image bytes in the recording's [`FrameFormat`].
    #[serde(skip)]
    pub data:   Vec<u8>,
}

impl fmt::Debug for Frame {
    /// Hand-written so a frame doesn't dump tens of kilobytes of pixels into
    /// a log line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("index", &self.index)
            .field("offset", &self.offset)
            .field("bytes", &self.data.len())
            .finish()
    }
}

/// One recorded region: the whole viewport, an explicit `bbox`, or one
/// selector's resolved rectangle.
#[derive(Debug, Clone, Serialize)]
pub struct RecordedRegion {
    /// Human-readable name, derived from the selector (its `name`, else its
    /// `value`) or `"viewport"` / `"bbox"`. Also the on-disk subdirectory
    /// name when `write_frames` is set.
    pub label:   String,
    /// The rectangle this region was cropped to, or `None` for the full
    /// frame. Resolved once at recording start — see the module docs on
    /// drift.
    pub bbox:    Option<Bbox>,
    /// The frames for this region, cropped.
    pub frames:  Vec<Frame>,
    /// Encoded artifacts produced for this region, if any.
    pub outputs: Vec<PathBuf>,
}

/// The result of a recording.
#[derive(Debug, Clone, Serialize)]
pub struct Recording {
    /// One entry per requested region; exactly one (`"viewport"`) when
    /// neither `bbox` nor `selectors` was given.
    pub regions:            Vec<RecordedRegion>,
    pub format:             FrameFormat,
    /// Wall-clock span from screencast start to stop.
    pub duration:           Duration,
    /// Frames kept, per region (every region has the same count).
    pub frames_captured:    usize,
    /// Frames Chrome delivered that were discarded by the fps ceiling or the
    /// `max_frames` cap. A large number next to a small `frames_captured`
    /// means `fps` is the binding constraint.
    pub frames_dropped:     usize,
    /// The page's `devicePixelRatio` at capture time, for translating frame
    /// pixels back to CSS pixels.
    pub device_pixel_ratio: f64,
    /// Whether this recording pinned the tab to the foreground and held the
    /// browser's capture lock — the resolved value of
    /// [`RecordingOptions::foreground`], which is normally auto-detected.
    /// `false` means the recording ran concurrently with the rest of the
    /// browser.
    pub foregrounded:       bool,
}

impl Recording {
    /// The measured frame rate actually achieved, which is at most
    /// [`RecordingOptions::fps`] and usually below it on a mostly-static
    /// page. Use this rather than the requested fps when reporting.
    pub fn effective_fps(&self) -> f64 {
        let secs = self.duration.as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        #[expect(clippy::cast_precision_loss, reason = "frame counts are small")]
        let frames = self.frames_captured as f64;
        frames / secs
    }
}

/// A recording in flight, returned by [`Page::start_recording`] and consumed
/// by [`RecordingHandle::stop`].
///
/// Deliberately holds no reference to the page — `stop` takes one — so the
/// handle can be parked in a registry between two separate calls (an MCP
/// `session_record_start` / `session_record_stop` pair) while the page it
/// belongs to stays behind its own lock. Same shape as [`DownloadCapture`]
/// for the same reason.
///
/// If the handle is dropped without `stop`, the collector task still winds
/// down at `max_duration` and releases anything it held, but the frames are
/// lost.
///
/// [`DownloadCapture`]: crate::DownloadCapture
#[derive(Debug)]
pub struct RecordingHandle {
    collector:        JoinHandle<CollectedFrames>,
    stop_tx:          Option<oneshot::Sender<()>>,
    /// The CDP handle for the recorded tab, kept so `stop` can halt the
    /// screencast without needing the wrapping [`Page`] first. Cheap to
    /// clone: `chromiumoxide::Page` is an `Arc` internally.
    cdp:              CdpPage,
    /// `Some` unless [`RecordingOptions::foreground`] was explicitly cleared
    /// — see that field for when clearing it is safe.
    capture_guard:    Option<OwnedMutexGuard<()>>,
    started:          Instant,
    regions:          Vec<(String, Option<Bbox>)>,
    restore_scroll:   Option<(f64, f64)>,
    /// Three genuinely distinct states: `None` = no override was applied and
    /// nothing needs restoring; `Some(None)` = an override was applied over a
    /// page that had none, so clear it; `Some(Some(v))` = restore `v`.
    #[expect(clippy::option_option, reason = "outer = did we override, inner = what was there")]
    restore_viewport: Option<Option<Viewport>>,
    /// The resolved foreground decision, reported on the [`Recording`].
    foregrounded:     bool,
    opts:             RecordingOptions,
}

/// What the collector task hands back.
#[derive(Debug)]
struct CollectedFrames {
    frames:  Vec<RawFrame>,
    dropped: usize,
}

#[derive(Debug)]
struct RawFrame {
    offset:       Duration,
    data:         Vec<u8>,
    /// Width in CSS pixels of the surface this frame depicts, from the
    /// screencast metadata. Divided into the decoded image width, this gives
    /// the image-pixels-per-CSS-pixel scale needed to place a CSS-pixel crop
    /// rectangle — Chrome may downscale frames, so the ratio is not
    /// necessarily the devicePixelRatio.
    device_width: f64,
}

impl RecordingHandle {
    /// Stop the recording, restore the page's viewport and scroll position,
    /// and post-process the collected frames.
    ///
    /// `page` must be the page this recording was started on; passing a
    /// different one restores the wrong viewport and scroll position.
    pub async fn stop(mut self, page: &Page) -> Result<Recording> {
        let duration = self.started.elapsed();

        // Signal the collector first so it stops acking, then tell Chrome to
        // stop producing. Both are best-effort: a page that navigated or
        // crashed mid-recording should still yield whatever was collected.
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        let _ = self.cdp.execute(StopScreencastParams::default()).await;

        let collected = self
            .collector
            .await
            .map_err(|e| VoidCrawlError::RecordingError(format!("collector task: {e}")))?;

        // Restore page state before any CPU-bound post-processing, so the
        // tab is usable again as early as possible.
        if let Some((x, y)) = self.restore_scroll {
            let _ = page.evaluate_js(&format!("window.scrollTo({x}, {y})")).await;
        }
        if let Some(prev) = self.restore_viewport.take() {
            let _ = match prev {
                Some(v) => page.set_viewport(v).await,
                None => page.clear_viewport().await,
            };
        }
        let dpr = page
            .evaluate_js("window.devicePixelRatio")
            .await
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);

        // Released here rather than at end-of-scope so that, in the opt-in
        // `foreground` mode, other tabs can capture again as soon as this tab
        // is restored — without waiting on cropping and encoding.
        drop(self.capture_guard.take());

        let mut recording =
            build_regions(collected, &self.regions, &self.opts, duration, dpr, self.foregrounded)
                .await?;

        if let Some(dir) = self.opts.dir.clone() {
            write_artifacts(&mut recording, &dir, &self.opts).await?;
        }
        Ok(recording)
    }
}

impl Page {
    /// Record this page for [`RecordingOptions::max_duration`] and return the
    /// frames.
    ///
    /// The one-shot form, and the direct analogue of [`Page::screenshot`]:
    /// it starts a screencast, waits the full duration, and stops. Use
    /// [`Page::start_recording`] instead when you need to *drive* the page
    /// (click, type, navigate) while it records.
    ///
    /// ```no_run
    /// # async fn f(page: &void_crawl_core::Page) -> void_crawl_core::Result<()> {
    /// use std::time::Duration;
    ///
    /// use void_crawl_core::RecordingOptions;
    ///
    /// let rec =
    ///     page.record(RecordingOptions::default().with_max_duration(Duration::from_secs(5))).await?;
    /// println!("{} frames at {:.1} fps", rec.frames_captured, rec.effective_fps());
    /// # Ok(()) }
    /// ```
    pub async fn record(&self, opts: RecordingOptions) -> Result<Recording> {
        let duration = opts.max_duration;
        let handle = self.start_recording(opts).await?;
        sleep(duration).await;
        handle.stop(self).await
    }

    /// Begin recording and return a handle to stop it.
    ///
    /// Drive the page normally in between — clicks, typing, and navigation
    /// all keep recording.
    ///
    /// By default the tab is pinned to the foreground and holds the
    /// browser-wide capture lock until [`RecordingHandle::stop`], so sibling
    /// tabs can't capture meanwhile. See [`RecordingOptions::foreground`]
    /// for how to record concurrently instead.
    ///
    /// ```no_run
    /// # async fn f(page: &void_crawl_core::Page) -> void_crawl_core::Result<()> {
    /// use void_crawl_core::RecordingOptions;
    ///
    /// let rec = page.start_recording(RecordingOptions::default()).await?;
    /// page.click_by_role("button", "Play", 0, false).await?;
    /// let out = rec.stop(page).await?;
    /// # Ok(()) }
    /// ```
    pub async fn start_recording(&self, opts: RecordingOptions) -> Result<RecordingHandle> {
        opts.validate()?;

        // One-shot viewport override, snapshotted for exact restore — same
        // leak-proofing as `screenshot`, and doubly important here because a
        // recording lives long enough for another caller to notice.
        let restore_viewport = if let Some(ref viewport) = opts.viewport {
            let prev = self.current_viewport();
            self.set_viewport(viewport.clone()).await?;
            Some(prev)
        } else {
            None
        };

        // From here on, any failure has to undo the viewport override before
        // returning, so the work is factored into a closure-like block.
        let started = self.begin_screencast(&opts).await;
        match started {
            Ok((
                collector,
                stop_tx,
                capture_guard,
                regions,
                restore_scroll,
                started_at,
                foregrounded,
            )) => Ok(RecordingHandle {
                collector,
                stop_tx: Some(stop_tx),
                cdp: self.cdp().clone(),
                capture_guard,
                started: started_at,
                regions,
                restore_scroll,
                restore_viewport,
                foregrounded,
                opts,
            }),
            Err(e) => {
                if let Some(prev) = restore_viewport {
                    let _ = match prev {
                        Some(v) => self.set_viewport(v).await,
                        None => self.clear_viewport().await,
                    };
                }
                Err(e)
            }
        }
    }

    /// Scroll, resolve regions, take the capture lock, and start the CDP
    /// screencast plus its collector task.
    async fn begin_screencast(
        &self,
        opts: &RecordingOptions,
    ) -> Result<(
        JoinHandle<CollectedFrames>,
        oneshot::Sender<()>,
        Option<OwnedMutexGuard<()>>,
        Vec<(String, Option<Bbox>)>,
        Option<(f64, f64)>,
        Instant,
        bool,
    )> {
        let restore_scroll = match opts.scroll {
            Some(target) => {
                let prev = self.scroll_position().await?;
                self.scroll_to(target).await?;
                Some(prev)
            }
            None => None,
        };

        // Resolve every selector to a fixed rectangle *before* the screencast
        // starts, so a bad selector fails fast instead of after N seconds of
        // capture. Viewport-relative, matching the frames they'll crop.
        let regions = self.resolve_regions(opts).await?;

        // A tab sharing its window stops painting the instant a sibling takes
        // focus, so it must hold the foreground (and therefore the lock) for
        // the whole recording. A tab alone in its window can't be occluded
        // that way and needs neither. When the check itself fails, assume the
        // constraining case: a slow recording beats an empty one.
        let foreground = match opts.foreground {
            Some(explicit) => explicit,
            None => !self.alone_in_window().await.unwrap_or(false),
        };

        let capture_guard = if foreground {
            let guard = self.capture_lock().lock_owned().await;
            self.cdp()
                .bring_to_front()
                .await
                .map_err(|e| VoidCrawlError::RecordingError(format!("bring to front: {e}")))?;
            Some(guard)
        } else {
            None
        };

        let events = self
            .cdp()
            .event_listener::<EventScreencastFrame>()
            .await
            .map_err(|e| VoidCrawlError::RecordingError(format!("screencast listener: {e}")))?;

        let mut params = StartScreencastParams::builder().format(opts.format.as_cdp());
        if matches!(opts.format, FrameFormat::Jpeg) {
            params = params.quality(i64::from(opts.quality));
        }
        self.cdp()
            .execute(params.build())
            .await
            .map_err(|e| VoidCrawlError::RecordingError(format!("startScreencast: {e}")))?;

        let started_at = Instant::now();
        let (stop_tx, stop_rx) = oneshot::channel();
        let collector = spawn_collector(
            self.cdp().clone(),
            events,
            stop_rx,
            started_at,
            opts.fps,
            opts.max_frames,
            opts.max_duration,
        );

        Ok((collector, stop_tx, capture_guard, regions, restore_scroll, started_at, foreground))
    }

    /// Turn `bbox` / `selectors` / neither into the labeled crop rectangles
    /// the frames will be cut with.
    async fn resolve_regions(
        &self,
        opts: &RecordingOptions,
    ) -> Result<Vec<(String, Option<Bbox>)>> {
        if let Some(bbox) = opts.bbox {
            return Ok(vec![("bbox".to_string(), Some(bbox))]);
        }
        if opts.selectors.is_empty() {
            return Ok(vec![("viewport".to_string(), None)]);
        }

        let mut regions = Vec::with_capacity(opts.selectors.len());
        for (i, entry) in opts.selectors.iter().enumerate() {
            let label = region_label(entry, i);
            match self.resolve_selector(entry).await? {
                SelectorResolution::Resolved { bbox } => regions.push((label, Some(bbox))),
                SelectorResolution::Empty { reason } => {
                    return Err(VoidCrawlError::ElementNotVisible(format!(
                        "recording region {label:?}: {reason}"
                    )));
                }
                SelectorResolution::Ambiguous { reason, .. } => {
                    return Err(VoidCrawlError::AmbiguousSelector(format!(
                        "recording region {label:?}: {reason}"
                    )));
                }
            }
        }
        Ok(regions)
    }
}

/// A stable, filesystem-safe name for a region.
fn region_label(entry: &SelectorEntry, index: usize) -> String {
    let raw =
        entry.name.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(entry.value.as_str());
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        format!("region{index}")
    } else {
        // Keep labels short enough to stay well inside filename limits even
        // when a CSS selector is long.
        let short: String = trimmed.chars().take(48).collect();
        format!("{index}_{short}")
    }
}

/// Drain screencast events until stopped, throttled to `fps`.
///
/// Every frame is acked immediately whether or not it's kept: Chrome pauses
/// the screencast until the previous frame is acknowledged, so skipping an
/// ack for a throttled-away frame would stall the whole stream.
fn spawn_collector(
    cdp: CdpPage,
    mut events: impl Stream<Item = Arc<EventScreencastFrame>> + Unpin + Send + 'static,
    stop_rx: oneshot::Receiver<()>,
    started: Instant,
    fps: u8,
    max_frames: usize,
    max_duration: Duration,
) -> JoinHandle<CollectedFrames> {
    let min_gap = Duration::from_secs_f64(1.0 / f64::from(fps));
    tokio::spawn(async move {
        let mut frames: Vec<RawFrame> = Vec::new();
        let mut dropped = 0usize;
        let mut last_kept: Option<Instant> = None;
        let mut stop_rx = stop_rx;
        // The hard stop is enforced here too, not just by `record`, so a
        // handle that is never stopped still releases the browser.
        let deadline = sleep(max_duration);
        tokio::pin!(deadline);

        loop {
            let event = tokio::select! {
                biased;
                _ = &mut stop_rx => break,
                () = &mut deadline => break,
                event = events.next() => event,
            };
            let Some(event) = event else { break };

            let _ = cdp.execute(ScreencastFrameAckParams::new(event.session_id)).await;

            let now = Instant::now();
            if frames.len() >= max_frames {
                dropped += 1;
                continue;
            }
            if let Some(last) = last_kept
                && now.duration_since(last) < min_gap
            {
                dropped += 1;
                continue;
            }

            let encoded: &str = event.data.as_ref();
            match B64.decode(encoded) {
                Ok(data) => {
                    last_kept = Some(now);
                    frames.push(RawFrame {
                        offset: now.duration_since(started),
                        data,
                        device_width: event.metadata.device_width,
                    });
                }
                // A frame that doesn't decode is a lost frame, not a lost
                // recording.
                Err(_) => dropped += 1,
            }
        }

        CollectedFrames { frames, dropped }
    })
}

/// Cut each region's frames out of the shared raw frame sequence.
async fn build_regions(
    collected: CollectedFrames,
    regions: &[(String, Option<Bbox>)],
    opts: &RecordingOptions,
    duration: Duration,
    device_pixel_ratio: f64,
    foregrounded: bool,
) -> Result<Recording> {
    let frames_captured = collected.frames.len();
    let frames_dropped = collected.dropped;
    let regions_spec: Vec<(String, Option<Bbox>)> = regions.to_vec();
    let format = opts.format;
    let quality = opts.quality;

    // Decoding and re-encoding every frame for every region is CPU-bound and
    // can run to hundreds of megapixels; keep it off the async runtime's
    // worker threads.
    let built =
        spawn_blocking(move || crop_regions(&collected.frames, &regions_spec, format, quality))
            .await
            .map_err(|e| VoidCrawlError::RecordingError(format!("crop task: {e}")))??;

    Ok(Recording {
        regions: built,
        format,
        duration,
        frames_captured,
        frames_dropped,
        device_pixel_ratio,
        foregrounded,
    })
}

fn crop_regions(
    raw: &[RawFrame],
    regions: &[(String, Option<Bbox>)],
    format: FrameFormat,
    quality: u8,
) -> Result<Vec<RecordedRegion>> {
    let mut out = Vec::with_capacity(regions.len());
    for (label, bbox) in regions {
        let frames = match bbox {
            // No crop: hand the original bytes straight through, so an
            // uncropped recording never pays a decode.
            None => raw
                .iter()
                .enumerate()
                .map(|(index, f)| Frame { index, offset: f.offset, data: f.data.clone() })
                .collect(),
            Some(bbox) => {
                let mut frames = Vec::with_capacity(raw.len());
                for (index, f) in raw.iter().enumerate() {
                    let data = crop_frame(f, *bbox, format, quality)?;
                    frames.push(Frame { index, offset: f.offset, data });
                }
                frames
            }
        };
        out.push(RecordedRegion { label: label.clone(), bbox: *bbox, frames, outputs: Vec::new() });
    }
    Ok(out)
}

/// Crop one frame to a CSS-pixel rectangle.
///
/// The frame's pixel dimensions need not match its CSS dimensions — Chrome
/// scales screencast output — so the rectangle is scaled by the ratio of the
/// decoded width to the metadata's `deviceWidth` before cropping. A region
/// partly outside the frame is clamped rather than erroring: a recording
/// where an element scrolls half out of view should still produce frames.
fn crop_frame(raw: &RawFrame, bbox: Bbox, format: FrameFormat, quality: u8) -> Result<Vec<u8>> {
    let img = load_from_memory_with_format(&raw.data, format.as_image())
        .map_err(|e| VoidCrawlError::RecordingError(format!("decode frame: {e}")))?;

    let scale =
        if raw.device_width > 0.0 { f64::from(img.width()) / raw.device_width } else { 1.0 };
    let (img_w, img_h) = img.dimensions();

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "scaled pixel coordinates, clamped to the frame below"
    )]
    let (x, y, w, h) = (
        (f64::from(bbox.x) * scale).round() as u32,
        (f64::from(bbox.y) * scale).round() as u32,
        (f64::from(bbox.width) * scale).round() as u32,
        (f64::from(bbox.height) * scale).round() as u32,
    );
    let x = x.min(img_w.saturating_sub(1));
    let y = y.min(img_h.saturating_sub(1));
    let w = w.min(img_w - x).max(1);
    let h = h.min(img_h - y).max(1);

    let cropped = imageops::crop_imm(&img, x, y, w, h).to_image();
    encode_image(&DynamicImage::ImageRgba8(cropped), format, quality)
}

fn encode_image(img: &DynamicImage, format: FrameFormat, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    match format {
        FrameFormat::Jpeg => {
            // JPEG has no alpha channel; go through RGB8 explicitly rather
            // than letting the encoder reject an RGBA buffer.
            let rgb = img.to_rgb8();
            let mut encoder = JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
            encoder
                .encode_image(&rgb)
                .map_err(|e| VoidCrawlError::RecordingError(format!("encode jpeg: {e}")))?;
        }
        FrameFormat::Png => {
            img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
                .map_err(|e| VoidCrawlError::RecordingError(format!("encode png: {e}")))?;
        }
    }
    Ok(buf)
}

/// Write frames and/or encoded artifacts to `dir`.
async fn write_artifacts(
    recording: &mut Recording,
    dir: &Path,
    opts: &RecordingOptions,
) -> Result<()> {
    fs::create_dir_all(dir)
        .map_err(|e| VoidCrawlError::RecordingError(format!("create {}: {e}", dir.display())))?;

    for region in &mut recording.regions {
        if opts.write_frames {
            let region_dir = dir.join(&region.label);
            fs::create_dir_all(&region_dir).map_err(|e| {
                VoidCrawlError::RecordingError(format!("create {}: {e}", region_dir.display()))
            })?;
            for frame in &region.frames {
                let path =
                    region_dir.join(format!("{:05}.{}", frame.index, opts.format.extension()));
                fs::write(&path, &frame.data).map_err(|e| {
                    VoidCrawlError::RecordingError(format!("write {}: {e}", path.display()))
                })?;
            }
        }

        for encoding in &opts.encode {
            let path = dir.join(format!("{}.{}", region.label, encoding.extension()));
            encode_region(region, *encoding, &path, opts).await?;
            region.outputs.push(path);
        }
    }
    Ok(())
}

#[cfg_attr(
    not(any(feature = "encode-gif", feature = "encode-ffmpeg")),
    expect(
        unused_variables,
        clippy::unused_async,
        reason = "every encoder branch is feature-gated off in this build"
    )
)]
async fn encode_region(
    region: &RecordedRegion,
    encoding: Encoding,
    path: &Path,
    opts: &RecordingOptions,
) -> Result<()> {
    match encoding {
        Encoding::Gif => {
            #[cfg(feature = "encode-gif")]
            {
                let frames = region.frames.clone();
                let format = opts.format;
                let path = path.to_path_buf();
                spawn_blocking(move || encoders::gif(&frames, format, &path))
                    .await
                    .map_err(|e| VoidCrawlError::RecordingEncodeError(format!("gif task: {e}")))?
            }
            #[cfg(not(feature = "encode-gif"))]
            Err(VoidCrawlError::RecordingEncodeError(
                "GIF encoding requires the `encode-gif` cargo feature; the frames are still \
                 available on the returned Recording"
                    .into(),
            ))
        }
        Encoding::Mp4 | Encoding::WebM => {
            #[cfg(feature = "encode-ffmpeg")]
            {
                encoders::ffmpeg(region, encoding, path, opts).await
            }
            #[cfg(not(feature = "encode-ffmpeg"))]
            Err(VoidCrawlError::RecordingEncodeError(format!(
                "{} encoding requires the `encode-ffmpeg` cargo feature and an ffmpeg binary on \
                 PATH; the frames are still available on the returned Recording",
                encoding.extension()
            )))
        }
    }
}

#[cfg(any(feature = "encode-gif", feature = "encode-ffmpeg"))]
mod encoders;
