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
