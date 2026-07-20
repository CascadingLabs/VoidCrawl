//! Opt-in, passive CDP response-body capture.
//!
//! Capture scopes subscribe to Network events before the triggering action.
//! They never intercept requests or inject page-world JavaScript.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chromiumoxide::{
    Page as CdpPage,
    cdp::browser_protocol::network::{
        EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent, EventResponseReceived,
        GetResponseBodyParams,
    },
    listeners::EventStream,
};
use futures::StreamExt;
use globset::{Glob, GlobMatcher};
use tokio::{sync::oneshot, task::JoinHandle, time};

use crate::error::{Result, VoidCrawlError};

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
    pub mime_type:           String,
    pub resource_type:       String,
    pub from_cache:          bool,
    pub from_service_worker: bool,
    pub body_state:          ResponseBodyState,
    pub body_error:          Option<String>,
    body:                    Arc<[u8]>,
}

impl CapturedResponse {
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
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
    pub max_response_bytes: usize,
    pub max_total_bytes:    usize,
}

impl Default for ResponseCaptureLimits {
    fn default() -> Self {
        Self {
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            max_total_bytes:    DEFAULT_MAX_TOTAL_RESPONSE_BYTES,
        }
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
        if limits.max_response_bytes == 0 || limits.max_total_bytes == 0 {
            return Err(VoidCrawlError::Other("response byte limits must be positive".into()));
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
                pending.insert(request_id, PendingResponse {
                    names,
                    url: event.response.url.clone(),
                    status: u16::try_from(event.response.status).unwrap_or_default(),
                    headers: flatten_headers(event.response.headers.inner()),
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
                let Some(meta) = pending.remove(&request_id) else { continue };
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
                        bounded_response(meta.clone(), decoded, &mut retained, limits)
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
                let Some(meta) = pending.remove(&request_id) else { continue };
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
) -> CapturedResponse {
    let remaining = limits.max_total_bytes.saturating_sub(*retained);
    let keep = body.len().min(limits.max_response_bytes).min(remaining);
    let truncated = keep < body.len();
    body.truncate(keep);
    *retained += keep;
    CapturedResponse {
        url:                 meta.url,
        status:              meta.status,
        headers:             meta.headers,
        mime_type:           meta.mime_type,
        resource_type:       meta.resource_type,
        from_cache:          meta.from_cache,
        from_service_worker: meta.from_service_worker,
        body_state:          if truncated {
            ResponseBodyState::Truncated
        } else {
            ResponseBodyState::Available
        },
        body_error:          None,
        body:                Arc::from(body),
    }
}

fn unavailable_response(meta: PendingResponse, error: String) -> CapturedResponse {
    CapturedResponse {
        url:                 meta.url,
        status:              meta.status,
        headers:             meta.headers,
        mime_type:           meta.mime_type,
        resource_type:       meta.resource_type,
        from_cache:          meta.from_cache,
        from_service_worker: meta.from_service_worker,
        body_state:          ResponseBodyState::Unavailable,
        body_error:          Some(error),
        body:                Arc::from([]),
    }
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
mod tests {
    use super::*;

    fn pending() -> PendingResponse {
        PendingResponse {
            names:               vec!["response".into()],
            url:                 "https://example.test/api".into(),
            status:              200,
            headers:             vec![],
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
            ResponseCaptureLimits { max_response_bytes: 2, max_total_bytes: 8 },
        );
        assert_eq!(response.body(), &[1, 2]);
        assert_eq!(response.body_state, ResponseBodyState::Truncated);
    }

    #[test]
    fn total_limit_is_shared() {
        let mut retained = 3;
        let response = bounded_response(
            pending(),
            vec![1, 2, 3, 4],
            &mut retained,
            ResponseCaptureLimits { max_response_bytes: 8, max_total_bytes: 5 },
        );
        assert_eq!(response.body(), &[1, 2]);
        assert_eq!(retained, 5);
    }
}
