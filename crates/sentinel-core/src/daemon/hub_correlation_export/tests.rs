use super::*;
use crate::detect::FindingType;

const T0: u64 = 1_800_000_000_000;
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
