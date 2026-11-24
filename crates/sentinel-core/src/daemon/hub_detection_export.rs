//! The detection ledger on its way to the Hub: the rows of each day that
//! moved since the Hub last accepted it, in pages shaped like
//! `GET /api/detections`, at most one snapshot a minute outside the drain.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::detection_ledger::{
    CountRow, DayRow, DaySnapshot, DetectionLedger, DetectionLedgerStatus, MAX_PAGE_BYTES,
    split_name,
};
use super::hub_export::{
    ExportOutcome, HubClient, bounded_body, classify, next_delay, post_batch_with_reply,
};
use super::hub_table::{import_route, is_missing_route};
use crate::http_client;
use crate::report::metrics::MetricsState;

/// Count rows per push page, the Hub's cap.
const MAX_PUSH_ROWS: usize = 5_000;
/// The shortest wait between two snapshots, whatever `flush_interval` says:
/// today's counts move with every batch.
const MIN_SNAPSHOT_INTERVAL: Duration = Duration::from_mins(1);

/// The detection import beside the configured findings import.
pub(super) fn detections_endpoint(findings_endpoint: &str) -> String {
    import_route(findings_endpoint, "/api/import/detections")
}

/// The detection import taking rows per (signature, grouping).
pub(super) fn split_detections_endpoint(findings_endpoint: &str) -> String {
    import_route(findings_endpoint, "/api/import/detections/by-grouping")
}

/// The part of the Hub's import reply the daemon reads.
#[derive(Deserialize)]
struct ImportReply {
    #[serde(default)]
    watch: Option<Vec<String>>,
}

fn is_auth_refusal(status: hyper::StatusCode) -> bool {
    matches!(
        status,
        hyper::StatusCode::UNAUTHORIZED | hyper::StatusCode::FORBIDDEN
    )
}

/// The watch list a reply carries: `None` when it does not parse, empty
/// when it parses without `watch`.
fn watch_of(reply: &[u8]) -> Option<Vec<String>> {
    serde_json::from_slice::<ImportReply>(reply)
        .ok()
        .map(|reply| reply.watch.unwrap_or_default())
}

/// A `DetectionsPage` over borrowed rows, so paging a day copies nothing.
#[derive(Serialize)]
struct PushPage<'a> {
    ledger_id: &'a str,
    started_ms: u64,
    current_day: u32,
    oldest_day: u32,
    sealed_through: Option<u32>,
    counts: &'a [&'a CountRow],
    days: &'a [DayRow],
    next_after: Option<&'a str>,
    #[serde(
        skip_serializing_if = "std::ops::Not::not",
        serialize_with = "split_name"
    )]
    split: bool,
}

/// The rows of `day` from `from` on that fit one page, and its body. The
/// day's `DayRow` rides only on the page that carries its last row.
fn next_page(
    header: &DetectionLedgerStatus,
    day: &DaySnapshot,
    from: usize,
    split: bool,
) -> Option<(usize, Vec<u8>)> {
    let rest = &day.rows[from..];
    let serialize = |counts: &[&CountRow]| {
        let last = counts.len() == rest.len();
        serde_json::to_vec(&PushPage {
            ledger_id: &header.ledger_id,
            started_ms: header.started_ms,
            current_day: header.current_day,
            oldest_day: header.oldest_day,
            sealed_through: header.sealed_through,
            counts,
            days: if last {
                std::slice::from_ref(&day.meta)
            } else {
                &[]
            },
            next_after: None,
            split,
        })
    };
    if rest.is_empty() {
        return serialize(&[]).ok().map(|body| (0, body));
    }
    let mut batch: Vec<&CountRow> = rest.iter().take(MAX_PUSH_ROWS).collect();
    let body = bounded_body(&mut batch, MAX_PAGE_BYTES, serialize)?;
    Some((batch.len(), body))
}

/// The exporter task's detection half: it snapshots the dirty days of the
/// ledger and sends them page by page, on its own clock.
pub(crate) struct DetectionExport {
    ledger: Arc<DetectionLedger>,
    /// The split route, until a Hub answers that it has none.
    split_uri: Option<http_client::Uri>,
    /// Set once the split route accepted a page: that Hub has the route.
    accepted_split: bool,
    /// The unsplit route, for a Hub without the split one.
    uri: http_client::Uri,
    /// The days of the last snapshot still to send, oldest first.
    queue: VecDeque<DaySnapshot>,
    /// What every page of that snapshot carries beside its rows.
    header: DetectionLedgerStatus,
    /// Rows of the front day the Hub already accepted or dropped.
    sent: usize,
    failures: u32,
    due: tokio::time::Instant,
    /// Set once a Hub answered that it has no such route.
    unsupported: bool,
    /// Set once the drain pass ran, so it runs once.
    closed: bool,
}

impl DetectionExport {
    pub(crate) fn new(
        ledger: Arc<DetectionLedger>,
        split_uri: http_client::Uri,
        uri: http_client::Uri,
    ) -> Self {
        Self {
            header: ledger.status(),
            ledger,
            split_uri: Some(split_uri),
            accepted_split: false,
            uri,
            queue: VecDeque::new(),
            sent: 0,
            failures: 0,
            due: tokio::time::Instant::now(),
            unsupported: false,
            closed: false,
        }
    }

    pub(super) fn due(&self) -> tokio::time::Instant {
        self.due
    }

    pub(super) fn start_at(&mut self, at: tokio::time::Instant) {
        self.due = at;
    }

    /// One page when due. While draining, one forced pass over every dirty
    /// day, then nothing. True when a page went out.
    pub(crate) async fn round(
        &mut self,
        client: &HubClient,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
        draining: bool,
    ) -> bool {
        if draining {
            return self.drain(client, api_key, metrics, flush_interval).await;
        }
        if self.due > tokio::time::Instant::now() {
            return false;
        }
        self.step(client, api_key, metrics, flush_interval).await
    }

    async fn drain(
        &mut self,
        client: &HubClient,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
    ) -> bool {
        if std::mem::replace(&mut self.closed, true) {
            return false;
        }
        // A snapshot from before the drain would miss today's drain gap.
        self.queue.clear();
        let mut sent = false;
        while self.step(client, api_key, metrics, flush_interval).await {
            sent = true;
            if self.queue.is_empty() || self.failures > 0 {
                break;
            }
        }
        sent
    }

    /// Send the next page of the oldest dirty day, then set when the next
    /// is due. False when nothing is dirty or the Hub has no such route.
    async fn step(
        &mut self,
        client: &HubClient,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
    ) -> bool {
        let pace = flush_interval.max(MIN_SNAPSHOT_INTERVAL);
        if self.unsupported || !self.refill() {
            self.due = tokio::time::Instant::now() + pace;
            return false;
        }
        let split = self.split_uri.is_some();
        let Some((rows, body)) = next_page(&self.header, &self.queue[0], self.sent, split) else {
            // A row alone over the cap: no page can carry it.
            self.drop_rows(usize::from(self.sent < self.queue[0].rows.len()), metrics);
            self.due = tokio::time::Instant::now();
            return true;
        };
        let reply = post_batch_with_reply(client, self.uri(), api_key, body).await;
        // An older Hub with sign-in answers the unknown split route 401: ask the older route.
        if split
            && !self.accepted_split
            && matches!(&reply, Ok((status, _)) if is_auth_refusal(*status))
            && self.older_route_accepts(client, api_key).await
        {
            self.fall_back();
            self.due = tokio::time::Instant::now();
            return true;
        }
        let outcome = match reply {
            Ok((status, _)) if is_missing_route(status) && split && !self.accepted_split => {
                self.fall_back();
                self.due = tokio::time::Instant::now();
                return true;
            }
            // Unsplit rows would land beside the split ones of a Hub that has the route.
            Ok((status, _)) if is_missing_route(status) && split => ExportOutcome::Retry(format!(
                "per-grouping route answered {status} after accepting a page"
            )),
            Ok((status, _)) if is_missing_route(status) => {
                self.unsupported();
                self.due = tokio::time::Instant::now() + pace;
                return false;
            }
            reply => self.outcome_of(reply),
        };
        self.accepted_split |= split && matches!(outcome, ExportOutcome::Accepted);
        let mut delay = self.apply(&outcome, rows, metrics, flush_interval);
        if self.queue.is_empty() {
            delay = delay.max(pace);
        }
        self.due = tokio::time::Instant::now() + delay;
        true
    }

    /// The outcome of one page. An accepted reply replaces the watch list
    /// when it parses.
    fn outcome_of(
        &self,
        reply: Result<(hyper::StatusCode, bytes::Bytes), http_client::FetchError>,
    ) -> ExportOutcome {
        match reply {
            Ok((status, body)) => {
                let outcome = classify(status);
                if matches!(outcome, ExportOutcome::Accepted)
                    && let Some(watch) = watch_of(&body)
                {
                    self.ledger.set_watch(&watch);
                }
                outcome
            }
            // The Hub answers errors with short bodies, so an oversized body
            // is an accepted page carrying a long list.
            Err(http_client::FetchError::BodyTooLarge(_)) => {
                tracing::warn!(
                    endpoint = %http_client::redact_endpoint(self.uri()),
                    "PerfSentinelHub detection reply is over the size cap; counting the page as accepted and keeping the watch list"
                );
                ExportOutcome::Accepted
            }
            Err(error) => ExportOutcome::Retry(error.to_string()),
        }
    }

    /// Snapshot the dirty days once the last snapshot is sent. False when
    /// none is dirty.
    fn refill(&mut self) -> bool {
        if self.queue.is_empty() {
            // The header first, so `sealed_through` never runs ahead of the rows.
            self.header = self.ledger.status();
            self.queue = self.ledger.dirty_days(self.split_uri.is_some()).into();
            self.sent = 0;
        }
        !self.queue.is_empty()
    }

    fn apply(
        &mut self,
        outcome: &ExportOutcome,
        rows: usize,
        metrics: &MetricsState,
        flush_interval: Duration,
    ) -> Duration {
        match outcome {
            ExportOutcome::Accepted => self.advance(rows),
            ExportOutcome::Rejected(status) => {
                tracing::error!(
                    endpoint = %http_client::redact_endpoint(self.uri()),
                    status = *status,
                    dropped = rows,
                    "PerfSentinelHub rejected a detection page permanently; dropping it and flagging its day incomplete"
                );
                self.drop_rows(rows, metrics);
            }
            ExportOutcome::Busy | ExportOutcome::Retry(_) => {}
        }
        let delay = next_delay(
            outcome,
            &mut self.failures,
            flush_interval,
            !self.queue.is_empty(),
        );
        if let ExportOutcome::Retry(reason) = outcome {
            tracing::warn!(
                endpoint = %http_client::redact_endpoint(self.uri()),
                reason = %reason,
                retry_seconds = delay.as_secs_f64(),
                "PerfSentinelHub detection export failed; retaining the day"
            );
        }
        delay
    }

    /// Count `rows` of the front day as dropped and flag the day a gap, in
    /// the ledger and on the page still to carry its `DayRow`.
    fn drop_rows(&mut self, rows: usize, metrics: &MetricsState) {
        metrics.hub_export_dropped_total.inc_by(rows as u64);
        if let Some(front) = self.queue.front_mut() {
            self.ledger.mark_gap_quiet(front.day);
            front.meta.gap = true;
        }
        self.advance(rows);
    }

    /// `rows` more of the front day are settled. Once all of them are, the
    /// Hub holds the day as of the snapshot.
    fn advance(&mut self, rows: usize) {
        self.sent += rows;
        let done = self
            .queue
            .front()
            .filter(|front| self.sent >= front.rows.len())
            .map(|front| (front.day, front.rev));
        if let Some((day, rev)) = done {
            self.ledger.ack_day(day, rev);
            self.queue.pop_front();
            self.sent = 0;
        }
    }

    /// One unsplit page of the front day to the older route: true when it is
    /// accepted, so the split route's 401 or 403 meant a Hub without it.
    async fn older_route_accepts(&self, client: &HubClient, api_key: &str) -> bool {
        let day = self.queue[0].day;
        let Some(snapshot) = self
            .ledger
            .dirty_days(false)
            .into_iter()
            .find(|snapshot| snapshot.day == day)
        else {
            return false;
        };
        let Some((_, body)) = next_page(&self.header, &snapshot, 0, false) else {
            return false;
        };
        matches!(
            post_batch_with_reply(client, &self.uri, api_key, body).await,
            Ok((status, _)) if status.is_success()
        )
    }

    /// The route in use: the split one until the Hub refuses it.
    fn uri(&self) -> &http_client::Uri {
        self.split_uri.as_ref().unwrap_or(&self.uri)
    }

    /// An older Hub, refusing the split route before accepting any page of
    /// it: the unsplit route from now on, every retained day sent
    /// again whole, the refused page's rows among them, so none is lost.
    fn fall_back(&mut self) {
        if let Some(split_uri) = self.split_uri.take() {
            tracing::warn!(
                endpoint = %http_client::redact_endpoint(&split_uri),
                "PerfSentinelHub has no per-grouping detection import route; pushing detection counts summed per signature to the older route until restart"
            );
        }
        // No split page was accepted before the first refusal, but the older
        // route must get full totals whatever this process sent before.
        self.ledger.forget_acks();
        self.queue.clear();
        self.refill();
        // A drain stops on a failure; the split route's ones are not the old route's.
        self.failures = 0;
    }

    fn unsupported(&mut self) {
        self.unsupported = true;
        self.queue.clear();
        tracing::warn!(
            endpoint = %http_client::redact_endpoint(self.uri()),
            "PerfSentinelHub has no detection import route; detection counts are not pushed until restart, the other exports are"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use crate::config::DaemonDetectionsConfig;
    use crate::daemon::detection_ledger::{DAY_MS, DayCarbon, DayThresholds, DetectionLedger};
    use crate::daemon::hub_export::HubClient;
    use crate::daemon::hub_export::HubExportBuffer;
    use crate::daemon::hub_export::tests::{count, finding, spawn_with_detections, wait_for};
    use crate::daemon::hub_incident_export::tests::{Seen, fake_hub, fake_hub_routes4};
    use crate::report::metrics::MetricsState;

    const DAY: u64 = 20_000;

    fn ledger_at(day: u64) -> Arc<DetectionLedger> {
        Arc::new(DetectionLedger::new(
            &DaemonDetectionsConfig::default(),
            true,
            1.0,
            DayThresholds::default(),
            day * DAY_MS,
        ))
    }

    fn record(ledger: &DetectionLedger, day: u64, signatures: &[&str]) {
        let findings: Vec<_> = signatures.iter().map(|s| finding(s, "trace")).collect();
        ledger.record(&findings, day * DAY_MS + 1);
    }

    fn export(base: &str, ledger: &Arc<DetectionLedger>) -> DetectionExport {
        let uri = |route: fn(&str) -> String| {
            format!(
                "{}?source_id=lab",
                route(&format!("{base}/api/import/findings"))
            )
            .parse()
            .expect("uri")
        };
        DetectionExport::new(
            ledger.clone(),
            uri(split_detections_endpoint),
            uri(detections_endpoint),
        )
    }

    fn record_grouped(ledger: &DetectionLedger, day: u64, rows: &[(&str, &str)]) {
        let findings: Vec<_> = rows
            .iter()
            .map(|(signature, grouping)| {
                let mut f = finding(signature, "trace");
                f.grouping = vec![crate::event::GroupingAttribute {
                    key: Arc::from("k8s.namespace.name"),
                    value: Arc::from(*grouping),
                }];
                f
            })
            .collect();
        ledger.record(&findings, day * DAY_MS + 1);
    }

    /// A Hub that answers the split route with `split` and any other with 200.
    async fn fake_hub_split(split: u16) -> (String, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let store = seen.clone();
        let app =
            axum::Router::new().fallback(move |uri: axum::http::Uri, body: axum::body::Bytes| {
                let store = store.clone();
                async move {
                    let status = if uri.path().ends_with("/by-grouping") {
                        split
                    } else {
                        200
                    };
                    store
                        .lock()
                        .expect("lock")
                        .push((uri.to_string(), None, body.to_vec()));
                    axum::http::StatusCode::from_u16(status).expect("status")
                }
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{address}"), seen)
    }

    /// The bodies posted to the unsplit route alone.
    fn unsplit_bodies(seen: &Seen) -> Vec<serde_json::Value> {
        seen.lock()
            .expect("lock")
            .iter()
            .filter(|(uri, _, _)| uri.starts_with("/api/import/detections?"))
            .map(|(_, _, body)| serde_json::from_slice(body).expect("json"))
            .collect()
    }

    /// Rounds until one sends nothing, flush interval zero so none waits.
    async fn rounds(export: &mut DetectionExport, metrics: &MetricsState) -> usize {
        let client = HubClient::new(Duration::from_secs(10));
        let mut sent = 0;
        while export
            .round(&client, &"k".repeat(32), metrics, Duration::ZERO, false)
            .await
        {
            sent += 1;
            assert!(sent < 100, "the export never goes quiet");
        }
        sent
    }

    fn bodies(seen: &Seen) -> Vec<serde_json::Value> {
        seen.lock()
            .expect("lock")
            .iter()
            .filter(|(uri, _, _)| uri.starts_with("/api/import/detections"))
            .map(|(_, _, body)| serde_json::from_slice(body).expect("json"))
            .collect()
    }

    fn day_row(ledger: &DetectionLedger, day: u64) -> DayRow {
        ledger
            .page(None, None, 5000, false)
            .days
            .into_iter()
            .find(|row| u64::from(row.day) == day)
            .expect("day")
    }

    #[test]
    fn the_detections_route_sits_beside_the_findings_one() {
        assert_eq!(
            "https://hub.example.com/api/import/detections",
            detections_endpoint("https://hub.example.com/api/import/findings")
        );
    }

    #[test]
    fn the_split_route_sits_beside_the_detections_one() {
        assert_eq!(
            "https://hub.example.com/api/import/detections/by-grouping",
            split_detections_endpoint("https://hub.example.com/api/import/findings")
        );
    }

    #[tokio::test]
    async fn pushes_split_pages_to_the_by_grouping_route() {
        let (base, seen) = fake_hub(200).await;
        let ledger = ledger_at(DAY);
        record_grouped(&ledger, DAY, &[("a", "ns-a"), ("a", "ns-b")]);
        record(&ledger, DAY, &["b"]);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);
        assert_eq!(1, rounds(&mut export, &metrics).await);

        // A changed grouping row goes out again, alone.
        record_grouped(&ledger, DAY, &[("a", "ns-b")]);
        export.start_at(tokio::time::Instant::now());
        assert_eq!(1, rounds(&mut export, &metrics).await);

        for (uri, _, _) in seen.lock().expect("lock").iter() {
            assert!(
                uri.ends_with("/api/import/detections/by-grouping?source_id=lab"),
                "{uri}"
            );
        }
        let pages = bodies(&seen);
        assert_eq!(2, pages.len());
        assert_eq!("grouping", pages[0]["split"]);
        let rows = |page: &serde_json::Value| -> Vec<(String, String, u64)> {
            page["counts"]
                .as_array()
                .expect("counts")
                .iter()
                .map(|row| {
                    (
                        row["signature"].as_str().expect("signature").to_string(),
                        row["grouping"].as_str().unwrap_or("").to_string(),
                        row["detections"].as_u64().expect("detections"),
                    )
                })
                .collect()
        };
        let row = |s: &str, g: &str, n: u64| (s.to_string(), g.to_string(), n);
        assert_eq!(
            vec![row("a", "ns-a", 1), row("a", "ns-b", 1), row("b", "", 1)],
            rows(&pages[0])
        );
        assert!(pages[0]["counts"][2].get("grouping").is_none());
        assert_eq!(vec![row("a", "ns-b", 2)], rows(&pages[1]));
        assert_eq!(DAY, pages[1]["days"][0]["day"]);
        assert!(ledger.dirty_days(true).is_empty());
    }

    #[tokio::test]
    async fn an_older_hub_gets_the_same_counts_unsplit_after_a_404() {
        let (base, seen) = fake_hub_split(404).await;
        let ledger = ledger_at(DAY);
        ledger.tick((DAY + 1) * DAY_MS, 0, 0);
        record_grouped(
            &ledger,
            DAY + 1,
            &[("a", "ns-a"), ("a", "ns-b"), ("a", "ns-b")],
        );
        record(&ledger, DAY + 1, &["a", "b"]);
        let metrics = MetricsState::new();

        assert_eq!(3, rounds(&mut export(&base, &ledger), &metrics).await);

        assert_eq!(1, count(&seen, "/api/import/detections/by-grouping"));
        let pages = unsplit_bodies(&seen);
        assert_eq!(2, pages.len(), "both retained days, unsplit");
        let mut pushed = std::collections::BTreeMap::new();
        for page in &pages {
            assert!(page.get("split").is_none());
            for row in page["counts"].as_array().expect("counts") {
                assert!(row.get("grouping").is_none());
                let key = (
                    row["day"].as_u64().expect("day"),
                    row["signature"].as_str().expect("signature").to_string(),
                );
                *pushed.entry(key).or_insert(0) += row["detections"].as_u64().expect("n");
            }
        }
        let held: std::collections::BTreeMap<_, _> = ledger
            .page(None, None, 5000, false)
            .counts
            .iter()
            .map(|row| ((u64::from(row.day), row.signature.clone()), row.detections))
            .collect();
        assert_eq!(held, pushed);
        assert_eq!(4, held[&(DAY + 1, "a".to_string())]);
        assert_eq!(DAY + 1, pages[1]["days"][0]["day"]);
        assert_eq!(false, pages[1]["days"][0]["gap"]);
        assert!(!day_row(&ledger, DAY + 1).gap);
        assert_eq!(0, metrics.hub_export_dropped_total.get());
        assert!(ledger.dirty_days(false).is_empty());
    }

    #[tokio::test]
    async fn the_fallback_holds_for_the_rest_of_the_process() {
        let (base, seen) = fake_hub_split(405).await;
        let ledger = ledger_at(DAY);
        record_grouped(&ledger, DAY, &[("a", "ns-a")]);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);
        assert_eq!(2, rounds(&mut export, &metrics).await);

        record_grouped(&ledger, DAY, &[("a", "ns-b")]);
        export.start_at(tokio::time::Instant::now());
        assert_eq!(1, rounds(&mut export, &metrics).await);

        assert_eq!(1, count(&seen, "/api/import/detections/by-grouping"));
        let pages = unsplit_bodies(&seen);
        assert_eq!(2, pages.len());
        assert_eq!(2, pages[1]["counts"][0]["detections"]);
        assert!(ledger.dirty_days(false).is_empty());
    }

    #[tokio::test]
    async fn an_older_hub_refusing_the_split_route_401_is_probed_and_falls_back() {
        // An older Hub with sign-in answers its unknown routes 401.
        let (base, seen) = fake_hub_split(401).await;
        let ledger = ledger_at(DAY);
        record_grouped(&ledger, DAY, &[("a", "ns-a"), ("a", "ns-b")]);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);
        rounds(&mut export, &metrics).await;

        assert_eq!(1, count(&seen, "/api/import/detections/by-grouping"));
        let pages = unsplit_bodies(&seen);
        assert_eq!(2, pages.len(), "the probe, then the whole day again");
        for page in &pages {
            assert!(page.get("split").is_none());
            assert_eq!(2, page["counts"][0]["detections"]);
        }
        assert!(ledger.dirty_days(false).is_empty());
        assert_eq!(0, metrics.hub_export_dropped_total.get());

        record_grouped(&ledger, DAY, &[("a", "ns-b")]);
        export.start_at(tokio::time::Instant::now());
        rounds(&mut export, &metrics).await;
        assert_eq!(1, count(&seen, "/api/import/detections/by-grouping"));
        assert_eq!(3, unsplit_bodies(&seen).len());
    }

    #[tokio::test]
    async fn a_hub_refusing_both_routes_403_keeps_the_split_route() {
        // A wrong key: the older route refuses the probe too.
        let (base, seen) = fake_hub(403).await;
        let ledger = ledger_at(DAY);
        record_grouped(&ledger, DAY, &[("a", "ns-a")]);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);
        assert_eq!(1, rounds(&mut export, &metrics).await);
        assert!(export.due() > tokio::time::Instant::now(), "it backs off");

        export.start_at(tokio::time::Instant::now());
        assert_eq!(1, rounds(&mut export, &metrics).await);

        assert_eq!(2, count(&seen, "/api/import/detections/by-grouping"));
        assert_eq!(2, unsplit_bodies(&seen).len(), "one probe a refusal");
        assert_eq!(1, ledger.dirty_days(true).len());
        assert_eq!(0, metrics.hub_export_dropped_total.get());
    }

    #[tokio::test]
    async fn a_split_route_lost_after_an_accepted_page_is_retried() {
        let (base, seen) = fake_hub_sequence(vec![200, 404]).await;
        let ledger = ledger_at(DAY);
        record_grouped(&ledger, DAY, &[("a", "ns-a")]);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);
        assert_eq!(1, rounds(&mut export, &metrics).await);

        // A Hub that took a split page has the route: a 404 is transient.
        record_grouped(&ledger, DAY, &[("a", "ns-b")]);
        export.start_at(tokio::time::Instant::now());
        assert_eq!(1, rounds(&mut export, &metrics).await);

        assert_eq!(2, count(&seen, "/api/import/detections/by-grouping"));
        assert_eq!(0, unsplit_bodies(&seen).len());
        assert_eq!(1, ledger.dirty_days(true).len());
        assert!(export.due() > tokio::time::Instant::now(), "it backs off");
        assert_eq!(0, metrics.hub_export_dropped_total.get());
    }

    #[tokio::test]
    async fn a_drain_after_a_failure_finishes_the_unsplit_resend() {
        let (base, seen) = fake_hub_sequence(vec![503, 404, 200]).await;
        let ledger = ledger_at(DAY);
        record_grouped(&ledger, DAY, &[("a", "ns-a")]);
        let metrics = MetricsState::new();
        let client = HubClient::new(Duration::from_secs(10));
        let mut export = export(&base, &ledger);
        let key = "k".repeat(32);
        assert!(
            export
                .round(&client, &key, &metrics, Duration::ZERO, false)
                .await
        );

        assert!(
            export
                .round(&client, &key, &metrics, Duration::ZERO, true)
                .await
        );

        assert_eq!(1, unsplit_bodies(&seen).len());
        assert!(ledger.dirty_days(false).is_empty());
    }

    #[tokio::test]
    async fn pushes_each_dirty_day_once_then_goes_quiet() {
        let (base, seen) = fake_hub(200).await;
        let ledger = ledger_at(DAY);
        record(&ledger, DAY, &["a", "b"]);
        ledger.tick((DAY + 1) * DAY_MS, 0, 0);
        record(&ledger, DAY + 1, &["c"]);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);

        assert_eq!(2, rounds(&mut export, &metrics).await);
        assert_eq!(0, rounds(&mut export, &metrics).await);

        let requests = seen.lock().expect("lock").clone();
        assert_eq!(2, requests.len());
        for (uri, key, _) in &requests {
            assert!(
                uri.ends_with("/api/import/detections/by-grouping?source_id=lab"),
                "{uri}"
            );
            assert_eq!(Some("k".repeat(32)), *key);
        }
        let pages = bodies(&seen);
        assert_eq!(DAY, pages[0]["days"][0]["day"]);
        assert_eq!(true, pages[0]["days"][0]["sealed"]);
        assert_eq!(2, pages[0]["counts"].as_array().expect("counts").len());
        assert_eq!(DAY + 1, pages[1]["days"][0]["day"]);
        assert_eq!("c", pages[1]["counts"][0]["signature"]);
        let status = ledger.status();
        for page in &pages {
            assert_eq!(status.ledger_id, page["ledger_id"]);
            assert_eq!(DAY + 1, page["current_day"]);
            assert_eq!(DAY, page["oldest_day"]);
            assert_eq!(DAY, page["sealed_through"]);
            assert!(page["next_after"].is_null());
        }
        assert_eq!(0, ledger.dirty_days(false).len());
    }

    #[tokio::test]
    async fn only_rows_changed_since_the_last_ack_are_pushed() {
        let (base, seen) = fake_hub(200).await;
        let ledger = ledger_at(DAY);
        record(&ledger, DAY, &["a", "b"]);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);
        assert_eq!(1, rounds(&mut export, &metrics).await);

        record(&ledger, DAY, &["a"]);
        export.start_at(tokio::time::Instant::now());
        assert_eq!(1, rounds(&mut export, &metrics).await);

        let pages = bodies(&seen);
        assert_eq!(2, pages[0]["counts"].as_array().expect("counts").len());
        let counts = pages[1]["counts"].as_array().expect("counts");
        assert_eq!(1, counts.len());
        assert_eq!("a", counts[0]["signature"]);
        assert_eq!(2, counts[0]["detections"]);
        assert_eq!(DAY, pages[1]["days"][0]["day"]);
        assert_eq!(0, ledger.dirty_days(false).len());
    }

    fn some_carbon() -> DayCarbon {
        DayCarbon {
            gco2_mid: 0.5,
            gco2_low: 0.25,
            avoidable_io_ops: 4,
            total_io_ops: 16,
        }
    }

    #[tokio::test]
    async fn the_day_row_carries_the_avoidable_carbon() {
        let (base, seen) = fake_hub(200).await;
        let ledger = ledger_at(DAY);
        record(&ledger, DAY, &["a"]);
        ledger.record_avoidable(some_carbon(), DAY * DAY_MS + 1);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);
        assert_eq!(1, rounds(&mut export, &metrics).await);

        let day = &bodies(&seen)[0]["days"][0];
        assert_eq!(0.5, day["avoidable_gco2"]);
        assert_eq!(0.25, day["avoidable_gco2_low"]);
        assert_eq!(4, day["avoidable_io_ops"]);
        assert_eq!(16, day["total_io_ops"]);
    }

    #[tokio::test]
    async fn a_day_without_carbon_pushes_no_carbon_keys() {
        let (base, seen) = fake_hub(200).await;
        let ledger = ledger_at(DAY);
        record(&ledger, DAY, &["a"]);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);
        assert_eq!(1, rounds(&mut export, &metrics).await);

        let day = &bodies(&seen)[0]["days"][0];
        for key in [
            "avoidable_gco2",
            "avoidable_gco2_low",
            "avoidable_io_ops",
            "total_io_ops",
        ] {
            assert!(day.get(key).is_none(), "{key}");
        }
    }

    #[tokio::test]
    async fn a_carbon_only_change_resends_the_day_row() {
        let (base, seen) = fake_hub(200).await;
        let ledger = ledger_at(DAY);
        record(&ledger, DAY, &["a"]);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);
        assert_eq!(1, rounds(&mut export, &metrics).await);

        ledger.record_avoidable(some_carbon(), DAY * DAY_MS + 1);
        export.start_at(tokio::time::Instant::now());
        assert_eq!(1, rounds(&mut export, &metrics).await);

        let pages = bodies(&seen);
        assert_eq!(2, pages.len());
        assert_eq!(pages[1]["counts"], serde_json::json!([]));
        assert_eq!(4, pages[1]["days"][0]["avoidable_io_ops"]);
        assert_eq!(0, ledger.dirty_days(false).len());
    }

    #[tokio::test]
    async fn the_detection_half_waits_at_least_a_minute_between_rounds() {
        let (base, _seen) = fake_hub(200).await;
        let ledger = ledger_at(DAY);
        record(&ledger, DAY, &["a"]);
        let client = HubClient::new(Duration::from_secs(10));
        let mut export = export(&base, &ledger);

        let before = tokio::time::Instant::now();
        assert!(
            export
                .round(
                    &client,
                    &"k".repeat(32),
                    &MetricsState::new(),
                    Duration::from_secs(5),
                    false
                )
                .await
        );
        assert_eq!(0, ledger.dirty_days(false).len());
        assert!(export.due() >= before + Duration::from_mins(1));
    }

    #[tokio::test]
    async fn a_large_day_goes_out_in_pages_with_its_meta_on_the_last() {
        let (base, seen) = fake_hub(200).await;
        let ledger = ledger_at(DAY);
        let signatures: Vec<String> = (0..5_001).map(|i| format!("s{i:05}")).collect();
        let refs: Vec<&str> = signatures.iter().map(String::as_str).collect();
        record(&ledger, DAY, &refs);

        assert_eq!(
            2,
            rounds(&mut export(&base, &ledger), &MetricsState::new()).await
        );

        let pages = bodies(&seen);
        assert_eq!(5_000, pages[0]["counts"].as_array().expect("counts").len());
        assert_eq!(pages[0]["days"], serde_json::json!([]));
        assert_eq!("s05000", pages[1]["counts"][0]["signature"]);
        assert_eq!(DAY, pages[1]["days"][0]["day"]);
    }

    #[tokio::test]
    async fn a_rejected_page_is_dropped_and_the_day_marked_gap() {
        let (base, seen) = fake_hub(400).await;
        let ledger = ledger_at(DAY);
        ledger.tick((DAY + 1) * DAY_MS, 0, 0);
        record(&ledger, DAY + 1, &["a", "b"]);
        assert!(!day_row(&ledger, DAY + 1).gap);
        let metrics = MetricsState::new();
        let mut export = export(&base, &ledger);

        assert_eq!(2, rounds(&mut export, &metrics).await);
        assert_eq!(0, rounds(&mut export, &metrics).await);

        assert_eq!(2, metrics.hub_export_dropped_total.get());
        assert_eq!(2, seen.lock().expect("lock").len());
        // A 400 on the split route drops the page, it never falls back.
        assert_eq!(2, count(&seen, "/api/import/detections/by-grouping"));
        assert!(day_row(&ledger, DAY + 1).gap);
        assert_eq!(0, ledger.dirty_days(false).len());
    }

    #[tokio::test]
    async fn a_hub_without_the_route_stops_detection_export_for_good() {
        let (base, seen) = fake_hub_routes4(200, 200, 200, 404).await;
        let findings = Arc::new(HubExportBuffer::new(100));
        let ledger = ledger_at(DAY);
        record(&ledger, DAY, &["a"]);
        findings.push_batch(&[finding("first", "trace")], 1);
        let (handle, _shutdown) = spawn_with_detections(
            &base,
            &findings,
            export(&base, &ledger),
            Duration::from_millis(50),
        );

        // One probe of each detection route.
        wait_for(|| {
            count(&seen, "/api/import/detections") == 2 && count(&seen, "/api/import/findings") == 1
        })
        .await;
        record(&ledger, DAY, &["b"]);
        findings.push_batch(&[finding("second", "trace")], 1);
        wait_for(|| count(&seen, "/api/import/findings") == 2).await;
        // A few flush intervals more, for a probe that leaked.
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        assert_eq!(1, count(&seen, "/api/import/detections/by-grouping"));
        assert_eq!(2, count(&seen, "/api/import/detections"));
        assert_eq!(2, count(&seen, "/api/import/findings"));
    }

    /// A Hub that answers each request with the next of `statuses`, the
    /// last one from then on.
    async fn fake_hub_sequence(statuses: Vec<u16>) -> (String, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let store = seen.clone();
        let app =
            axum::Router::new().fallback(move |uri: axum::http::Uri, body: axum::body::Bytes| {
                let store = store.clone();
                let statuses = statuses.clone();
                async move {
                    let mut requests = store.lock().expect("lock");
                    let status = statuses[requests.len().min(statuses.len() - 1)];
                    requests.push((uri.to_string(), None, body.to_vec()));
                    axum::http::StatusCode::from_u16(status).expect("status")
                }
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{address}"), seen)
    }

    #[tokio::test]
    async fn a_busy_hub_keeps_the_day_dirty() {
        let (base, seen) = fake_hub_sequence(vec![503, 200]).await;
        let ledger = ledger_at(DAY);
        record(&ledger, DAY, &["a"]);
        let metrics = MetricsState::new();
        let client = HubClient::new(Duration::from_secs(10));
        let mut export = export(&base, &ledger);

        assert!(
            export
                .round(&client, &"k".repeat(32), &metrics, Duration::ZERO, false)
                .await
        );
        assert_eq!(1, ledger.dirty_days(false).len());
        assert!(export.due() > tokio::time::Instant::now(), "it backs off");
        export.start_at(tokio::time::Instant::now());
        assert_eq!(1, rounds(&mut export, &metrics).await);

        let pages = bodies(&seen);
        assert_eq!(2, pages.len());
        assert_eq!(pages[0]["counts"], pages[1]["counts"]);
        assert_eq!(0, metrics.hub_export_dropped_total.get());
        assert_eq!(0, ledger.dirty_days(false).len());
    }

    #[tokio::test]
    async fn a_refused_page_leaves_the_rest_of_the_day_to_go_out_flagged_gap() {
        let (base, seen) = fake_hub_sequence(vec![400, 200]).await;
        let ledger = ledger_at(DAY);
        ledger.tick((DAY + 1) * DAY_MS, 0, 0);
        // The Hub already holds the sealed day, so the 400 lands on the large one.
        for day in ledger.dirty_days(false) {
            ledger.ack_day(day.day, day.rev);
        }
        let signatures: Vec<String> = (0..5_001).map(|i| format!("s{i:05}")).collect();
        let refs: Vec<&str> = signatures.iter().map(String::as_str).collect();
        record(&ledger, DAY + 1, &["s00000"]);
        assert!(!day_row(&ledger, DAY + 1).gap);
        record(&ledger, DAY + 1, &refs);
        let metrics = MetricsState::new();

        assert_eq!(2, rounds(&mut export(&base, &ledger), &metrics).await);

        let pages = bodies(&seen);
        assert_eq!(2, pages.len());
        assert_eq!("s05000", pages[1]["counts"][0]["signature"]);
        assert_eq!(DAY + 1, pages[1]["days"][0]["day"]);
        assert_eq!(true, pages[1]["days"][0]["gap"]);
        assert_eq!(5_000, metrics.hub_export_dropped_total.get());
        assert_eq!(0, ledger.dirty_days(false).len());
    }

    #[tokio::test]
    async fn a_drain_against_a_hub_without_the_route_probes_once_and_ends() {
        let (base, seen) = fake_hub_routes4(200, 200, 200, 404).await;
        let ledger = ledger_at(DAY);
        record(&ledger, DAY, &["a"]);
        let (handle, shutdown) = spawn_with_detections(
            &base,
            &Arc::new(HubExportBuffer::new(10)),
            export(&base, &ledger),
            Duration::from_hours(1),
        );
        shutdown.notify_one();

        // Far inside the 10 s drain budget.
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("the drain ends on the missing route")
            .expect("exporter task panicked");
        assert_eq!(1, count(&seen, "/api/import/detections/by-grouping"));
        assert_eq!(2, count(&seen, "/api/import/detections"));
        // Not dropped as a refused page: the day waits for a Hub that has the route.
        assert_eq!(1, ledger.dirty_days(false).len());
    }

    #[tokio::test]
    async fn a_drain_sends_every_findings_batch_before_the_detections() {
        let (base, seen) = fake_hub(200).await;
        let ledger = ledger_at(DAY);
        record(&ledger, DAY, &["a"]);
        let findings = Arc::new(HubExportBuffer::new(300));
        // Batches of 100: three findings rounds before the queue is empty.
        let batch: Vec<_> = (0..250)
            .map(|i| finding(&format!("f{i}"), "trace"))
            .collect();
        findings.push_batch(&batch, 1);
        let (handle, shutdown) = spawn_with_detections(
            &base,
            &findings,
            export(&base, &ledger),
            Duration::from_hours(1),
        );
        shutdown.notify_one();

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("the drain ends once everything is sent")
            .expect("exporter task panicked");
        let paths: Vec<String> = seen
            .lock()
            .expect("lock")
            .iter()
            .map(|(uri, _, _)| uri.clone())
            .collect();
        let last_findings = paths
            .iter()
            .rposition(|p| p.starts_with("/api/import/findings"))
            .expect("findings were sent");
        let first_detections = paths
            .iter()
            .position(|p| p.starts_with("/api/import/detections"))
            .expect("the ledger was sent");
        assert_eq!(3, count(&seen, "/api/import/findings"));
        assert!(last_findings < first_detections, "{paths:?}");
    }

    #[tokio::test]
    async fn a_drain_forces_one_last_push_and_returns() {
        let (base, seen) = fake_hub(200).await;
        let now = crate::daemon::event_loop::current_time_ms();
        let today = now / DAY_MS;
        // Started yesterday, so today opens without the restart gap.
        let ledger = ledger_at(today - 1);
        ledger.tick(now, 0, 0);
        ledger.record(&[finding("a", "trace")], now);
        assert!(!day_row(&ledger, today).gap);
        let (handle, shutdown) = spawn_with_detections(
            &base,
            &Arc::new(HubExportBuffer::new(10)),
            export(&base, &ledger),
            Duration::from_hours(1),
        );
        tokio::task::yield_now().await;
        ledger.mark_drain_gap(now);
        shutdown.notify_one();

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("the drain ends once the ledger is sent")
            .expect("exporter task panicked");
        let pages = bodies(&seen);
        let page = pages
            .iter()
            .find(|page| page["days"][0]["day"] == today)
            .expect("the open day was sent");
        assert_eq!(true, page["days"][0]["gap"]);
        assert_eq!("a", page["counts"][0]["signature"]);
        assert_eq!(0, ledger.dirty_days(false).len());
    }

    const SLOW: &str = "slow_sql:svc:GET__a:0123456789abcdef0123456789abcdef";

    /// A Hub that answers every request with `status` and `reply` as body.
    async fn fake_hub_replying(status: u16, reply: Vec<u8>) -> (String, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let store = seen.clone();
        let app =
            axum::Router::new().fallback(move |uri: axum::http::Uri, body: axum::body::Bytes| {
                let store = store.clone();
                let reply = reply.clone();
                async move {
                    store
                        .lock()
                        .expect("lock")
                        .push((uri.to_string(), None, body.to_vec()));
                    (
                        axum::http::StatusCode::from_u16(status).expect("status"),
                        reply,
                    )
                }
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{address}"), seen)
    }

    /// One dirty day pushed to a Hub answering `status` with `reply`, the
    /// watch list preset to `SLOW` when `preset`.
    async fn push_once(status: u16, reply: Vec<u8>, preset: bool) -> (Arc<DetectionLedger>, Seen) {
        let (base, seen) = fake_hub_replying(status, reply).await;
        let ledger = ledger_at(DAY);
        if preset {
            ledger.set_watch(&[SLOW.to_string()]);
        }
        record(&ledger, DAY, &["a"]);
        assert_eq!(
            1,
            rounds(&mut export(&base, &ledger), &MetricsState::new()).await
        );
        (ledger, seen)
    }

    #[tokio::test]
    async fn an_accepted_reply_replaces_the_watch_list() {
        let reply = format!(r#"{{"accepted":1,"rejected":0,"watch":["{SLOW}"]}}"#);
        let (ledger, _seen) = push_once(200, reply.into_bytes(), false).await;
        assert_eq!(1, ledger.watch().len());
    }

    #[tokio::test]
    async fn a_reply_without_watch_clears_the_list() {
        let (ledger, _seen) =
            push_once(200, br#"{"accepted":1,"rejected":0}"#.to_vec(), true).await;
        assert!(ledger.watch().is_empty());
    }

    #[tokio::test]
    async fn an_unparsable_reply_keeps_the_list_and_the_page_counts() {
        let (ledger, seen) = push_once(200, b"not json".to_vec(), true).await;
        assert_eq!(1, ledger.watch().len());
        assert!(ledger.dirty_days(false).is_empty());
        assert_eq!(1, seen.lock().expect("lock").len());
    }

    #[tokio::test]
    async fn a_non_string_entry_keeps_the_list() {
        let (ledger, _seen) = push_once(200, br#"{"watch":[1]}"#.to_vec(), true).await;
        assert_eq!(1, ledger.watch().len());
        assert!(ledger.dirty_days(false).is_empty());
    }

    #[tokio::test]
    async fn a_failed_page_keeps_the_list() {
        let (ledger, _seen) = push_once(503, br#"{"watch":[]}"#.to_vec(), true).await;
        assert_eq!(1, ledger.watch().len());
        assert_eq!(1, ledger.dirty_days(false).len());
    }

    #[tokio::test]
    async fn an_oversized_reply_counts_as_accepted() {
        let reply = vec![b' '; http_client::MAX_BODY_BYTES + 1];
        let (ledger, seen) = push_once(200, reply, true).await;
        assert!(ledger.dirty_days(false).is_empty());
        assert_eq!(1, seen.lock().expect("lock").len());
        assert_eq!(1, ledger.watch().len());
    }
}
