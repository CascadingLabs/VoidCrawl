//! Rich, header/body-bearing network capture — an arm/wait pair over the core
//! crate's CDP-backed [`ResponseCapture`], mirroring `download_arm` /
//! `download_wait`.
//!
//! This is deliberately a second tool, not a replacement for `network_capture`:
//! that tool reads `performance.getEntriesByType('resource')` after the fact
//! (cheap, no arming, no headers/body); this one arms real CDP `Network.*`
//! listeners for named URL-glob expectations *before* the triggering
//! navigation/click, and returns request headers, response headers, status, and
//! (opt-in) body for each match.
//!
//! # Credential handling
//!
//! `request_headers` is where a live credential actually appears — an
//! `Authorization` bearer set by page code. Response headers practically never
//! carry one. Values for credential-bearing header names are
//! replaced with a bare `"<redacted>"` by default (no length, so the
//! placeholder itself leaks nothing). Matching is substring-based and
//! deny-by-default: it over-redacts rather than miss a novel header name.
//!
//! Seeing raw values requires BOTH `include_sensitive_headers: true` AND the
//! operator setting [`ENABLE_ENV`], matching how `download` gates a lesser risk
//! behind `VOIDCRAWL_ALLOW_DOWNLOADS`. A model-controlled boolean alone must
//! not be able to pull live credentials into a transcript.
//!
//! # Known limits
//!
//! * **URLs are returned raw and unredacted.** A presigned URL or a
//!   `?access_token=` query parameter is a credential in the URL, and this tool
//!   does not sanitize it — replay needs the real URL, so redacting it would
//!   defeat the tool's purpose. Treat `url` as potentially secret.
//! * **Bodies are not scanned.** With `capture_body`, an endpoint that echoes
//!   auth context into its JSON (whoami/auth-check responses often do) returns
//!   it verbatim in `body_base64`.
//! * **Cookies are never captured from the wire.** Neither the request-side
//!   `Cookie` nor the response-side `Set-Cookie` appears: Chrome reports both
//!   only via its `*ExtraInfo` events, and subscribing to those was measured as
//!   delivering zero events through the vendored chromiumoxide 0.9.1. Read
//!   cookie state from the cookie store (`session_cookies`) instead.

use std::{collections::HashMap, env, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::time;
use void_crawl_core::{
    CapturedResponse, LeaseScope, ResponseBodyState, ResponseCaptureLimits, VoidCrawlError,
    fork_scoped,
};

use crate::{errors::map_err, server::VoidCrawlServer, sessions::PendingNetworkCapture};

/// Default seconds `network_capture_wait` waits, measured from the moment
/// `wait` is called.
const DEFAULT_WAIT_SECS: u64 = 30;
/// Default ceiling on how long an armed capture stays live, measured from
/// `arm`. Generous so it acts as a leak backstop rather than as the effective
/// timeout — the caller's budget belongs to `wait`.
const DEFAULT_ARM_CEILING_SECS: u64 = 300;

/// Environment variable that lets callers read raw credential values —
/// `include_sensitive_headers: true` on the capture tools, and
/// `session_cookies` at all. Unset (or `0`/`false`/empty) → refused. Off by
/// default because these paths expose live authenticated session material.
pub const ENABLE_ENV: &str = "VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE";

/// Substrings that mark a header name as credential-bearing. Matched
/// case-insensitively against the whole lowercased name, so `authorization`
/// also covers `proxy-authorization` and `x-authorization`, and `token` covers
/// `x-csrf-token`, `x-xsrf-token`, `x-access-token`, `x-amz-security-token`, …
///
/// Deny-by-default: over-redacting a benign header is a cosmetic annoyance,
/// missing a credential is a leak.
const SENSITIVE_HEADER_SUBSTRINGS: &[&str] = &[
    "authorization",
    "authenticate",
    "authentication",
    "x-auth",
    "cookie",
    "token",
    "api-key",
    "apikey",
    "secret",
    "credential",
    "password",
    "signature",
    "session",
];

/// Placeholder substituted for a redacted header value. Carries no length —
/// a secret's length is itself information.
const REDACTED: &str = "<redacted>";

fn is_sensitive_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SENSITIVE_HEADER_SUBSTRINGS.iter().any(|needle| lower.contains(needle))
}

/// Whether raw credential access is enabled via [`ENABLE_ENV`].
fn raw_access_enabled() -> bool {
    enabled_from(env::var(ENABLE_ENV).ok().as_deref())
}

/// Pure gate logic: enabled iff present and not a falsey token. Mirrors
/// `download::enabled_from` so the two opt-ins behave identically.
fn enabled_from(value: Option<&str>) -> bool {
    match value {
        Some(v) => {
            let v = v.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        }
        None => false,
    }
}

fn raw_access_denied_err(what: &'static str) -> VoidCrawlError {
    VoidCrawlError::InvalidInput {
        operation: what,
        reason:    "raw credential access is disabled; set VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE=1 to enable",
    }
}

// ── Arm ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NetworkPatternArg {
    /// Caller-chosen name for this expectation (the key in
    /// `network_capture_wait`'s result map).
    pub name:     String,
    /// Glob against the full request URL, e.g. `"**/api/v1/orders*"`.
    pub url_glob: String,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct NetworkCaptureArmArgs {
    pub session_id:                String,
    /// One or more named URL-glob expectations. Every one must be observed
    /// before `network_capture_wait` returns.
    pub patterns:                  Vec<NetworkPatternArg>,
    /// Ceiling in seconds on how long this armed capture stays live, from
    /// `arm` (default 300). This is a leak backstop, NOT the wait budget —
    /// pass `timeout_secs` to `network_capture_wait` for that.
    #[serde(default)]
    pub arm_ceiling_secs:          Option<u64>,
    /// Also capture and return response bodies (default false — headers and
    /// status only), subject to `max_response_bytes` / `max_total_bytes`.
    /// Bodies are returned verbatim and are NOT credential-scanned.
    #[serde(default)]
    pub capture_body:              bool,
    /// Max bytes retained for one response body (default 2 MiB).
    #[serde(default)]
    pub max_response_bytes:        Option<usize>,
    /// Max bytes retained across all captured bodies in this arm (default
    /// 8 MiB).
    #[serde(default)]
    pub max_total_bytes:           Option<usize>,
    /// Return raw credential-bearing header values instead of `<redacted>`.
    /// Requires the operator to also set
    /// `VOIDCRAWL_ALLOW_CREDENTIAL_CAPTURE=1`; otherwise the call is
    /// refused rather than silently redacted.
    #[serde(default)]
    pub include_sensitive_headers: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NetworkCaptureArmResult {
    pub armed:   bool,
    pub message: String,
}

pub async fn arm(
    server: &VoidCrawlServer,
    args: NetworkCaptureArmArgs,
) -> Result<NetworkCaptureArmResult, ErrorData> {
    // Refuse loudly rather than downgrade to redacted output — a caller that
    // asked for raw values should not believe it got them.
    if args.include_sensitive_headers && !raw_access_enabled() {
        return Err(map_err(raw_access_denied_err("include_sensitive_headers")));
    }
    if args.patterns.is_empty() {
        return Err(map_err(VoidCrawlError::InvalidInput {
            operation: "network_capture_arm",
            reason:    "at least one pattern is required",
        }));
    }

    let session = server
        .state()
        .sessions
        .get(&args.session_id)
        .await
        .ok_or_else(|| VoidCrawlError::Other(format!("no such session: {}", args.session_id)))
        .map_err(map_err)?;

    // Hold the slot lock across the arm so two concurrent `arm` calls can't
    // both pass the vacancy check and have the first capture silently dropped.
    // Lock order is slot → page; nothing takes them the other way round.
    let mut slot = session.pending_network_capture.lock().await;
    if slot.is_some() {
        return Err(map_err(VoidCrawlError::Other(
            "a network capture is already armed on this session; call network_capture_wait first"
                .into(),
        )));
    }

    let patterns = args.patterns.into_iter().map(|p| (p.name, p.url_glob)).collect::<Vec<_>>();
    let ceiling = Duration::from_secs(args.arm_ceiling_secs.unwrap_or(DEFAULT_ARM_CEILING_SECS));
    let limits = ResponseCaptureLimits::new(
        void_crawl_core::BrowserByteLimit::try_from(
            args.max_response_bytes.unwrap_or(void_crawl_core::DEFAULT_MAX_RESPONSE_BYTES),
        )
        .map_err(|error| map_err(VoidCrawlError::Other(error.to_string())))?,
        void_crawl_core::BrowserByteLimit::try_from(
            args.max_total_bytes.unwrap_or(void_crawl_core::DEFAULT_MAX_TOTAL_RESPONSE_BYTES),
        )
        .map_err(|error| map_err(VoidCrawlError::Other(error.to_string())))?,
    );

    let capture = {
        let page = session.page.lock().await;
        page.expect_responses(patterns, ceiling, limits).await.map_err(map_err)?
    };

    *slot = Some(PendingNetworkCapture {
        capture,
        include_sensitive_headers: args.include_sensitive_headers,
        capture_body: args.capture_body,
    });

    Ok(NetworkCaptureArmResult {
        armed:   true,
        message: "network capture armed — perform the action that triggers the requests, then \
call network_capture_wait"
            .into(),
    })
}

// ── Wait ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct NetworkCaptureWaitArgs {
    pub session_id:   String,
    /// Seconds to wait for every armed pattern to be observed, measured from
    /// THIS call (default 30). The clock does not start at `arm`, so time spent
    /// clicking and typing between arm and wait does not eat the budget.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CapturedResponseJson {
    /// Returned RAW and unredacted — may itself embed a credential (presigned
    /// URL, `?access_token=`). Replay needs the real URL.
    pub url:                 String,
    pub status:              u16,
    /// Headers the browser SENT. This is where `Authorization` / `Cookie`
    /// appear; values are `<redacted>` unless raw access was granted.
    pub request_headers:     Vec<(String, String)>,
    /// Response headers. Note raw `Set-Cookie` is not among them (see module
    /// docs).
    pub headers:             Vec<(String, String)>,
    pub mime_type:           String,
    pub resource_type:       String,
    pub from_cache:          bool,
    pub from_service_worker: bool,
    /// `"available"`, `"truncated"`, or `"unavailable"`.
    pub body_state:          String,
    pub body_error:          Option<String>,
    /// Canonical byte accounting for the response body.
    pub byte_report:         serde_json::Value,
    /// Base64 body, present when `capture_body` was set and any bytes were
    /// retained — including a `truncated` body, whose retained prefix is
    /// returned rather than discarded. NOT credential-scanned.
    pub body_base64:         Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NetworkCaptureWaitResult {
    pub captures: HashMap<String, CapturedResponseJson>,
}

fn redact_headers(headers: &[(String, String)], include_sensitive: bool) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| {
            if !include_sensitive && is_sensitive_header(k) {
                (k.clone(), REDACTED.to_string())
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect()
}

fn to_json(
    name_to_response: HashMap<String, CapturedResponse>,
    include_sensitive_headers: bool,
    capture_body: bool,
) -> HashMap<String, CapturedResponseJson> {
    name_to_response
        .into_iter()
        .map(|(name, resp)| {
            // A truncated body still has retained bytes worth returning; only
            // `Unavailable` means there is genuinely nothing.
            let body_base64 = (capture_body
                && resp.body_state != ResponseBodyState::Unavailable
                && !resp.body().is_empty())
            .then(|| BASE64.encode(resp.body()));
            let json = CapturedResponseJson {
                url: resp.url.clone(),
                status: resp.status,
                request_headers: redact_headers(&resp.request_headers, include_sensitive_headers),
                headers: redact_headers(&resp.headers, include_sensitive_headers),
                mime_type: resp.mime_type.clone(),
                resource_type: resp.resource_type.clone(),
                from_cache: resp.from_cache,
                from_service_worker: resp.from_service_worker,
                body_state: resp.body_state.as_str().to_string(),
                body_error: resp.body_error.clone(),
                byte_report: serde_json::to_value(resp.byte_report().unwrap_or_else(|_| {
                    void_crawl_core::BrowserByteReport::unavailable(
                        void_crawl_core::BrowserByteDomain::CdpDecodedBody,
                        void_crawl_core::BrowserPayloadUnavailableReason::ProviderDidNotReport,
                    )
                }))
                .unwrap_or(serde_json::Value::Null),
                body_base64,
            };
            (name, json)
        })
        .collect()
}

pub async fn wait(
    server: &VoidCrawlServer,
    args: NetworkCaptureWaitArgs,
) -> Result<NetworkCaptureWaitResult, ErrorData> {
    let session = server
        .state()
        .sessions
        .get(&args.session_id)
        .await
        .ok_or_else(|| VoidCrawlError::Other(format!("no such session: {}", args.session_id)))
        .map_err(map_err)?;

    let pending = session.pending_network_capture.lock().await.take().ok_or_else(|| {
        map_err(VoidCrawlError::Other(
            "no armed network capture for this session; call network_capture_arm first".into(),
        ))
    })?;
    let PendingNetworkCapture { capture, include_sensitive_headers, capture_body } = pending;

    let budget = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_WAIT_SECS));
    // Enforce the caller's budget here, from now — not from when `arm` ran.
    // Dropping the capture on timeout aborts its worker.
    let result = match time::timeout(budget, capture.wait()).await {
        Ok(inner) => inner.map_err(map_err)?,
        Err(_) => {
            return Err(map_err(VoidCrawlError::Timeout(format!(
                "no matching responses observed within {}s of network_capture_wait; the armed \
                 patterns may not match the requests the page actually made",
                budget.as_secs()
            ))));
        }
    };
    Ok(NetworkCaptureWaitResult {
        captures: to_json(result, include_sensitive_headers, capture_body),
    })
}

// ── Cookies (interim, raw — NOT the CAS-251 leased/provenance jar) ────────
//
// A direct MCP wrapper over the core crate's `Page::get_cookies()`, which was
// previously PyO3-only. It returns full cookie attributes INCLUDING raw values
// (HttpOnly and Secure cookies alike) with no scoping, leasing, TTL, or
// classification, so it is gated behind `ENABLE_ENV` in full.
//
// Prefer `cookie_lease_open` for anything that is not local debugging: it
// returns the same facts as value-free provenance and never puts a value on the
// wire. This tool exists only for the case where a human needs the literal
// value in front of them.

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct SessionCookiesArgs {
    pub session_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionCookiesResult {
    /// Raw CDP cookie objects: name, value, domain, path, expires, size,
    /// httpOnly, secure, session, sameSite, priority, … Values are unredacted.
    pub cookies: Vec<serde_json::Value>,
}

pub async fn cookies(
    server: &VoidCrawlServer,
    args: SessionCookiesArgs,
) -> Result<SessionCookiesResult, ErrorData> {
    if !raw_access_enabled() {
        return Err(map_err(raw_access_denied_err("session_cookies")));
    }

    let session = server
        .state()
        .sessions
        .get(&args.session_id)
        .await
        .ok_or_else(|| VoidCrawlError::Other(format!("no such session: {}", args.session_id)))
        .map_err(map_err)?;

    let page = session.page.lock().await;
    let raw_cookies = page.get_cookies().await.map_err(map_err)?;
    // Propagate rather than substituting `null`: a null element in this array
    // would crash callers that index `cookie["name"]`.
    let cookies = raw_cookies
        .into_iter()
        .map(|c| {
            serde_json::to_value(c).map_err(|e| {
                map_err(VoidCrawlError::Other(format!("failed to serialize cookie: {e}")))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SessionCookiesResult { cookies })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_headers_are_detected_case_insensitively() {
        for name in [
            "authorization",
            "Authorization",
            "AUTHORIZATION",
            "proxy-authorization",
            "x-authorization",
            "cookie",
            "set-cookie",
            "x-api-key",
            "api-key",
            "x-apikey",
            "x-goog-api-key",
            "x-csrf-token",
            "x-xsrf-token",
            "x-access-token",
            "x-refresh-token",
            "x-session-token",
            "x-amz-security-token",
            "x-amz-signature",
            "www-authenticate",
            "authentication-info",
            "x-auth-user",
            "x-client-secret",
            "x-password",
        ] {
            assert!(is_sensitive_header(name), "{name} should be treated as credential-bearing");
        }
    }

    #[test]
    fn ordinary_headers_are_not_redacted() {
        for name in [
            "content-type",
            "content-length",
            "accept",
            "accept-encoding",
            "accept-language",
            "date",
            "server",
            "user-agent",
            "referer",
            "cache-control",
            "etag",
            "location",
            "origin",
        ] {
            assert!(!is_sensitive_header(name), "{name} should NOT be redacted");
        }
    }

    #[test]
    fn redaction_hides_the_value_and_its_length_but_keeps_the_name() {
        let headers = vec![
            ("authorization".to_string(), "Bearer supersecrettoken".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ];
        let out = redact_headers(&headers, false);
        // Name preserved (it is the useful, non-secret half)…
        assert_eq!(out[0].0, "authorization");
        // …value fully replaced, and the placeholder does not encode the
        // original length.
        assert_eq!(out[0].1, "<redacted>");
        assert!(!out[0].1.contains("22"));
        // Non-sensitive header untouched.
        assert_eq!(out[1].1, "application/json");
    }

    #[test]
    fn opting_in_returns_raw_values() {
        let headers = vec![("authorization".to_string(), "Bearer tok".to_string())];
        assert_eq!(redact_headers(&headers, true)[0].1, "Bearer tok");
    }

    #[test]
    fn raw_access_gate_is_closed_by_default_and_rejects_falsey_values() {
        assert!(!enabled_from(None));
        assert!(!enabled_from(Some("")));
        assert!(!enabled_from(Some("   ")));
        assert!(!enabled_from(Some("0")));
        assert!(!enabled_from(Some("false")));
        assert!(!enabled_from(Some("FALSE")));
        assert!(enabled_from(Some("1")));
        assert!(enabled_from(Some("true")));
        assert!(enabled_from(Some("yes")));
    }
}

// ── Cookie handoff leases (CAS-251, VoidCrawl side) ───────────────────────
//
// These tools deliberately never return a cookie value. `cookie_lease_open`
// forks the in-scope cookies into an in-memory lease owned by the session and
// returns only the lease id plus a value-free provenance record per cookie.
// Because nothing secret crosses the wire, these need no operator gate — the
// gate on `session_cookies` exists precisely because that tool DOES return
// values, and it remains a debugging escape hatch rather than the handoff path.
//
// Classification (eligible / expired / HTTPS-only / partitioned-context) is not
// here on purpose: VoidCrawl reports facts, the caller owns replay policy.
// Applying a lease to an outbound request is likewise not implemented yet.

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct CookieLeaseOpenArgs {
    pub session_id:    String,
    /// Absolute URL of the origin the lease is for, e.g.
    /// `"https://api.example.com"`. Only cookies whose domain could be sent to
    /// this origin enter the lease — a caller cannot request every cookie.
    pub replay_origin: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CookieLeaseOpenResult {
    pub lease_id:      String,
    pub replay_origin: String,
    /// Value-free provenance, one entry per in-scope cookie. Safe to persist.
    pub cookies:       Vec<serde_json::Value>,
    /// Top-level URL of the page when the snapshot was taken, or null if the
    /// page had no URL. Recorded because it is ambient and unrecoverable later.
    pub observed_in:   Option<String>,
}

pub async fn cookie_lease_open(
    server: &VoidCrawlServer,
    args: CookieLeaseOpenArgs,
) -> Result<CookieLeaseOpenResult, ErrorData> {
    let scope = LeaseScope::from_url(&args.replay_origin).map_err(map_err)?;
    let session = server
        .state()
        .sessions
        .get(&args.session_id)
        .await
        .ok_or_else(|| VoidCrawlError::Other(format!("no such session: {}", args.session_id)))
        .map_err(map_err)?;

    let (raw_cookies, observed_in) = {
        let page = session.page.lock().await;
        let cookies = page.get_cookies().await.map_err(map_err)?;
        let url = page.url().await.map_err(map_err)?;
        (cookies, url)
    };

    let lease = fork_scoped(&raw_cookies, scope, &args.session_id, observed_in.as_deref());
    let lease_id = lease.id().to_string();
    let replay_origin = lease.scope().origin();
    let cookies = lease
        .provenance()
        .into_iter()
        .map(|p| {
            serde_json::to_value(p).map_err(|e| {
                map_err(VoidCrawlError::Other(format!("failed to serialize provenance: {e}")))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    session.cookie_leases.lock().await.insert(lease_id.clone(), lease);
    Ok(CookieLeaseOpenResult { lease_id, replay_origin, cookies, observed_in })
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct CookieLeaseRevokeArgs {
    pub session_id: String,
    pub lease_id:   String,
    /// Machine-readable reason, e.g. `"auth_failed"` or `"drift_detected"`.
    /// Recorded so a downstream failure can be explained rather than guessed.
    #[serde(default)]
    pub reason:     Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CookieLeaseRevokeResult {
    pub revoked: bool,
    pub reason:  String,
}

pub async fn cookie_lease_revoke(
    server: &VoidCrawlServer,
    args: CookieLeaseRevokeArgs,
) -> Result<CookieLeaseRevokeResult, ErrorData> {
    let session = server
        .state()
        .sessions
        .get(&args.session_id)
        .await
        .ok_or_else(|| VoidCrawlError::Other(format!("no such session: {}", args.session_id)))
        .map_err(map_err)?;

    let reason = args.reason.unwrap_or_else(|| "explicit_revoke".to_string());
    // Removing it from the map drops the lease, whose Drop scrubs the values;
    // revoke() first so the reason is recorded rather than the generic default.
    let mut leases = session.cookie_leases.lock().await;
    match leases.remove(&args.lease_id) {
        Some(mut lease) => {
            lease.revoke(reason.clone());
            Ok(CookieLeaseRevokeResult { revoked: true, reason })
        }
        // Idempotent: an already-revoked or session-reaped lease is gone, which
        // is the state the caller asked for.
        None => Ok(CookieLeaseRevokeResult {
            revoked: false,
            reason:  format!("no such lease {} on this session (already revoked?)", args.lease_id),
        }),
    }
}
