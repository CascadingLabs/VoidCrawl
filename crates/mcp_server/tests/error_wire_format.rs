//! Verify the JSON shape of typed errors on the MCP wire.
//!
//! Downstream agents dispatch on `data.exception`, so the field names
//! here are load-bearing. If rmcp ever renames them, this test fails
//! loud before anything ships.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::absolute_paths)]

use serde_json::Value;
use void_crawl_core::VoidCrawlError;
use voidcrawl_mcp::errors::map_err;

fn mapped(err: VoidCrawlError) -> Value {
    serde_json::to_value(map_err(err)).expect("serialise ErrorData")
}

fn data(err: VoidCrawlError) -> Value {
    mapped(err).get("data").cloned().unwrap_or(Value::Null)
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
fn profile_not_found_omits_machine_local_search_paths() {
    let d = data(VoidCrawlError::ProfileNotFound {
        name:     "Missing".into(),
        searched: vec!["/one".into(), "/two".into()],
    });
    assert_eq!(d["exception"], "ProfileNotFound");
    assert!(d.get("name").is_none());
    assert!(d.get("searched").is_none());
}

#[test]
fn interrupt_errors_carry_redacted_lifecycle_ids() {
    let interrupted =
        data(VoidCrawlError::SessionInterrupted { interrupt_id: "interrupt-1".into() });
    assert_eq!(interrupted["exception"], "SessionInterrupted");
    assert_eq!(interrupted["interrupt_id"], "interrupt-1");

    let expired = data(VoidCrawlError::InterruptExpired { interrupt_id: "interrupt-1".into() });
    assert_eq!(expired["exception"], "InterruptExpired");
    assert_eq!(expired["interrupt_id"], "interrupt-1");
}

#[test]
fn every_error_carries_stable_safe_summary_data() {
    let d = data(VoidCrawlError::BrowserClosed);
    assert_eq!(d["code"], "voidcrawl.browser.closed");
    assert_eq!(d["category"], "provider_failure");
}

#[test]
fn raw_diagnostics_do_not_cross_the_default_mcp_error_boundary() {
    let error = mapped(VoidCrawlError::NavigationFailed(
        "https://example.test/?access_token=secret".into(),
    ));
    let serialized = error.to_string();
    assert_eq!(error["message"], "navigation failed");
    assert_eq!(error["data"]["code"], "voidcrawl.navigation.failed");
    assert!(!serialized.contains("access_token"));
    assert!(!serialized.contains("secret"));
}
