use std::time::Duration;

use super::*;
use crate::daemon::hub_incident_export::tests::fake_hub;
use crate::detect::correlate_cross::CorrelationConfig;
use crate::detect::{FindingType, Severity};

const T0: u64 = 1_800_000_000_000;
const WINDOW: u64 = 600_000;
const GAP: u64 = 600_000;
const MAX_CLOSED: usize = 10_000;

fn endpoint(service: &str, finding_type: FindingType, template: &str) -> CorrelationEndpoint {
    CorrelationEndpoint {
        finding_type,
        service: service.to_string(),
        template: template.to_string(),
        grouping_key: None,
        grouping_value: None,
    }
}

/// One snapshot row of the pair `order-svc` N+1 then `payment-svc` pool
/// saturation.
pub(crate) fn correlation(
    first_seen_ms: u64,
    last_seen_ms: u64,
    confidence: f64,
    co_occurrences: u32,
) -> CrossTraceCorrelation {
    CrossTraceCorrelation {
        source: endpoint(
            "order-svc",
            FindingType::NPlusOneSql,
            "SELECT * FROM orders WHERE id = ?",
        ),
        target: endpoint("payment-svc", FindingType::PoolSaturation, "pool"),
        co_occurrence_count: co_occurrences,
        source_total_occurrences: co_occurrences,
        confidence,
        median_lag_ms: 1_200.0,
        first_seen: crate::time::millis_to_iso8601(first_seen_ms),
        last_seen: crate::time::millis_to_iso8601(last_seen_ms),
        sample_trace_id: Some("target-trace".to_string()),
        source_sample_trace_id: Some("source-trace".to_string()),
    }
}

/// An open episode of that pair, under the given id.
pub(crate) fn episode(id: &str) -> Episode {
    let row = correlation(T0 - 5_000, T0, 0.8, 10);
    let mut episode = Episode::open(
        &row,
        pair_key(&row.source, &row.target),
        T0 - 5_000,
        T0,
        GAP,
    );
    episode.id = id.to_string();
    episode
}

#[test]
fn the_pair_key_and_episode_id_are_pinned() {
    let row = correlation(T0, T0, 0.8, 10);
    let key = pair_key(&row.source, &row.target);
    // Stable across releases and daemons: the Hub groups episodes on it.
    assert_eq!("0d671493b960acaebfa5688cb91fe4a9", key);
    assert_eq!("5d8c7d7f98c7e297ae27b52b61bff20d", episode_id(&key, T0));
}

#[test]
fn the_pair_key_follows_the_direction_of_the_pair() {
    let row = correlation(T0, T0, 0.8, 10);
    assert_ne!(
        pair_key(&row.source, &row.target),
        pair_key(&row.target, &row.source)
    );
}

#[test]
fn two_namespaces_never_share_a_pair_key() {
    let row = correlation(T0, T0, 0.8, 10);
    let (mut source_a, mut target_a) = (row.source.clone(), row.target.clone());
    for side in [&mut source_a, &mut target_a] {
        side.grouping_key = Some("k8s.namespace.name".to_string());
        side.grouping_value = Some("team-a".to_string());
    }
    let (mut source_b, mut target_b) = (source_a.clone(), target_a.clone());
    source_b.grouping_value = Some("team-b".to_string());
    target_b.grouping_value = Some("team-b".to_string());

    assert_ne!(
        pair_key(&source_a, &target_a),
        pair_key(&source_b, &target_b)
    );
}

#[test]
fn each_start_is_its_own_episode() {
    let key = pair_key(
        &correlation(T0, T0, 0.8, 10).source,
        &correlation(T0, T0, 0.8, 10).target,
    );
    let first = episode_id(&key, T0);
    assert_eq!(32, first.len());
    assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(first, episode_id(&key, T0 + 1));
}

#[test]
fn an_episode_serializes_to_the_wire_format() {
    let json = serde_json::to_value(episode("a")).expect("json");

    let mut keys: Vec<&str> = json
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        vec![
            "ended_at_ms",
            "gap_ms",
            "id",
            "last_co_occurrences",
            "last_confidence",
            "last_seen_ms",
            "median_lag_ms",
            "pair_key",
            "peak_co_occurrences",
            "peak_confidence",
            "sample_trace_id",
            "source",
            "source_sample_trace_id",
            "started_at_ms",
            "target",
        ],
        keys
    );
    assert!(json["ended_at_ms"].is_null());
    assert_eq!("n_plus_one_sql", json["source"]["finding_type"]);
    assert_eq!("payment-svc", json["target"]["service"]);
    assert_eq!(GAP, json["gap_ms"]);
}

#[test]
fn the_gap_is_the_window_up_to_thirty_minutes() {
    assert_eq!(600_000, episode_gap_ms(600_000));
    assert_eq!(1_800_000, episode_gap_ms(7 * 24 * 3_600_000));
}

#[test]
fn a_new_pair_opens_an_episode_at_the_correlator_first_seen() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);

    let queued = tracker.observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0);

    assert_eq!(1, queued.len());
    assert_eq!(T0 - 5_000, queued[0].started_at_ms);
    assert_eq!(T0, queued[0].last_seen_ms);
    assert_eq!(None, queued[0].ended_at_ms);
    assert_eq!(episode_id(&queued[0].pair_key, T0 - 5_000), queued[0].id);
}

#[test]
fn an_advance_under_a_minute_is_held_back_and_one_past_it_is_sent() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);

    assert_eq!(
        0,
        tracker
            .observe(&[correlation(T0, T0 + 10_000, 0.8, 10)], T0 + 10_000)
            .len()
    );
    let queued = tracker.observe(&[correlation(T0, T0 + 60_000, 0.8, 10)], T0 + 60_000);

    assert_eq!(1, queued.len());
    assert_eq!(T0 + 60_000, queued[0].last_seen_ms);
}

#[test]
fn a_rising_peak_is_sent_at_once() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);

    let queued = tracker.observe(&[correlation(T0, T0 + 5_000, 0.9, 10)], T0 + 5_000);
    assert_eq!(1, queued.len());
    assert!((queued[0].peak_confidence - 0.9).abs() < f64::EPSILON);

    let queued = tracker.observe(&[correlation(T0, T0 + 6_000, 0.85, 12)], T0 + 6_000);
    assert_eq!(1, queued.len());
    assert_eq!(12, queued[0].peak_co_occurrences);
    assert!((queued[0].peak_confidence - 0.9).abs() < f64::EPSILON);
    assert!((queued[0].last_confidence - 0.85).abs() < f64::EPSILON);
}

#[test]
fn an_episode_closes_once_last_seen_stood_still_for_the_gap() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);
    let still = correlation(T0, T0, 0.8, 10);
    tracker.observe(std::slice::from_ref(&still), T0);

    assert_eq!(
        0,
        tracker
            .observe(std::slice::from_ref(&still), T0 + GAP - 1)
            .len()
    );
    let closed = tracker.observe(std::slice::from_ref(&still), T0 + GAP);

    assert_eq!(1, closed.len());
    assert_eq!(Some(T0), closed[0].ended_at_ms);
}

#[test]
fn a_pair_gone_from_the_snapshot_closes_after_the_gap() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);

    assert_eq!(0, tracker.observe(&[], T0 + GAP - 1).len());
    let closed = tracker.observe(&[], T0 + GAP);

    assert_eq!(1, closed.len());
    assert_eq!(Some(T0), closed[0].ended_at_ms);
}

#[test]
fn activity_after_the_gap_opens_a_new_episode_at_its_own_start() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);
    let first = tracker.observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0);
    tracker.observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0 + GAP);

    let back = T0 + GAP + 30_000;
    let second = tracker.observe(&[correlation(T0 - 5_000, back, 0.8, 10)], back);

    assert_eq!(1, second.len());
    assert_eq!(back, second[0].started_at_ms);
    assert_eq!(first[0].pair_key, second[0].pair_key);
    assert_ne!(first[0].id, second[0].id);
}

#[test]
fn a_pair_above_the_thresholds_with_a_still_last_seen_stays_closed() {
    let week = 7 * 24 * 3_600_000;
    let gap = episode_gap_ms(week);
    let mut tracker = EpisodeTracker::new(week, MAX_CLOSED);
    let still = correlation(T0, T0, 0.8, 10);
    tracker.observe(std::slice::from_ref(&still), T0);

    let closed = tracker.observe(std::slice::from_ref(&still), T0 + gap);
    assert_eq!(Some(T0), closed[0].ended_at_ms);

    assert_eq!(
        0,
        tracker
            .observe(std::slice::from_ref(&still), T0 + gap + 5_000)
            .len()
    );
    assert_eq!(
        0,
        tracker
            .observe(std::slice::from_ref(&still), T0 + 2 * gap)
            .len()
    );
}

#[test]
fn a_pair_back_in_the_snapshot_does_not_start_before_its_last_episode_ended() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);
    tracker.observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0);
    tracker.observe(&[], T0 + GAP);

    let back = T0 + GAP + 1_000;
    let reopened = tracker.observe(&[correlation(T0 - 5_000, back, 0.8, 10)], back);

    // The correlator still holds the pair's old first_seen, which predates
    // the episode already closed: the new one starts at its own activity.
    assert_eq!(back, reopened[0].started_at_ms);
}

#[test]
fn a_pair_first_met_after_it_went_quiet_opens_nothing() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);

    assert_eq!(
        0,
        tracker
            .observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0 + GAP)
            .len()
    );
    assert_eq!(
        0,
        tracker
            .observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0 + GAP + 5_000)
            .len()
    );
}

#[test]
fn a_pair_back_after_the_gap_starts_a_new_episode_with_its_own_id() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);
    let opened = tracker.observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0);
    // Closed one millisecond past the gap, not on its boundary.
    tracker.observe(&[], T0 + GAP + 1);

    let back = T0 + GAP + 1_000;
    let reopened = tracker.observe(&[correlation(T0 - 5_000, back, 0.8, 10)], back);

    assert_eq!(back, reopened[0].started_at_ms);
    assert_ne!(opened[0].id, reopened[0].id);
}

#[test]
fn closed_pairs_beyond_the_cap_are_forgotten_oldest_first() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);
    tracker.max_closed = 1;
    let mut other = correlation(T0 + 1_000, T0 + 1_000, 0.8, 10);
    other.target.service = "billing-svc".to_string();
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);
    tracker.observe(std::slice::from_ref(&other), T0 + 1_000);
    tracker.observe(&[], T0 + 1_000 + GAP);

    assert_eq!(1, tracker.pairs.len());
    assert!(
        tracker
            .pairs
            .contains_key(&pair_key(&other.source, &other.target))
    );
}

/// A raised `max_tracked_pairs` keeps as many closed pairs, so a pair the
/// correlator still holds is never forgotten and reopened under an old id.
#[test]
fn the_closed_pair_memory_follows_max_tracked_pairs() {
    let config = CorrelationConfig {
        max_tracked_pairs: 50_000,
        ..CorrelationConfig::default()
    };
    let export = CorrelationExport::new(
        empty_correlator(),
        &config,
        Arc::new(CorrelationExportBuffer::new(10)),
        "http://hub/api/import/correlations".parse().expect("uri"),
    );

    assert_eq!(50_000, export.tracker.max_closed);
}

#[test]
fn a_never_seen_pair_starts_no_earlier_than_the_window() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);

    let opened = tracker.observe(&[correlation(T0 - 3 * WINDOW, T0, 0.8, 10)], T0);

    assert_eq!(T0 - WINDOW, opened[0].started_at_ms);
}

#[test]
fn shutdown_closes_open_episodes_at_their_latest_last_seen() {
    let mut tracker = EpisodeTracker::new(WINDOW, MAX_CLOSED);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);
    // Held back by the one-minute throttle, yet the close must carry it.
    tracker.observe(&[correlation(T0, T0 + 10_000, 0.8, 10)], T0 + 10_000);

    let closed = tracker.close_all();

    assert_eq!(1, closed.len());
    assert_eq!(T0 + 10_000, closed[0].last_seen_ms);
    assert_eq!(Some(T0 + 10_000), closed[0].ended_at_ms);
    assert_eq!(0, tracker.close_all().len());
}

/// A correlator that reports nothing.
pub(crate) fn empty_correlator() -> Arc<tokio::sync::Mutex<CrossTraceCorrelator>> {
    Arc::new(tokio::sync::Mutex::new(CrossTraceCorrelator::new(
        CorrelationConfig {
            enabled: true,
            ..CorrelationConfig::default()
        },
    )))
}

/// Feed `svc-a` then `svc-b` one second apart, analysed at `now_ms`.
pub(crate) async fn co_occur(correlator: &tokio::sync::Mutex<CrossTraceCorrelator>, now_ms: u64) {
    let finding = |service: &str, at_ms: u64| {
        let mut finding =
            crate::test_helpers::make_finding(FindingType::NPlusOneSql, Severity::Warning);
        finding.service = service.to_string();
        finding.first_timestamp = crate::time::millis_to_iso8601(at_ms);
        finding
    };
    let mut correlator = correlator.lock().await;
    let _ = correlator.ingest(&[finding("svc-a", now_ms)], now_ms);
    let _ = correlator.ingest(&[finding("svc-b", now_ms + 1_000)], now_ms);
}

/// A correlator reporting one pair, `svc-a` then `svc-b`, active now.
pub(crate) async fn correlator_with_one_pair() -> Arc<tokio::sync::Mutex<CrossTraceCorrelator>> {
    let correlator = Arc::new(tokio::sync::Mutex::new(CrossTraceCorrelator::new(
        CorrelationConfig {
            enabled: true,
            min_co_occurrences: 1,
            min_confidence: 0.0,
            ..CorrelationConfig::default()
        },
    )));
    co_occur(&correlator, current_time_ms()).await;
    correlator
}

fn export(
    base: &str,
    correlator: &Arc<tokio::sync::Mutex<CrossTraceCorrelator>>,
    buffer: &Arc<CorrelationExportBuffer>,
) -> CorrelationExport {
    CorrelationExport::new(
        correlator.clone(),
        &CorrelationConfig::default(),
        buffer.clone(),
        format!("{base}/api/import/correlations?source_id=lab")
            .parse()
            .expect("uri"),
    )
}

fn open_last_seen(export: &CorrelationExport) -> Option<u64> {
    export
        .tracker
        .pairs
        .values()
        .find_map(|track| track.open.as_ref())
        .map(|open| open.episode.last_seen_ms)
}

#[test]
fn the_correlations_route_sits_beside_the_findings_one() {
    assert_eq!(
        "https://hub.example.com/api/import/correlations",
        correlations_endpoint("https://hub.example.com/api/import/findings")
    );
}

#[test]
fn a_late_acknowledgement_keeps_the_closing_copy() {
    let buffer = CorrelationExportBuffer::new(10);
    let open = episode("a");
    buffer.push(&open);
    let in_flight = buffer.snapshot(10);
    let mut closed = open.clone();
    closed.ended_at_ms = Some(closed.last_seen_ms);
    buffer.push(&closed);

    buffer.acknowledge(&in_flight);

    let batch = buffer.snapshot(10);
    assert_eq!(1, batch.len(), "the close is still to be sent");
    assert_eq!(Some(open.last_seen_ms), batch[0].value.ended_at_ms);
}

#[tokio::test]
async fn a_round_posts_the_open_episode_to_the_correlations_route() {
    let (base, seen) = fake_hub(200).await;
    let correlator = correlator_with_one_pair().await;
    let buffer = Arc::new(CorrelationExportBuffer::new(10));
    let metrics = MetricsState::new();
    let client = HubClient::new(Duration::from_secs(10));

    let sent = export(&base, &correlator, &buffer)
        .round(
            &client,
            &"k".repeat(32),
            &metrics,
            Duration::from_secs(5),
            false,
        )
        .await;

    assert!(sent);
    assert_eq!(0, buffer.len());
    let requests = seen.lock().expect("lock");
    let (uri, key, body) = &requests[0];
    assert_eq!("/api/import/correlations?source_id=lab", uri);
    assert_eq!(Some("k".repeat(32)), *key);
    let body: serde_json::Value = serde_json::from_slice(body).expect("json");
    assert_eq!(env!("CARGO_PKG_VERSION"), body["producer_version"]);
    let episode = &body["episodes"][0];
    assert_eq!("svc-a", episode["source"]["service"]);
    assert_eq!("svc-b", episode["target"]["service"]);
    assert_eq!(32, episode["pair_key"].as_str().expect("pair key").len());
    assert!(episode["ended_at_ms"].is_null());
    assert_eq!(GAP, episode["gap_ms"]);
}

#[tokio::test]
async fn an_episode_over_the_cap_alone_is_dropped_and_counted() {
    let (base, seen) = fake_hub(200).await;
    let buffer = Arc::new(CorrelationExportBuffer::new(10));
    let mut big = episode("a");
    big.source.template = "x".repeat(MAX_EPISODE_BODY_BYTES);
    buffer.push(&big);
    let metrics = MetricsState::new();
    let client = HubClient::new(Duration::from_secs(10));

    export(&base, &empty_correlator(), &buffer)
        .round(
            &client,
            &"k".repeat(32),
            &metrics,
            Duration::from_secs(5),
            false,
        )
        .await;

    assert_eq!(0, buffer.len());
    assert_eq!(1, metrics.hub_export_correlations_dropped_total.get());
    assert!(seen.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn a_hub_without_the_route_stops_correlation_export_for_good() {
    let (base, seen) = fake_hub(404).await;
    let correlator = correlator_with_one_pair().await;
    let buffer = Arc::new(CorrelationExportBuffer::new(10));
    buffer.push(&episode("a"));
    let metrics = MetricsState::new();
    let client = HubClient::new(Duration::from_secs(10));
    let mut export = export(&base, &correlator, &buffer);

    // The reading queues the live pair beside "a", then the 404 drops both.
    assert!(
        !export
            .round(&client, &"k".repeat(32), &metrics, Duration::ZERO, false)
            .await
    );
    co_occur(&correlator, current_time_ms() + 1_000).await;
    assert!(
        !export
            .round(&client, &"k".repeat(32), &metrics, Duration::ZERO, false)
            .await
    );

    assert!(buffer.is_unsupported());
    assert_eq!(0, buffer.len());
    assert_eq!(2, metrics.hub_export_correlations_dropped_total.get());
    assert!(metrics.hub_export_correlations_pending.get().abs() < f64::EPSILON);
    assert_eq!(
        1,
        seen.lock().expect("lock").len(),
        "one probe, then silence"
    );
}

#[tokio::test]
async fn a_failing_route_keeps_reading_the_correlator_on_its_own_clock() {
    let (base, seen) = fake_hub(500).await;
    let correlator = correlator_with_one_pair().await;
    let buffer = Arc::new(CorrelationExportBuffer::new(10));
    let metrics = MetricsState::new();
    let client = HubClient::new(Duration::from_secs(10));
    let flush = Duration::from_millis(50);
    let mut export = export(&base, &correlator, &buffer);

    assert!(
        export
            .round(&client, &"k".repeat(32), &metrics, flush, false)
            .await
    );
    assert!(
        export.due() < export.send.due,
        "the send backs off, the reading does not"
    );
    let before = open_last_seen(&export).expect("an open episode");

    tokio::time::sleep_until(export.due()).await;
    co_occur(&correlator, current_time_ms() + 1_000).await;
    assert!(
        !export
            .round(&client, &"k".repeat(32), &metrics, flush, false)
            .await
    );

    assert!(open_last_seen(&export).expect("still open") > before);
    assert_eq!(
        1,
        seen.lock().expect("lock").len(),
        "no send before the backoff ends"
    );
}
