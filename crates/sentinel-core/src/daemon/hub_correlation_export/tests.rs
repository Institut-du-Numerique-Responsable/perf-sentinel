use super::*;
use crate::detect::FindingType;

const T0: u64 = 1_800_000_000_000;
const WINDOW: u64 = 600_000;
const GAP: u64 = 600_000;

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
    let mut tracker = EpisodeTracker::new(WINDOW);

    let queued = tracker.observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0);

    assert_eq!(1, queued.len());
    assert_eq!(T0 - 5_000, queued[0].started_at_ms);
    assert_eq!(T0, queued[0].last_seen_ms);
    assert_eq!(None, queued[0].ended_at_ms);
    assert_eq!(episode_id(&queued[0].pair_key, T0 - 5_000), queued[0].id);
}

#[test]
fn an_advance_under_a_minute_is_held_back_and_one_past_it_is_sent() {
    let mut tracker = EpisodeTracker::new(WINDOW);
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
    let mut tracker = EpisodeTracker::new(WINDOW);
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
    let mut tracker = EpisodeTracker::new(WINDOW);
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
    let mut tracker = EpisodeTracker::new(WINDOW);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);

    assert_eq!(0, tracker.observe(&[], T0 + GAP - 1).len());
    let closed = tracker.observe(&[], T0 + GAP);

    assert_eq!(1, closed.len());
    assert_eq!(Some(T0), closed[0].ended_at_ms);
}

#[test]
fn activity_after_the_gap_opens_a_new_episode_at_its_own_start() {
    let mut tracker = EpisodeTracker::new(WINDOW);
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
    let mut tracker = EpisodeTracker::new(week);
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
    let mut tracker = EpisodeTracker::new(WINDOW);
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
    let mut tracker = EpisodeTracker::new(WINDOW);

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
fn a_closed_pair_is_forgotten_one_window_after_its_last_co_occurrence() {
    let mut tracker = EpisodeTracker::new(WINDOW);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);
    tracker.observe(&[], T0 + GAP);
    assert_eq!(1, tracker.pairs.len());

    tracker.observe(&[], T0 + WINDOW + 1);

    assert_eq!(0, tracker.pairs.len());
}

#[test]
fn shutdown_closes_open_episodes_at_their_latest_last_seen() {
    let mut tracker = EpisodeTracker::new(WINDOW);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);
    // Held back by the one-minute throttle, yet the close must carry it.
    tracker.observe(&[correlation(T0, T0 + 10_000, 0.8, 10)], T0 + 10_000);

    let closed = tracker.close_all();

    assert_eq!(1, closed.len());
    assert_eq!(T0 + 10_000, closed[0].last_seen_ms);
    assert_eq!(Some(T0 + 10_000), closed[0].ended_at_ms);
    assert_eq!(0, tracker.close_all().len());
}
