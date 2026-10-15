//! Daily rollups of the disclosure archive. One chained NDJSON line per
//! UTC day carries the [`PeriodState`] the aggregator would build from
//! that day's windows, so a quarter is 92 lines instead of 500,000. See
//! `docs/design/08-PERIODIC-DISCLOSURE.md`.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;

use super::aggregator::PeriodState;
#[cfg(any(feature = "daemon", test))]
use super::hasher::archive_chain_hash;
#[cfg(any(feature = "daemon", test))]
use crate::report::Report;

/// Tag every rollup line carries, inside the hashed body.
pub(crate) const ROLLUP_FORMAT: &str = "perf-sentinel-rollup/v1";

#[cfg(any(feature = "daemon", test))]
/// `<stem>-daily-YYYY-MM.ndjson`: one file per month, so retention deletes
/// whole files and never rewrites a chained one.
pub(crate) fn rollup_file_name(stem: &str, day: NaiveDate) -> String {
    format!("{stem}-daily-{}.ndjson", day.format("%Y-%m"))
}

/// The `(stem, year, month)` a rollup file name encodes.
pub(crate) fn parse_rollup_file_name(name: &str) -> Option<(&str, i32, u32)> {
    let body = name.strip_suffix(".ndjson")?;
    let (stem, year_month) = body.rsplit_once("-daily-")?;
    let (year, month) = year_month.split_once('-')?;
    if stem.is_empty() || year.len() != 4 || month.len() != 2 {
        return None;
    }
    let year: i32 = year.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    (1..=12).contains(&month).then_some((stem, year, month))
}

#[cfg(any(feature = "daemon", test))]
/// The crash snapshot of the open day. `.json`, so a directory passed to
/// `disclose --input` never picks it up.
pub(crate) fn partial_file_name(stem: &str) -> String {
    format!("{stem}-daily.partial.json")
}

#[cfg(any(feature = "daemon", test))]
/// One UTC day's windows, folded.
pub(crate) struct ClosedDay {
    pub day: NaiveDate,
    pub first_ts: DateTime<Utc>,
    pub last_ts: DateTime<Utc>,
    pub state: PeriodState,
}

#[cfg(any(feature = "daemon", test))]
/// Folds windows into the open day and hands the day back once a window
/// lands on another one.
#[derive(Default)]
pub(crate) struct DayAccumulator {
    open: Option<ClosedDay>,
}

#[cfg(any(feature = "daemon", test))]
impl DayAccumulator {
    /// Fold one window. A window on another UTC day closes the open one,
    /// in either direction: a clock stepped back across midnight closes it
    /// too, and the earlier day gets a second line that merges with its
    /// first.
    pub(crate) fn observe(&mut self, ts: DateTime<Utc>, report: &Report) -> Option<ClosedDay> {
        let day = ts.date_naive();
        let closed = self.open.take_if(|open| open.day != day);
        let open = self.open.get_or_insert_with(|| ClosedDay {
            day,
            first_ts: ts,
            last_ts: ts,
            state: PeriodState::default(),
        });
        open.first_ts = open.first_ts.min(ts);
        open.last_ts = open.last_ts.max(ts);
        // Never strict here: `--strict-attribution` is the reader's call,
        // made from `first_unattributed`.
        if let Err(err) = open.state.process_window(report, ts, false) {
            tracing::warn!(error = %err, "window not folded into the daily rollup");
        }
        closed
    }

    pub(crate) fn take(&mut self) -> Option<ClosedDay> {
        self.open.take()
    }

    pub(crate) fn peek(&self) -> Option<&ClosedDay> {
        self.open.as_ref()
    }
}

#[cfg(any(feature = "daemon", test))]
fn body(day: &ClosedDay) -> serde_json::Value {
    serde_json::json!({
        "format": ROLLUP_FORMAT,
        "ts": day.first_ts,
        "day": day.day,
        "last_ts": day.last_ts,
        "state": &day.state,
    })
}

#[cfg(any(feature = "daemon", test))]
/// Serialise one day and chain it to `prev`, the way the window writer
/// chains a window. `ts` is the day's first window, so the reader's
/// period and reach checks work on it unchanged.
pub(crate) fn serialize_rollup(
    day: &ClosedDay,
    prev: &str,
    seq: u64,
    drops: u64,
) -> Result<String, serde_json::Error> {
    let mut line = body(day);
    if let Some(obj) = line.as_object_mut() {
        obj.insert("prev".into(), prev.into());
        obj.insert("seq".into(), seq.into());
        obj.insert("drops".into(), drops.into());
    }
    let hash = archive_chain_hash(&line)?;
    if let Some(obj) = line.as_object_mut() {
        obj.insert("hash".into(), hash.into());
    }
    serde_json::to_string(&line)
}

#[cfg(any(feature = "daemon", test))]
/// The open day, unchained and without the format tag, so it can never
/// pass for a rollup line.
pub(crate) fn serialize_partial(day: &ClosedDay) -> Result<String, serde_json::Error> {
    let mut snapshot = body(day);
    if let Some(obj) = snapshot.as_object_mut() {
        obj.remove("format");
    }
    serde_json::to_string(&snapshot)
}

#[cfg(any(feature = "daemon", test))]
pub(crate) fn parse_partial(text: &str) -> Option<ClosedDay> {
    #[derive(Deserialize)]
    struct Snapshot {
        ts: DateTime<Utc>,
        day: NaiveDate,
        last_ts: DateTime<Utc>,
        state: PeriodState,
    }
    let snapshot: Snapshot = serde_json::from_str(text).ok()?;
    Some(ClosedDay {
        day: snapshot.day,
        first_ts: snapshot.ts,
        last_ts: snapshot.last_ts,
        state: snapshot.state,
    })
}

pub(crate) fn is_rollup_value(value: &serde_json::Value) -> bool {
    value.get("format").and_then(serde_json::Value::as_str) == Some(ROLLUP_FORMAT)
}

/// The fields the reader folds. `prev`, `seq`, `drops` and `hash` are
/// read by the chain walk on the raw value. `last_ts` is left unread.
#[derive(Deserialize)]
pub(crate) struct RollupLine {
    pub ts: DateTime<Utc>,
    pub day: NaiveDate,
    pub state: PeriodState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Report;
    use chrono::TimeZone;

    fn at(d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, d, h, 0, 0).unwrap()
    }

    fn window() -> Report {
        serde_json::from_str(include_str!(
            "../../../../../tests/fixtures/periodic/window-report.json"
        ))
        .unwrap()
    }

    #[test]
    fn a_new_day_closes_the_previous_one() {
        let mut acc = DayAccumulator::default();
        assert!(acc.observe(at(10, 9), &window()).is_none());
        assert!(acc.observe(at(10, 23), &window()).is_none());
        let closed = acc.observe(at(11, 0), &window()).unwrap();
        assert_eq!(closed.day, at(10, 0).date_naive());
        assert_eq!((closed.first_ts, closed.last_ts), (at(10, 9), at(10, 23)));
        assert_eq!(closed.state.windows_aggregated(), 2);
        assert_eq!(acc.peek().unwrap().day, at(11, 0).date_naive());
    }

    #[test]
    fn a_clock_stepped_back_across_midnight_opens_a_second_line_for_the_earlier_day() {
        let mut acc = DayAccumulator::default();
        acc.observe(at(10, 23), &window());
        let first = acc.observe(at(11, 0), &window()).unwrap();
        let second = acc.observe(at(10, 23), &window()).unwrap();
        assert_eq!(first.day, at(10, 0).date_naive());
        assert_eq!(second.day, at(11, 0).date_naive());
        assert_eq!(acc.take().unwrap().day, at(10, 0).date_naive());
    }

    #[test]
    fn rollup_file_names_round_trip() {
        let name = rollup_file_name("archive", at(9, 0).date_naive());
        assert_eq!(name, "archive-daily-2026-01.ndjson");
        assert_eq!(parse_rollup_file_name(&name), Some(("archive", 2026, 1)));
        assert_eq!(
            parse_rollup_file_name("archive-20260110T000000000Z.ndjson"),
            None
        );
        assert_eq!(parse_rollup_file_name("archive-daily-2026-13.ndjson"), None);
        assert_eq!(parse_rollup_file_name("-daily-2026-01.ndjson"), None);
    }

    #[test]
    fn a_rollup_line_verifies_with_the_archive_chain() {
        let mut acc = DayAccumulator::default();
        acc.observe(at(10, 9), &window());
        let day = acc.take().unwrap();
        let line = serialize_rollup(&day, "seed", 0, 3).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(is_rollup_value(&value));
        let stated = value.as_object_mut().unwrap().remove("hash").unwrap();
        let recomputed = crate::report::periodic::hasher::archive_chain_hash(&value).unwrap();
        assert_eq!(stated.as_str(), Some(recomputed.as_str()));
        let parsed: RollupLine = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.day, day.day);
    }

    #[test]
    fn a_partial_snapshot_round_trips_and_is_not_a_rollup_line() {
        let mut acc = DayAccumulator::default();
        acc.observe(at(10, 9), &window());
        let text = serialize_partial(acc.peek().unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(!is_rollup_value(&value));
        let back = parse_partial(&text).unwrap();
        assert_eq!(back.first_ts, at(10, 9));
        assert_eq!(back.state.windows_aggregated(), 1);
    }
}
