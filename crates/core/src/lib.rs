//! `void_crawl_core` — a clean async CDP wrapper built on chromiumoxide.
//!
//! This crate provides `BrowserSession` and `Page` as the primary API.

pub mod antibot;
pub mod ax;
pub mod captcha;
pub mod challenge;
pub mod cookie_jar;
pub mod error;
pub mod input;
pub mod interrupt;
mod lease;
pub mod managed_profile;
pub mod page;
pub mod pool;
pub mod profile;
pub mod recording;
pub mod response;
pub mod scanner;
pub mod selector;
pub mod session;
pub mod stealth;
pub mod viewport;

// Re-export CDP types for downstream crates (pyo3_bindings).
pub use antibot::{AntibotVerdict, Evidence as AntibotEvidence, classify as classify_antibot};
pub use captcha::{
    CaptchaInfo, CaptchaKind, WidgetRect, capture_captcha, detect_captcha, inject_captcha_token,
};
pub use challenge::{
    AttachCoordinates, ChallengeSnapshot, ChallengeStatus, DomCaptchaSnapshot, ResolutionOutcome,
    ResolutionRequest, ResolverType, captcha_is_active,
};
pub use chromiumoxide::{
    CdpMode,
    cdp::browser_protocol::{
        input::{DispatchKeyEventType, DispatchMouseEventType, MouseButton},
        network::{Cookie, CookieParam, DeleteCookiesParams},
    },
};
pub use cookie_jar::{CookieLease, CookieProvenance, LeaseScope, fork_scoped};
pub use error::{Result, VoidCrawlError};
pub use interrupt::{InterruptInfo, InterruptRegistry, InterruptRequest, InterruptState};
pub use managed_profile::{
    MAX_PROFILE_SPLIT_COPIES, ManagedProfile, ManagedProfileDescription, ManagedProfileLease,
    ManagedProfileSnapshot, ProfilePool, ProfileRegistry, ProfileStatus, ResolvedProfilePool,
    default_profile_root,
};
pub use page::{
    Bbox, DownloadCapture, DownloadOutcome, Page, PageResponse, ScreenshotOptions, ScreenshotOutput,
};
pub use pool::{BrowserPool, PoolConfig, PooledTab};
pub use profile::{
    ProfileHandle, ProfileInfo, acquire_profile, acquire_profile_in, chrome_user_data_dirs,
    list_profiles, release_profile, resolve_profile,
};
pub use recording::{
    Encoding, Frame, FrameFormat, MaskRegion, MaskReport, MaskSpec, RecordedRegion, Recording,
    RecordingHandle, RecordingOptions,
};
pub use response::{
    CapturedResponse, DEFAULT_MAX_RESPONSE_BYTES, DEFAULT_MAX_TOTAL_RESPONSE_BYTES,
    ResponseBodyState, ResponseCapture, ResponseCaptureLimits,
};
pub use scanner::{DEFAULT_MAX_BYTES, ScanConfig, ScanReport, Verdict, scan_bytes, scan_path};
pub use selector::{SelectorEntry, SelectorKind, SelectorResolution};
pub use session::{BrowserMode, BrowserSession, BrowserSessionBuilder};
pub use stealth::StealthConfig;
pub use viewport::{ScrollTarget, Viewport, all_presets, preset as viewport_preset, preset_names};
