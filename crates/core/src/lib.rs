//! `void_crawl_core` — a clean async CDP wrapper built on chromiumoxide.
//!
//! This crate provides `BrowserSession` and `Page` as the primary API.

pub mod ax;
pub mod captcha;
pub mod error;
pub mod page;
pub mod pool;
pub mod profile;
pub mod session;
pub mod stealth;

// Re-export CDP types for downstream crates (pyo3_bindings).
pub use captcha::{
    CaptchaInfo, CaptchaKind, WidgetRect, capture_captcha, detect_captcha, inject_captcha_token,
};
pub use chromiumoxide::cdp::browser_protocol::{
    input::{DispatchKeyEventType, DispatchMouseEventType, MouseButton},
    network::{Cookie, CookieParam, DeleteCookiesParams},
};
pub use error::{Result, VoidCrawlError};
pub use page::{Bbox, Page, PageResponse, ScreenshotOptions, ScreenshotOutput};
pub use pool::{BrowserPool, PoolConfig, PooledTab};
pub use profile::{
    ProfileHandle, ProfileInfo, acquire_profile, acquire_profile_in, chrome_user_data_dirs,
    list_profiles, release_profile, resolve_profile,
};
pub use session::{BrowserMode, BrowserSession, BrowserSessionBuilder};
pub use stealth::StealthConfig;
