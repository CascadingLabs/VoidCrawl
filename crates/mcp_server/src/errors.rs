//! `VoidCrawlError` → `rmcp::ErrorData` mapping.

use rmcp::ErrorData;
use serde_json::{Map, Value, json};

fn obj(m: Map<String, Value>) -> Value {
    Value::Object(m)
}
use void_crawl_core::{VoidCrawlError, VoidCrawlErrorSummary};

/// Map a core error into the MCP wire error. User-caused errors
/// (bad URL, bad selector, bad JS) surface as `invalid_params`;
/// everything else surfaces as `internal_error`. Typed exceptions
/// (captcha, profile failures) carry a structured `data` payload so
/// clients can dispatch on `data.exception`.
#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err is used directly by Result::map_err call sites"
)]
pub fn map_err(err: VoidCrawlError) -> ErrorData {
    let summary = err.safe_summary();
    let message = summary.message;
    match err {
        VoidCrawlError::ElementNotFound(_)
        | VoidCrawlError::FrameNotFound(_)
        | VoidCrawlError::AmbiguousFrame(_)
        | VoidCrawlError::NavigationFailed(_)
        | VoidCrawlError::JsEvalError(_)
        | VoidCrawlError::ElementNotVisible(_)
        | VoidCrawlError::UnsupportedVisualTarget
        | VoidCrawlError::AmbiguousSelector(_) => {
            ErrorData::invalid_params(message, Some(obj(summary_data(summary))))
        }
        VoidCrawlError::Timeout(_) | VoidCrawlError::BrowserClosed => {
            ErrorData::internal_error(message, Some(obj(summary_data(summary))))
        }
        VoidCrawlError::CaptchaDetected { ref kind } => {
            let data = tagged(summary, "CaptchaDetected", json!({ "kind": kind }));
            ErrorData::internal_error(message, Some(obj(data)))
        }
        VoidCrawlError::AntibotChallenge { ref vendor } => {
            let data = tagged(summary, "AntibotChallenge", json!({ "vendor": vendor }));
            ErrorData::internal_error(message, Some(obj(data)))
        }
        VoidCrawlError::ProfileBusy { ref name, pid, acquired_at } => {
            let data = tagged(
                summary,
                "ProfileBusy",
                json!({ "name": name, "pid": pid, "acquired_at": acquired_at }),
            );
            ErrorData::internal_error(message, Some(obj(data)))
        }
        VoidCrawlError::ProfileLeaseExpired { ref name, timeout_secs } => {
            let data = tagged(
                summary,
                "ProfileLeaseExpired",
                json!({ "name": name, "timeout_secs": timeout_secs }),
            );
            ErrorData::internal_error(message, Some(obj(data)))
        }
        VoidCrawlError::ProfileNotFound { .. } => {
            let data = tagged(summary, "ProfileNotFound", json!({}));
            ErrorData::invalid_params(message, Some(obj(data)))
        }
        VoidCrawlError::SessionInterrupted { ref interrupt_id } => {
            let data =
                tagged(summary, "SessionInterrupted", json!({ "interrupt_id": interrupt_id }));
            ErrorData::internal_error(message, Some(obj(data)))
        }
        VoidCrawlError::InterruptExpired { ref interrupt_id } => {
            let data = tagged(summary, "InterruptExpired", json!({ "interrupt_id": interrupt_id }));
            ErrorData::internal_error(message, Some(obj(data)))
        }
        VoidCrawlError::InterruptTerminal { ref interrupt_id, ref state } => {
            let data = tagged(
                summary,
                "InterruptTerminal",
                json!({ "interrupt_id": interrupt_id, "state": state }),
            );
            ErrorData::internal_error(message, Some(obj(data)))
        }
        VoidCrawlError::InterruptNotFound { ref interrupt_id } => {
            let data =
                tagged(summary, "InterruptNotFound", json!({ "interrupt_id": interrupt_id }));
            ErrorData::invalid_params(message, Some(obj(data)))
        }
        _ => ErrorData::internal_error(message, Some(obj(summary_data(summary)))),
    }
}

fn summary_data(summary: VoidCrawlErrorSummary) -> Map<String, Value> {
    let mut data = Map::new();
    data.insert("code".into(), Value::String(summary.code.as_str().into()));
    data.insert("category".into(), Value::String(summary.category.as_str().into()));
    data
}

fn tagged(summary: VoidCrawlErrorSummary, exception: &str, extra: Value) -> Map<String, Value> {
    let mut data = summary_data(summary);
    data.insert("exception".into(), Value::String(exception.into()));
    if let Value::Object(extra) = extra {
        data.extend(extra);
    }
    data
}
