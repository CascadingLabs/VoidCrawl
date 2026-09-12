//! Verify the secret-safe JSON shape of core errors on the MCP wire.

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
fn compatibility_tags_are_bounded_and_sensitive_fields_are_absent() {
    let cases = [
        VoidCrawlError::CaptchaDetected { kind: "vendor-secret-captcha".into() },
        VoidCrawlError::AntibotChallenge { vendor: "vendor-secret-wall".into() },
        VoidCrawlError::ProfileBusy {
            name:        "secret-profile".into(),
            pid:         Some(42),
            acquired_at: Some(123),
        },
        VoidCrawlError::ProfileLeaseExpired {
            name:         "secret-profile".into(),
            timeout_secs: 42,
        },
        VoidCrawlError::ProfileNotFound {
            name:     "secret-profile".into(),
            searched: vec!["/home/secret-profile".into()],
        },
        VoidCrawlError::SessionInterrupted { interrupt_id: "secret-interrupt-id".into() },
        VoidCrawlError::InterruptExpired { interrupt_id: "secret-interrupt-id".into() },
        VoidCrawlError::InterruptTerminal {
            interrupt_id: "secret-interrupt-id".into(),
            state:        "secret-vendor-state".into(),
        },
    ];

    for error in cases {
        let serialized = mapped(error).to_string();
        for secret in ["secret-profile", "/home/", "secret-interrupt-id", "vendor-secret"] {
            assert!(!serialized.contains(secret), "MCP data leaked {secret}: {serialized}");
        }
    }

    let captcha = data(VoidCrawlError::CaptchaDetected { kind: "recaptcha".into() });
    assert_eq!(captcha["exception"], "CaptchaDetected");
    assert_eq!(captcha["kind"], "recaptcha");
    let unknown = data(VoidCrawlError::CaptchaDetected { kind: "arbitrary vendor".into() });
    assert_eq!(unknown["kind"], "unknown");
}

#[test]
fn compatibility_tags_retain_only_safe_fields() {
    let busy = data(VoidCrawlError::ProfileBusy {
        name:        "Default".into(),
        pid:         Some(42),
        acquired_at: Some(123),
    });
    assert_eq!(busy["exception"], "ProfileBusy");
    assert!(busy.get("name").is_none());
    assert!(busy.get("pid").is_none());

    let lease = data(VoidCrawlError::ProfileLeaseExpired {
        name:         "Profile 1".into(),
        timeout_secs: 42,
    });
    assert_eq!(lease["exception"], "ProfileLeaseExpired");
    assert_eq!(lease["timeout_secs"], 42);
    assert!(lease.get("name").is_none());

    let interrupted = data(VoidCrawlError::SessionInterrupted { interrupt_id: "id".into() });
    assert_eq!(interrupted["exception"], "SessionInterrupted");
    assert!(interrupted.get("interrupt_id").is_none());
}

#[test]
fn stable_categories_determine_json_rpc_status_and_codes() {
    let cases = [
        (
            VoidCrawlError::InvalidInput { operation: "test", reason: "bad" },
            -32602,
            "invalid_input",
            "voidcrawl.input.invalid",
        ),
        (
            VoidCrawlError::UnsupportedVisualTarget,
            -32602,
            "unsupported",
            "voidcrawl.target.visual_geometry_unsupported",
        ),
        (
            VoidCrawlError::Timeout("secret".into()),
            -32603,
            "timeout",
            "voidcrawl.operation.timeout",
        ),
        (
            VoidCrawlError::SessionInterrupted { interrupt_id: "secret".into() },
            -32603,
            "interrupted",
            "voidcrawl.interrupt.active",
        ),
        (
            VoidCrawlError::ElementNotFound("secret".into()),
            -32603,
            "unavailable",
            "voidcrawl.target.element_not_found",
        ),
        (VoidCrawlError::BrowserClosed, -32603, "provider_failure", "voidcrawl.browser.closed"),
        (VoidCrawlError::Other("secret".into()), -32603, "internal", "voidcrawl.internal.other"),
    ];

    for (error, status, category, code) in cases {
        let wire = mapped(error);
        assert_eq!(wire["code"], status);
        assert_eq!(wire["data"]["category"], category);
        assert_eq!(wire["data"]["code"], code);
    }
}

#[test]
fn navigation_and_response_timeouts_are_timeout_classified() {
    let navigation = mapped(VoidCrawlError::NavigationTimeout {
        url:          "https://example.test/?token=secret".into(),
        wait_phase:   "secret-phase".into(),
        timeout_secs: 1.0,
        elapsed_secs: 1.0,
    });
    let response = mapped(VoidCrawlError::ResponseTimeout {
        patterns:     vec!["secret-pattern".into()],
        timeout_secs: 1.0,
    });
    for wire in [navigation, response] {
        assert_eq!(wire["code"], -32603);
        assert_eq!(wire["data"]["category"], "timeout");
        assert!(!wire.to_string().contains("secret"));
    }
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

#[test]
fn startup_shutdown_logs_and_owned_descriptions_are_safe() {
    let main = include_str!("../src/main.rs");
    for forbidden in [
        "tracing::info!(profile",
        "path = %handle.path()",
        "user_data_root = %user_data_root",
        "reason = ?quit",
    ] {
        assert!(!main.contains(forbidden), "unsafe log source: {forbidden}");
    }

    for source in [
        include_str!("../src/server.rs"),
        include_str!("../src/tools/screenshot.rs"),
        include_str!("../src/tools/selector.rs"),
    ] {
        assert!(!source.contains("Yosoi selector"));
    }
}
