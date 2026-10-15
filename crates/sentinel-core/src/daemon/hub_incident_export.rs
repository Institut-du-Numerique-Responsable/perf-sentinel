//! The daemon's incidents on their way to the Hub, beside its
//! findings: a latest-value table per incident id, fed by the incident
//! store's change hooks and drained by the exporter task.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use super::hub_export::{
    ExportOutcome, bounded_body, classify, evict_by_order, next_delay, post_batch,
    snapshot_by_order,
};
use super::incidents::Incident;
use crate::http_client;
use crate::report::metrics::MetricsState;

/// Incidents per request. One carries up to a thousand frozen findings.
pub(super) const MAX_INCIDENT_BATCH: usize = 10;
/// The Hub's cap on an incident import, the size of one of its poll pages.
pub(super) const MAX_INCIDENT_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct PendingIncident {
    // Shared, so a snapshot copies pointers rather than frozen findings.
    pub(crate) incident: Arc<Incident>,
    revision: u64,
}

#[derive(Default)]
struct PendingIncidents {
    entries: HashMap<String, PendingIncident>,
    order: VecDeque<(String, u64)>,
    next_revision: u64,
}

pub(crate) struct IncidentExportBuffer {
    inner: Mutex<PendingIncidents>,
    max_pending: usize,
    // Set once a Hub answered that it has no incident route. Until restart,
    // queueing would only grow a table nothing can drain.
    unsupported: AtomicBool,
}

impl IncidentExportBuffer {
    pub(crate) fn new(max_pending: usize) -> Self {
        Self {
            inner: Mutex::new(PendingIncidents::default()),
            max_pending,
            unsupported: AtomicBool::new(false),
        }
    }

    /// Queue the incident's current state, replacing an older one still
    /// pending. Every change is a new revision, so the acknowledgment of a
    /// batch already in flight cannot remove it. Returns how many incidents
    /// were evicted to stay within `max_pending`.
    pub(crate) fn push(&self, incident: &Incident) -> u64 {
        let mut state = self.lock();
        // Read under the lock `mark_unsupported` sets it under, so a push
        // racing a 404 cannot land after the table is cleared.
        if self.is_unsupported() {
            return 0;
        }
        state.next_revision = state.next_revision.wrapping_add(1);
        let revision = state.next_revision;
        state.entries.insert(
            incident.id.clone(),
            PendingIncident {
                incident: Arc::new(incident.clone()),
                revision,
            },
        );
        state.order.push_back((incident.id.clone(), revision));
        let PendingIncidents { entries, order, .. } = &mut *state;
        evict_by_order(entries, order, self.max_pending, |entry| entry.revision)
    }

    /// Up to `limit` pending incidents, least recently changed first.
    pub(crate) fn snapshot(&self, limit: usize) -> Vec<PendingIncident> {
        let state = self.lock();
        snapshot_by_order(&state.entries, &state.order, limit, |entry| entry.revision)
    }

    /// Remove what the Hub accepted, unless it changed again meanwhile.
    pub(crate) fn acknowledge(&self, sent: &[PendingIncident]) {
        let mut state = self.lock();
        for item in sent {
            if state
                .entries
                .get(&item.incident.id)
                .is_some_and(|entry| entry.revision == item.revision)
            {
                state.entries.remove(&item.incident.id);
            }
        }
    }

    /// The Hub has no incident route: stop queueing until the daemon
    /// restarts. Returns how many pending incidents this drops.
    pub(crate) fn mark_unsupported(&self) -> u64 {
        let mut state = self.lock();
        self.unsupported.store(true, Ordering::Relaxed);
        let dropped = state.entries.len() as u64;
        state.entries.clear();
        state.order.clear();
        dropped
    }

    pub(crate) fn is_unsupported(&self) -> bool {
        self.unsupported.load(Ordering::Relaxed)
    }

    pub(crate) fn len(&self) -> usize {
        self.lock().entries.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PendingIncidents> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Serialize)]
struct IncidentPayload<'a> {
    producer_version: &'static str,
    incidents: Vec<&'a Incident>,
}

/// At most this many incidents wait for the Hub. One can carry a thousand
/// frozen findings, so a findings-sized table of 10,000 could pin ten million
/// of them through a Hub outage.
const MAX_PENDING_INCIDENTS: usize = 1_000;

/// The incident table's bound, from the findings one.
pub(super) fn incident_capacity(max_pending: usize) -> usize {
    max_pending.min(MAX_PENDING_INCIDENTS)
}

/// The incident import beside the configured findings import. Config
/// validation guarantees the suffix, so this only swaps it.
pub(super) fn incidents_endpoint(findings_endpoint: &str) -> String {
    format!(
        "{}/api/import/incidents",
        findings_endpoint
            .strip_suffix("/api/import/findings")
            .unwrap_or(findings_endpoint)
    )
}

/// The exporter task's incident half: the table, where it goes, and its own
/// failure count, so a findings backoff never slows incidents down.
pub(crate) struct IncidentExport {
    buffer: Arc<IncidentExportBuffer>,
    uri: http_client::Uri,
    failures: u32,
    /// When this half's next round is due, so its backoff holds whatever
    /// pace the findings half keeps.
    pub(super) due: tokio::time::Instant,
}

impl IncidentExport {
    pub(crate) fn new(buffer: Arc<IncidentExportBuffer>, uri: http_client::Uri) -> Self {
        Self {
            buffer,
            uri,
            failures: 0,
            due: tokio::time::Instant::now(),
        }
    }

    /// Send one batch of what is pending, then set the time of the next.
    /// False when nothing was pending, or when the Hub turned out to have
    /// no incident route.
    pub(crate) async fn round(
        &mut self,
        client: &http_client::HttpClientWithBody,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
    ) -> bool {
        let mut batch = self.buffer.snapshot(MAX_INCIDENT_BATCH);
        if batch.is_empty() {
            self.due = tokio::time::Instant::now() + flush_interval;
            return false;
        }
        let Some(body) = bounded_body(&mut batch, MAX_INCIDENT_BODY_BYTES, |pending| {
            serde_json::to_vec(&IncidentPayload {
                producer_version: env!("CARGO_PKG_VERSION"),
                incidents: pending.iter().map(|entry| &*entry.incident).collect(),
            })
        }) else {
            // One incident alone over the cap: the poll can still copy it.
            self.buffer.acknowledge(&batch[..1]);
            metrics.hub_export_incidents_dropped_total.inc();
            self.publish(metrics);
            self.due = tokio::time::Instant::now();
            return true;
        };
        let outcome = match post_batch(client, &self.uri, api_key, body).await {
            Ok(status) if is_missing_route(status) => {
                self.unsupported(metrics);
                self.due = tokio::time::Instant::now() + flush_interval;
                return false;
            }
            Ok(status) => classify(status),
            Err(reason) => ExportOutcome::Retry(reason),
        };
        let delay = self.apply(&outcome, &batch, metrics, flush_interval);
        self.publish(metrics);
        self.due = tokio::time::Instant::now() + delay;
        true
    }

    fn apply(
        &mut self,
        outcome: &ExportOutcome,
        batch: &[PendingIncident],
        metrics: &MetricsState,
        flush_interval: Duration,
    ) -> Duration {
        match outcome {
            ExportOutcome::Accepted => self.buffer.acknowledge(batch),
            ExportOutcome::Rejected(status) => {
                self.buffer.acknowledge(batch);
                metrics
                    .hub_export_incidents_dropped_total
                    .inc_by(batch.len() as u64);
                tracing::error!(
                    endpoint = %http_client::redact_endpoint(&self.uri),
                    status = *status,
                    dropped = batch.len(),
                    "PerfSentinelHub rejected the incident batch permanently; dropping it"
                );
            }
            ExportOutcome::Busy | ExportOutcome::Retry(_) => {}
        }
        let delay = next_delay(
            outcome,
            &mut self.failures,
            flush_interval,
            self.buffer.len() > 0,
        );
        if let ExportOutcome::Retry(reason) = outcome {
            // A Hub older than the incident route answers 401, not 404, when
            // its sign-in is on: the retry is logged so that case is visible.
            tracing::warn!(
                endpoint = %http_client::redact_endpoint(&self.uri),
                reason = %reason,
                retry_seconds = delay.as_secs_f64(),
                "PerfSentinelHub incident export failed; retaining the batch"
            );
        }
        delay
    }

    fn unsupported(&self, metrics: &MetricsState) {
        let dropped = self.buffer.mark_unsupported();
        metrics.hub_export_incidents_dropped_total.inc_by(dropped);
        self.publish(metrics);
        tracing::warn!(
            endpoint = %http_client::redact_endpoint(&self.uri),
            dropped,
            "PerfSentinelHub has no incident import route; incidents are not pushed until restart, findings are"
        );
    }

    fn publish(&self, metrics: &MetricsState) {
        metrics
            .hub_export_incidents_pending
            .set(self.buffer.len() as f64);
    }
}

/// A Hub that predates the route answers it as any unmapped path.
fn is_missing_route(status: hyper::StatusCode) -> bool {
    matches!(
        status,
        hyper::StatusCode::NOT_FOUND | hyper::StatusCode::METHOD_NOT_ALLOWED
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) type Seen = Arc<Mutex<Vec<(String, Option<String>, Vec<u8>)>>>;

    /// A Hub that records each request and answers `status`.
    pub(crate) async fn fake_hub(status: u16) -> (String, Seen) {
        fake_hub_routes(status, status).await
    }

    /// A Hub that answers the findings import with `findings` and every
    /// other path, the incident import included, with `incidents`.
    pub(crate) async fn fake_hub_routes(findings: u16, incidents: u16) -> (String, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let store = seen.clone();
        let app = axum::Router::new().fallback(
            move |uri: axum::http::Uri, headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let store = store.clone();
                async move {
                    let key = headers
                        .get("x-api-key")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string);
                    let status = if uri.path() == "/api/import/findings" {
                        findings
                    } else {
                        incidents
                    };
                    store
                        .lock()
                        .expect("lock")
                        .push((uri.to_string(), key, body.to_vec()));
                    axum::http::StatusCode::from_u16(status).expect("status")
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{address}"), seen)
    }

    fn export(base: &str, buffer: &Arc<IncidentExportBuffer>) -> IncidentExport {
        IncidentExport::new(
            buffer.clone(),
            format!("{base}/api/import/incidents?source_id=lab")
                .parse()
                .expect("uri"),
        )
    }

    #[test]
    fn the_incident_table_is_capped_below_the_findings_one() {
        assert_eq!(1_000, incident_capacity(10_000));
        assert_eq!(50, incident_capacity(50));
    }

    #[test]
    fn the_incidents_route_sits_beside_the_findings_one() {
        assert_eq!(
            "https://hub.example.com/api/import/incidents",
            incidents_endpoint("https://hub.example.com/api/import/findings")
        );
    }

    #[tokio::test]
    async fn a_round_posts_the_pending_incidents_and_clears_them() {
        let (base, seen) = fake_hub(200).await;
        let buffer = Arc::new(IncidentExportBuffer::new(10));
        buffer.push(&incident("a", None));
        let metrics = MetricsState::new();
        let client = http_client::build_client_with_body();

        let next = export(&base, &buffer)
            .round(&client, &"k".repeat(32), &metrics, Duration::from_secs(5))
            .await;

        assert!(next);
        assert_eq!(0, buffer.len());
        let requests = seen.lock().expect("lock");
        let (uri, key, body) = &requests[0];
        assert_eq!("/api/import/incidents?source_id=lab", uri);
        assert_eq!(Some("k".repeat(32)), *key);
        let body: serde_json::Value = serde_json::from_slice(body).expect("json");
        assert_eq!(env!("CARGO_PKG_VERSION"), body["producer_version"]);
        assert_eq!("a", body["incidents"][0]["id"]);
    }

    #[tokio::test]
    async fn an_empty_table_sends_nothing() {
        let (base, seen) = fake_hub(200).await;
        let buffer = Arc::new(IncidentExportBuffer::new(10));
        let client = http_client::build_client_with_body();

        let next = export(&base, &buffer)
            .round(
                &client,
                &"k".repeat(32),
                &MetricsState::new(),
                Duration::from_secs(5),
            )
            .await;

        assert!(!next);
        assert!(seen.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_hub_without_the_route_stops_incident_export_for_good() {
        let (base, seen) = fake_hub(404).await;
        let buffer = Arc::new(IncidentExportBuffer::new(10));
        buffer.push(&incident("a", None));
        buffer.push(&incident("b", None));
        let metrics = MetricsState::new();
        let client = http_client::build_client_with_body();
        let mut export = export(&base, &buffer);

        assert!(
            !export
                .round(&client, &"k".repeat(32), &metrics, Duration::from_secs(5))
                .await
        );
        buffer.push(&incident("c", None));
        assert!(
            !export
                .round(&client, &"k".repeat(32), &metrics, Duration::from_secs(5))
                .await
        );

        assert!(buffer.is_unsupported());
        assert_eq!(2, metrics.hub_export_incidents_dropped_total.get());
        assert_eq!(
            1,
            seen.lock().expect("lock").len(),
            "one probe, then silence"
        );
    }

    #[tokio::test]
    async fn an_incident_over_the_cap_alone_is_dropped_and_counted() {
        let (base, seen) = fake_hub(200).await;
        let buffer = Arc::new(IncidentExportBuffer::new(10));
        let mut big = incident("a", None);
        big.detail = Some("x".repeat(MAX_INCIDENT_BODY_BYTES));
        buffer.push(&big);
        let metrics = MetricsState::new();
        let client = http_client::build_client_with_body();

        export(&base, &buffer)
            .round(&client, &"k".repeat(32), &metrics, Duration::from_secs(5))
            .await;

        assert_eq!(0, buffer.len());
        assert_eq!(1, metrics.hub_export_incidents_dropped_total.get());
        assert!(seen.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_busy_hub_keeps_the_batch() {
        let (base, _seen) = fake_hub(503).await;
        let buffer = Arc::new(IncidentExportBuffer::new(10));
        buffer.push(&incident("a", None));
        let client = http_client::build_client_with_body();

        let next = export(&base, &buffer)
            .round(
                &client,
                &"k".repeat(32),
                &MetricsState::new(),
                Duration::from_secs(5),
            )
            .await;

        assert!(next);
        assert_eq!(1, buffer.len());
    }

    pub(crate) fn incident(id: &str, ended_at_ms: Option<u64>) -> Incident {
        let mut value = serde_json::json!({
            "id": id,
            "service": "checkout",
            "kind": "oom_kill",
            "at_ms": 1_800_000_000_000_u64,
            "window_from_ms": 1_799_999_700_000_u64,
            "window_to_ms": 1_800_000_060_000_u64,
            "findings": []
        });
        if let Some(ended) = ended_at_ms {
            value["ended_at_ms"] = serde_json::json!(ended);
        }
        serde_json::from_value(value).expect("a valid incident")
    }

    fn ids(batch: Vec<PendingIncident>) -> Vec<String> {
        batch
            .into_iter()
            .map(|pending| pending.incident.id.clone())
            .collect()
    }

    #[test]
    fn a_change_replaces_the_pending_copy_of_the_same_incident() {
        let buffer = IncidentExportBuffer::new(10);
        buffer.push(&incident("a", None));
        buffer.push(&incident("a", Some(1_800_000_045_000)));

        let batch = buffer.snapshot(10);
        assert_eq!(1, batch.len());
        assert_eq!(Some(1_800_000_045_000), batch[0].incident.ended_at_ms);
    }

    #[test]
    fn a_late_acknowledgement_keeps_the_newer_revision() {
        let buffer = IncidentExportBuffer::new(10);
        buffer.push(&incident("a", None));
        let in_flight = buffer.snapshot(10);
        buffer.push(&incident("a", Some(1_800_000_045_000)));

        buffer.acknowledge(&in_flight);

        let batch = buffer.snapshot(10);
        assert_eq!(1, batch.len(), "the close is still to be sent");
        assert_eq!(Some(1_800_000_045_000), batch[0].incident.ended_at_ms);
    }

    #[test]
    fn an_acknowledged_incident_leaves_the_table() {
        let buffer = IncidentExportBuffer::new(10);
        buffer.push(&incident("a", None));
        buffer.acknowledge(&buffer.snapshot(10));
        assert_eq!(0, buffer.len());
    }

    #[test]
    fn the_table_evicts_the_least_recently_changed_incident_past_its_cap() {
        let buffer = IncidentExportBuffer::new(2);
        assert_eq!(0, buffer.push(&incident("a", None)));
        assert_eq!(0, buffer.push(&incident("b", None)));
        buffer.push(&incident("a", Some(1)));
        assert_eq!(1, buffer.push(&incident("c", None)));

        assert_eq!(
            vec!["a".to_string(), "c".to_string()],
            ids(buffer.snapshot(10))
        );
    }

    #[test]
    fn snapshot_follows_change_order_and_honours_the_limit() {
        let buffer = IncidentExportBuffer::new(10);
        for id in ["a", "b", "c"] {
            buffer.push(&incident(id, None));
        }
        assert_eq!(
            vec!["a".to_string(), "b".to_string()],
            ids(buffer.snapshot(2))
        );
    }

    #[test]
    fn an_unsupported_hub_clears_the_table_and_refuses_later_changes() {
        let buffer = IncidentExportBuffer::new(10);
        buffer.push(&incident("a", None));
        buffer.push(&incident("b", None));

        assert_eq!(2, buffer.mark_unsupported());
        assert!(buffer.is_unsupported());
        assert_eq!(0, buffer.push(&incident("c", None)));
        assert_eq!(0, buffer.len());
    }
}
