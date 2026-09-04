//! Pre-navigation, bounded browser observation scope.
//!
//! The scope records only secret-safe lifecycle markers. Artifact payloads
//! such as response bodies, console text, and exception details belong to
//! focused collectors layered on this lifecycle in later work.

use std::{
    collections::HashSet,
    fmt, mem, str,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chromiumoxide::{
    Page as CdpPage,
    cdp::{
        IntoEventKind,
        browser_protocol::network::{
            EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent,
            EventResponseReceived, ResourceType,
        },
        js_protocol::runtime::{EventConsoleApiCalled, EventExceptionThrown},
    },
    listeners::EventStream,
};
use futures::StreamExt;
use serde::Serialize;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time,
};

use crate::{Result, VoidCrawlError};

/// Collectors and hard bounds for one observation scope.
#[derive(Debug, Clone, Copy)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag selects an independent concrete CDP collector"
)]
pub struct ObservationOptions {
    pub collect_network:      bool,
    pub collect_console:      bool,
    pub collect_exceptions:   bool,
    pub max_events:           usize,
    pub max_diagnostic_bytes: usize,
    pub max_duration:         Duration,
}

impl Default for ObservationOptions {
    fn default() -> Self {
        Self {
            collect_network:      true,
            collect_console:      true,
            collect_exceptions:   true,
            max_events:           2_048,
            max_diagnostic_bytes: 64 * 1024,
            max_duration:         Duration::from_secs(30),
        }
    }
}

impl ObservationOptions {
    pub(crate) fn validate(self) -> Result<Self> {
        if !(self.collect_network || self.collect_console || self.collect_exceptions) {
            return Err(VoidCrawlError::InvalidInput {
                operation: "observation_scope",
                reason:    "at least one collector is required",
            });
        }
        if self.max_events == 0 || self.max_diagnostic_bytes == 0 {
            return Err(VoidCrawlError::InvalidInput {
                operation: "observation_scope",
                reason:    "event and diagnostic-byte limits must be positive",
            });
        }
        if self.max_duration.is_zero() {
            return Err(VoidCrawlError::InvalidInput {
                operation: "observation_scope",
                reason:    "duration must be positive",
            });
        }
        Ok(self)
    }
}

/// Secret-safe marker for one observed CDP lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationEventKind {
    DocumentRequestStarted,
    ResourceRequestStarted,
    ResponseReceived,
    RequestFinished,
    RequestFailed,
    ConsoleApiCalled,
    RuntimeExceptionThrown,
}

/// Protected runtime text omitted from serialization and default formatting.
#[derive(Clone, PartialEq, Eq)]
pub struct ProtectedDiagnosticText(Arc<[u8]>);

impl ProtectedDiagnosticText {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn text(&self) -> Option<&str> {
        str::from_utf8(&self.0).ok()
    }
}

impl fmt::Debug for ProtectedDiagnosticText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtectedDiagnosticText")
            .field("retained_bytes", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Kind of bounded runtime diagnostic payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeDiagnosticKind {
    Console { level: String },
    Exception,
}

/// One protected console or exception payload associated with an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeDiagnostic {
    pub event_sequence: u64,
    pub kind:           RuntimeDiagnosticKind,
    pub retained_bytes: usize,
    pub complete_bytes: usize,
    pub truncated:      bool,
    #[serde(skip)]
    text:               ProtectedDiagnosticText,
}

impl RuntimeDiagnostic {
    pub fn text(&self) -> &ProtectedDiagnosticText {
        &self.text
    }
}

/// One retained event in provider receipt order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ObservationEvent {
    pub sequence:      u64,
    pub offset_micros: u64,
    pub kind:          ObservationEventKind,
}

/// Why an observation scope stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationTermination {
    Finished,
    Cancelled,
    Interrupted,
    DeadlineReached,
    EventLimitReached,
    ProviderDisconnected,
}

/// A measured count or an explicit unavailable result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MeasuredCount {
    Known { value: u64 },
    Unavailable { reason: MeasurementUnavailableReason },
}

/// Why a count could not be reported honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementUnavailableReason {
    NotCollected,
    ProviderDidNotReport,
}

/// Admission, retention, and loss facts for one measurement class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ObservationCountAccounting {
    pub admitted: MeasuredCount,
    pub retained: MeasuredCount,
    pub dropped:  MeasuredCount,
}

/// Terminal accounting for one provider observation scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ObservationAccounting {
    pub events:             ObservationCountAccounting,
    pub bytes:              ObservationCountAccounting,
    pub in_flight_requests: MeasuredCount,
}

/// Owned, secret-safe terminal result of one observation scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservationReport {
    pub started_at_unix_ms:        Option<u64>,
    pub elapsed_micros:            u64,
    pub termination:               ObservationTermination,
    pub events:                    Vec<ObservationEvent>,
    pub diagnostics:               Vec<RuntimeDiagnostic>,
    pub diagnostic_bytes_retained: usize,
    pub diagnostic_bytes_dropped:  usize,
    pub accounting:                ObservationAccounting,
    pub cleanup_complete:          bool,
}

/// An armed observation that owns all listener and collector tasks.
///
/// Dropping the scope, including by cancelling a future that owns it, aborts
/// the coordinator. Its task guard then aborts every collector so listeners
/// cannot outlive the scope.
pub struct ObservationScope {
    stop:   Option<oneshot::Sender<StopRequest>>,
    worker: Option<JoinHandle<ObservationReport>>,
}

impl fmt::Debug for ObservationScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ObservationScope").finish_non_exhaustive()
    }
}

impl ObservationScope {
    pub(crate) async fn arm(page: CdpPage, options: ObservationOptions) -> Result<Self> {
        let options = options.validate()?;
        let started = Instant::now();
        let started_at_unix_ms = unix_millis(SystemTime::now());
        let capacity = options.max_events.clamp(1, 1_024);
        let (event_tx, event_rx) = mpsc::channel(capacity);
        let mut streams = RegisteredStreams::new();

        if options.collect_network {
            streams.requests = Some(
                page.event_listener::<EventRequestWillBeSent>()
                    .await
                    .map_err(|error| VoidCrawlError::PageError(error.to_string()))?,
            );
            streams.responses = Some(
                page.event_listener::<EventResponseReceived>()
                    .await
                    .map_err(|error| VoidCrawlError::PageError(error.to_string()))?,
            );
            streams.finished = Some(
                page.event_listener::<EventLoadingFinished>()
                    .await
                    .map_err(|error| VoidCrawlError::PageError(error.to_string()))?,
            );
            streams.failed = Some(
                page.event_listener::<EventLoadingFailed>()
                    .await
                    .map_err(|error| VoidCrawlError::PageError(error.to_string()))?,
            );
        }
        if options.collect_console {
            streams.console = Some(
                page.event_listener::<EventConsoleApiCalled>()
                    .await
                    .map_err(|error| VoidCrawlError::PageError(error.to_string()))?,
            );
        }
        if options.collect_exceptions {
            streams.exceptions = Some(
                page.event_listener::<EventExceptionThrown>()
                    .await
                    .map_err(|error| VoidCrawlError::PageError(error.to_string()))?,
            );
        }

        let collectors = streams.spawn(event_tx);
        let (stop, stop_rx) = oneshot::channel();
        let worker = tokio::spawn(run_scope(
            event_rx,
            stop_rx,
            collectors,
            options,
            started,
            started_at_unix_ms,
        ));
        Ok(Self { stop: Some(stop), worker: Some(worker) })
    }

    /// Stop normally and return every event retained before the stop won.
    pub async fn finish(self) -> Result<ObservationReport> {
        self.stop_with(StopRequest::Finished).await
    }

    /// Stop because the caller cancelled the operation explicitly.
    pub async fn cancel(self) -> Result<ObservationReport> {
        self.stop_with(StopRequest::Cancelled).await
    }

    /// Stop because an explicit interruption parked the page.
    pub async fn interrupt(self) -> Result<ObservationReport> {
        self.stop_with(StopRequest::Interrupted).await
    }

    /// Wait for a deadline, event limit, or provider disconnect.
    pub async fn wait(mut self) -> Result<ObservationReport> {
        self.join_worker().await
    }

    async fn stop_with(mut self, stop: StopRequest) -> Result<ObservationReport> {
        if let Some(sender) = self.stop.take() {
            let _ = sender.send(stop);
        }
        self.join_worker().await
    }

    async fn join_worker(&mut self) -> Result<ObservationReport> {
        let worker = self
            .worker
            .take()
            .ok_or_else(|| VoidCrawlError::Other("observation scope already consumed".into()))?;
        worker
            .await
            .map_err(|error| VoidCrawlError::Other(format!("observation worker failed: {error}")))
    }
}

impl Drop for ObservationScope {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum StopRequest {
    Finished,
    Cancelled,
    Interrupted,
}

struct RegisteredStreams {
    requests:   Option<EventStream<EventRequestWillBeSent>>,
    responses:  Option<EventStream<EventResponseReceived>>,
    finished:   Option<EventStream<EventLoadingFinished>>,
    failed:     Option<EventStream<EventLoadingFailed>>,
    console:    Option<EventStream<EventConsoleApiCalled>>,
    exceptions: Option<EventStream<EventExceptionThrown>>,
}

impl RegisteredStreams {
    const fn new() -> Self {
        Self {
            requests:   None,
            responses:  None,
            finished:   None,
            failed:     None,
            console:    None,
            exceptions: None,
        }
    }

    fn spawn(mut self, sender: mpsc::Sender<RawSignal>) -> CollectorTasks {
        let mut tasks = Vec::new();
        if let Some(stream) = self.requests.take() {
            tasks.push(spawn_requests(stream, sender.clone()));
        }
        if let Some(stream) = self.responses.take() {
            tasks.push(spawn_simple(
                stream,
                sender.clone(),
                ObservationEventKind::ResponseReceived,
                |_| None,
            ));
        }
        if let Some(stream) = self.finished.take() {
            tasks.push(spawn_simple(
                stream,
                sender.clone(),
                ObservationEventKind::RequestFinished,
                |event: &EventLoadingFinished| {
                    Some(RequestTransition::Finished(event.request_id.inner().clone()))
                },
            ));
        }
        if let Some(stream) = self.failed.take() {
            tasks.push(spawn_simple(
                stream,
                sender.clone(),
                ObservationEventKind::RequestFailed,
                |event: &EventLoadingFailed| {
                    Some(RequestTransition::Finished(event.request_id.inner().clone()))
                },
            ));
        }
        if let Some(stream) = self.console.take() {
            tasks.push(spawn_console(stream, sender.clone()));
        }
        if let Some(stream) = self.exceptions.take() {
            tasks.push(spawn_exceptions(stream, sender));
        }
        CollectorTasks(tasks)
    }
}

struct CollectorTasks(Vec<JoinHandle<()>>);

impl CollectorTasks {
    async fn shutdown(&mut self) {
        for task in &self.0 {
            task.abort();
        }
        for task in mem::take(&mut self.0) {
            let _ = task.await;
        }
    }
}

impl Drop for CollectorTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

#[derive(Debug)]
enum RawSignal {
    Event {
        kind:       ObservationEventKind,
        request:    Option<RequestTransition>,
        diagnostic: Option<RawDiagnostic>,
    },
    StreamClosed,
}

#[derive(Debug)]
enum RequestTransition {
    Started(String),
    Finished(String),
}

#[derive(Debug)]
struct RawDiagnostic {
    kind: RuntimeDiagnosticKind,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetainOutcome {
    Continue,
    EventLimitReached,
    StreamClosed,
}

#[expect(clippy::too_many_arguments, reason = "bounded collector accounting is kept explicit")]
fn retain_signal(
    signal: RawSignal,
    events: &mut Vec<ObservationEvent>,
    in_flight: &mut HashSet<String>,
    max_events: usize,
    offset_micros: u64,
    diagnostics: &mut Vec<RuntimeDiagnostic>,
    diagnostic_bytes_retained: &mut usize,
    diagnostic_bytes_dropped: &mut usize,
    max_diagnostic_bytes: usize,
) -> RetainOutcome {
    let RawSignal::Event { kind, request, diagnostic } = signal else {
        return RetainOutcome::StreamClosed;
    };
    if let Some(request) = request {
        match request {
            RequestTransition::Started(id) => {
                in_flight.insert(id);
            }
            RequestTransition::Finished(id) => {
                in_flight.remove(&id);
            }
        }
    }
    let sequence = u64::try_from(events.len()).unwrap_or(u64::MAX);
    events.push(ObservationEvent { sequence, offset_micros, kind });
    if let Some(diagnostic) = diagnostic {
        let bytes = diagnostic.text.into_bytes();
        let complete_bytes = bytes.len();
        let remaining = max_diagnostic_bytes.saturating_sub(*diagnostic_bytes_retained);
        let retained_bytes = complete_bytes.min(remaining);
        *diagnostic_bytes_retained = diagnostic_bytes_retained.saturating_add(retained_bytes);
        *diagnostic_bytes_dropped =
            diagnostic_bytes_dropped.saturating_add(complete_bytes.saturating_sub(retained_bytes));
        diagnostics.push(RuntimeDiagnostic {
            event_sequence: sequence,
            kind: diagnostic.kind,
            retained_bytes,
            complete_bytes,
            truncated: retained_bytes < complete_bytes,
            text: ProtectedDiagnosticText(Arc::from(bytes[..retained_bytes].to_vec())),
        });
    }
    if events.len() >= max_events {
        RetainOutcome::EventLimitReached
    } else {
        RetainOutcome::Continue
    }
}

async fn run_scope(
    mut events_rx: mpsc::Receiver<RawSignal>,
    mut stop_rx: oneshot::Receiver<StopRequest>,
    mut collectors: CollectorTasks,
    options: ObservationOptions,
    started: Instant,
    started_at_unix_ms: Option<u64>,
) -> ObservationReport {
    let deadline = time::sleep(options.max_duration);
    tokio::pin!(deadline);
    let mut events = Vec::with_capacity(options.max_events.min(1_024));
    let mut diagnostics = Vec::new();
    let mut diagnostic_bytes_retained = 0usize;
    let mut diagnostic_bytes_dropped = 0usize;
    let mut in_flight = HashSet::new();

    let mut termination = loop {
        tokio::select! {
            biased;
            stop = &mut stop_rx => {
                break match stop {
                    Ok(StopRequest::Finished) => ObservationTermination::Finished,
                    Ok(StopRequest::Cancelled) | Err(_) => ObservationTermination::Cancelled,
                    Ok(StopRequest::Interrupted) => ObservationTermination::Interrupted,
                };
            }
            () = &mut deadline => break ObservationTermination::DeadlineReached,
            signal = events_rx.recv() => {
                match signal {
                    Some(signal) => match retain_signal(
                        signal,
                        &mut events,
                        &mut in_flight,
                        options.max_events,
                        duration_micros(started.elapsed()),
                        &mut diagnostics,
                        &mut diagnostic_bytes_retained,
                        &mut diagnostic_bytes_dropped,
                        options.max_diagnostic_bytes,
                    ) {
                        RetainOutcome::Continue => {}
                        RetainOutcome::EventLimitReached => {
                            break ObservationTermination::EventLimitReached;
                        }
                        RetainOutcome::StreamClosed => {
                            break ObservationTermination::ProviderDisconnected;
                        }
                    },
                    None => break ObservationTermination::ProviderDisconnected,
                }
            }
        }
    };

    collectors.shutdown().await;
    while events.len() < options.max_events {
        let Ok(signal) = events_rx.try_recv() else { break };
        match retain_signal(
            signal,
            &mut events,
            &mut in_flight,
            options.max_events,
            duration_micros(started.elapsed()),
            &mut diagnostics,
            &mut diagnostic_bytes_retained,
            &mut diagnostic_bytes_dropped,
            options.max_diagnostic_bytes,
        ) {
            RetainOutcome::Continue | RetainOutcome::StreamClosed => {}
            RetainOutcome::EventLimitReached => {
                if termination == ObservationTermination::Finished {
                    termination = ObservationTermination::EventLimitReached;
                }
                break;
            }
        }
    }
    let event_count = u64::try_from(events.len()).unwrap_or(u64::MAX);
    let dropped =
        MeasuredCount::Unavailable { reason: MeasurementUnavailableReason::ProviderDidNotReport };
    ObservationReport {
        started_at_unix_ms,
        elapsed_micros: duration_micros(started.elapsed()),
        termination,
        accounting: ObservationAccounting {
            events:             ObservationCountAccounting {
                admitted: MeasuredCount::Known { value: event_count },
                retained: MeasuredCount::Known { value: event_count },
                dropped,
            },
            bytes:              ObservationCountAccounting {
                admitted: MeasuredCount::Known {
                    value: u64::try_from(
                        diagnostic_bytes_retained.saturating_add(diagnostic_bytes_dropped),
                    )
                    .unwrap_or(u64::MAX),
                },
                retained: MeasuredCount::Known {
                    value: u64::try_from(diagnostic_bytes_retained).unwrap_or(u64::MAX),
                },
                dropped:  MeasuredCount::Known {
                    value: u64::try_from(diagnostic_bytes_dropped).unwrap_or(u64::MAX),
                },
            },
            in_flight_requests: if options.collect_network {
                MeasuredCount::Known { value: u64::try_from(in_flight.len()).unwrap_or(u64::MAX) }
            } else {
                MeasuredCount::Unavailable { reason: MeasurementUnavailableReason::NotCollected }
            },
        },
        events,
        diagnostics,
        diagnostic_bytes_retained,
        diagnostic_bytes_dropped,
        cleanup_complete: true,
    }
}

fn spawn_requests(
    mut stream: EventStream<EventRequestWillBeSent>,
    sender: mpsc::Sender<RawSignal>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            let kind = if matches!(event.r#type, Some(ResourceType::Document)) {
                ObservationEventKind::DocumentRequestStarted
            } else {
                ObservationEventKind::ResourceRequestStarted
            };
            let signal = RawSignal::Event {
                kind,
                request: Some(RequestTransition::Started(event.request_id.inner().clone())),
                diagnostic: None,
            };
            if sender.send(signal).await.is_err() {
                return;
            }
        }
        let _ = sender.send(RawSignal::StreamClosed).await;
    })
}

fn spawn_simple<T, F>(
    mut stream: EventStream<T>,
    sender: mpsc::Sender<RawSignal>,
    kind: ObservationEventKind,
    transition: F,
) -> JoinHandle<()>
where
    T: IntoEventKind + Send + Sync + Unpin + 'static,
    F: Fn(&T) -> Option<RequestTransition> + Send + 'static,
{
    tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            let signal = RawSignal::Event { kind, request: transition(&event), diagnostic: None };
            if sender.send(signal).await.is_err() {
                return;
            }
        }
        let _ = sender.send(RawSignal::StreamClosed).await;
    })
}

fn spawn_console(
    mut stream: EventStream<EventConsoleApiCalled>,
    sender: mpsc::Sender<RawSignal>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            let text = event
                .args
                .iter()
                .filter_map(|argument| {
                    argument.value.as_ref().map_or_else(
                        || argument.description.clone(),
                        |value| {
                            value.as_str().map(str::to_string).or_else(|| Some(value.to_string()))
                        },
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            let signal = RawSignal::Event {
                kind:       ObservationEventKind::ConsoleApiCalled,
                request:    None,
                diagnostic: Some(RawDiagnostic {
                    kind: RuntimeDiagnosticKind::Console {
                        level: event.r#type.as_ref().to_string(),
                    },
                    text,
                }),
            };
            if sender.send(signal).await.is_err() {
                return;
            }
        }
        let _ = sender.send(RawSignal::StreamClosed).await;
    })
}

fn spawn_exceptions(
    mut stream: EventStream<EventExceptionThrown>,
    sender: mpsc::Sender<RawSignal>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            let mut text = event.exception_details.text.clone();
            if let Some(description) = event
                .exception_details
                .exception
                .as_ref()
                .and_then(|exception| exception.description.as_ref())
                && description != &text
            {
                if !text.is_empty() {
                    text.push_str(": ");
                }
                text.push_str(description);
            }
            let signal = RawSignal::Event {
                kind:       ObservationEventKind::RuntimeExceptionThrown,
                request:    None,
                diagnostic: Some(RawDiagnostic { kind: RuntimeDiagnosticKind::Exception, text }),
            };
            if sender.send(signal).await.is_err() {
                return;
            }
        }
        let _ = sender.send(RawSignal::StreamClosed).await;
    })
}

fn unix_millis(now: SystemTime) -> Option<u64> {
    now.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn options_reject_zero_bounds_and_empty_collector_set() {
        assert!(
            ObservationOptions {
                collect_network: false,
                collect_console: false,
                collect_exceptions: false,
                ..ObservationOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            ObservationOptions { max_events: 0, ..ObservationOptions::default() }
                .validate()
                .is_err()
        );
        assert!(
            ObservationOptions { max_diagnostic_bytes: 0, ..ObservationOptions::default() }
                .validate()
                .is_err()
        );
        assert!(
            ObservationOptions { max_duration: Duration::ZERO, ..ObservationOptions::default() }
                .validate()
                .is_err()
        );
    }

    #[test]
    fn report_serialization_contains_no_payload_or_runtime_handle_fields() {
        let report = ObservationReport {
            started_at_unix_ms:        Some(1),
            elapsed_micros:            2,
            termination:               ObservationTermination::Finished,
            events:                    vec![ObservationEvent {
                sequence:      0,
                offset_micros: 1,
                kind:          ObservationEventKind::RuntimeExceptionThrown,
            }],
            diagnostics:               vec![RuntimeDiagnostic {
                event_sequence: 0,
                kind:           RuntimeDiagnosticKind::Exception,
                retained_bytes: 6,
                complete_bytes: 6,
                truncated:      false,
                text:           ProtectedDiagnosticText(Arc::from(b"secret".as_slice())),
            }],
            diagnostic_bytes_retained: 6,
            diagnostic_bytes_dropped:  0,
            accounting:                ObservationAccounting {
                events:             ObservationCountAccounting {
                    admitted: MeasuredCount::Known { value: 1 },
                    retained: MeasuredCount::Known { value: 1 },
                    dropped:  MeasuredCount::Unavailable {
                        reason: MeasurementUnavailableReason::ProviderDidNotReport,
                    },
                },
                bytes:              ObservationCountAccounting {
                    admitted: MeasuredCount::Unavailable {
                        reason: MeasurementUnavailableReason::NotCollected,
                    },
                    retained: MeasuredCount::Unavailable {
                        reason: MeasurementUnavailableReason::NotCollected,
                    },
                    dropped:  MeasuredCount::Unavailable {
                        reason: MeasurementUnavailableReason::NotCollected,
                    },
                },
                in_flight_requests: MeasuredCount::Known { value: 0 },
            },
            cleanup_complete:          true,
        };
        let serialized = serde_json::to_string(&report).expect("serialize report");
        for forbidden in [
            "url",
            "message",
            "exception_text",
            "request_id",
            "headers",
            "body",
            "ws_url",
            "profile_path",
            "secret",
        ] {
            assert!(!serialized.contains(forbidden), "report leaked {forbidden}");
        }
    }
}
