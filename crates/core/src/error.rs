//! Error types for `void_crawl_core`.

use chromiumoxide::error::CdpError;
use thiserror::Error;

/// All errors produced by this crate.
#[derive(Debug, Error)]
pub enum VoidCrawlError {
    #[error("browser launch failed: {0}")]
    LaunchFailed(String),

    #[error("browser connection failed: {0}")]
    ConnectionFailed(String),

    #[error("navigation failed: {0}")]
    NavigationFailed(String),

    #[error("navigation to {url:?} timed out waiting for {wait_phase} after {timeout_secs:.3}s")]
    NavigationTimeout {
        url:          String,
        wait_phase:   String,
        timeout_secs: f64,
        elapsed_secs: f64,
    },

    #[error("page error: {0}")]
    PageError(String),

    #[error("javascript evaluation failed: {0}")]
    JsEvalError(String),

    #[error("screenshot failed: {0}")]
    ScreenshotError(String),

    #[error("pdf generation failed: {0}")]
    PdfError(String),

    #[error("element not found: {0}")]
    ElementNotFound(String),

    #[error("frame not found: {0}")]
    FrameNotFound(String),

    #[error("ambiguous frame pattern: {0}")]
    AmbiguousFrame(String),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("timed out after {timeout_secs:.3}s waiting for responses matching {patterns:?}")]
    ResponseTimeout { patterns: Vec<String>, timeout_secs: f64 },

    #[error("response body error: {0}")]
    ResponseBody(String),

    #[error("browser closed")]
    BrowserClosed,

    #[error("chromium fetch failed: {0}")]
    FetchChromiumError(String),

    #[error(
        "profile {name:?} is already leased by another process{owner}",
        owner = owner_suffix(*pid, *acquired_at)
    )]
    ProfileBusy { name: String, pid: Option<u32>, acquired_at: Option<u64> },

    #[error("profile lease for {name:?} expired after {timeout_secs}s")]
    ProfileLeaseExpired { name: String, timeout_secs: u64 },

    #[error("Chrome refused profile {name:?} because its own lock exists at {lock_path}")]
    ChromeProfileBusy { name: String, lock_path: String },

    #[error("profile {name:?} not found (looked in {searched:?})")]
    ProfileNotFound { name: String, searched: Vec<String> },

    #[error("captcha detected: {kind}")]
    CaptchaDetected { kind: String },

    /// An anti-bot vendor is actively challenging the response (an active wall,
    /// not mere CDN presence). Typed so opt-in callers can route on the vendor.
    ///
    /// Deliberately **not** raised automatically on the `fetch` / `fetch_many`
    /// path — that path surfaces the verdict as a non-fatal annotation on
    /// `PageResponse` so a 403-with-usable-HTML stays a success and batch
    /// per-item isolation holds. Reserved for explicit detect/routing callers.
    #[error("anti-bot challenge by {vendor}")]
    AntibotChallenge { vendor: String },

    #[error("{0}")]
    Other(String),
}

use core::result;

fn owner_suffix(pid: Option<u32>, acquired_at: Option<u64>) -> String {
    match (pid, acquired_at) {
        (Some(pid), Some(at)) => format!(" (pid {pid}, acquired at unix {at})"),
        (Some(pid), None) => format!(" (pid {pid})"),
        _ => String::new(),
    }
}

/// Convenience alias.
pub type Result<T> = result::Result<T, VoidCrawlError>;

impl From<CdpError> for VoidCrawlError {
    fn from(e: CdpError) -> Self {
        Self::Other(e.to_string())
    }
}
