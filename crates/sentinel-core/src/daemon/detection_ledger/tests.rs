use super::*;
use crate::detect::{FindingType, Severity};
use crate::test_helpers::make_finding;

fn cfg(retained_days: u32, max_signatures_per_day: usize) -> DaemonDetectionsConfig {
    DaemonDetectionsConfig {
        enabled: true,
        retained_days,
        max_signatures_per_day,
    }
}

fn ledger_at(day: u64, io_tracked: bool) -> DetectionLedger {
    DetectionLedger::new(
        &cfg(8, 10_000),
        io_tracked,
        1.0,
        DayThresholds::default(),
        day * DAY_MS,
    )
}

fn finding(signature: &str, io: usize) -> Finding {
    let mut f = make_finding(FindingType::NPlusOneSql, Severity::Warning);
    f.signature = signature.to_string();
    f.green_impact.as_mut().unwrap().estimated_extra_io_ops = io;
    f
}

fn bucket<R>(ledger: &DetectionLedger, day: u32, read: impl FnOnce(&DayBucket) -> R) -> R {
    read(&ledger.lock().days[&day])
}

#[test]
fn first_day_of_a_ledger_is_a_gap() {
    let ledger = DetectionLedger::new(
        &cfg(8, 10_000),
        true,
        1.0,
        DayThresholds::default(),
        3 * DAY_MS + 5,
    );
    assert_eq!(ledger.status().current_day, 3);
    assert!(bucket(&ledger, 3, |b| b.gap));
}

#[test]
fn record_counts_per_day_and_signature_with_exact_io() {
    let ledger = ledger_at(3, true);
    let overflowed = ledger.record(
        &[finding("a", 5), finding("a", 7), finding("b", 1)],
        3 * DAY_MS + 10,
    );
    assert_eq!(overflowed, 0);
    assert_eq!(ledger.tally(3, "a"), Some((2, Some(12))));
    assert_eq!(ledger.tally(3, "b"), Some((1, Some(1))));
}

#[test]
fn io_is_untracked_when_green_is_off() {
    let ledger = ledger_at(3, false);
    ledger.record(&[finding("a", 5)], 3 * DAY_MS);
    assert_eq!(ledger.tally(3, "a"), Some((1, None)));
}

#[test]
fn empty_signatures_are_skipped() {
    let ledger = ledger_at(3, true);
    assert_eq!(ledger.record(&[finding("", 5)], 3 * DAY_MS), 0);
    assert_eq!(ledger.overflow(3), 0);
    assert!(bucket(&ledger, 3, |b| b.counts.is_empty()));
}

#[test]
fn overflow_takes_new_signatures_past_the_cap() {
    let ledger = DetectionLedger::new(&cfg(8, 2), true, 1.0, DayThresholds::default(), 3 * DAY_MS);
    assert_eq!(
        ledger.record(&[finding("a", 1), finding("b", 1)], 3 * DAY_MS),
        0
    );
    assert_eq!(
        ledger.record(&[finding("c", 4), finding("a", 1)], 3 * DAY_MS),
        1
    );
    assert_eq!(ledger.tally(3, "a"), Some((2, Some(2))));
    assert_eq!(ledger.tally(3, "b"), Some((1, Some(1))));
    assert_eq!(ledger.tally(3, "c"), None);
    assert_eq!(ledger.overflow(3), 1);
    assert_eq!(bucket(&ledger, 3, |b| b.overflow.extra_io_ops), 4);
}

#[test]
fn hostile_signatures_go_to_overflow() {
    let ledger = ledger_at(3, true);
    let long = "s".repeat(MAX_TRACKED_SIGNATURE_BYTES + 1);
    let overflowed = ledger.record(&[finding(&long, 1), finding("x\ny", 1)], 3 * DAY_MS);
    assert_eq!(overflowed, 2);
    assert_eq!(ledger.tally(3, &long), None);
    assert_eq!(ledger.tally(3, "x\ny"), None);
    assert_eq!(ledger.overflow(3), 2);
}

#[test]
fn a_detection_never_reopens_a_sealed_day() {
    let ledger = ledger_at(3, true);
    ledger.record(&[finding("a", 1)], 3 * DAY_MS);
    ledger.tick(4 * DAY_MS, 0, 0);
    assert!(bucket(&ledger, 3, |b| b.sealed));
    ledger.record(&[finding("a", 1)], 3 * DAY_MS + 100);
    assert_eq!(ledger.tally(3, "a"), Some((1, Some(1))));
    assert_eq!(ledger.tally(4, "a"), Some((1, Some(1))));
    assert_eq!(ledger.status().sealed_through, Some(3));
}

#[test]
fn tick_flags_shed_and_rejects_on_both_days_across_midnight() {
    let ledger = ledger_at(3, true);
    ledger.tick(3 * DAY_MS, 0, 0);
    ledger.tick(4 * DAY_MS + 1, 5, 2);
    for day in [3, 4] {
        assert_eq!(
            bucket(&ledger, day, |b| (b.shed_traces, b.ingest_rejected)),
            (5, 2)
        );
    }
    let rev = bucket(&ledger, 4, |b| b.rev);
    ledger.tick(4 * DAY_MS + 2, 5, 2);
    assert_eq!(
        bucket(&ledger, 4, |b| (b.shed_traces, b.ingest_rejected)),
        (5, 2)
    );
    assert_eq!(bucket(&ledger, 4, |b| b.rev), rev);
}

#[test]
fn tick_drops_days_past_retention() {
    let ledger = DetectionLedger::new(&cfg(2, 10_000), true, 1.0, DayThresholds::default(), DAY_MS);
    for day in 2..=4 {
        ledger.record(&[finding("a", 1)], day * DAY_MS);
    }
    assert_eq!(ledger.tick(4 * DAY_MS, 0, 0), 2);
    assert_eq!(
        ledger.lock().days.keys().copied().collect::<Vec<_>>(),
        [3, 4]
    );
    let status = ledger.status();
    assert_eq!((status.oldest_day, status.current_day), (3, 4));
    assert_eq!(status.sealed_through, Some(3));
}

#[test]
fn drain_marks_today_as_a_gap() {
    let ledger = ledger_at(3, true);
    ledger.tick(4 * DAY_MS, 0, 0);
    assert!(!bucket(&ledger, 4, |b| b.gap));
    ledger.mark_drain_gap(4 * DAY_MS + 1);
    assert!(bucket(&ledger, 4, |b| b.gap));
}

#[test]
fn tick_once_carries_metric_losses_and_sets_the_gauge() {
    let ledger = ledger_at(3, true);
    ledger.record(&[finding("a", 1), finding("b", 1)], 3 * DAY_MS);
    let metrics = MetricsState::new();
    metrics.analysis_shed_traces_total.inc_by(4);
    metrics.otlp_rejected_channel_full.inc_by(2);
    metrics.otlp_rejected_memory_pressure.inc();
    tick_once(&ledger, &metrics, 3 * DAY_MS + 1);
    assert_eq!(
        bucket(&ledger, 3, |b| (b.shed_traces, b.ingest_rejected)),
        (4, 3)
    );
    assert_eq!(metrics.detection_ledger_signatures.get(), 2);
}

/// Days 3 and 4, each with signatures `a`, `b`, `c`.
fn two_full_days() -> DetectionLedger {
    let ledger = ledger_at(3, true);
    let batch = [finding("a", 1), finding("b", 2), finding("c", 3)];
    ledger.record(&batch, 3 * DAY_MS);
    ledger.record(&batch, 4 * DAY_MS);
    ledger
}

fn keys(page: &DetectionsPage) -> Vec<String> {
    page.counts
        .iter()
        .map(|r| format!("{}:{}", r.day, r.signature))
        .collect()
}

fn page_days(page: &DetectionsPage) -> Vec<u32> {
    page.days.iter().map(|d| d.day).collect()
}

#[test]
fn page_walks_days_and_signatures_in_order() {
    let ledger = two_full_days();
    let first = ledger.page(None, None, 2);
    assert_eq!(first.next_after.as_deref(), Some("3:b"));
    let second = ledger.page(None, Some((3, "b")), 2);
    assert_eq!(second.next_after.as_deref(), Some("4:a"));
    let third = ledger.page(None, Some((4, "a")), 2);
    assert_eq!(third.next_after, None);
    let all: Vec<String> = [&first, &second, &third]
        .into_iter()
        .flat_map(keys)
        .collect();
    assert_eq!(all, ["3:a", "3:b", "3:c", "4:a", "4:b", "4:c"]);
    assert_eq!(third.counts[0].detections, 1);
    assert_eq!(third.counts[0].extra_io_ops, Some(2));
    assert_eq!((third.oldest_day, third.current_day), (3, 4));
    assert_eq!(third.ledger_id, ledger.status().ledger_id);
}

#[test]
fn day_rows_follow_their_last_count() {
    let ledger = two_full_days();
    assert_eq!(page_days(&ledger.page(None, None, 2)), Vec::<u32>::new());
    let second = ledger.page(None, Some((3, "b")), 2);
    assert_eq!(page_days(&second), [3]);
    assert!(second.days[0].gap, "day 3 opened the ledger");
    assert_eq!(page_days(&ledger.page(None, Some((4, "a")), 2)), [4]);
    // A page ending on day 3's last row carries day 3, the next one never.
    let ends_on_3 = ledger.page(None, None, 3);
    assert_eq!(
        (keys(&ends_on_3).len(), page_days(&ends_on_3)),
        (3, vec![3])
    );
    assert_eq!(page_days(&ledger.page(None, Some((3, "c")), 3)), [4]);
}

#[test]
fn an_empty_day_still_gets_its_row() {
    let ledger = ledger_at(3, false);
    ledger.tick(4 * DAY_MS, 7, 0);
    ledger.record(&[finding("a", 1)], 5 * DAY_MS);
    let page = ledger.page(None, None, 10);
    assert_eq!(keys(&page), ["5:a"]);
    assert_eq!(page_days(&page), [3, 4, 5]);
    assert_eq!(page.days[1].shed_traces, 7);
    assert!(page.days[0].sealed && !page.days[2].sealed);
    assert_eq!(page.counts[0].extra_io_ops, None, "green off");
    assert_eq!(page.days[2].overflow_extra_io_ops, None);
    assert_eq!(page.sealed_through, Some(3), "day 4 seals on the next tick");
}

#[test]
fn an_empty_day_rides_one_page_only() {
    let ledger = ledger_at(3, true);
    ledger.record(
        &[finding("a", 1), finding("b", 1), finding("c", 1)],
        3 * DAY_MS,
    );
    ledger.tick(4 * DAY_MS, 0, 0);
    ledger.record(&[finding("a", 1)], 5 * DAY_MS);
    let first = ledger.page(None, None, 3);
    assert_eq!(first.next_after.as_deref(), Some("3:c"));
    let second = ledger.page(None, Some((3, "c")), 3);
    assert_eq!(keys(&second), ["5:a"]);
    let all: Vec<u32> = [&first, &second].into_iter().flat_map(page_days).collect();
    assert_eq!(all, [3, 4, 5]);
}

#[test]
fn page_starts_at_since_day() {
    let ledger = two_full_days();
    let page = ledger.page(Some(4), None, 10);
    assert_eq!(keys(&page), ["4:a", "4:b", "4:c"]);
    assert_eq!(page_days(&page), [4]);
    assert_eq!(
        page.oldest_day, 3,
        "the horizon is the ledger's, not the page's"
    );
    assert!(ledger.page(Some(5), None, 10).counts.is_empty());
}

#[test]
fn page_stops_at_the_byte_budget() {
    let ledger = ledger_at(3, true);
    let batch: Vec<Finding> = (0..4000)
        .map(|i| finding(&format!("{i:04}{}", "x".repeat(996)), 0))
        .collect();
    ledger.record(&batch, 3 * DAY_MS);
    let page = ledger.page(None, None, 5000);
    let fits = MAX_PAGE_BYTES / (1000 + ROW_OVERHEAD_BYTES);
    assert_eq!(page.counts.len(), fits);
    assert!(page.days.is_empty(), "day 3 has rows left");
    let last = &page.counts[fits - 1].signature;
    assert_eq!(page.next_after, Some(format!("3:{last}")));
}

#[test]
fn page_bytes_count_the_json_escapes() {
    let ledger = ledger_at(3, true);
    let batch: Vec<Finding> = (0..4000)
        .map(|i| finding(&format!("{i:04}{}", "\"".repeat(996)), 0))
        .collect();
    ledger.record(&batch, 3 * DAY_MS);
    let page = ledger.page(None, None, 5000);
    assert!(page.next_after.is_some(), "the budget must cut the page");
    let body = serde_json::to_vec(&page).expect("serialize");
    assert!(body.len() <= MAX_PAGE_BYTES, "{} bytes", body.len());
}

fn day_row(ledger: &DetectionLedger, day: u32) -> DayRow {
    ledger
        .page(None, None, 5000)
        .days
        .into_iter()
        .find(|row| row.day == day)
        .expect("day")
}

fn carbon(mid: f64, low: f64, avoidable: u64, total: u64) -> DayCarbon {
    DayCarbon {
        gco2_mid: mid,
        gco2_low: low,
        avoidable_io_ops: avoidable,
        total_io_ops: total,
    }
}

#[test]
fn avoidable_carbon_sums_across_the_batches_of_a_day() {
    let ledger = ledger_at(3, true);
    ledger.record_avoidable(carbon(0.5, 0.25, 10, 40), 3 * DAY_MS + 1);
    ledger.record_avoidable(carbon(0.25, 0.125, 5, 20), 3 * DAY_MS + 2);
    let row = day_row(&ledger, 3);
    assert_eq!(row.avoidable_gco2, Some(0.75));
    assert_eq!(row.avoidable_gco2_low, Some(0.375));
    assert_eq!(row.avoidable_io_ops, Some(15));
    assert_eq!(row.total_io_ops, Some(60));
}

#[test]
fn avoidable_carbon_is_absent_until_a_batch_brings_it() {
    let ledger = ledger_at(3, true);
    let row = day_row(&ledger, 3);
    assert_eq!(row.avoidable_gco2, None);
    assert_eq!(row.avoidable_gco2_low, None);
    assert_eq!(row.avoidable_io_ops, None);
    assert_eq!(row.total_io_ops, None);
    let json = serde_json::to_value(&row).expect("serialize");
    for key in [
        "avoidable_gco2",
        "avoidable_gco2_low",
        "avoidable_io_ops",
        "total_io_ops",
    ] {
        assert!(json.get(key).is_none(), "{key}");
    }
}

#[test]
fn a_batch_without_avoidable_io_still_reports_zero() {
    let ledger = ledger_at(3, true);
    ledger.record_avoidable(carbon(0.0, 0.0, 0, 12), 3 * DAY_MS);
    let row = day_row(&ledger, 3);
    assert_eq!(row.avoidable_gco2, Some(0.0));
    assert_eq!(row.avoidable_gco2_low, Some(0.0));
    assert_eq!(row.avoidable_io_ops, Some(0));
    assert_eq!(row.total_io_ops, Some(12));
}

#[test]
fn avoidable_carbon_never_lands_on_a_sealed_day() {
    let ledger = ledger_at(3, true);
    ledger.record_avoidable(carbon(1.0, 0.5, 2, 8), 3 * DAY_MS);
    ledger.tick(4 * DAY_MS, 0, 0);
    ledger.record_avoidable(carbon(1.0, 0.5, 2, 8), 3 * DAY_MS + 100);
    for day in [3, 4] {
        let row = day_row(&ledger, day);
        assert_eq!(row.avoidable_gco2, Some(1.0), "day {day}");
        assert_eq!(row.total_io_ops, Some(8), "day {day}");
    }
}

#[test]
fn non_finite_or_negative_carbon_is_ignored() {
    let ledger = ledger_at(3, true);
    for bad in [
        carbon(f64::NAN, 0.1, 3, 9),
        carbon(f64::INFINITY, 0.1, 3, 9),
        carbon(-1.0, 0.1, 3, 9),
        carbon(0.1, f64::NAN, 3, 9),
        carbon(0.1, -1.0, 3, 9),
    ] {
        ledger.record_avoidable(bad, 3 * DAY_MS);
    }
    let row = day_row(&ledger, 3);
    assert_eq!(row.avoidable_gco2, None);
    assert_eq!(row.avoidable_gco2_low, None);
    assert_eq!(row.avoidable_io_ops, None);
    assert_eq!(row.total_io_ops, None);
}

#[test]
fn avoidable_carbon_makes_an_acked_day_dirty() {
    let ledger = ledger_at(3, true);
    ledger.record(&[finding("a", 1)], 3 * DAY_MS);
    let snapshot = ledger.dirty_days().remove(0);
    ledger.ack_day(3, snapshot.rev);
    assert!(ledger.dirty_days().is_empty());
    ledger.record_avoidable(carbon(0.5, 0.25, 1, 4), 3 * DAY_MS);
    let dirty = ledger.dirty_days();
    assert_eq!(dirty.len(), 1);
    assert_eq!(dirty[0].day, 3);
    assert!(dirty[0].rows.is_empty());
    assert_eq!(dirty[0].meta.avoidable_gco2, Some(0.5));
}

const SLOW: &str = "slow_sql:svc:GET__a:0123456789abcdef0123456789abcdef";
const CHATTY: &str = "chatty_service:svc:GET__b:0123456789abcdef0123456789abcdef";

fn passes(entries: &[(&str, u64)]) -> HashMap<Arc<str>, u64> {
    entries.iter().map(|&(s, n)| (Arc::from(s), n)).collect()
}

fn row(ledger: &DetectionLedger, day: u32, signature: &str) -> Option<CountRow> {
    ledger
        .page(None, None, 5000)
        .counts
        .into_iter()
        .find(|r| r.day == day && r.signature == signature)
}

#[test]
fn set_watch_keeps_trackable_thresholded_signatures_up_to_the_cap() {
    let ledger = DetectionLedger::new(&cfg(8, 2), true, 1.0, DayThresholds::default(), 3 * DAY_MS);
    let hex = "0123456789abcdef0123456789abcdef";
    let head = "slow_sql:svc:";
    let long = format!(
        "{head}{}:{hex}",
        "e".repeat(MAX_TRACKED_SIGNATURE_BYTES + 1 - head.len() - 1 - hex.len())
    );
    assert_eq!(long.len(), MAX_TRACKED_SIGNATURE_BYTES + 1);
    let control = format!("slow_sql:s\nvc:GET__a:{hex}");
    let serialized = format!("serialized_calls:svc:GET__c:{hex}");
    ledger.set_watch(&[
        long,
        control,
        SLOW.to_string(),
        CHATTY.to_string(),
        serialized,
    ]);
    let watch = ledger.watch();
    assert_eq!(watch.len(), 2);
    assert!(watch.watches(&FindingType::SlowSql));
    assert!(watch.watches(&FindingType::ChattyService));
    assert!(!watch.watches(&FindingType::SerializedCalls));
    ledger.set_watch(&[]);
    assert!(ledger.watch().is_empty());
}

#[test]
fn below_passes_open_a_row_without_detections() {
    let ledger = ledger_at(3, true);
    ledger.record_below(&passes(&[(SLOW, 3)]), 3 * DAY_MS);
    let row = row(&ledger, 3, SLOW).expect("row");
    assert_eq!(row.detections, 0);
    assert_eq!(row.below_threshold_passes, Some(3));
    let json = serde_json::to_value(&row).expect("serialize");
    assert_eq!(json["below_threshold_passes"], 3);
}

#[test]
fn below_passes_add_to_a_detected_row() {
    let ledger = ledger_at(3, true);
    ledger.record(&[finding(SLOW, 0)], 3 * DAY_MS);
    ledger.record_below(&passes(&[(SLOW, 2)]), 3 * DAY_MS);
    let row = row(&ledger, 3, SLOW).expect("row");
    assert_eq!(row.detections, 1);
    assert_eq!(row.below_threshold_passes, Some(2));
}

#[test]
fn a_row_without_passes_has_no_below_key() {
    let ledger = ledger_at(3, true);
    ledger.record(&[finding("a", 1)], 3 * DAY_MS);
    let json = serde_json::to_value(row(&ledger, 3, "a").expect("row")).expect("serialize");
    assert!(json.get("below_threshold_passes").is_none());
}

#[test]
fn below_passes_never_land_on_a_sealed_day() {
    let ledger = ledger_at(3, true);
    ledger.tick(4 * DAY_MS, 0, 0);
    ledger.record_below(&passes(&[(SLOW, 1)]), 3 * DAY_MS + 5);
    assert!(row(&ledger, 3, SLOW).is_none());
    assert_eq!(
        row(&ledger, 4, SLOW).expect("row").below_threshold_passes,
        Some(1)
    );
}

#[test]
fn below_passes_respect_the_watch_row_cap() {
    let ledger = DetectionLedger::new(&cfg(8, 1), true, 1.0, DayThresholds::default(), 3 * DAY_MS);
    ledger.record_below(&passes(&[(SLOW, 4)]), 3 * DAY_MS);
    ledger.record_below(&passes(&[(CHATTY, 2)]), 3 * DAY_MS);
    assert!(row(&ledger, 3, SLOW).is_some());
    assert!(row(&ledger, 3, CHATTY).is_none());
    assert_eq!(ledger.overflow(3), 0);
}

#[test]
fn a_watch_row_never_pushes_a_detection_into_overflow() {
    let ledger = DetectionLedger::new(&cfg(8, 1), true, 1.0, DayThresholds::default(), 3 * DAY_MS);
    ledger.record_below(&passes(&[(SLOW, 4)]), 3 * DAY_MS);
    ledger.record(&[finding("a", 1)], 3 * DAY_MS);
    assert_eq!(row(&ledger, 3, "a").expect("row").detections, 1);
    assert_eq!(ledger.overflow(3), 0);
    assert_eq!(
        row(&ledger, 3, SLOW).expect("row").below_threshold_passes,
        Some(4)
    );
    ledger.record(&[finding("b", 1)], 3 * DAY_MS);
    assert_eq!(ledger.overflow(3), 1, "the detection cap still holds");
}

#[test]
fn a_detection_on_a_watch_row_frees_its_watch_slot() {
    let ledger = DetectionLedger::new(&cfg(8, 1), true, 1.0, DayThresholds::default(), 3 * DAY_MS);
    ledger.record_below(&passes(&[(SLOW, 1)]), 3 * DAY_MS);
    ledger.record(&[finding(SLOW, 0)], 3 * DAY_MS);
    ledger.record_below(&passes(&[(CHATTY, 1)]), 3 * DAY_MS);
    assert!(row(&ledger, 3, CHATTY).is_some());
}

#[test]
fn a_day_never_holds_more_than_twice_the_cap() {
    let ledger = DetectionLedger::new(&cfg(8, 1), true, 1.0, DayThresholds::default(), 3 * DAY_MS);
    ledger.record(&[finding("a", 1)], 3 * DAY_MS);
    ledger.record_below(&passes(&[(SLOW, 1)]), 3 * DAY_MS);
    ledger.record(&[finding(SLOW, 0)], 3 * DAY_MS);
    ledger.record_below(&passes(&[(CHATTY, 1)]), 3 * DAY_MS);
    assert!(row(&ledger, 3, CHATTY).is_none());
    assert_eq!(ledger.overflow(3), 0);
}

#[test]
fn below_passes_make_an_acked_day_dirty() {
    let ledger = ledger_at(3, true);
    ledger.record(&[finding("a", 1)], 3 * DAY_MS);
    ledger.ack_day(3, ledger.dirty_days()[0].rev);
    ledger.record_below(&passes(&[(SLOW, 1)]), 3 * DAY_MS);
    let dirty = ledger.dirty_days();
    assert_eq!(dirty.len(), 1);
    assert_eq!(dirty[0].day, 3);
    let rows: Vec<&str> = dirty[0].rows.iter().map(|r| r.signature.as_str()).collect();
    assert_eq!(rows, [SLOW]);
}

#[test]
fn every_day_row_carries_the_ledger_thresholds() {
    let ledger = ledger_at(3, true);
    let json = serde_json::to_value(day_row(&ledger, 3)).expect("serialize");
    assert_eq!(
        json["thresholds"],
        serde_json::json!({
            "slow_query_threshold_ms": 500,
            "slow_http_threshold_ms": 500,
            "max_fanout": 20,
            "chatty_threshold": 15,
            "pool_saturation_threshold": 10,
            "serialized_min_calls": 3
        })
    );
}
