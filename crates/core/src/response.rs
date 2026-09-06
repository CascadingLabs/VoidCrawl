//! Opt-in, passive CDP response-body capture.
//!
//! Capture scopes subscribe to Network events before the triggering action.
//! They never intercept requests or inject page-world JavaScript.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    result::Result as StdResult,
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chromiumoxide::{
    Page as CdpPage,
    cdp::browser_protocol::network::{
        EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent, EventResponseReceived,
        GetResponseBodyParams, Headers,
    },
    listeners::EventStream,
};
use futures::StreamExt;
use globset::{Glob, GlobMatcher};
use tokio::{sync::oneshot, task::JoinHandle, time};

use crate::{
    BrowserBudgetScope, BrowserByteCount, BrowserByteDomain, BrowserByteLimit, BrowserByteReport,
    BrowserByteReportError, BrowserByteSpec, BrowserLimitScope, BrowserPayloadUnavailableReason,
    error::{Result, VoidCrawlError},
};

/// Default maximum retained body size for one captured response (2 MiB).
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
/// Default maximum retained body size across one expectation (8 MiB).
pub const DEFAULT_MAX_TOTAL_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Whether a captured response body is complete, truncated, or unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseBodyState {
    Available,
    Truncated,
    Unavailable,
}

impl ResponseBodyState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Truncated => "truncated",
            Self::Unavailable => "unavailable",
        }
    }
}

/// A complete response observation captured from CDP.
#[derive(Debug, Clone)]
pub struct CapturedResponse {
    pub url:                 String,
    pub status:              u16,
    pub headers:             Vec<(String, String)>,
    /// Headers the browser SENT for this request, sorted by name.
    ///
    /// This is the only place a request-side credential (`Authorization`,
    /// `Cookie`) is observable — `headers` above is response-side only. Callers
    /// that serialize this MUST treat the values as secret.
    ///
    /// Merged from `Network.requestWillBeSent`'s author-level headers and, as a
    /// lower-authority fallback, `Network.Response.requestHeaders` (which is
    /// absent in practice). An `Authorization` header set by page code — the
    /// common bearer-token case — IS captured here.
    ///
    /// KNOWN GAP: browser-managed `Cookie` is NOT captured. Chrome attaches it
    /// after `requestWillBeSent` and reports it only via
    /// `requestWillBeSentExtraInfo`; subscribing to that event was tried and
    /// measured as delivering zero events through the vendored chromiumoxide
    /// 0.9.1, whose network handler does not route it. Read cookie state from
    /// the cookie store instead. Raw `Set-Cookie` is likewise unavailable
    /// (`responseReceivedExtraInfo`, same limitation).
    pub request_headers:     Vec<(String, String)>,
    pub mime_type:           String,
    pub resource_type:       String,
    pub from_cache:          bool,
    pub from_service_worker: bool,
    pub body_state:          ResponseBodyState,
    pub body_error:          Option<String>,
    body:                    Arc<[u8]>,
    limits:                  ResponseCaptureLimits,
    complete_bytes:          Option<usize>,
}

impl CapturedResponse {
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn byte_report(&self) -> StdResult<BrowserByteReport, BrowserByteReportError> {
        if self.body_state == ResponseBodyState::Unavailable {
            return Ok(BrowserByteReport::unavailable(
                BrowserByteDomain::CdpDecodedBody,
                BrowserPayloadUnavailableReason::ProviderDidNotReport,
            ));
        }
        let spec = BrowserByteSpec::new(
            BrowserByteDomain::CdpDecodedBody,
            self.limits.per_response(),
            BrowserLimitScope::RetentionAfterProviderMaterialization,
            BrowserBudgetScope::PerPayload,
        );
        BrowserByteReport::from_known_extent(
            BrowserByteDomain::CdpDecodedBody,
            Some(spec),
            BrowserByteCount::try_from_usize(self.complete_bytes.unwrap_or(self.body.len()))?,
            BrowserByteCount::try_from_usize(self.body.len())?,
        )
    }

    pub fn text(&self) -> Result<String> {
        String::from_utf8(self.body.to_vec()).map_err(|e| {
            VoidCrawlError::ResponseBody(format!("response body is not valid UTF-8: {e}"))
        })
    }

    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(&self.body)
            .map_err(|e| VoidCrawlError::ResponseBody(format!("invalid JSON response body: {e}")))
    }
}

/// Memory limits for one response expectation.
#[derive(Debug, Clone, Copy)]
pub struct ResponseCaptureLimits {
    per_response: BrowserByteLimit,
    aggregate:    BrowserByteLimit,
}

impl ResponseCaptureLimits {
    pub fn new(per_response: BrowserByteLimit, aggregate: BrowserByteLimit) -> Self {
        Self { per_response, aggregate }
    }

    pub fn per_response(self) -> BrowserByteLimit {
        self.per_response
    }

    pub fn aggregate(self) -> BrowserByteLimit {
        self.aggregate
    }
}

impl Default for ResponseCaptureLimits {
    fn default() -> Self {
        Self::new(
            BrowserByteLimit::try_from(DEFAULT_MAX_RESPONSE_BYTES)
                .unwrap_or(BrowserByteLimit::one()),
            BrowserByteLimit::try_from(DEFAULT_MAX_TOTAL_RESPONSE_BYTES)
                .unwrap_or(BrowserByteLimit::one()),
        )
    }
}

#[derive(Debug)]
struct Matcher {
    name:    String,
    pattern: String,
    glob:    GlobMatcher,
}

#[derive(Debug, Clone)]
struct PendingResponse {
    names:               Vec<String>,
    url:                 String,
    status:              u16,
    headers:             Vec<(String, String)>,
    request_headers:     Vec<(String, String)>,
    mime_type:           String,
    resource_type:       String,
    from_cache:          bool,
    from_service_worker: bool,
}

/// An armed capture. Dropping it aborts its event worker and unregisters its
/// listeners as their streams are dropped.
pub struct ResponseCapture {
    receiver: Option<oneshot::Receiver<Result<HashMap<String, CapturedResponse>>>>,
    worker:   JoinHandle<()>,
}

impl fmt::Debug for ResponseCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseCapture").finish_non_exhaustive()
    }
}

impl ResponseCapture {
    pub(crate) async fn arm(
        page: CdpPage,
        patterns: Vec<(String, String)>,
        timeout: Duration,
        limits: ResponseCaptureLimits,
    ) -> Result<Self> {
        if patterns.is_empty() {
            return Err(VoidCrawlError::Other("at least one response pattern is required".into()));
        }
        let mut names = HashSet::with_capacity(patterns.len());
        if let Some((duplicate, _)) = patterns.iter().find(|(name, _)| !names.insert(name.clone()))
        {
            return Err(VoidCrawlError::Other(format!(
                "duplicate response expectation name {duplicate:?}"
            )));
        }

        let matchers = patterns
            .into_iter()
            .map(|(name, pattern)| {
                let matcher = Glob::new(&pattern)
                    .map_err(|e| {
                        VoidCrawlError::Other(format!("invalid URL glob {pattern:?}: {e}"))
                    })?
                    .compile_matcher();
                Ok(Matcher { name, pattern, glob: matcher })
            })
            .collect::<Result<Vec<_>>>()?;

        // Register every stream before returning the armed scope.
        let requests = page
            .event_listener::<EventRequestWillBeSent>()
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let responses = page
            .event_listener::<EventResponseReceived>()
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let finished = page
            .event_listener::<EventLoadingFinished>()
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;
        let failed = page
            .event_listener::<EventLoadingFailed>()
            .await
            .map_err(|e| VoidCrawlError::PageError(e.to_string()))?;

        let (sender, receiver) = oneshot::channel();
        let worker = tokio::spawn(async move {
            let result =
                run_capture(page, matchers, requests, responses, finished, failed, timeout, limits)
                    .await;
            let _ = sender.send(result);
        });
        Ok(Self { receiver: Some(receiver), worker })
    }

    pub async fn wait(mut self) -> Result<HashMap<String, CapturedResponse>> {
        let Some(receiver) = self.receiver.take() else {
            return Err(VoidCrawlError::Other("response capture already consumed".into()));
        };
        match receiver.await {
            Ok(result) => result,
            Err(_) => Err(VoidCrawlError::BrowserClosed),
        }
    }
}

impl Drop for ResponseCapture {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
async fn run_capture(
    page: CdpPage,
    matchers: Vec<Matcher>,
    mut requests: EventStream<EventRequestWillBeSent>,
    mut responses: EventStream<EventResponseReceived>,
    mut finished: EventStream<EventLoadingFinished>,
    mut failed: EventStream<EventLoadingFailed>,
    timeout: Duration,
    limits: ResponseCaptureLimits,
) -> Result<HashMap<String, CapturedResponse>> {
    let wanted = matchers.len();
    let pattern_names =
        matchers.iter().map(|m| format!("{}={}", m.name, m.pattern)).collect::<Vec<_>>();
    let mut pending: HashMap<String, PendingResponse> = HashMap::new();
    let mut captured: HashMap<String, CapturedResponse> = HashMap::new();
    // Request headers arrive on their own events, before the response they
    // belong to, so they are accumulated by request id and attached later.
    let mut sent_headers: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut retained = 0usize;
    let deadline = time::sleep(timeout);
    tokio::pin!(deadline);

    loop {
        if captured.len() == wanted {
            return Ok(captured);
        }
        tokio::select! {
            maybe_request = requests.next() => {
                let Some(event) = maybe_request else {
                    return Err(VoidCrawlError::BrowserClosed);
                };
                // Author-level headers (what the page asked to send). Recorded
                // for every request, not just matching ones — the glob is
                // checked against the response, which arrives later.
                merge_sent_headers(
                    &mut sent_headers,
                    event.request_id.inner(),
                    flatten_headers(event.request.headers.inner()),
                    false,
                );
                let Some(response) = event.redirect_response.as_ref() else { continue };
                let names = matchers.iter()
                    .filter(|m| !captured.contains_key(&m.name) && m.glob.is_match(&response.url))
                    .map(|m| m.name.clone())
                    .collect::<Vec<_>>();
                if names.is_empty() {
                    continue;
                }
                let meta = PendingResponse {
                    names,
                    url: response.url.clone(),
                    status: u16::try_from(response.status).unwrap_or_default(),
                    headers: flatten_headers(response.headers.inner()),
                    request_headers: take_sent_headers(
                        &mut sent_headers,
                        event.request_id.inner(),
                    ),
                    mime_type: response.mime_type.clone(),
                    resource_type: event.r#type.as_ref().map_or_else(
                        || "other".to_string(),
                        |kind| format!("{kind:?}").to_lowercase(),
                    ),
                    from_cache: response.from_disk_cache.unwrap_or(false)
                        || response.from_prefetch_cache.unwrap_or(false),
                    from_service_worker: response.from_service_worker.unwrap_or(false),
                };
                let response = unavailable_response(
                    meta.clone(),
                    "redirect response bodies are unavailable through CDP".into(),
                );
                for name in meta.names {
                    captured.entry(name).or_insert_with(|| response.clone());
                }
            }
            maybe_response = responses.next() => {
                let Some(event) = maybe_response else {
                    return Err(VoidCrawlError::BrowserClosed);
                };
                let names = matchers.iter()
                    .filter(|m| !captured.contains_key(&m.name) && m.glob.is_match(&event.response.url))
                    .map(|m| m.name.clone())
                    .collect::<Vec<_>>();
                if names.is_empty() {
                    continue;
                }
                let request_id = event.request_id.inner().clone();
                // Lowest-authority source; usually absent in practice, so it
                // only fills gaps the request events did not already cover.
                merge_sent_headers(
                    &mut sent_headers,
                    &request_id,
                    optional_headers(event.response.request_headers.as_ref()),
                    false,
                );
                pending.insert(request_id, PendingResponse {
                    names,
                    url: event.response.url.clone(),
                    status: u16::try_from(event.response.status).unwrap_or_default(),
                    headers: flatten_headers(event.response.headers.inner()),
                    // Resolved at finalize, not here: requestWillBeSentExtraInfo
                    // may still be in flight and is the only source of `Cookie`.
                    request_headers: Vec::new(),
                    mime_type: event.response.mime_type.clone(),
                    resource_type: format!("{:?}", event.r#type).to_lowercase(),
                    from_cache: event.response.from_disk_cache.unwrap_or(false)
                        || event.response.from_prefetch_cache.unwrap_or(false),
                    from_service_worker: event.response.from_service_worker.unwrap_or(false),
                });
            }
            maybe_finished = finished.next() => {
                let Some(event) = maybe_finished else {
                    return Err(VoidCrawlError::BrowserClosed);
                };
                let request_id = event.request_id.inner().clone();
                let Some(mut meta) = pending.remove(&request_id) else { continue };
                meta.request_headers = take_sent_headers(&mut sent_headers, &request_id);
                let body_result = page.execute(GetResponseBodyParams::new(event.request_id.clone())).await;
                let response = match body_result {
                    Ok(result) => {
                        let decoded = if result.base64_encoded {
                            BASE64.decode(result.body.as_bytes()).map_err(|e| {
                                VoidCrawlError::ResponseBody(format!("invalid base64 response body: {e}"))
                            })?
                        } else {
                            result.body.as_bytes().to_vec()
                        };
                        bounded_response(meta.clone(), decoded, &mut retained, limits)?
                    }
                    Err(error) => unavailable_response(meta.clone(), error.to_string()),
                };
                for name in meta.names {
                    captured.entry(name).or_insert_with(|| response.clone());
                }
            }
            maybe_failed = failed.next() => {
                let Some(event) = maybe_failed else {
                    return Err(VoidCrawlError::BrowserClosed);
                };
                let request_id = event.request_id.inner().clone();
                let Some(mut meta) = pending.remove(&request_id) else { continue };
                meta.request_headers = take_sent_headers(&mut sent_headers, &request_id);
                let response = unavailable_response(meta.clone(), event.error_text.clone());
                for name in meta.names {
                    captured.entry(name).or_insert_with(|| response.clone());
                }
            }
            () = &mut deadline => {
                return Err(VoidCrawlError::ResponseTimeout {
                    patterns: pattern_names,
                    timeout_secs: timeout.as_secs_f64(),
                });
            }
        }
    }
}

fn bounded_response(
    meta: PendingResponse,
    mut body: Vec<u8>,
    retained: &mut usize,
    limits: ResponseCaptureLimits,
) -> Result<CapturedResponse> {
    let complete_bytes = body.len();
    let aggregate =
        limits.aggregate().as_usize().map_err(|error| VoidCrawlError::Other(error.to_string()))?;
    let per_response = limits
        .per_response()
        .as_usize()
        .map_err(|error| VoidCrawlError::Other(error.to_string()))?;
    let remaining = aggregate.saturating_sub(*retained);
    let keep = body.len().min(per_response).min(remaining);
    let truncated = keep < body.len();
    body.truncate(keep);
    *retained += keep;
    Ok(CapturedResponse {
        url: meta.url,
        status: meta.status,
        headers: meta.headers,
        request_headers: meta.request_headers,
        mime_type: meta.mime_type,
        resource_type: meta.resource_type,
        from_cache: meta.from_cache,
        from_service_worker: meta.from_service_worker,
        body_state: if truncated {
            ResponseBodyState::Truncated
        } else {
            ResponseBodyState::Available
        },
        body_error: None,
        body: Arc::from(body),
        limits,
        complete_bytes: Some(complete_bytes),
    })
}

fn unavailable_response(meta: PendingResponse, error: String) -> CapturedResponse {
    CapturedResponse {
        url:                 meta.url,
        status:              meta.status,
        headers:             meta.headers,
        request_headers:     meta.request_headers,
        mime_type:           meta.mime_type,
        resource_type:       meta.resource_type,
        from_cache:          meta.from_cache,
        from_service_worker: meta.from_service_worker,
        body_state:          ResponseBodyState::Unavailable,
        body_error:          Some(error),
        body:                Arc::from([]),
        limits:              ResponseCaptureLimits::default(),
        complete_bytes:      None,
    }
}

/// Upper bound on in-flight request ids whose headers are held. A page can
/// issue far more requests than a capture cares about, and the capture may be
/// armed for minutes, so the accumulator is bounded rather than unbounded.
const MAX_TRACKED_REQUESTS: usize = 2048;

/// Fold one event's headers into the per-request accumulator. `authoritative`
/// entries overwrite; non-authoritative ones only fill absent keys, so the raw
/// wire value always wins over an author-level or response-echoed duplicate.
fn merge_sent_headers(
    store: &mut HashMap<String, HashMap<String, String>>,
    request_id: &str,
    incoming: Vec<(String, String)>,
    authoritative: bool,
) {
    if incoming.is_empty() {
        return;
    }
    // Don't let an unmatched flood of requests grow the map without limit;
    // ids already tracked keep updating.
    if store.len() >= MAX_TRACKED_REQUESTS && !store.contains_key(request_id) {
        return;
    }
    let entry = store.entry(request_id.to_string()).or_default();
    for (name, value) in incoming {
        if authoritative {
            entry.insert(name, value);
        } else {
            entry.entry(name).or_insert(value);
        }
    }
}

/// Remove and return one request's accumulated headers, sorted by name for a
/// deterministic order.
fn take_sent_headers(
    store: &mut HashMap<String, HashMap<String, String>>,
    request_id: &str,
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> =
        store.remove(request_id).map(|m| m.into_iter().collect()).unwrap_or_default();
    headers.sort_by(|a, b| a.0.cmp(&b.0));
    headers
}

/// Flatten CDP's optional `requestHeaders`. Chrome omits the field for some
/// requests (notably cache hits and certain service-worker paths), which is
/// reported as an empty vec rather than an error — absence of observed request
/// headers is normal, not a failure.
fn optional_headers(headers: Option<&Headers>) -> Vec<(String, String)> {
    headers.map(|h| flatten_headers(h.inner())).unwrap_or_default()
}

fn flatten_headers(value: &serde_json::Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.to_lowercase(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn pending() -> PendingResponse {
        PendingResponse {
            names:               vec!["response".into()],
            url:                 "https://example.test/api".into(),
            status:              200,
            headers:             vec![],
            request_headers:     vec![],
            mime_type:           "application/json".into(),
            resource_type:       "xhr".into(),
            from_cache:          false,
            from_service_worker: false,
        }
    }

    #[test]
    fn per_response_limit_is_explicit() {
        let mut retained = 0;
        let response = bounded_response(
            pending(),
            vec![1, 2, 3, 4],
            &mut retained,
            ResponseCaptureLimits::new(
                BrowserByteLimit::try_from(2_u64).expect("positive"),
                BrowserByteLimit::try_from(8_u64).expect("positive"),
            ),
        )
        .expect("limits originate from usize values");
        assert_eq!(response.body(), &[1, 2]);
        assert_eq!(response.body_state, ResponseBodyState::Truncated);
    }

    #[test]
    fn absent_request_headers_are_empty_not_an_error() {
        // Chrome omits `Network.Response.requestHeaders` for some requests
        // (cache hits, certain service-worker paths). That must degrade to an
        // empty vec, never a panic or a spurious failure.
        assert!(optional_headers(None).is_empty());
    }

    #[test]
    fn request_headers_are_lowercased_and_carried_onto_the_capture() {
        // Case-normalization matters: the MCP layer's redaction matches on
        // lowercase names, so a `Authorization` header from the wire must
        // arrive here as `authorization` or redaction silently misses it.
        let headers = Headers::new(serde_json::json!({
            "Authorization": "Bearer secret-token",
            "Accept": "application/json",
        }));
        let flattened = optional_headers(Some(&headers));
        assert!(flattened.contains(&("authorization".into(), "Bearer secret-token".into())));

        let mut meta = pending();
        meta.request_headers = flattened;
        let mut retained = 0;
        let captured =
            bounded_response(meta, vec![], &mut retained, ResponseCaptureLimits::default())
                .expect("default limits are representable");
        assert!(
            captured
                .request_headers
                .iter()
                .any(|(k, v)| k == "authorization" && v == "Bearer secret-token")
        );
    }

    #[test]
    fn raw_wire_headers_win_over_author_level_ones() {
        // requestWillBeSent may report an author-level value that the network
        // stack then rewrites; the ExtraInfo (wire) value is the truth.
        let mut store = HashMap::new();
        merge_sent_headers(
            &mut store,
            "req-1",
            vec![("accept".into(), "*/*".into()), ("cookie".into(), "stale=1".into())],
            false,
        );
        merge_sent_headers(&mut store, "req-1", vec![("cookie".into(), "real=2".into())], true);
        let headers = take_sent_headers(&mut store, "req-1");
        assert_eq!(
            headers,
            vec![("accept".to_string(), "*/*".to_string()), ("cookie".into(), "real=2".into())]
        );
    }

    #[test]
    fn a_non_authoritative_source_never_clobbers_a_wire_value() {
        // Response.requestHeaders is merged last but must not overwrite.
        let mut store = HashMap::new();
        merge_sent_headers(&mut store, "req-1", vec![("cookie".into(), "real=2".into())], true);
        merge_sent_headers(&mut store, "req-1", vec![("cookie".into(), "echo=3".into())], false);
        assert_eq!(take_sent_headers(&mut store, "req-1")[0].1, "real=2");
    }

    #[test]
    fn taking_headers_frees_the_slot() {
        // The accumulator is bounded, so a captured response must release its
        // entry rather than retain it for the capture's lifetime.
        let mut store = HashMap::new();
        merge_sent_headers(&mut store, "req-1", vec![("accept".into(), "*/*".into())], true);
        assert_eq!(take_sent_headers(&mut store, "req-1").len(), 1);
        assert!(store.is_empty());
        // A second take is empty, not a panic.
        assert!(take_sent_headers(&mut store, "req-1").is_empty());
    }

    #[test]
    fn tracking_is_bounded_but_keeps_updating_known_ids() {
        let mut store = HashMap::new();
        for i in 0..MAX_TRACKED_REQUESTS {
            merge_sent_headers(
                &mut store,
                &format!("req-{i}"),
                vec![("a".into(), "1".into())],
                true,
            );
        }
        assert_eq!(store.len(), MAX_TRACKED_REQUESTS);
        // A brand-new id past the cap is dropped…
        merge_sent_headers(&mut store, "overflow", vec![("a".into(), "1".into())], true);
        assert_eq!(store.len(), MAX_TRACKED_REQUESTS);
        assert!(take_sent_headers(&mut store, "overflow").is_empty());
        // …but an already-tracked id still accepts its wire headers.
        merge_sent_headers(&mut store, "req-0", vec![("cookie".into(), "real=1".into())], true);
        assert!(take_sent_headers(&mut store, "req-0").iter().any(|(k, _)| k == "cookie"));
    }

    #[test]
    fn empty_incoming_headers_do_not_create_an_entry() {
        let mut store = HashMap::new();
        merge_sent_headers(&mut store, "req-1", vec![], true);
        assert!(store.is_empty());
    }

    #[test]
    fn total_limit_is_shared() {
        let mut retained = 3;
        let response = bounded_response(
            pending(),
            vec![1, 2, 3, 4],
            &mut retained,
            ResponseCaptureLimits::new(
                BrowserByteLimit::try_from(8_u64).expect("positive"),
                BrowserByteLimit::try_from(5_u64).expect("positive"),
            ),
        )
        .expect("limits originate from usize values");
        assert_eq!(response.body(), &[1, 2]);
        assert_eq!(retained, 5);
    }
}
