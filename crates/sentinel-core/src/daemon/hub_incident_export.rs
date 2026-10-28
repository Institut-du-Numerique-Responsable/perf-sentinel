//! The daemon's incidents on their way to the Hub, beside its
//! findings: a latest-value table per incident id, fed by the incident
//! store's change hooks and drained by the exporter task.

use std::sync::Arc;

use prometheus::{Gauge, IntCounter};
use serde::Serialize;

use super::hub_table::{Keyed, LatestTable, Pending, Route, RouteExport, import_route};
use super::incidents::Incident;
use crate::http_client;
use crate::report::metrics::MetricsState;

/// Incidents per request. One carries up to a thousand frozen findings.
pub(super) const MAX_INCIDENT_BATCH: usize = 10;
/// The Hub's cap on an incident import, the size of one of its poll pages.
pub(super) const MAX_INCIDENT_BODY_BYTES: usize = 4 * 1024 * 1024;

pub(crate) type IncidentExportBuffer = LatestTable<Incident>;
pub(crate) type PendingIncident = Pending<Incident>;
/// The exporter task's incident half.
pub(crate) type IncidentExport = RouteExport<Incident>;

impl Keyed for Incident {
    fn key(&self) -> &str {
        &self.id
    }
}

#[derive(Serialize)]
struct IncidentPayload<'a> {
    producer_version: &'static str,
    incidents: Vec<&'a Incident>,
}

fn incident_body(pending: &[PendingIncident]) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&IncidentPayload {
        producer_version: env!("CARGO_PKG_VERSION"),
        incidents: pending.iter().map(|entry| &*entry.value).collect(),
    })
}

fn incidents_pending(metrics: &MetricsState) -> &Gauge {
    &metrics.hub_export_incidents_pending
}

fn incidents_dropped(metrics: &MetricsState) -> &IntCounter {
    &metrics.hub_export_incidents_dropped_total
}

static INCIDENT_ROUTE: Route<Incident> = Route {
    max_batch: MAX_INCIDENT_BATCH,
    max_body: MAX_INCIDENT_BODY_BYTES,
    body: incident_body,
    pending: incidents_pending,
    dropped: incidents_dropped,
    rejected: "PerfSentinelHub rejected the incident batch permanently; dropping it",
    retrying: "PerfSentinelHub incident export failed; retaining the batch",
    missing: "PerfSentinelHub has no incident import route; incidents are not pushed until restart, findings are",
};

impl RouteExport<Incident> {
    pub(crate) fn new(buffer: Arc<IncidentExportBuffer>, uri: http_client::Uri) -> Self {
        Self::with_route(&INCIDENT_ROUTE, buffer, uri)
    }
}

/// At most this many incidents wait for the Hub. One can carry a thousand
/// frozen findings, so a findings-sized table of 10,000 could pin ten million
/// of them through a Hub outage.
const MAX_PENDING_INCIDENTS: usize = 1_000;

/// The incident table's bound, from the findings one.
pub(super) fn incident_capacity(max_pending: usize) -> usize {
    max_pending.min(MAX_PENDING_INCIDENTS)
}

/// The incident import beside the configured findings import.
pub(super) fn incidents_endpoint(findings_endpoint: &str) -> String {
    import_route(findings_endpoint, "/api/import/incidents")
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use crate::daemon::hub_export::HubClient;

    pub(crate) type Seen = Arc<Mutex<Vec<(String, Option<String>, Vec<u8>)>>>;

    /// A Hub that records each request and answers `status`.
    pub(crate) async fn fake_hub(status: u16) -> (String, Seen) {
        fake_hub_routes(status, status).await
    }

    /// A Hub that answers the findings import with `findings` and every
    /// other path, the incident import included, with `incidents`.
    pub(crate) async fn fake_hub_routes(findings: u16, incidents: u16) -> (String, Seen) {
        fake_hub_paths(findings, incidents, incidents).await
    }

    /// A Hub that answers the findings import with `findings`, the
    /// correlation import with `correlations`, and every other path, the
    /// incident import included, with `incidents`.
    pub(crate) async fn fake_hub_paths(
        findings: u16,
        incidents: u16,
        correlations: u16,
    ) -> (String, Seen) {
        fake_hub_routes4(findings, incidents, correlations, incidents).await
    }

    /// `fake_hub_paths`, with the detection import answered `detections`.
    pub(crate) async fn fake_hub_routes4(
        findings: u16,
        incidents: u16,
        correlations: u16,
        detections: u16,
    ) -> (String, Seen) {
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
                    let status = match uri.path() {
                        "/api/import/findings" => findings,
                        "/api/import/correlations" => correlations,
                        "/api/import/detections" => detections,
                        _ => incidents,
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
        let client = HubClient::new(Duration::from_secs(10));

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
        let client = HubClient::new(Duration::from_secs(10));

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
        let client = HubClient::new(Duration::from_secs(10));
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
        let client = HubClient::new(Duration::from_secs(10));

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
        let client = HubClient::new(Duration::from_secs(10));

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
            .map(|pending| pending.value.id.clone())
            .collect()
    }

    #[test]
    fn a_change_replaces_the_pending_copy_of_the_same_incident() {
        let buffer = IncidentExportBuffer::new(10);
        buffer.push(&incident("a", None));
        buffer.push(&incident("a", Some(1_800_000_045_000)));

        let batch = buffer.snapshot(10);
        assert_eq!(1, batch.len());
        assert_eq!(Some(1_800_000_045_000), batch[0].value.ended_at_ms);
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
        assert_eq!(Some(1_800_000_045_000), batch[0].value.ended_at_ms);
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
