use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use chrono::Utc;
use serde::Serialize;

use super::ack::{AckEntry, AckStore};
use super::detection_ledger::DetectionLedger;
// The event loop's clock, not a second one: `push_batch` stamps findings with
// it and the hourly re-send suppression compares against those stamps.
use super::event_loop::current_time_ms as unix_time_ms;
use super::findings_store::StoredFinding;
use super::hub_correlation_export::{
    CorrelationExport, CorrelationExportBuffer, correlations_endpoint,
};
use super::hub_detection_export::{DetectionExport, detections_endpoint};
use super::hub_incident_export::{
    IncidentExport, IncidentExportBuffer, incident_capacity, incidents_endpoint,
};
use super::query_api::{FindingResponse, lookup_ack};
use crate::config::DaemonHubExportConfig;
use crate::detect::Finding;
use crate::detect::correlate_cross::{CorrelationConfig, CrossTraceCorrelator};
use crate::http_client;
use crate::report::metrics::MetricsState;

/// The acknowledgment sources the query API reads, shared with the exporter so
/// a pushed envelope carries the same `acknowledged_by` as a polled one.
struct AckSources {
    toml: Arc<crate::daemon::ack_toml_state::AckTomlState>,
    store: Option<Arc<AckStore>>,
}

impl AckSources {
    async fn snapshot(&self) -> Arc<HashMap<String, AckEntry>> {
        match &self.store {
            Some(store) => store.snapshot_active().await,
            None => Arc::new(HashMap::new()),
        }
    }
}

const MAX_EXPORT_BODY_BYTES: usize = 2 * 1024 * 1024;
const EXPORT_REFRESH_INTERVAL_MS: u64 = 60 * 60 * 1_000;
/// Backoff ceiling for a busy Hub: `retry_delay(3, _)` stays within a few
/// seconds, matching the `Retry-After: 1` the Hub sends with its 503.
const BUSY_FAILURE_CAP: u32 = 3;

#[derive(Clone)]
struct PendingExport {
    finding: StoredFinding,
    revision: u64,
}

struct SentExport {
    sent_at_ms: u64,
    severity: crate::detect::Severity,
    revision: u64,
}

#[derive(Default)]
struct PendingState {
    entries: HashMap<String, PendingExport>,
    order: VecDeque<(String, u64)>,
    sent_entries: HashMap<String, SentExport>,
    sent_order: VecDeque<(String, u64)>,
    next_revision: u64,
}

pub(super) struct HubExportBuffer {
    inner: Mutex<PendingState>,
    max_pending: usize,
}

pub(super) struct HubExporter {
    buffer: Arc<HubExportBuffer>,
    incident_buffer: Option<Arc<IncidentExportBuffer>>,
    correlation_buffer: Option<Arc<CorrelationExportBuffer>>,
    handle: tokio::task::JoinHandle<()>,
    shutdown: Arc<tokio::sync::Notify>,
}

/// How long a graceful shutdown waits for the exporter to flush what it
/// still holds.
///
/// Bounded because an unreachable Hub must not hold the daemon past
/// the orchestrator's grace period, where the next signal is SIGKILL and
/// nothing gets flushed at all. Ten seconds is one full request timeout
/// plus room for the retry the flush may need, at the default
/// `request_timeout_secs`: a longer timeout can leave a request cut off here.
const SHUTDOWN_DRAIN_BUDGET: Duration = Duration::from_secs(10);

impl HubExporter {
    #[allow(clippy::too_many_arguments)] // one optional half per daemon section it pushes
    pub(super) fn spawn(
        config: &DaemonHubExportConfig,
        metrics: Arc<MetricsState>,
        toml_acks: Arc<crate::daemon::ack_toml_state::AckTomlState>,
        ack_store: Option<Arc<AckStore>>,
        push_incidents: bool,
        correlator: Option<Arc<tokio::sync::Mutex<CrossTraceCorrelator>>>,
        correlation: &CorrelationConfig,
        detection_ledger: Option<Arc<DetectionLedger>>,
    ) -> Result<Option<Self>, super::DaemonError> {
        if !config.enabled {
            return Ok(None);
        }
        let key_path = config.api_key_file.as_deref().expect("config validation");
        let api_key = std::fs::read_to_string(key_path).map_err(|source| {
            super::DaemonError::HubExportSecretRead {
                path: key_path.to_string(),
                source,
            }
        })?;
        let api_key = api_key.trim().to_string();
        if api_key.len() < 32 || api_key.chars().any(char::is_control) {
            return Err(super::DaemonError::HubExportSecretInvalid {
                path: key_path.to_string(),
            });
        }
        let endpoint = config.endpoint.as_deref().expect("config validation");
        let source_id = config.source_id.as_deref().expect("config validation");
        let uri = source_uri(endpoint, source_id)?;
        let buffer = Arc::new(HubExportBuffer::new(config.max_pending));
        let incident_buffer = push_incidents.then(|| {
            Arc::new(IncidentExportBuffer::new(incident_capacity(
                config.max_pending,
            )))
        });
        let incidents = incident_buffer
            .as_ref()
            .map(|incidents| {
                source_uri(&incidents_endpoint(endpoint), source_id)
                    .map(|uri| IncidentExport::new(incidents.clone(), uri))
            })
            .transpose()?;
        let correlations = correlator
            .map(|correlator| {
                let table = Arc::new(CorrelationExportBuffer::new(config.max_pending));
                source_uri(&correlations_endpoint(endpoint), source_id)
                    .map(|uri| CorrelationExport::new(correlator, correlation, table, uri))
            })
            .transpose()?;
        let correlation_buffer = correlations.as_ref().map(CorrelationExport::buffer);
        let detections = detection_ledger
            .map(|ledger| {
                source_uri(&detections_endpoint(endpoint), source_id)
                    .map(|uri| DetectionExport::new(ledger, uri))
            })
            .transpose()?;
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let handle = tokio::spawn(run_exporter(
            buffer.clone(),
            shutdown.clone(),
            metrics,
            uri,
            api_key,
            config.batch_size,
            Duration::from_secs(config.flush_interval_secs),
            HubClient::new(Duration::from_secs(config.request_timeout_secs)),
            AckSources {
                toml: toml_acks,
                store: ack_store,
            },
            incidents,
            correlations,
            detections,
        ));
        Ok(Some(Self {
            buffer,
            incident_buffer,
            correlation_buffer,
            handle,
            shutdown,
        }))
    }

    pub(super) fn buffer(&self) -> Arc<HubExportBuffer> {
        self.buffer.clone()
    }

    /// The incident table, when `[daemon.incidents]` is on as well.
    pub(super) fn incident_buffer(&self) -> Option<Arc<IncidentExportBuffer>> {
        self.incident_buffer.clone()
    }

    fn pending(&self) -> usize {
        self.buffer.len()
            + self
                .incident_buffer
                .as_ref()
                .map_or(0, |incidents| incidents.len())
            + self
                .correlation_buffer
                .as_ref()
                .map_or(0, |correlations| correlations.len())
    }

    /// Flush what the exporter still holds, then stop it.
    ///
    /// Without this call, dropping the exporter aborts the task, and every
    /// pending signature dies with it, so a rolling upgrade loses each finding
    /// discovered since the last flush. Mirrors how the archive writer is
    /// drained, with a budget on top because the Hub, unlike the archive,
    /// is remote. The wait ends either when the buffer empties or when
    /// `SHUTDOWN_DRAIN_BUDGET` runs out, and the `Drop` below still aborts
    /// whatever is left.
    pub(super) async fn shutdown(&mut self) {
        let pending = self.pending();
        self.shutdown.notify_one();
        if tokio::time::timeout(SHUTDOWN_DRAIN_BUDGET, &mut self.handle)
            .await
            .is_err()
        {
            tracing::warn!(
                pending = self.pending(),
                budget_secs = SHUTDOWN_DRAIN_BUDGET.as_secs(),
                "Hub export drain timed out, the findings, incidents, correlation episodes and detection counts still pending are dropped"
            );
        } else if pending > 0 {
            tracing::info!(pending, "Hub export drained before shutdown");
        }
    }
}

/// A Hub import route, scoped to this daemon's source.
fn source_uri(endpoint: &str, source_id: &str) -> Result<http_client::Uri, super::DaemonError> {
    format!("{endpoint}?source_id={source_id}")
        .parse::<http_client::Uri>()
        .map_err(|_| super::DaemonError::HubExportEndpoint)
}

impl Drop for HubExporter {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl HubExportBuffer {
    pub(super) fn new(max_pending: usize) -> Self {
        Self {
            inner: Mutex::new(PendingState::default()),
            max_pending,
        }
    }

    pub(super) fn push_batch(&self, findings: &[Finding], now_ms: u64) -> u64 {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut dropped = 0;
        for finding in findings {
            if finding.signature.is_empty() {
                dropped += 1;
                continue;
            }
            if state.entries.contains_key(&finding.signature) {
                merge_pending(&mut state, finding, now_ms);
                continue;
            }
            if suppressed_by_recent_send(&state, finding, now_ms) {
                continue;
            }
            insert_pending(&mut state, finding, now_ms);
        }

        dropped + evict_overflow(&mut state, self.max_pending)
    }

    fn snapshot(&self, limit: usize) -> Vec<PendingExport> {
        let state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot_by_order(&state.entries, &state.order, limit, |entry| entry.revision)
    }

    fn acknowledge(&self, sent: &[PendingExport], acknowledged_at_ms: u64) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for item in sent {
            let signature = &item.finding.finding.signature;
            if state
                .entries
                .get(signature)
                .is_some_and(|entry| entry.revision == item.revision)
            {
                state.entries.remove(signature);
            }
            state.next_revision = state.next_revision.wrapping_add(1);
            let revision = state.next_revision;
            state.sent_entries.insert(
                signature.clone(),
                SentExport {
                    sent_at_ms: acknowledged_at_ms,
                    severity: item.finding.finding.severity.clone(),
                    revision,
                },
            );
            state.sent_order.push_back((signature.clone(), revision));
        }
        while state.sent_entries.len() > self.max_pending {
            let Some((signature, revision)) = state.sent_order.pop_front() else {
                break;
            };
            if state
                .sent_entries
                .get(&signature)
                .is_some_and(|entry| entry.revision == revision)
            {
                state.sent_entries.remove(&signature);
            }
        }
        if state.sent_order.len() > self.max_pending.saturating_mul(2) {
            let mut order = std::mem::take(&mut state.sent_order);
            order.retain(|(signature, revision)| {
                state
                    .sent_entries
                    .get(signature)
                    .is_some_and(|entry| entry.revision == *revision)
            });
            state.sent_order = order;
        }
    }

    pub(super) fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .len()
    }
}

/// Fold a repeat of an already-pending signature into its entry.
///
/// `seen_count` and `first_seen_ms` accumulate the way the findings store
/// folds at read time, so a hot finding is not exported as a single
/// detection. The revision only moves when the severity worsens: an equal
/// repeat that lands while the batch is in flight must still be cleared by
/// its acknowledgment, or the exporter re-sends the same batch every round
/// trip instead of honouring the flush interval. The trade is that such a
/// repeat refreshes the payload the acknowledgment then drops, so the Hub
/// keeps the snapshot's trace id, never one newer than the send.
fn merge_pending(state: &mut PendingState, finding: &Finding, now_ms: u64) {
    // Severity is ordered Critical < Warning < Info: strictly smaller is worse.
    let worsened = state
        .entries
        .get(&finding.signature)
        .is_some_and(|pending| finding.severity < pending.finding.finding.severity);
    let revision = if worsened {
        state.next_revision = state.next_revision.wrapping_add(1);
        Some(state.next_revision)
    } else {
        None
    };
    if let Some(pending) = state.entries.get_mut(&finding.signature) {
        pending.finding.seen_count = pending.finding.seen_count.saturating_add(1);
        pending.finding.first_seen_ms = pending.finding.first_seen_ms.min(now_ms);
        pending.finding.stored_at_ms = pending.finding.stored_at_ms.max(now_ms);
        if finding.severity <= pending.finding.finding.severity {
            pending.finding.finding = finding.clone();
        }
        if let Some(revision) = revision {
            pending.revision = revision;
        }
    }
    if let Some(revision) = revision {
        state.order.push_back((finding.signature.clone(), revision));
    }
}

fn suppressed_by_recent_send(state: &PendingState, finding: &Finding, now_ms: u64) -> bool {
    state
        .sent_entries
        .get(&finding.signature)
        .is_some_and(|sent| {
            now_ms.saturating_sub(sent.sent_at_ms) < EXPORT_REFRESH_INTERVAL_MS
                && finding.severity >= sent.severity
        })
}

fn insert_pending(state: &mut PendingState, finding: &Finding, now_ms: u64) {
    state.next_revision = state.next_revision.wrapping_add(1);
    let revision = state.next_revision;
    let signature = finding.signature.clone();
    state.sent_entries.remove(&signature);
    state.entries.insert(
        signature.clone(),
        PendingExport {
            finding: StoredFinding {
                finding: finding.clone(),
                stored_at_ms: now_ms,
                first_seen_ms: now_ms,
                seen_count: 1,
            },
            revision,
        },
    );
    state.order.push_back((signature, revision));
}

fn evict_overflow(state: &mut PendingState, max_pending: usize) -> u64 {
    evict_by_order(&mut state.entries, &mut state.order, max_pending, |entry| {
        entry.revision
    })
}

/// Up to `limit` entries in queueing order, each at its latest place: an
/// `order` item whose revision no longer matches its entry is skipped.
pub(super) fn snapshot_by_order<V: Clone>(
    entries: &HashMap<String, V>,
    order: &VecDeque<(String, u64)>,
    limit: usize,
    revision: impl Fn(&V) -> u64,
) -> Vec<V> {
    order
        .iter()
        .filter_map(|(key, queued)| entries.get(key).filter(|entry| revision(entry) == *queued))
        .take(limit)
        .cloned()
        .collect()
}

/// Drop the least recently queued entries until `entries` fits `max`, then
/// compact `order` once stale revisions make it twice the cap. An `order`
/// item whose revision no longer matches its entry is a superseded queueing
/// and is skipped, not counted. Returns how many entries were dropped.
pub(super) fn evict_by_order<V>(
    entries: &mut HashMap<String, V>,
    order: &mut VecDeque<(String, u64)>,
    max: usize,
    revision: impl Fn(&V) -> u64,
) -> u64 {
    let mut dropped = 0;
    while entries.len() > max {
        let Some((key, queued)) = order.pop_front() else {
            break;
        };
        if entries
            .get(&key)
            .is_some_and(|entry| revision(entry) == queued)
        {
            entries.remove(&key);
            dropped += 1;
        }
    }
    if order.len() > max.saturating_mul(2) {
        order.retain(|(key, queued)| {
            entries
                .get(key)
                .is_some_and(|entry| revision(entry) == *queued)
        });
    }
    dropped
}

#[derive(Serialize)]
struct ExportPayload<'a> {
    producer_version: &'static str,
    findings: &'a [FindingResponse],
}

/// What the Hub's answer means for the batch that produced it.
pub(super) enum ExportOutcome {
    Accepted,
    /// A rejection no retry can change: the payload or the route is wrong.
    /// Retaining the batch would retry it forever while live findings are
    /// evicted from the bounded buffer.
    Rejected(u16),
    /// Advisory backpressure from a Hub that is busy writing. Retry soon
    /// rather than escalating to the multi-minute ceiling.
    Busy,
    /// Transient: a network error, a 5xx, or a credential the operator can
    /// still fix. Keep the batch and back off.
    Retry(String),
}

#[allow(clippy::too_many_arguments)] // one task, every dependency injected for the tests
async fn run_exporter(
    buffer: Arc<HubExportBuffer>,
    shutdown: Arc<tokio::sync::Notify>,
    metrics: Arc<MetricsState>,
    uri: http_client::Uri,
    api_key: String,
    batch_size: usize,
    flush_interval: Duration,
    client: HubClient,
    acks: AckSources,
    incidents: Option<IncidentExport>,
    correlations: Option<CorrelationExport>,
    detections: Option<DetectionExport>,
) {
    let start = tokio::time::Instant::now();
    let mut halves = Halves {
        findings: FindingsExport {
            buffer,
            uri,
            batch_size,
            acks,
            failures: 0,
            due: start,
        },
        incidents,
        correlations,
        detections,
    };
    halves.start_at(start + flush_interval);
    // Once shutdown fires the loop stops waiting for any half's next round
    // and flushes until every table empties. It never returns on a table
    // it cannot send: `HubExporter::shutdown` owns the time budget and
    // aborts, so the retry policy stays the same on both paths.
    let mut draining = false;
    loop {
        // Each half keeps its own pace: one's backoff is not cut short by
        // another's flush, nor stretched by it.
        let wake = halves.wake();
        if draining {
            // Still yield between attempts, otherwise an unreachable Hub
            // turns the drain budget into a spin.
            let yield_at = tokio::time::Instant::now() + Duration::from_millis(250);
            tokio::time::sleep_until(wake.min(yield_at)).await;
        } else {
            tokio::select! {
                () = tokio::time::sleep_until(wake) => {}
                () = shutdown.notified() => draining = true,
            }
        }
        let sent = halves
            .round(&client, &api_key, &metrics, flush_interval, draining)
            .await;
        if draining && !sent {
            return;
        }
    }
}

/// The exporter task's halves, findings always, incidents, correlations
/// and detections when configured.
struct Halves {
    findings: FindingsExport,
    incidents: Option<IncidentExport>,
    correlations: Option<CorrelationExport>,
    detections: Option<DetectionExport>,
}

impl Halves {
    fn start_at(&mut self, at: tokio::time::Instant) {
        self.findings.due = at;
        if let Some(export) = self.incidents.as_mut() {
            export.due = at;
        }
        if let Some(export) = self.correlations.as_mut() {
            export.start_at(at);
        }
        if let Some(export) = self.detections.as_mut() {
            export.start_at(at);
        }
    }

    /// The earliest of the halves' next rounds.
    fn wake(&self) -> tokio::time::Instant {
        let mut wake = self.findings.due;
        if let Some(export) = &self.incidents {
            wake = wake.min(export.due);
        }
        if let Some(export) = &self.correlations {
            wake = wake.min(export.due());
        }
        if let Some(export) = &self.detections {
            wake = wake.min(export.due());
        }
        wake
    }

    /// One round of every half that is due, of all of them while draining.
    /// True when one of them sent a batch.
    async fn round(
        &mut self,
        client: &HubClient,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
        draining: bool,
    ) -> bool {
        let now = tokio::time::Instant::now();
        let mut sent = false;
        if draining || self.findings.due <= now {
            sent |= self
                .findings
                .round(client, api_key, metrics, flush_interval)
                .await;
        }
        if let Some(export) = self.incidents.as_mut()
            && (draining || export.due <= now)
        {
            sent |= export.round(client, api_key, metrics, flush_interval).await;
        }
        if let Some(export) = self.correlations.as_mut() {
            sent |= export
                .round(client, api_key, metrics, flush_interval, draining)
                .await;
        }
        if let Some(export) = self.detections.as_mut() {
            sent |= export
                .round(client, api_key, metrics, flush_interval, draining)
                .await;
        }
        sent
    }
}

/// The exporter task's findings half, with its own failure count and pace.
struct FindingsExport {
    buffer: Arc<HubExportBuffer>,
    uri: http_client::Uri,
    batch_size: usize,
    acks: AckSources,
    failures: u32,
    due: tokio::time::Instant,
}

impl FindingsExport {
    /// One findings batch, then the time of the next. False when nothing
    /// was pending.
    async fn round(
        &mut self,
        client: &HubClient,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
    ) -> bool {
        let mut batch = self.buffer.snapshot(self.batch_size);
        if batch.is_empty() {
            self.due = tokio::time::Instant::now() + flush_interval;
            return false;
        }
        let mut annotated = annotate(&batch, &self.acks).await;
        let Some(body) = bounded_payload(&mut annotated) else {
            self.buffer.acknowledge(&batch[..1], unix_time_ms());
            metrics.hub_export_dropped_total.inc();
            metrics.hub_export_pending.set(self.buffer.len() as f64);
            self.due = tokio::time::Instant::now();
            return true;
        };
        // bounded_payload only pops from the end, so truncating keeps the
        // acknowledgment aligned with what went over the wire.
        batch.truncate(annotated.len());
        let outcome = match post_batch(client, &self.uri, api_key, body).await {
            Ok(status) => classify(status),
            Err(reason) => ExportOutcome::Retry(reason),
        };
        let delay = apply_outcome(
            &outcome,
            &batch,
            &self.buffer,
            metrics,
            &self.uri,
            flush_interval,
            &mut self.failures,
        );
        self.due = tokio::time::Instant::now() + delay;
        true
    }
}

/// The client every half of the exporter posts with, and how long one
/// request may take (`request_timeout_secs`).
pub(crate) struct HubClient {
    http: http_client::HttpClientWithBody,
    timeout: Duration,
}

impl HubClient {
    pub(crate) fn new(timeout: Duration) -> Self {
        Self {
            http: http_client::build_client_with_body(),
            timeout,
        }
    }
}

/// POST one JSON batch to a Hub import route: the status the Hub answered,
/// or the transport error.
pub(super) async fn post_batch(
    client: &HubClient,
    uri: &http_client::Uri,
    api_key: &str,
    body: Vec<u8>,
) -> Result<hyper::StatusCode, String> {
    http_client::fetch_with_body(
        &client.http,
        hyper::Method::POST,
        uri,
        "perf-sentinel-hub-export",
        client.timeout,
        Some(api_key),
        Bytes::from(body),
    )
    .await
    .map(|(status, _)| status)
    .map_err(|error| error.to_string())
}

pub(super) fn classify(status: hyper::StatusCode) -> ExportOutcome {
    if status.is_success() {
        ExportOutcome::Accepted
    } else if status == hyper::StatusCode::SERVICE_UNAVAILABLE {
        ExportOutcome::Busy
    } else if is_permanent_rejection(status) {
        ExportOutcome::Rejected(status.as_u16())
    } else {
        ExportOutcome::Retry(format!("status {}", status.as_u16()))
    }
}

/// A 4xx a retry cannot change. 401/403 stay retryable so a rotated secret
/// recovers once the operator fixes it, and 408/429 are transient by
/// definition.
fn is_permanent_rejection(status: hyper::StatusCode) -> bool {
    status.is_client_error()
        && !matches!(
            status,
            hyper::StatusCode::UNAUTHORIZED
                | hyper::StatusCode::FORBIDDEN
                | hyper::StatusCode::REQUEST_TIMEOUT
                | hyper::StatusCode::TOO_MANY_REQUESTS
        )
}

fn apply_outcome(
    outcome: &ExportOutcome,
    batch: &[PendingExport],
    buffer: &HubExportBuffer,
    metrics: &MetricsState,
    uri: &http_client::Uri,
    flush_interval: Duration,
    failures: &mut u32,
) -> Duration {
    match outcome {
        ExportOutcome::Accepted => buffer.acknowledge(batch, unix_time_ms()),
        ExportOutcome::Rejected(status) => {
            buffer.acknowledge(batch, unix_time_ms());
            metrics.hub_export_dropped_total.inc_by(batch.len() as u64);
            tracing::error!(
                endpoint = %http_client::redact_endpoint(uri),
                status = *status,
                dropped = batch.len(),
                "PerfSentinelHub rejected the batch permanently; dropping it"
            );
        }
        ExportOutcome::Busy | ExportOutcome::Retry(_) => {}
    }
    let delay = next_delay(outcome, failures, flush_interval, buffer.len() > 0);
    if let ExportOutcome::Retry(reason) = outcome {
        tracing::warn!(
            endpoint = %http_client::redact_endpoint(uri),
            reason = %reason,
            retry_seconds = delay.as_secs_f64(),
            "PerfSentinelHub export failed; retaining the coalesced batch"
        );
    }
    metrics.hub_export_pending.set(buffer.len() as f64);
    delay
}

/// Annotate each pending finding with its active acknowledgment, producing
/// the same envelope shape the daemon's own `/api/findings` serves.
async fn annotate(batch: &[PendingExport], acks: &AckSources) -> Vec<FindingResponse> {
    let daemon = acks.snapshot().await;
    // One snapshot for the whole batch, so a reload mid-annotation cannot split
    // an envelope across two revisions of the file.
    let toml = acks.toml.load();
    let now = Utc::now();
    batch
        .iter()
        .map(|item| FindingResponse {
            acknowledged_by: lookup_ack(&item.finding.finding.signature, &toml, &daemon, now),
            stored: item.finding.clone(),
        })
        .collect()
}

fn bounded_payload(batch: &mut Vec<FindingResponse>) -> Option<Vec<u8>> {
    bounded_body(batch, MAX_EXPORT_BODY_BYTES, |findings| {
        serde_json::to_vec(&ExportPayload {
            producer_version: env!("CARGO_PKG_VERSION"),
            findings,
        })
    })
}

/// Serialize the largest prefix of `batch` that fits `cap`, popping from
/// the end so the caller can keep its acknowledgment aligned.
///
/// An oversized body shrinks proportionally to how far over the cap it is,
/// not one entry at a time. Entries are within an order of magnitude of
/// each other, so this converges in a couple of passes instead of
/// re-serializing up to the batch size for one huge outlier.
pub(super) fn bounded_body<T>(
    batch: &mut Vec<T>,
    cap: usize,
    serialize: impl Fn(&[T]) -> serde_json::Result<Vec<u8>>,
) -> Option<Vec<u8>> {
    while !batch.is_empty() {
        let over_by = match serialize(batch) {
            Ok(body) if body.len() <= cap => return Some(body),
            // Serialization of owned scalars does not fail, but a poisoned
            // entry must still shrink the batch rather than spin.
            Err(_) => batch.len(),
            Ok(body) => body.len(),
        };
        if batch.len() == 1 {
            return None;
        }
        // Keep the prefix the cap can hold, and always drop at least one.
        let keep = batch
            .len()
            .saturating_mul(cap)
            .checked_div(over_by)
            .unwrap_or(0)
            .min(batch.len() - 1)
            .max(1);
        batch.truncate(keep);
    }
    None
}

/// The delay before a half's next round, by what the Hub answered. Shared by
/// the findings and the incident halves, so both back off the same way.
pub(super) fn next_delay(
    outcome: &ExportOutcome,
    failures: &mut u32,
    flush_interval: Duration,
    more_pending: bool,
) -> Duration {
    match outcome {
        ExportOutcome::Accepted => {
            *failures = 0;
            if more_pending {
                Duration::ZERO
            } else {
                flush_interval
            }
        }
        ExportOutcome::Rejected(_) => {
            *failures = 0;
            flush_interval
        }
        ExportOutcome::Busy => {
            // Advisory backpressure, not a fault: stay near the Hub's own
            // Retry-After instead of climbing to the five-minute ceiling.
            *failures = failures.saturating_add(1).min(BUSY_FAILURE_CAP);
            retry_delay(*failures, jitter_seed())
        }
        ExportOutcome::Retry(_) => {
            *failures = failures.saturating_add(1);
            retry_delay(*failures, jitter_seed())
        }
    }
}

pub(super) fn retry_delay(failures: u32, jitter: u64) -> Duration {
    let base_secs = 1_u64
        .checked_shl(failures.saturating_sub(1).min(8))
        .unwrap_or(256);
    let capped_ms = base_secs.min(300) * 1_000;
    let percent = 80 + jitter % 41;
    Duration::from_millis(capped_ms * percent / 100)
}

pub(super) fn jitter_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| u64::from(duration.subsec_nanos()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::daemon::query_api::{AckSource, ResolvedTomlAck};
    use crate::detect::{Confidence, Finding, FindingType, Pattern, Severity};

    /// A Hub that accepts the connection and never answers: the push gives
    /// up after the configured timeout, not after a fixed ten seconds.
    #[tokio::test]
    async fn a_push_gives_up_after_the_configured_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let silent = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.expect("accept");
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        let uri = format!("http://{address}/api/import/findings?source_id=a")
            .parse::<http_client::Uri>()
            .expect("uri");

        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            post_batch(
                &HubClient::new(Duration::from_millis(300)),
                &uri,
                &"k".repeat(32),
                b"[]".to_vec(),
            ),
        )
        .await
        .expect("the push gives up on its own");
        silent.abort();

        assert!(outcome.is_err(), "{outcome:?}");
        // It waited for the timeout, rather than failing for another reason.
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "{:?}",
            started.elapsed()
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    pub(crate) fn finding(signature: &str, trace_id: &str) -> Finding {
        Finding {
            finding_type: FindingType::NPlusOneSql,
            severity: Severity::Warning,
            trace_id: trace_id.to_string(),
            service: "checkout".to_string(),
            grouping: Vec::new(),
            source_endpoint: "POST /checkout".to_string(),
            pattern: Pattern {
                template: "SELECT * FROM orders".to_string(),
                occurrences: 5,
                window_ms: 200,
                distinct_params: 5,
                ..Default::default()
            },
            suggestion: "batch".to_string(),
            first_timestamp: "2026-08-10T10:00:00Z".to_string(),
            last_timestamp: "2026-08-10T10:00:01Z".to_string(),
            green_impact: None,
            confidence: Confidence::DaemonProduction,
            classification_method: None,
            code_location: None,
            instrumentation_scopes: Vec::new(),
            suggested_fix: None,
            signature: signature.to_string(),
        }
    }

    #[test]
    fn pending_table_coalesces_and_evicts_the_least_recent_signature() {
        let buffer = HubExportBuffer::new(2);

        assert_eq!(buffer.push_batch(&[finding("a", "old")], 1), 0);
        assert_eq!(
            buffer.push_batch(&[finding("a", "new"), finding("b", "b")], 2),
            0
        );
        assert_eq!(buffer.push_batch(&[finding("c", "c")], 3), 1);

        let snapshot = buffer.snapshot(100);
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].finding.finding.signature, "b");
        assert_eq!(snapshot[1].finding.finding.signature, "c");
    }

    #[test]
    fn acknowledging_an_old_revision_does_not_delete_a_severity_escalation() {
        let buffer = HubExportBuffer::new(10);
        buffer.push_batch(&[finding("a", "warning")], 1);
        let sent = buffer.snapshot(100);
        let mut critical = finding("a", "critical");
        critical.severity = Severity::Critical;
        buffer.push_batch(&[critical], 2);

        buffer.acknowledge(&sent, 1);

        let pending = buffer.snapshot(100);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].finding.finding.trace_id, "critical");
    }

    #[test]
    fn an_equal_repeat_in_flight_is_cleared_by_its_acknowledgment() {
        let buffer = HubExportBuffer::new(10);
        buffer.push_batch(&[finding("a", "first")], 1);
        let sent = buffer.snapshot(100);
        buffer.push_batch(&[finding("a", "repeat")], 2);

        buffer.acknowledge(&sent, 2);

        // Retaining it would make run_exporter re-POST the same batch on
        // every round trip instead of honouring the flush interval.
        assert!(buffer.snapshot(100).is_empty());
    }

    #[test]
    fn repeats_accumulate_the_occurrence_count_the_hub_reads() {
        let buffer = HubExportBuffer::new(10);

        buffer.push_batch(&[finding("a", "one"), finding("a", "two")], 10);
        buffer.push_batch(&[finding("a", "three")], 20);

        let pending = buffer.snapshot(100);
        assert_eq!(pending[0].finding.seen_count, 3);
        assert_eq!(pending[0].finding.first_seen_ms, 10);
        assert_eq!(pending[0].finding.stored_at_ms, 20);
    }

    #[test]
    fn pending_repeat_keeps_the_latest_finding_payload() {
        let buffer = HubExportBuffer::new(10);

        buffer.push_batch(&[finding("a", "old-trace")], 10);
        buffer.push_batch(&[finding("a", "latest-trace")], 20);

        assert_eq!(
            buffer.snapshot(100)[0].finding.finding.trace_id,
            "latest-trace"
        );
    }

    #[test]
    fn acknowledged_signatures_only_refresh_hourly_unless_severity_worsens() {
        let buffer = HubExportBuffer::new(10);
        buffer.push_batch(&[finding("a", "first")], 1);
        buffer.acknowledge(&buffer.snapshot(100), 1);

        buffer.push_batch(&[finding("a", "duplicate")], 2);
        assert!(buffer.snapshot(100).is_empty());

        let mut critical = finding("a", "critical");
        critical.severity = Severity::Critical;
        buffer.push_batch(&[critical], 3);
        assert_eq!(buffer.snapshot(100).len(), 1);
        buffer.acknowledge(&buffer.snapshot(100), 3);

        buffer.push_batch(&[finding("a", "refresh")], 3_600_003);
        assert_eq!(buffer.snapshot(100).len(), 1);
    }

    #[test]
    fn refresh_interval_starts_when_the_hub_acknowledges() {
        let buffer = HubExportBuffer::new(10);
        buffer.push_batch(&[finding("a", "before-outage")], 1);
        buffer.acknowledge(&buffer.snapshot(100), 3_600_001);

        buffer.push_batch(&[finding("a", "after-ack")], 3_600_002);

        assert!(buffer.snapshot(100).is_empty());
    }

    #[test]
    fn acknowledged_signature_cache_is_bounded() {
        let buffer = HubExportBuffer::new(2);
        for signature in ["a", "b", "c"] {
            buffer.push_batch(&[finding(signature, signature)], 1);
            buffer.acknowledge(&buffer.snapshot(100), 1);
        }

        let state = buffer
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.sent_entries.len(), 2);
        assert!(!state.sent_entries.contains_key("a"));
    }

    #[test]
    fn acknowledged_signature_order_is_compacted() {
        let buffer = HubExportBuffer::new(2);
        for hour in 0..6 {
            let now = hour * EXPORT_REFRESH_INTERVAL_MS;
            buffer.push_batch(&[finding("a", "a")], now);
            buffer.acknowledge(&buffer.snapshot(100), now);
        }

        let state = buffer
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(state.sent_order.len() <= 4);
    }

    #[test]
    fn pending_coalescing_keeps_the_worst_severity() {
        let buffer = HubExportBuffer::new(10);
        let mut critical = finding("a", "critical");
        critical.severity = Severity::Critical;

        buffer.push_batch(&[critical], 1);
        buffer.push_batch(&[finding("a", "warning")], 2);

        assert_eq!(
            buffer.snapshot(100)[0].finding.finding.severity,
            Severity::Critical
        );
    }

    #[tokio::test]
    async fn payload_matches_the_hub_batch_contract_and_backoff_is_bounded() {
        let buffer = HubExportBuffer::new(10);
        buffer.push_batch(&[finding("a", "trace")], 1);
        let batch = buffer.snapshot(100);

        let mut annotated = annotate(&batch, &acks(None)).await;
        let body = bounded_payload(&mut annotated).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["producer_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["findings"].as_array().unwrap().len(), 1);
        assert_eq!(json["findings"][0]["seen_count"], 1);
        assert!(json["findings"][0].get("acknowledged_by").is_none());
        assert_eq!(retry_delay(1, 0), Duration::from_millis(800));
        assert!(retry_delay(32, 40) <= Duration::from_mins(6));
    }

    #[tokio::test]
    async fn pushed_envelope_carries_the_acknowledgment_the_poll_path_adds() {
        let buffer = HubExportBuffer::new(10);
        buffer.push_batch(&[finding("acked-signature", "trace")], 1);
        let batch = buffer.snapshot(100);

        let annotated = annotate(&batch, &acks(Some("acked-signature"))).await;

        // Without it a pushed envelope overwrites the acked one the Hub
        // stored from the poll, and the finding reappears as open.
        assert!(matches!(
            annotated[0].acknowledged_by,
            Some(AckSource::Toml { .. })
        ));
    }

    #[tokio::test]
    async fn an_oversized_batch_shrinks_to_the_prefix_that_fits() {
        let buffer = HubExportBuffer::new(10);
        for i in 0..3 {
            let mut big = finding(&format!("sig-{i}"), "trace");
            big.pattern.template = "x".repeat(MAX_EXPORT_BODY_BYTES / 2);
            buffer.push_batch(&[big], 1);
        }
        let mut annotated = annotate(&buffer.snapshot(100), &acks(None)).await;

        let body = bounded_payload(&mut annotated).expect("a prefix must fit");

        assert!(body.len() <= MAX_EXPORT_BODY_BYTES);
        // Popped from the end only, so the caller's `batch.truncate` stays
        // aligned with what went over the wire.
        assert_eq!(annotated.len(), 1);
        assert_eq!(annotated[0].stored.finding.signature, "sig-0");

        // A single finding that cannot fit has no prefix left to keep.
        let mut alone = annotate(&buffer.snapshot(1), &acks(None)).await;
        alone[0].stored.finding.pattern.template = "x".repeat(MAX_EXPORT_BODY_BYTES + 1);
        assert!(bounded_payload(&mut alone).is_none());
    }

    #[test]
    fn only_an_unfixable_rejection_drops_the_batch() {
        use hyper::StatusCode;

        assert!(matches!(
            classify(StatusCode::BAD_REQUEST),
            ExportOutcome::Rejected(400)
        ));
        assert!(matches!(
            classify(StatusCode::PAYLOAD_TOO_LARGE),
            ExportOutcome::Rejected(413)
        ));
        // A rotated secret and a busy or broken Hub must keep the batch.
        assert!(matches!(
            classify(StatusCode::UNAUTHORIZED),
            ExportOutcome::Retry(_)
        ));
        assert!(matches!(
            classify(StatusCode::TOO_MANY_REQUESTS),
            ExportOutcome::Retry(_)
        ));
        assert!(matches!(
            classify(StatusCode::INTERNAL_SERVER_ERROR),
            ExportOutcome::Retry(_)
        ));
        assert!(matches!(
            classify(StatusCode::SERVICE_UNAVAILABLE),
            ExportOutcome::Busy
        ));
        assert!(matches!(classify(StatusCode::OK), ExportOutcome::Accepted));
        // Advisory backpressure stays near the Hub's own Retry-After.
        assert!(retry_delay(BUSY_FAILURE_CAP, 40) <= Duration::from_secs(6));
    }

    /// An exporter waiting in `sleep(flush_interval)` with no way out
    /// leaves shutdown nothing but `abort()`, which drops the buffer. With
    /// an hour between flushes, a task that still honours the interval
    /// fails this test by timing out.
    #[tokio::test]
    async fn shutdown_stops_the_exporter_instead_of_waiting_for_the_next_flush() {
        let buffer = Arc::new(HubExportBuffer::new(10));
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let handle = tokio::spawn(run_exporter(
            buffer,
            shutdown.clone(),
            Arc::new(MetricsState::new()),
            "http://127.0.0.1:1/api/import/findings"
                .parse::<http_client::Uri>()
                .expect("static uri"),
            "k".repeat(32),
            100,
            Duration::from_hours(1),
            HubClient::new(Duration::from_secs(10)),
            acks(None),
            None,
            None,
            None,
        ));

        // Let the task reach its wait before signalling, so the test covers
        // the select! arm rather than a notify that lands first.
        tokio::task::yield_now().await;
        shutdown.notify_one();

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("shutdown must end the exporter well inside its flush interval")
            .expect("exporter task panicked");
    }

    /// A pending buffer the Hub cannot accept must not turn the drain into
    /// an unbounded wait: `HubExporter::shutdown` owns the budget, and the
    /// loop only has to keep yielding until it expires.
    #[tokio::test]
    async fn a_drain_against_an_unreachable_hub_still_yields() {
        let buffer = Arc::new(HubExportBuffer::new(10));
        buffer.push_batch(&[finding("a", "trace")], 1);
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let handle = tokio::spawn(run_exporter(
            buffer.clone(),
            shutdown.clone(),
            Arc::new(MetricsState::new()),
            "http://127.0.0.1:1/api/import/findings"
                .parse::<http_client::Uri>()
                .expect("static uri"),
            "k".repeat(32),
            100,
            Duration::from_hours(1),
            HubClient::new(Duration::from_secs(10)),
            acks(None),
            None,
            None,
            None,
        ));

        tokio::task::yield_now().await;
        shutdown.notify_one();

        // The connection is refused, so the entry stays pending and the task
        // keeps retrying. It must not spin or exit silently, and the caller
        // must be able to abort it.
        assert!(
            tokio::time::timeout(Duration::from_millis(600), handle)
                .await
                .is_err(),
            "an undeliverable batch must keep the drain alive for its budget"
        );
        assert_eq!(
            1,
            buffer.len(),
            "nothing was acknowledged, so nothing is lost"
        );
    }

    #[tokio::test]
    async fn a_drain_with_only_incidents_pending_still_sends_them() {
        use crate::daemon::hub_incident_export::{IncidentExport, IncidentExportBuffer, tests};
        let (base, seen) = tests::fake_hub(200).await;
        let incidents = Arc::new(IncidentExportBuffer::new(10));
        incidents.push(&tests::incident("a", None));
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let handle = tokio::spawn(run_exporter(
            Arc::new(HubExportBuffer::new(10)),
            shutdown.clone(),
            Arc::new(MetricsState::new()),
            format!("{base}/api/import/findings?source_id=lab")
                .parse()
                .expect("uri"),
            "k".repeat(32),
            100,
            Duration::from_hours(1),
            HubClient::new(Duration::from_secs(10)),
            acks(None),
            Some(IncidentExport::new(
                incidents.clone(),
                format!("{base}/api/import/incidents?source_id=lab")
                    .parse()
                    .expect("uri"),
            )),
            None,
            None,
        ));
        tokio::task::yield_now().await;
        shutdown.notify_one();

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("the drain ends once both tables are empty")
            .expect("exporter task panicked");
        assert_eq!(0, incidents.len());
        assert_eq!(1, seen.lock().expect("lock").len());
    }

    #[test]
    fn a_busy_hub_keeps_either_half_near_its_retry_after() {
        let mut failures = 10;
        let delay = next_delay(
            &ExportOutcome::Busy,
            &mut failures,
            Duration::from_secs(5),
            false,
        );
        assert_eq!(BUSY_FAILURE_CAP, failures);
        assert!(delay <= Duration::from_millis(4_800), "{delay:?}");
        let mut failures = 0;
        assert_eq!(
            Duration::from_secs(5),
            next_delay(
                &ExportOutcome::Accepted,
                &mut failures,
                Duration::from_secs(5),
                false
            )
        );
        assert_eq!(
            Duration::ZERO,
            next_delay(
                &ExportOutcome::Accepted,
                &mut failures,
                Duration::from_secs(5),
                true
            )
        );
    }

    pub(crate) fn count(
        seen: &crate::daemon::hub_incident_export::tests::Seen,
        path: &str,
    ) -> usize {
        seen.lock()
            .expect("lock")
            .iter()
            .filter(|(uri, _, _)| uri.starts_with(path))
            .count()
    }

    fn spawn_with_incidents(
        base: &str,
        findings: &Arc<HubExportBuffer>,
        incidents: &Arc<IncidentExportBuffer>,
        flush_interval: Duration,
    ) -> (tokio::task::JoinHandle<()>, Arc<tokio::sync::Notify>) {
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let handle = tokio::spawn(run_exporter(
            findings.clone(),
            shutdown.clone(),
            Arc::new(MetricsState::new()),
            format!("{base}/api/import/findings?source_id=lab")
                .parse()
                .expect("uri"),
            "k".repeat(32),
            100,
            flush_interval,
            HubClient::new(Duration::from_secs(10)),
            acks(None),
            Some(IncidentExport::new(
                incidents.clone(),
                format!("{base}/api/import/incidents?source_id=lab")
                    .parse()
                    .expect("uri"),
            )),
            None,
            None,
        ));
        (handle, shutdown)
    }

    /// Steady findings must not drag a failing incident route along at their
    /// own pace: the incident half keeps its backoff, one second and up.
    #[tokio::test]
    async fn a_failing_incident_route_keeps_its_backoff_while_findings_flow() {
        use crate::daemon::hub_incident_export::{IncidentExportBuffer, tests};
        let (base, seen) = tests::fake_hub_routes(200, 500).await;
        let findings = Arc::new(HubExportBuffer::new(100));
        let incidents = Arc::new(IncidentExportBuffer::new(10));
        incidents.push(&tests::incident("a", None));
        let (handle, _shutdown) =
            spawn_with_incidents(&base, &findings, &incidents, Duration::from_millis(50));

        // Read in arrival order, so a slow runner only stretches the wait.
        let mut i = 0;
        wait_for(|| {
            findings.push_batch(&[finding(&format!("s{i}"), "trace")], unix_time_ms());
            i += 1;
            findings_between_incident_posts(&seen) >= 3
        })
        .await;
        handle.abort();

        assert!(
            findings_between_incident_posts(&seen) >= 3,
            "findings flow while the incident half backs off"
        );
    }

    /// Findings posts after the first incident post and before the next.
    fn findings_between_incident_posts(
        seen: &crate::daemon::hub_incident_export::tests::Seen,
    ) -> usize {
        seen.lock()
            .expect("lock")
            .iter()
            .map(|(uri, _, _)| uri.starts_with("/api/import/incidents"))
            .skip_while(|incident| !incident)
            .skip(1)
            .take_while(|incident| !incident)
            .count()
    }

    /// Poll `done` every 20 ms, five seconds at most.
    pub(crate) async fn wait_for(mut done: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !done() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A Hub without the incident route stops incident pushes, never findings.
    #[tokio::test]
    async fn findings_keep_flowing_after_the_incident_route_answers_404() {
        use crate::daemon::hub_incident_export::{IncidentExportBuffer, tests};
        let (base, seen) = tests::fake_hub_routes(200, 404).await;
        let findings = Arc::new(HubExportBuffer::new(100));
        let incidents = Arc::new(IncidentExportBuffer::new(10));
        incidents.push(&tests::incident("a", None));
        findings.push_batch(&[finding("first", "trace")], unix_time_ms());
        let (handle, _shutdown) =
            spawn_with_incidents(&base, &findings, &incidents, Duration::from_millis(50));

        wait_for(|| incidents.is_unsupported() && count(&seen, "/api/import/findings") == 1).await;
        incidents.push(&tests::incident("b", None));
        findings.push_batch(&[finding("second", "trace")], unix_time_ms());
        wait_for(|| count(&seen, "/api/import/findings") == 2).await;
        // A few flush intervals more, for a refused change that leaked.
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        assert_eq!(1, count(&seen, "/api/import/incidents"));
        assert_eq!(2, count(&seen, "/api/import/findings"));
        assert!(incidents.is_unsupported());
    }

    /// The exporter task with its findings and detection halves.
    pub(crate) fn spawn_with_detections(
        base: &str,
        findings: &Arc<HubExportBuffer>,
        detections: DetectionExport,
        flush_interval: Duration,
    ) -> (tokio::task::JoinHandle<()>, Arc<tokio::sync::Notify>) {
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let handle = tokio::spawn(run_exporter(
            findings.clone(),
            shutdown.clone(),
            Arc::new(MetricsState::new()),
            route(base, "/api/import/findings"),
            "k".repeat(32),
            100,
            flush_interval,
            HubClient::new(Duration::from_secs(10)),
            acks(None),
            None,
            None,
            Some(detections),
        ));
        (handle, shutdown)
    }

    fn route(base: &str, path: &str) -> http_client::Uri {
        format!("{base}{path}?source_id=lab").parse().expect("uri")
    }

    /// The exporter task with all three halves against the fake Hub at `base`.
    fn spawn_all(
        base: &str,
        findings: &Arc<HubExportBuffer>,
        incidents: &Arc<IncidentExportBuffer>,
        correlations: &Arc<CorrelationExportBuffer>,
        correlator: Arc<tokio::sync::Mutex<CrossTraceCorrelator>>,
        flush_interval: Duration,
    ) -> (tokio::task::JoinHandle<()>, Arc<tokio::sync::Notify>) {
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let handle = tokio::spawn(run_exporter(
            findings.clone(),
            shutdown.clone(),
            Arc::new(MetricsState::new()),
            route(base, "/api/import/findings"),
            "k".repeat(32),
            100,
            flush_interval,
            HubClient::new(Duration::from_secs(10)),
            acks(None),
            Some(IncidentExport::new(
                incidents.clone(),
                route(base, "/api/import/incidents"),
            )),
            Some(CorrelationExport::new(
                correlator,
                &CorrelationConfig::default(),
                correlations.clone(),
                route(base, "/api/import/correlations"),
            )),
            None,
        ));
        (handle, shutdown)
    }

    /// A Hub without the correlation route stops correlation pushes, never
    /// findings or incidents.
    #[tokio::test]
    async fn findings_and_incidents_keep_flowing_after_the_correlation_route_answers_404() {
        use crate::daemon::hub_correlation_export::tests as episodes;
        use crate::daemon::hub_incident_export::{IncidentExportBuffer, tests};
        let (base, seen) = tests::fake_hub_paths(200, 200, 404).await;
        let findings = Arc::new(HubExportBuffer::new(100));
        let incidents = Arc::new(IncidentExportBuffer::new(10));
        let correlations = Arc::new(CorrelationExportBuffer::new(10));
        findings.push_batch(&[finding("first", "trace")], unix_time_ms());
        incidents.push(&tests::incident("a", None));
        correlations.push(&episodes::episode("a"));
        let (handle, _shutdown) = spawn_all(
            &base,
            &findings,
            &incidents,
            &correlations,
            episodes::empty_correlator(),
            Duration::from_millis(50),
        );

        wait_for(|| {
            correlations.is_unsupported()
                && count(&seen, "/api/import/findings") == 1
                && count(&seen, "/api/import/incidents") == 1
        })
        .await;
        findings.push_batch(&[finding("second", "trace")], unix_time_ms());
        incidents.push(&tests::incident("b", None));
        correlations.push(&episodes::episode("b"));
        wait_for(|| {
            count(&seen, "/api/import/findings") == 2 && count(&seen, "/api/import/incidents") == 2
        })
        .await;
        // A few flush intervals more, for a refused episode that leaked.
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        assert_eq!(1, count(&seen, "/api/import/correlations"));
        assert_eq!(2, count(&seen, "/api/import/findings"));
        assert_eq!(2, count(&seen, "/api/import/incidents"));
    }

    /// A restart must not leave the pair's episode open in the Hub.
    #[tokio::test]
    async fn a_drain_closes_the_open_episodes_and_sends_them() {
        use crate::daemon::hub_correlation_export::tests as episodes;
        use crate::daemon::hub_incident_export::{IncidentExportBuffer, tests};
        let (base, seen) = tests::fake_hub(200).await;
        let correlations = Arc::new(CorrelationExportBuffer::new(10));
        let (handle, shutdown) = spawn_all(
            &base,
            &Arc::new(HubExportBuffer::new(10)),
            &Arc::new(IncidentExportBuffer::new(10)),
            &correlations,
            episodes::correlator_with_one_pair().await,
            Duration::from_hours(1),
        );
        tokio::task::yield_now().await;
        shutdown.notify_one();

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("the drain ends once the three tables are empty")
            .expect("exporter task panicked");
        assert_eq!(0, correlations.len());
        let requests = seen.lock().expect("lock");
        let (_, _, body) = requests
            .iter()
            .find(|(uri, _, _)| uri.starts_with("/api/import/correlations"))
            .expect("the closed episode was sent");
        let body: serde_json::Value = serde_json::from_slice(body).expect("json");
        let episode = &body["episodes"][0];
        assert_eq!(episode["last_seen_ms"], episode["ended_at_ms"]);
    }

    /// `[daemon.correlation]` off: no correlator, so no table and no route.
    #[tokio::test]
    async fn the_correlation_table_exists_only_with_a_correlator() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = dir.path().join("hub-key");
        std::fs::write(&key, "k".repeat(32)).expect("key");
        let config = DaemonHubExportConfig {
            enabled: true,
            endpoint: Some("http://127.0.0.1:1/api/import/findings".to_string()),
            source_id: Some("lab".to_string()),
            api_key_file: Some(key.display().to_string()),
            ..DaemonHubExportConfig::default()
        };
        let spawn = |correlator: Option<Arc<tokio::sync::Mutex<CrossTraceCorrelator>>>| {
            HubExporter::spawn(
                &config,
                Arc::new(MetricsState::new()),
                Arc::new(crate::daemon::ack_toml_state::AckTomlState::new(
                    HashMap::new(),
                )),
                None,
                false,
                correlator,
                &CorrelationConfig::default(),
                None,
            )
            .expect("spawn")
            .expect("enabled")
        };

        assert!(spawn(None).correlation_buffer.is_none());
        let with = spawn(Some(
            crate::daemon::hub_correlation_export::tests::empty_correlator(),
        ));
        assert!(with.correlation_buffer.is_some());
    }

    fn acks(acked_signature: Option<&str>) -> AckSources {
        let mut toml = HashMap::new();
        if let Some(signature) = acked_signature {
            toml.insert(
                signature.to_string(),
                ResolvedTomlAck {
                    inner: crate::acknowledgments::Acknowledgment {
                        signature: signature.to_string(),
                        acknowledged_by: "team@example.com".to_string(),
                        acknowledged_at: "2026-08-01".to_string(),
                        reason: "accepted debt".to_string(),
                        expires_at: None,
                        service: None,
                        source_endpoint: None,
                    },
                    expires_at_dt: None,
                },
            );
        }
        AckSources {
            toml: Arc::new(crate::daemon::ack_toml_state::AckTomlState::new(toml)),
            store: None,
        }
    }
}
