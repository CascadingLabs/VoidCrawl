//! `VoidCrawlError` → `rmcp::ErrorData` mapping.

use rmcp::ErrorData;
use serde_json::{Map, Value, json};

fn obj(m: Map<String, Value>) -> Value {
    Value::Object(m)
}
use void_crawl_core::VoidCrawlError;

/// Map a core error into the MCP wire error. User-caused errors
/// (bad URL, bad selector, bad JS) surface as `invalid_params`;
/// everything else surfaces as `internal_error`. Typed exceptions
/// (captcha, profile failures) carry a structured `data` payload so
/// clients can dispatch on `data.exception`.
pub fn map_err(err: VoidCrawlError) -> ErrorData {
    match err {
        VoidCrawlError::ElementNotFound(s)
        | VoidCrawlError::NavigationFailed(s)
        | VoidCrawlError::JsEvalError(s) => ErrorData::invalid_params(s, None),
        VoidCrawlError::Timeout(s) => ErrorData::internal_error(format!("timeout: {s}"), None),
        VoidCrawlError::BrowserClosed => ErrorData::internal_error("browser closed", None),
        VoidCrawlError::CaptchaDetected { ref kind } => {
            let data = tagged("CaptchaDetected", json!({ "kind": kind }));
            ErrorData::internal_error(err.to_string(), Some(obj(data)))
        }
        VoidCrawlError::AntibotChallenge { ref vendor } => {
            let data = tagged("AntibotChallenge", json!({ "vendor": vendor }));
            ErrorData::internal_error(err.to_string(), Some(obj(data)))
        }
        VoidCrawlError::ProfileBusy { ref name } => {
            let data = tagged("ProfileBusy", json!({ "name": name }));
            ErrorData::internal_error(err.to_string(), Some(obj(data)))
        }
        VoidCrawlError::ProfileLeaseExpired { ref name, timeout_secs } => {
            let data = tagged(
                "ProfileLeaseExpired",
                json!({ "name": name, "timeout_secs": timeout_secs }),
            );
            ErrorData::internal_error(err.to_string(), Some(obj(data)))
        }
        VoidCrawlError::ProfileNotFound { ref name, ref searched } => {
            let data = tagged("ProfileNotFound", json!({ "name": name, "searched": searched }));
            ErrorData::invalid_params(err.to_string(), Some(obj(data)))
        }
        other => ErrorData::internal_error(other.to_string(), None),
    }
}

fn tagged(exception: &str, extra: Value) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("exception".into(), Value::String(exception.into()));
    if let Value::Object(obj) = extra {
        for (k, v) in obj {
            m.insert(k, v);
        }
    }
    m
}
