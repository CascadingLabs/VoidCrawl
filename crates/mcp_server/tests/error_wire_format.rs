//! Verify the JSON shape of typed errors on the MCP wire.
//!
//! Downstream agents dispatch on `data.exception`, so the field names
//! here are load-bearing. If rmcp ever renames them, this test fails
//! loud before anything ships.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::absolute_paths)]

use serde_json::Value;
use void_crawl_core::VoidCrawlError;
use voidcrawl_mcp::errors::map_err;

fn data(err: VoidCrawlError) -> Value {
    let mapped = map_err(err);
    // ErrorData serialises to `{ code, message, data? }`. We only care
    // about the data payload.
    let as_json = serde_json::to_value(&mapped).expect("serialise ErrorData");
    as_json.get("data").cloned().unwrap_or(Value::Null)
}

#[test]
fn captcha_detected_carries_exception_tag_and_kind() {
    let d = data(VoidCrawlError::CaptchaDetected { kind: "recaptcha".into() });
    assert_eq!(d["exception"], "CaptchaDetected");
    assert_eq!(d["kind"], "recaptcha");
}

#[test]
fn profile_busy_carries_exception_tag_and_name() {
    let d = data(VoidCrawlError::ProfileBusy {
        name:        "Default".into(),
        pid:         Some(42),
        acquired_at: Some(123),
    });
    assert_eq!(d["exception"], "ProfileBusy");
    assert_eq!(d["name"], "Default");
}

#[test]
fn profile_lease_expired_carries_timeout() {
    let d = data(VoidCrawlError::ProfileLeaseExpired {
        name:         "Profile 1".into(),
        timeout_secs: 42,
    });
    assert_eq!(d["exception"], "ProfileLeaseExpired");
    assert_eq!(d["name"], "Profile 1");
    assert_eq!(d["timeout_secs"], 42);
}

#[test]
fn profile_not_found_carries_searched_list() {
    let d = data(VoidCrawlError::ProfileNotFound {
        name:     "Missing".into(),
        searched: vec!["/one".into(), "/two".into()],
    });
    assert_eq!(d["exception"], "ProfileNotFound");
    assert_eq!(d["name"], "Missing");
    assert_eq!(d["searched"], serde_json::json!(["/one", "/two"]));
}

#[test]
fn plain_errors_have_no_data_payload() {
    let d = data(VoidCrawlError::BrowserClosed);
    assert_eq!(d, Value::Null);
}
