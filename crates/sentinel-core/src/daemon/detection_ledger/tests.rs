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
    DetectionLedger::new(&cfg(8, 10_000), io_tracked, 1.0, day * DAY_MS)
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
    let ledger = DetectionLedger::new(&cfg(8, 10_000), true, 1.0, 3 * DAY_MS + 5);
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
    let ledger = DetectionLedger::new(&cfg(8, 2), true, 1.0, 3 * DAY_MS);
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
    let ledger = DetectionLedger::new(&cfg(2, 10_000), true, 1.0, DAY_MS);
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
