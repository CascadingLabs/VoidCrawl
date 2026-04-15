//! `VoidCrawlError` → `rmcp::ErrorData` mapping.

use rmcp::ErrorData;
use void_crawl_core::VoidCrawlError;

/// Map a core error into the MCP wire error. User-caused errors
/// (bad URL, bad selector, bad JS) surface as `invalid_params`;
/// everything else surfaces as `internal_error`.
pub fn map_err(err: VoidCrawlError) -> ErrorData {
    match err {
        VoidCrawlError::ElementNotFound(s)
        | VoidCrawlError::NavigationFailed(s)
        | VoidCrawlError::JsEvalError(s) => ErrorData::invalid_params(s, None),
        VoidCrawlError::Timeout(s) => ErrorData::internal_error(format!("timeout: {s}"), None),
        VoidCrawlError::BrowserClosed => ErrorData::internal_error("browser closed", None),
        other => ErrorData::internal_error(other.to_string(), None),
    }
}
