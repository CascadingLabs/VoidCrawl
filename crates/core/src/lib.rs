//! `void_crawl_core` — a clean async CDP wrapper built on chromiumoxide.
//!
//! This crate provides `BrowserSession` and `Page` as the primary API.

pub mod error;
pub mod page;
pub mod pool;
pub mod session;
pub mod stealth;

// Re-export CDP types for downstream crates (pyo3_bindings).
pub use chromiumoxide::cdp::browser_protocol::{
    input::{DispatchKeyEventType, DispatchMouseEventType, MouseButton},
    network::{Cookie, CookieParam, DeleteCookiesParams},
};
pub use error::{Result, VoidCrawlError};
pub use page::{Page, PageResponse};
pub use pool::{BrowserPool, PoolConfig, PooledTab};
pub use session::{BrowserMode, BrowserSession, BrowserSessionBuilder};
pub use stealth::StealthConfig;
