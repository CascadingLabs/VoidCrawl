//! Error types for `void_crawl_core`.

use std::fmt;

use chromiumoxide::error::CdpError;
use serde::Serialize;
use thiserror::Error;

/// All errors produced by this crate.
#[derive(Error)]
pub enum VoidCrawlError {
    #[error("invalid input for {operation}")]
    InvalidInput { operation: &'static str, reason: &'static str },

    #[error("browser launch failed")]
    LaunchFailed(String),

    #[error("browser connection failed")]
    ConnectionFailed(String),

    #[error("navigation failed")]
    NavigationFailed(String),

    #[error("navigation timed out")]
    NavigationTimeout {
        url:          String,
        wait_phase:   String,
        timeout_secs: f64,
        elapsed_secs: f64,
    },

    #[error("page operation failed")]
    PageError(String),

    #[error("JavaScript evaluation failed")]
    JsEvalError(String),

    #[error("screenshot capture failed")]
    ScreenshotError(String),

    #[error("PDF generation failed")]
    PdfError(String),

    /// A screencast recording failed to start, collect, or post-process.
    /// Distinct from [`VoidCrawlError::ScreenshotError`] because a recording
    /// spans time: it can fail after frames have already been collected.
    #[error("recording failed")]
    RecordingError(String),

    /// Frames were collected but encoding them to a video/animation failed —
    /// e.g. the `encode-ffmpeg` feature is on but no `ffmpeg` binary is on
    /// PATH. The frames themselves survive on the returned [`Recording`].
    #[error("recording encoding failed")]
    RecordingEncodeError(String),

    #[error("target element was not found")]
    ElementNotFound(String),

    #[error("target frame was not found")]
    FrameNotFound(String),

    #[error("target frame was ambiguous")]
    AmbiguousFrame(String),

    /// A `screenshot(selector: ...)` resolution came back
    /// [`TargetResolution::Empty`](crate::selector::TargetResolution::Empty)
    /// — nothing usable to crop. Distinct from `ElementNotFound`: the
    /// selector may have matched, but hidden/zero-area/detached.
    #[error("target element was not visible")]
    ElementNotVisible(String),

    /// A `screenshot(selector: ...)` resolution came back
    /// [`TargetResolution::Ambiguous`](crate::selector::TargetResolution::Ambiguous)
    /// — multiple visible candidates, no `nth` to disambiguate.
    #[error("target kind does not support visual geometry")]
    UnsupportedVisualTarget,

    #[error("target selector was ambiguous")]
    AmbiguousSelector(String),

    #[error("operation timed out")]
    Timeout(String),

    #[error("response observation timed out")]
    ResponseTimeout { patterns: Vec<String>, timeout_secs: f64 },

    #[error("response body processing failed")]
    ResponseBody(String),

    #[error("browser closed")]
    BrowserClosed,

    #[error("interrupt request was invalid")]
    InvalidInterruptRequest(String),

    #[error("session is interrupted")]
    SessionInterrupted { interrupt_id: String },

    #[error("an interrupt is already active")]
    InterruptAlreadyActive { target_id: String },

    #[error("page does not belong to this browser session")]
    InterruptPageNotOwned,

    #[error("interrupt was not found")]
    InterruptNotFound { interrupt_id: String },

    #[error("interrupt expired")]
    InterruptExpired { interrupt_id: String },

    #[error("interrupt is already terminal")]
    InterruptTerminal { interrupt_id: String, state: String },

    #[error("Chromium acquisition failed")]
    FetchChromiumError(String),

    #[error("browser profile is busy")]
    ProfileBusy { name: String, pid: Option<u32>, acquired_at: Option<u64> },

    #[error("browser profile lease expired")]
    ProfileLeaseExpired { name: String, timeout_secs: u64 },

    #[error("Chrome profile is already in use")]
    ChromeProfileBusy { name: String, lock_path: String },

    #[error("browser profile was not found")]
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

    // Compatibility escape hatch. New public operations should prefer typed
    // variants; facades and logs must use `safe_summary()` for this legacy
    // diagnostic-bearing case.
    #[error("{0}")]
    Other(String),
}

use core::result;

impl fmt::Debug for VoidCrawlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VoidCrawlError")
            .field("code", &self.code())
            .field("category", &self.category())
            .finish_non_exhaustive()
    }
}

/// Stable broad cause for a VoidCrawl failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum VoidCrawlErrorCategory {
    InvalidInput,
    Unsupported,
    Timeout,
    Interrupted,
    Unavailable,
    ProviderFailure,
    Internal,
}

impl VoidCrawlErrorCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::Unsupported => "unsupported",
            Self::Timeout => "timeout",
            Self::Interrupted => "interrupted",
            Self::Unavailable => "unavailable",
            Self::ProviderFailure => "provider_failure",
            Self::Internal => "internal",
        }
    }
}

/// Stable machine-readable identity for one VoidCrawl failure condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct VoidCrawlErrorCode(&'static str);

impl VoidCrawlErrorCode {
    const fn new(value: &'static str) -> Self {
        Self(value)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for VoidCrawlErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// Serializable, secret-safe error information for logs and adapters.
///
/// Raw provider diagnostics remain available by matching `VoidCrawlError`
/// locally. They are deliberately excluded from this transport-safe summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VoidCrawlErrorSummary {
    pub code:     VoidCrawlErrorCode,
    pub category: VoidCrawlErrorCategory,
    pub message:  &'static str,
}

impl VoidCrawlError {
    pub const fn code(&self) -> VoidCrawlErrorCode {
        let code = match self {
            Self::InvalidInput { .. } => "voidcrawl.input.invalid",
            Self::LaunchFailed(_) => "voidcrawl.browser.launch_failed",
            Self::ConnectionFailed(_) => "voidcrawl.browser.connection_failed",
            Self::NavigationFailed(_) => "voidcrawl.navigation.failed",
            Self::NavigationTimeout { .. } => "voidcrawl.navigation.timeout",
            Self::PageError(_) => "voidcrawl.page.failed",
            Self::JsEvalError(_) => "voidcrawl.javascript.evaluation_failed",
            Self::ScreenshotError(_) => "voidcrawl.visual.screenshot_failed",
            Self::PdfError(_) => "voidcrawl.document.pdf_failed",
            Self::RecordingError(_) => "voidcrawl.visual.recording_failed",
            Self::RecordingEncodeError(_) => "voidcrawl.visual.recording_encode_failed",
            Self::ElementNotFound(_) => "voidcrawl.target.element_not_found",
            Self::FrameNotFound(_) => "voidcrawl.target.frame_not_found",
            Self::AmbiguousFrame(_) => "voidcrawl.target.frame_ambiguous",
            Self::ElementNotVisible(_) => "voidcrawl.target.element_not_visible",
            Self::UnsupportedVisualTarget => "voidcrawl.target.visual_geometry_unsupported",
            Self::AmbiguousSelector(_) => "voidcrawl.target.selector_ambiguous",
            Self::Timeout(_) => "voidcrawl.operation.timeout",
            Self::ResponseTimeout { .. } => "voidcrawl.network.response_timeout",
            Self::ResponseBody(_) => "voidcrawl.network.response_body_failed",
            Self::BrowserClosed => "voidcrawl.browser.closed",
            Self::InvalidInterruptRequest(_) => "voidcrawl.interrupt.request_invalid",
            Self::SessionInterrupted { .. } => "voidcrawl.interrupt.active",
            Self::InterruptAlreadyActive { .. } => "voidcrawl.interrupt.already_active",
            Self::InterruptPageNotOwned => "voidcrawl.interrupt.page_not_owned",
            Self::InterruptNotFound { .. } => "voidcrawl.interrupt.not_found",
            Self::InterruptExpired { .. } => "voidcrawl.interrupt.expired",
            Self::InterruptTerminal { .. } => "voidcrawl.interrupt.already_terminal",
            Self::FetchChromiumError(_) => "voidcrawl.browser.fetch_chromium_failed",
            Self::ProfileBusy { .. } => "voidcrawl.profile.busy",
            Self::ProfileLeaseExpired { .. } => "voidcrawl.profile.lease_expired",
            Self::ChromeProfileBusy { .. } => "voidcrawl.profile.chrome_busy",
            Self::ProfileNotFound { .. } => "voidcrawl.profile.not_found",
            Self::CaptchaDetected { .. } => "voidcrawl.challenge.captcha_detected",
            Self::AntibotChallenge { .. } => "voidcrawl.challenge.antibot_detected",
            Self::Other(_) => "voidcrawl.internal.other",
        };
        VoidCrawlErrorCode::new(code)
    }

    pub const fn category(&self) -> VoidCrawlErrorCategory {
        match self {
            Self::InvalidInput { .. }
            | Self::AmbiguousFrame(_)
            | Self::AmbiguousSelector(_)
            | Self::InvalidInterruptRequest(_)
            | Self::InterruptAlreadyActive { .. }
            | Self::InterruptPageNotOwned
            | Self::InterruptNotFound { .. }
            | Self::InterruptTerminal { .. } => VoidCrawlErrorCategory::InvalidInput,
            Self::RecordingEncodeError(_) | Self::UnsupportedVisualTarget => {
                VoidCrawlErrorCategory::Unsupported
            }
            Self::NavigationTimeout { .. }
            | Self::Timeout(_)
            | Self::ResponseTimeout { .. }
            | Self::ProfileLeaseExpired { .. } => VoidCrawlErrorCategory::Timeout,
            Self::SessionInterrupted { .. }
            | Self::InterruptExpired { .. }
            | Self::CaptchaDetected { .. }
            | Self::AntibotChallenge { .. } => VoidCrawlErrorCategory::Interrupted,
            Self::ElementNotFound(_)
            | Self::FrameNotFound(_)
            | Self::ElementNotVisible(_)
            | Self::ProfileBusy { .. }
            | Self::ChromeProfileBusy { .. }
            | Self::ProfileNotFound { .. } => VoidCrawlErrorCategory::Unavailable,
            Self::LaunchFailed(_)
            | Self::ConnectionFailed(_)
            | Self::NavigationFailed(_)
            | Self::PageError(_)
            | Self::JsEvalError(_)
            | Self::ScreenshotError(_)
            | Self::PdfError(_)
            | Self::RecordingError(_)
            | Self::ResponseBody(_)
            | Self::BrowserClosed
            | Self::FetchChromiumError(_) => VoidCrawlErrorCategory::ProviderFailure,
            Self::Other(_) => VoidCrawlErrorCategory::Internal,
        }
    }

    pub const fn safe_message(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "invalid input",
            Self::LaunchFailed(_) => "browser launch failed",
            Self::ConnectionFailed(_) => "browser connection failed",
            Self::NavigationFailed(_) => "navigation failed",
            Self::NavigationTimeout { .. } => "navigation timed out",
            Self::PageError(_) => "page operation failed",
            Self::JsEvalError(_) => "JavaScript evaluation failed",
            Self::ScreenshotError(_) => "screenshot capture failed",
            Self::PdfError(_) => "PDF generation failed",
            Self::RecordingError(_) => "recording failed",
            Self::RecordingEncodeError(_) => "recording encoding failed",
            Self::ElementNotFound(_) => "target element was not found",
            Self::FrameNotFound(_) => "target frame was not found",
            Self::AmbiguousFrame(_) => "target frame was ambiguous",
            Self::ElementNotVisible(_) => "target element was not visible",
            Self::UnsupportedVisualTarget => "target kind does not support visual geometry",
            Self::AmbiguousSelector(_) => "target selector was ambiguous",
            Self::Timeout(_) => "operation timed out",
            Self::ResponseTimeout { .. } => "response observation timed out",
            Self::ResponseBody(_) => "response body processing failed",
            Self::BrowserClosed => "browser closed",
            Self::InvalidInterruptRequest(_) => "interrupt request was invalid",
            Self::SessionInterrupted { .. } => "session is interrupted",
            Self::InterruptAlreadyActive { .. } => "an interrupt is already active",
            Self::InterruptPageNotOwned => "page does not belong to this browser session",
            Self::InterruptNotFound { .. } => "interrupt was not found",
            Self::InterruptExpired { .. } => "interrupt expired",
            Self::InterruptTerminal { .. } => "interrupt is already terminal",
            Self::FetchChromiumError(_) => "Chromium acquisition failed",
            Self::ProfileBusy { .. } => "browser profile is busy",
            Self::ProfileLeaseExpired { .. } => "browser profile lease expired",
            Self::ChromeProfileBusy { .. } => "Chrome profile is already in use",
            Self::ProfileNotFound { .. } => "browser profile was not found",
            Self::CaptchaDetected { .. } => "captcha challenge detected",
            Self::AntibotChallenge { .. } => "anti-bot challenge detected",
            Self::Other(_) => "internal VoidCrawl failure",
        }
    }

    pub const fn safe_summary(&self) -> VoidCrawlErrorSummary {
        VoidCrawlErrorSummary {
            code:     self.code(),
            category: self.category(),
            message:  self.safe_message(),
        }
    }
}

/// Convenience alias.
pub type Result<T> = result::Result<T, VoidCrawlError>;

impl From<CdpError> for VoidCrawlError {
    fn from(e: CdpError) -> Self {
        Self::Other(e.to_string())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn safe_summaries_have_stable_codes_and_categories() {
        let timeout = VoidCrawlError::NavigationTimeout {
            url:          "https://example.test/?token=secret".into(),
            wait_phase:   "networkidle".into(),
            timeout_secs: 1.0,
            elapsed_secs: 1.0,
        };
        assert_eq!(timeout.code().as_str(), "voidcrawl.navigation.timeout");
        assert_eq!(timeout.category(), VoidCrawlErrorCategory::Timeout);

        let interrupted = VoidCrawlError::SessionInterrupted { interrupt_id: "secret-id".into() };
        assert_eq!(interrupted.code().as_str(), "voidcrawl.interrupt.active");
        assert_eq!(interrupted.category(), VoidCrawlErrorCategory::Interrupted);

        let unavailable = VoidCrawlError::ProfileNotFound {
            name:     "private-profile".into(),
            searched: vec!["/home/private/profile".into()],
        };
        assert_eq!(unavailable.category(), VoidCrawlErrorCategory::Unavailable);
    }

    #[test]
    fn default_display_excludes_raw_diagnostics() {
        for error in [
            VoidCrawlError::LaunchFailed("/home/private/chrome --token secret".into()),
            VoidCrawlError::NavigationFailed("https://example.test/?access_token=secret".into()),
            VoidCrawlError::ProfileNotFound {
                name:     "private-profile".into(),
                searched: vec!["/home/private/profile".into()],
            },
        ] {
            let displayed = error.to_string();
            let debugged = format!("{error:?}");
            for forbidden in ["/home/private", "access_token", "secret", "private-profile"] {
                assert!(!displayed.contains(forbidden), "display leaked {forbidden}");
                assert!(!debugged.contains(forbidden), "debug leaked {forbidden}");
            }
        }
    }

    #[test]
    fn serialized_safe_summary_excludes_raw_diagnostics() {
        for error in [
            VoidCrawlError::LaunchFailed("/home/private/chrome --token secret".into()),
            VoidCrawlError::NavigationFailed("https://example.test/?access_token=secret".into()),
            VoidCrawlError::ResponseBody("response body contains secret".into()),
            VoidCrawlError::Other("raw internal secret".into()),
        ] {
            let serialized =
                serde_json::to_string(&error.safe_summary()).expect("serialize safe summary");
            assert!(serialized.contains("voidcrawl."));
            for forbidden in ["/home/private", "access_token", "secret", "contains secret"] {
                assert!(!serialized.contains(forbidden), "safe summary leaked {forbidden}");
            }
        }
    }
}
