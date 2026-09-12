//! `VoidCrawlError` → `rmcp::ErrorData` mapping.

use rmcp::ErrorData;
use serde_json::{Map, Value};
use void_crawl_core::{VoidCrawlError, VoidCrawlErrorCategory, VoidCrawlErrorSummary};

/// Map a core error into the MCP wire error.
///
/// The JSON-RPC status is derived solely from the stable error category. A
/// small set of historic `data.exception` tags remains for client
/// compatibility, but diagnostic fields are never copied to the wire.
#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err is used directly by Result::map_err call sites"
)]
pub fn map_err(err: VoidCrawlError) -> ErrorData {
    let summary = err.safe_summary();
    let message = summary.message;
    let mut data = summary_data(summary);

    match &err {
        VoidCrawlError::CaptchaDetected { kind } => {
            tag(&mut data, "CaptchaDetected");
            data.insert("kind".into(), Value::String(challenge_tag(kind).into()));
        }
        VoidCrawlError::AntibotChallenge { vendor } => {
            tag(&mut data, "AntibotChallenge");
            data.insert("vendor".into(), Value::String(challenge_tag(vendor).into()));
        }
        VoidCrawlError::ProfileBusy { .. } => tag(&mut data, "ProfileBusy"),
        VoidCrawlError::ProfileLeaseExpired { timeout_secs, .. } => {
            tag(&mut data, "ProfileLeaseExpired");
            data.insert("timeout_secs".into(), Value::from(*timeout_secs));
        }
        VoidCrawlError::ProfileNotFound { .. } => tag(&mut data, "ProfileNotFound"),
        VoidCrawlError::SessionInterrupted { .. } => tag(&mut data, "SessionInterrupted"),
        VoidCrawlError::InterruptExpired { .. } => tag(&mut data, "InterruptExpired"),
        VoidCrawlError::InterruptTerminal { state, .. } => {
            tag(&mut data, "InterruptTerminal");
            data.insert("state".into(), Value::String(interrupt_state_tag(state).into()));
        }
        VoidCrawlError::InterruptNotFound { .. } => tag(&mut data, "InterruptNotFound"),
        _ => {}
    }

    let data = Some(Value::Object(data));
    match summary.category {
        VoidCrawlErrorCategory::InvalidInput | VoidCrawlErrorCategory::Unsupported => {
            ErrorData::invalid_params(message, data)
        }
        VoidCrawlErrorCategory::Timeout
        | VoidCrawlErrorCategory::Interrupted
        | VoidCrawlErrorCategory::Unavailable
        | VoidCrawlErrorCategory::ProviderFailure
        | VoidCrawlErrorCategory::Internal => ErrorData::internal_error(message, data),
        // `VoidCrawlErrorCategory` is non-exhaustive. Unknown future categories
        // fail closed rather than accidentally becoming client errors.
        _ => ErrorData::internal_error(message, data),
    }
}

fn summary_data(summary: VoidCrawlErrorSummary) -> Map<String, Value> {
    let mut data = Map::new();
    data.insert("code".into(), Value::String(summary.code.as_str().into()));
    data.insert("category".into(), Value::String(summary.category.as_str().into()));
    data
}

fn tag(data: &mut Map<String, Value>, exception: &'static str) {
    data.insert("exception".into(), Value::String(exception.into()));
}

fn challenge_tag(value: &str) -> &'static str {
    match value.to_ascii_lowercase().as_str() {
        "recaptcha" => "recaptcha",
        "hcaptcha" => "hcaptcha",
        "turnstile" => "turnstile",
        "cloudflare" | "cloudflare_challenge" => "cloudflare_challenge",
        "datadome" => "datadome",
        _ => "unknown",
    }
}

fn interrupt_state_tag(value: &str) -> &'static str {
    match value {
        "active" => "active",
        "resumed" => "resumed",
        "released" => "released",
        "expired" => "expired",
        _ => "unknown",
    }
}
