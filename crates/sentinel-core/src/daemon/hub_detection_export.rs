//! The detection ledger on its way to the Hub: the rows of each day that
//! moved since the Hub last accepted it, in pages shaped like
//! `GET /api/detections`, at most one snapshot a minute outside the drain.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use super::detection_ledger::{
    CountRow, DayRow, DaySnapshot, DetectionLedger, DetectionLedgerStatus, MAX_PAGE_BYTES,
};
use super::hub_export::{ExportOutcome, HubClient, bounded_body, classify, next_delay, post_batch};
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
}

/// The rows of `day` from `from` on that fit one page, and its body. The
/// day's `DayRow` rides only on the page that carries its last row.
fn next_page(
    header: &DetectionLedgerStatus,
    day: &DaySnapshot,
    from: usize,
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
    pub(crate) fn new(ledger: Arc<DetectionLedger>, uri: http_client::Uri) -> Self {
        Self {
            header: ledger.status(),
            ledger,
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
        let Some((rows, body)) = next_page(&self.header, &self.queue[0], self.sent) else {
            // A row alone over the cap: no page can carry it.
            self.drop_rows(usize::from(self.sent < self.queue[0].rows.len()), metrics);
            self.due = tokio::time::Instant::now();
            return true;
        };
        let outcome = match post_batch(client, &self.uri, api_key, body).await {
            Ok(status) if is_missing_route(status) => {
                self.unsupported();
                self.due = tokio::time::Instant::now() + pace;
                return false;
            }
            Ok(status) => classify(status),
            Err(reason) => ExportOutcome::Retry(reason),
        };
        let mut delay = self.apply(&outcome, rows, metrics, flush_interval);
        if self.queue.is_empty() {
            delay = delay.max(pace);
        }
        self.due = tokio::time::Instant::now() + delay;
        true
    }

    /// Snapshot the dirty days once the last snapshot is sent. False when
    /// none is dirty.
    fn refill(&mut self) -> bool {
        if self.queue.is_empty() {
            // The header first, so `sealed_through` never runs ahead of the rows.
            self.header = self.ledger.status();
            self.queue = self.ledger.dirty_days().into();
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
                    endpoint = %http_client::redact_endpoint(&self.uri),
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
                endpoint = %http_client::redact_endpoint(&self.uri),
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

    fn unsupported(&mut self) {
        self.unsupported = true;
        self.queue.clear();
        tracing::warn!(
            endpoint = %http_client::redact_endpoint(&self.uri),
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
    use crate::daemon::detection_ledger::{DAY_MS, DetectionLedger};
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
            day * DAY_MS,
        ))
    }

    fn record(ledger: &DetectionLedger, day: u64, signatures: &[&str]) {
        let findings: Vec<_> = signatures.iter().map(|s| finding(s, "trace")).collect();
        ledger.record(&findings, day * DAY_MS + 1);
    }

    fn export(base: &str, ledger: &Arc<DetectionLedger>) -> DetectionExport {
        let uri = format!(
            "{}?source_id=lab",
            detections_endpoint(&format!("{base}/api/import/findings"))
        );
        DetectionExport::new(ledger.clone(), uri.parse().expect("uri"))
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

    fn day_row(ledger: &DetectionLedger, day: u64) -> crate::daemon::detection_ledger::DayRow {
        ledger
            .page(None, None, 5000)
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
                uri.ends_with("/api/import/detections?source_id=lab"),
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
        assert_eq!(0, ledger.dirty_days().len());
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
        assert_eq!(0, ledger.dirty_days().len());
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
        assert_eq!(0, ledger.dirty_days().len());
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
        assert!(day_row(&ledger, DAY + 1).gap);
        assert_eq!(0, ledger.dirty_days().len());
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

        wait_for(|| {
            count(&seen, "/api/import/detections") == 1 && count(&seen, "/api/import/findings") == 1
        })
        .await;
        record(&ledger, DAY, &["b"]);
        findings.push_batch(&[finding("second", "trace")], 1);
        wait_for(|| count(&seen, "/api/import/findings") == 2).await;
        // A few flush intervals more, for a probe that leaked.
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        assert_eq!(1, count(&seen, "/api/import/detections"));
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
        assert_eq!(1, ledger.dirty_days().len());
        assert!(export.due() > tokio::time::Instant::now(), "it backs off");
        export.start_at(tokio::time::Instant::now());
        assert_eq!(1, rounds(&mut export, &metrics).await);

        let pages = bodies(&seen);
        assert_eq!(2, pages.len());
        assert_eq!(pages[0]["counts"], pages[1]["counts"]);
        assert_eq!(0, metrics.hub_export_dropped_total.get());
        assert_eq!(0, ledger.dirty_days().len());
    }

    #[tokio::test]
    async fn a_refused_page_leaves_the_rest_of_the_day_to_go_out_flagged_gap() {
        let (base, seen) = fake_hub_sequence(vec![400, 200]).await;
        let ledger = ledger_at(DAY);
        ledger.tick((DAY + 1) * DAY_MS, 0, 0);
        // The Hub already holds the sealed day, so the 400 lands on the large one.
        for day in ledger.dirty_days() {
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
        assert_eq!(0, ledger.dirty_days().len());
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
        assert_eq!(1, count(&seen, "/api/import/detections"));
        // Not dropped as a refused page: the day waits for a Hub that has the route.
        assert_eq!(1, ledger.dirty_days().len());
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
        assert_eq!(0, ledger.dirty_days().len());
    }
}
