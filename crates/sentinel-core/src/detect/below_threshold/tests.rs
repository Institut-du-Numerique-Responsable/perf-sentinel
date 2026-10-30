use std::collections::HashSet;

use super::reference::count_below_threshold_reference;
use super::*;
use crate::acknowledgments::enrich_with_signatures;
use crate::correlate;
use crate::detect::{DetectConfig, run_full_detection, run_full_detection_with_below};
use crate::event::SpanEvent;
use crate::ingest::IngestSource;
use crate::ingest::json::JsonIngest;
use crate::normalize;
use crate::synth::{PatternMix, generate_target_events};
use crate::test_helpers::{
    make_http_event_with_duration, make_sql_event_with_duration, make_trace,
};

const HEX: &str = "0123456789abcdef0123456789abcdef";

fn config(edit: impl FnOnce(&mut DetectConfig)) -> DetectConfig {
    let mut config = DetectConfig::default();
    edit(&mut config);
    config
}

fn kinds_found(trace: &Trace, config: &DetectConfig, kind: &FindingType) -> Vec<String> {
    let mut findings = run_full_detection(std::slice::from_ref(trace), config);
    enrich_with_signatures(&mut findings);
    findings
        .into_iter()
        .filter(|f| &f.finding_type == kind)
        .map(|f| f.signature)
        .collect()
}

/// The signature of the one finding of `kind` the trace yields under `config`.
fn signature_of(trace: &Trace, config: &DetectConfig, kind: &FindingType) -> String {
    let mut signatures = kinds_found(trace, config, kind);
    assert_eq!(signatures.len(), 1, "one {kind:?} finding expected");
    signatures.remove(0)
}

fn count(trace: &Trace, config: &DetectConfig, watch: &WatchIndex) -> HashMap<Arc<str>, u64> {
    run_full_detection_with_below(std::slice::from_ref(trace), config, Some(watch)).1
}

/// `low` detects the finding and counts no pass, `high` (threshold at the
/// measured value) detects nothing and counts `passes` for its signature.
fn assert_boundary(
    trace: &Trace,
    low: &DetectConfig,
    high: &DetectConfig,
    kind: &FindingType,
    passes: u64,
) {
    let signature = signature_of(trace, low, kind);
    let watch = WatchIndex::from_signatures([signature.as_str()], 10);
    assert_eq!(watch.len(), 1);
    assert!(count(trace, low, &watch).is_empty());
    assert_eq!(kinds_found(trace, high, kind), Vec::<String>::new());
    assert_eq!(
        count(trace, high, &watch),
        HashMap::from([(Arc::from(signature.as_str()), passes)])
    );
}

fn ts(ms: u32) -> String {
    format!("2025-07-10T14:32:01.{ms:03}Z")
}

fn child_of(mut event: SpanEvent, parent: &str) -> SpanEvent {
    event.parent_span_id = Some(parent.to_string());
    event
}

fn parent_span() -> SpanEvent {
    make_sql_event_with_duration("t", "p", "SELECT 1", &ts(0), 900_000)
}

fn slow_trace(service: &str, endpoint: &str) -> Trace {
    make_trace(
        (0..3)
            .map(|i| {
                let mut event = make_sql_event_with_duration(
                    "t",
                    &format!("s{i}"),
                    "SELECT * FROM users WHERE id = 1",
                    &ts(i * 300),
                    200_000,
                );
                event.service = Arc::from(service);
                event.source.endpoint = endpoint.to_string();
                event
            })
            .collect(),
    )
}

fn slow_at(threshold_ms: u64) -> DetectConfig {
    config(|c| {
        c.slow_threshold_ms = threshold_ms;
        c.slow_min_occurrences = 1;
    })
}

fn fanout_children() -> Vec<SpanEvent> {
    (0..5)
        .map(|i| {
            child_of(
                make_sql_event_with_duration(
                    "t",
                    &format!("c{i}"),
                    &format!("SELECT * FROM t WHERE id = {i}"),
                    &ts((i + 1) * 10),
                    800,
                ),
                "p",
            )
        })
        .collect()
}

fn fanout_at(max_fanout: u32) -> DetectConfig {
    config(|c| c.max_fanout = max_fanout)
}

fn pool_at(threshold: u32) -> DetectConfig {
    config(|c| c.pool_saturation_concurrent_threshold = threshold)
}

fn serialized_at(min: u32) -> DetectConfig {
    config(|c| c.serialized_min_sequential = min)
}

/// Children of `p`, one per `(table, start ms)`, 50 ms each.
fn serialized_trace(children: &[(&str, u32)]) -> Trace {
    let mut events = vec![parent_span()];
    events.extend(children.iter().enumerate().map(|(i, (table, start))| {
        child_of(
            make_sql_event_with_duration(
                "t",
                &format!("c{i}"),
                &format!("SELECT * FROM {table}"),
                &ts(*start),
                50_000,
            ),
            "p",
        )
    }));
    make_trace(events)
}

/// SQL spans of one service, 100 ms each, one per start ms.
fn pool_trace(starts: &[u32]) -> Trace {
    make_trace(
        starts
            .iter()
            .enumerate()
            .map(|(i, start)| {
                make_sql_event_with_duration(
                    "t",
                    &format!("q{i}"),
                    &format!("SELECT * FROM t{i}"),
                    &ts(*start),
                    100_000,
                )
            })
            .collect(),
    )
}

#[test]
fn slow_spans_at_the_threshold_count_one_pass_each() {
    let trace = slow_trace("order-svc", "POST /api/orders/42/submit");
    assert_boundary(
        &trace,
        &slow_at(100),
        &slow_at(200),
        &FindingType::SlowSql,
        3,
    );
    assert_boundary(
        &trace,
        &slow_at(199),
        &slow_at(200),
        &FindingType::SlowSql,
        3,
    );
}

#[test]
fn fanout_at_max_fanout_counts_one_pass() {
    let mut events = vec![parent_span()];
    events.extend(fanout_children());
    let trace = make_trace(events);
    assert_boundary(
        &trace,
        &fanout_at(4),
        &fanout_at(5),
        &FindingType::ExcessiveFanout,
        1,
    );
}

#[test]
fn fanout_without_its_parent_matches_on_the_dominant_child_template() {
    let trace = make_trace(fanout_children());
    assert_boundary(
        &trace,
        &fanout_at(4),
        &fanout_at(5),
        &FindingType::ExcessiveFanout,
        1,
    );
}

#[test]
fn chatty_at_the_call_threshold_counts_one_pass() {
    let trace = make_trace(
        (0..4)
            .map(|i| {
                make_http_event_with_duration(
                    "t",
                    &format!("h{i}"),
                    &format!("GET /api/users/{i}"),
                    &ts(i * 20),
                    12_000,
                )
            })
            .collect(),
    );
    let at = |min| config(|c| c.chatty_service_min_calls = min);
    assert_boundary(&trace, &at(3), &at(4), &FindingType::ChattyService, 1);
}

#[test]
fn pool_one_under_the_threshold_counts_one_pass() {
    let trace = pool_trace(&[0, 0, 0, 0]);
    assert_boundary(
        &trace,
        &pool_at(4),
        &pool_at(5),
        &FindingType::PoolSaturation,
        1,
    );
}

#[test]
fn pool_counts_on_the_measured_peak_past_the_span_count() {
    // Four spans, two at a time: the span count clears 3, the peak does not.
    let trace = pool_trace(&[0, 0, 500, 500]);
    assert_boundary(
        &trace,
        &pool_at(2),
        &pool_at(3),
        &FindingType::PoolSaturation,
        1,
    );
}

#[test]
fn serialized_one_under_the_minimum_counts_one_pass() {
    let trace = serialized_trace(&[("a", 0), ("b", 100), ("c", 200)]);
    assert_boundary(
        &trace,
        &serialized_at(3),
        &serialized_at(4),
        &FindingType::SerializedCalls,
        1,
    );
}

#[test]
fn serialized_counts_on_the_chain_past_the_child_count() {
    // Three children, the first two overlapping: the longest chain is 2.
    let trace = serialized_trace(&[("a", 0), ("b", 10), ("c", 100)]);
    assert_boundary(
        &trace,
        &serialized_at(2),
        &serialized_at(3),
        &FindingType::SerializedCalls,
        1,
    );
}

#[test]
fn serialized_counts_a_session_command_off_the_chain() {
    // Three children, the COMMIT left off the timeline: two timed siblings.
    let mut trace = serialized_trace(&[("a", 0), ("b", 100), ("c", 200)]);
    trace.spans[3].template = "COMMIT".into();
    assert_boundary(
        &trace,
        &serialized_at(2),
        &serialized_at(3),
        &FindingType::SerializedCalls,
        1,
    );
}

#[test]
fn colons_slashes_and_bidi_in_names_still_match() {
    let trace = slow_trace("svc:v2\u{202E}", "GET /a:b c");
    assert_boundary(
        &trace,
        &slow_at(100),
        &slow_at(200),
        &FindingType::SlowSql,
        3,
    );
}

#[test]
fn an_unwatched_head_counts_nothing() {
    let trace = slow_trace("order-svc", "POST /api/orders/42/submit");
    let signature = signature_of(&trace, &slow_at(100), &FindingType::SlowSql);
    let (_, hex) = signature.rsplit_once(':').unwrap();
    let other = format!("slow_sql:order-svc:GET__other:{hex}");
    let watch = WatchIndex::from_signatures([other.as_str()], 10);
    assert_eq!(watch.len(), 1);
    assert!(count(&trace, &slow_at(200), &watch).is_empty());
}

#[test]
fn an_empty_watch_counts_nothing() {
    let trace = slow_trace("order-svc", "POST /api/orders/42/submit");
    assert!(count(&trace, &slow_at(200), &WatchIndex::default()).is_empty());
}

#[test]
fn from_signatures_keeps_valid_thresholded_entries_once_up_to_the_cap() {
    let slow = format!("slow_sql:s:e:{HEX}");
    let chatty = format!("chatty_service:s:e:{HEX}");
    let n_plus_one = format!("n_plus_one_sql:s:e:{HEX}");
    let short = format!("slow_sql:s:e:{}", &HEX[1..]);
    let upper = format!("slow_sql:s:e:{}", HEX.to_uppercase());
    let serialized = format!("serialized_calls:s:e:{HEX}");
    let input = [
        slow.as_str(),
        slow.as_str(),
        chatty.as_str(),
        n_plus_one.as_str(),
        short.as_str(),
        upper.as_str(),
        "nope",
        serialized.as_str(),
    ];

    let capped = WatchIndex::from_signatures(input, 2);
    assert_eq!(capped.len(), 2);
    assert!(capped.watches(&FindingType::SlowSql));
    assert!(capped.watches(&FindingType::ChattyService));
    assert!(!capped.watches(&FindingType::SerializedCalls));

    assert_eq!(WatchIndex::from_signatures(input, 10).len(), 3);
}

/// The bench harvest config: every thresholded kind fires as early as it can.
fn floor_config() -> DetectConfig {
    DetectConfig {
        slow_threshold_ms: 1,
        slow_min_occurrences: 1,
        max_fanout: 1,
        chatty_service_min_calls: 1,
        pool_saturation_concurrent_threshold: 2,
        serialized_min_sequential: 2,
        ..DetectConfig::default()
    }
}

fn mid_config() -> DetectConfig {
    DetectConfig {
        slow_threshold_ms: 50,
        max_fanout: 3,
        chatty_service_min_calls: 3,
        pool_saturation_concurrent_threshold: 3,
        serialized_min_sequential: 3,
        ..DetectConfig::default()
    }
}

/// Every measured value stays under these, so every watched group passes.
fn ceiling_config() -> DetectConfig {
    DetectConfig {
        slow_threshold_ms: 1_000_000,
        max_fanout: 1_000_000,
        chatty_service_min_calls: 1_000_000,
        pool_saturation_concurrent_threshold: 1_000_000,
        serialized_min_sequential: 1_000_000,
        ..DetectConfig::default()
    }
}

/// The thresholded signatures the traces yield under `floor_config`.
fn harvest(traces: &[Trace]) -> Vec<String> {
    let mut findings = run_full_detection(traces, &floor_config());
    enrich_with_signatures(&mut findings);
    let mut signatures: Vec<String> = findings
        .into_iter()
        .filter(|f| f.finding_type.is_thresholded())
        .map(|f| f.signature)
        .collect();
    signatures.sort_unstable();
    signatures.dedup();
    signatures
}

/// The detectors' passes equal the reference walk's under four configs,
/// and watching changes no finding. Returns the kinds that had a pass.
fn assert_fused_equals_reference(traces: &[Trace]) -> HashSet<FindingType> {
    let signatures = harvest(traces);
    let watch = WatchIndex::from_signatures(signatures.iter().map(String::as_str), usize::MAX);
    let mut kinds = HashSet::new();
    for config in [
        DetectConfig::default(),
        floor_config(),
        mid_config(),
        ceiling_config(),
    ] {
        let (findings, fused) = run_full_detection_with_below(traces, &config, Some(&watch));
        assert_eq!(
            fused,
            count_below_threshold_reference(traces, &config, &watch),
            "{config:?}"
        );
        assert_eq!(findings, run_full_detection(traces, &config), "{config:?}");
        kinds.extend(fused.keys().map(|signature| {
            let (kind, _) = signature.split_once(':').unwrap();
            FindingType::from_kind_str(kind).unwrap()
        }));
    }
    kinds
}

fn fixture_traces(name: &str) -> Vec<Trace> {
    let path = format!("{}/../../tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let data = std::fs::read(&path).unwrap();
    let events = JsonIngest::new(10_000_000).ingest(&data).unwrap();
    let traces = correlate::correlate(normalize::normalize_all(events));
    assert!(!traces.is_empty(), "{name}");
    traces
}

/// The bench corpus `traces_for(10_000)`.
fn synth_traces() -> Vec<Trace> {
    let events = generate_target_events(10_000, 16, &PatternMix::default(), 42);
    correlate::correlate(normalize::normalize_all(events))
}

#[test]
fn fused_passes_equal_the_reference_on_the_synth_corpus() {
    let kinds = assert_fused_equals_reference(&synth_traces());
    for kind in [
        FindingType::SlowSql,
        FindingType::SlowHttp,
        FindingType::ExcessiveFanout,
        FindingType::ChattyService,
        FindingType::PoolSaturation,
    ] {
        assert!(kinds.contains(&kind), "no {kind:?} pass");
    }
}

#[test]
fn fused_passes_equal_the_reference_on_the_fixtures() {
    let mut kinds = HashSet::new();
    for name in [
        "demo.json",
        "running_example.json",
        "tied_findings.json",
        "messaging_chain.json",
        "fanout.json",
        "slow_queries.json",
        "mixed.json",
        "otlp_php_doctrine_split.json",
    ] {
        kinds.extend(assert_fused_equals_reference(&fixture_traces(name)));
    }
    let thresholded: HashSet<FindingType> = [
        FindingType::SlowSql,
        FindingType::SlowHttp,
        FindingType::SlowMessaging,
        FindingType::ExcessiveFanout,
        FindingType::ChattyService,
        FindingType::PoolSaturation,
        FindingType::SerializedCalls,
    ]
    .into();
    assert_eq!(kinds, thresholded);
}

#[test]
fn watch_none_and_watch_empty_run_plain_detection() {
    let traces = synth_traces();
    let plain = run_full_detection(&traces, &DetectConfig::default());
    for watch in [None, Some(&WatchIndex::default())] {
        let (findings, passes) =
            run_full_detection_with_below(&traces, &DetectConfig::default(), watch);
        assert_eq!(findings, plain);
        assert!(passes.is_empty());
    }
}
