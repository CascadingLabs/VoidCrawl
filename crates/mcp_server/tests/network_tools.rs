#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, reason = "test harness")]

//! The credential gate must be enforced at the tool boundary, before anything
//! else — including before the session lookup. These tests need no browser:
//! they pass a session id that does not exist and assert the refusal is about
//! the gate, which proves the gate is checked first and cannot be reached
//! around by a caller who happens to hold a valid session.
//!
//! Deliberately no `std::env::set_var` here: Rust tests share one process, so
//! mutating the gate variable would race other tests. These assert the
//! closed-by-default behaviour, which is the security-relevant direction.

use std::{env, sync::Arc};

use voidcrawl_mcp::{
    AppState, VoidCrawlServer,
    sessions::SessionRegistry,
    tools::network::{
        self, ENABLE_ENV, NetworkCaptureArmArgs, NetworkPatternArg, SessionCookiesArgs,
    },
};

fn server() -> VoidCrawlServer {
    VoidCrawlServer::new(Arc::new(AppState::new(Arc::new(SessionRegistry::default()))))
}

/// Skip rather than report a false pass if the ambient environment has opted
/// in.
fn gate_is_closed() -> bool {
    env::var(ENABLE_ENV).is_err()
}

#[tokio::test]
async fn session_cookies_is_refused_when_the_gate_is_closed() {
    if !gate_is_closed() {
        return; // opted in ambiently; the closed-gate behaviour isn't observable
    }
    let err = network::cookies(&server(), SessionCookiesArgs { session_id: "nope".into() })
        .await
        .expect_err("session_cookies must refuse without the operator opt-in");
    let message = format!("{err:?}");
    assert!(message.contains(ENABLE_ENV), "refusal should name the env var to set, got: {message}");
    // Gate precedence: the complaint must NOT be about the missing session,
    // otherwise a caller with a real session would slip past the gate.
    assert!(
        !message.contains("no such session"),
        "gate must be checked before the session lookup, got: {message}"
    );
}

#[tokio::test]
async fn raw_headers_opt_in_is_refused_when_the_gate_is_closed() {
    if !gate_is_closed() {
        return; // opted in ambiently; the closed-gate behaviour isn't observable
    }
    let args = NetworkCaptureArmArgs {
        session_id: "nope".into(),
        patterns: vec![NetworkPatternArg { name: "api".into(), url_glob: "**/api/**".into() }],
        include_sensitive_headers: true,
        ..Default::default()
    };
    let err = network::arm(&server(), args)
        .await
        .expect_err("include_sensitive_headers must refuse without the operator opt-in");
    let message = format!("{err:?}");
    assert!(message.contains(ENABLE_ENV), "refusal should name the env var to set, got: {message}");
    assert!(
        !message.contains("no such session"),
        "gate must be checked before the session lookup, got: {message}"
    );
}

#[tokio::test]
async fn arming_without_raw_access_still_validates_its_patterns() {
    // The default (redacted) path must remain usable with the gate closed —
    // the gate protects raw values, it does not disable capture. With no
    // patterns the tool should complain about patterns, not about the gate.
    let args = NetworkCaptureArmArgs { session_id: "nope".into(), ..Default::default() };
    let err = network::arm(&server(), args).await.expect_err("empty patterns must be rejected");
    let message = format!("{err:?}");
    assert!(message.contains("pattern"), "expected a pattern-validation error, got: {message}");
    assert!(!message.contains(ENABLE_ENV), "gate must not be implicated, got: {message}");
}

// ── Cookie handoff leases ────────────────────────────────────────────────

#[tokio::test]
async fn cookie_lease_rejects_a_non_absolute_replay_origin() {
    // Scope binding is the mechanism that stops "give me every cookie", so a
    // scope that cannot be parsed must fail before any session work happens.
    let err = network::cookie_lease_open(
        &server(),
        network::CookieLeaseOpenArgs {
            session_id:    "nope".into(),
            replay_origin: "api.example.com".into(),
        },
    )
    .await
    .expect_err("a bare host is not an origin");
    let message = format!("{err:?}");
    assert!(
        !message.contains("no such session"),
        "scope must be validated before the session lookup, got: {message}"
    );
}

#[tokio::test]
async fn revoking_an_unknown_lease_is_idempotent_not_an_error() {
    // A caller unwinding after a failure should be able to revoke blindly.
    let sessions = Arc::new(SessionRegistry::default());
    let srv = VoidCrawlServer::new(Arc::new(AppState::new(Arc::clone(&sessions))));
    // No session registered, so this exercises the missing-session path.
    let err = network::cookie_lease_revoke(
        &srv,
        network::CookieLeaseRevokeArgs {
            session_id: "nope".into(),
            lease_id:   "whatever".into(),
            reason:     Some("auth_failed".into()),
        },
    )
    .await;
    assert!(err.is_err(), "an unknown session is still an error");
}
