//! Daily rollup files beside the window archive. One chained line per
//! UTC day in `<stem>-daily-YYYY-MM.ndjson`, the chain carried across
//! months and restarts, a partial snapshot so a crash costs at most
//! [`SNAPSHOT_EVERY`] windows, and retention by age. Runs on the archive
//! writer's blocking thread. See `docs/design/08-PERIODIC-DISCLOSURE.md`.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Days, NaiveDate, Utc};
use serde_json::Value;

use crate::report::Report;
use crate::report::metrics::MetricsState;
use crate::report::periodic::hasher::ARCHIVE_CHAIN_SEED;
use crate::report::periodic::rollup::{self, ClosedDay, DayAccumulator};

/// Windows between two snapshots of the open day: about five minutes at
/// the default 15-second window.
pub(super) const SNAPSHOT_EVERY: u64 = 20;

/// Closed days kept in memory while the directory refuses them, retried at
/// each close. Two months of days, then the oldest goes.
const MAX_PENDING: usize = 62;

pub(super) struct RollupWriter {
    dir: PathBuf,
    stem: String,
    retention_days: u32,
    acc: DayAccumulator,
    prev: String,
    seq: u64,
    since_snapshot: u64,
    /// Closed days whose line could not be written yet, oldest first.
    pending: Vec<ClosedDay>,
}

impl RollupWriter {
    pub(super) fn open(
        archive_path: &Path,
        retention_days: u32,
        drops: u64,
        metrics: &MetricsState,
    ) -> Self {
        let stem = archive_path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("archive")
            .to_string();
        let mut writer = Self {
            dir: rollup_dir(archive_path),
            stem,
            retention_days,
            acc: DayAccumulator::default(),
            prev: ARCHIVE_CHAIN_SEED.to_string(),
            seq: 0,
            since_snapshot: 0,
            pending: Vec::new(),
        };
        let written = writer.resume();
        writer.recover_snapshot(&written, drops);
        writer.refresh_horizon(metrics);
        writer
    }

    pub(super) fn observe(
        &mut self,
        ts: DateTime<Utc>,
        report: &Report,
        drops: u64,
        metrics: &MetricsState,
    ) {
        if let Some(day) = self.acc.observe(ts, report) {
            self.close(day, drops, metrics);
        }
        self.since_snapshot += 1;
        if self.since_snapshot >= SNAPSHOT_EVERY {
            self.snapshot();
        }
    }

    /// Graceful stop: the day so far becomes a line and the snapshot it
    /// supersedes goes. A restart the same day opens a second line for it.
    /// A day the directory refuses stays in the snapshot for the next start.
    pub(super) fn shutdown(mut self, drops: u64, metrics: &MetricsState) {
        self.flush_pending(drops);
        match self.acc.take() {
            Some(day) if self.pending.is_empty() && self.append(&day, drops) => {
                self.prune(day.day);
                let _ = std::fs::remove_file(self.partial_path());
            }
            Some(day) => self.write_snapshot(&day),
            None => {
                let _ = std::fs::remove_file(self.partial_path());
            }
        }
        if !self.pending.is_empty() {
            tracing::error!(
                days = self.pending.len(),
                "daily rollups lost at shutdown, the archive directory refused them"
            );
        }
        self.refresh_horizon(metrics);
    }

    fn close(&mut self, day: ClosedDay, drops: u64, metrics: &MetricsState) {
        let closed = day.day;
        self.flush_pending(drops);
        // Behind a refused day, a later one waits too: lines go out in order.
        if !(self.pending.is_empty() && self.append(&day, drops)) {
            self.keep_pending(day);
        }
        self.prune(closed);
        self.refresh_horizon(metrics);
    }

    fn flush_pending(&mut self, drops: u64) {
        while !self.pending.is_empty() {
            let day = self.pending.remove(0);
            if !self.append(&day, drops) {
                self.pending.insert(0, day);
                break;
            }
        }
    }

    fn keep_pending(&mut self, day: ClosedDay) {
        if self.pending.len() >= MAX_PENDING {
            let lost = self.pending.remove(0);
            tracing::error!(
                day = %lost.day,
                "daily rollup dropped, the archive directory has refused rollups for two months"
            );
        }
        self.pending.push(day);
    }

    fn partial_path(&self) -> PathBuf {
        self.dir.join(rollup::partial_file_name(&self.stem))
    }

    /// Rollup files of this archive, oldest month first.
    fn files(&self) -> Vec<(i32, u32, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                let (stem, year, month) = rollup::parse_rollup_file_name(&name)?;
                (stem == self.stem).then(|| (year, month, entry.path()))
            })
            .collect();
        files.sort();
        files
    }

    /// Continue the chain from the last line on disk, whichever month holds
    /// it, and return the first window of every line written, so a snapshot
    /// already appended is never appended again.
    fn resume(&mut self) -> BTreeSet<DateTime<Utc>> {
        let mut written = BTreeSet::new();
        for (_, _, path) in self.files() {
            for value in rollup_values(&path) {
                written.extend(value_ts(&value));
                let hash = value.get("hash").and_then(Value::as_str);
                let seq = value.get("seq").and_then(Value::as_u64);
                if let (Some(hash), Some(seq)) = (hash, seq) {
                    (self.prev, self.seq) = (hash.to_string(), seq.saturating_add(1));
                }
            }
        }
        written
    }

    /// A snapshot some line already holds (same first window) is stale. Any
    /// other is the only copy of its windows: appended, or kept to retry when
    /// the directory refuses it.
    fn recover_snapshot(&mut self, written: &BTreeSet<DateTime<Utc>>, drops: u64) {
        let path = self.partial_path();
        if super::archive::is_symlink(&path) {
            tracing::warn!(path = %path.display(), "daily rollup snapshot is a symlink, refusing to follow it");
            return;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        if let Some(day) = rollup::parse_partial(&text)
            && !written.contains(&day.first_ts)
            && !self.append(&day, drops)
        {
            self.keep_pending(day);
            return;
        }
        let _ = std::fs::remove_file(path);
    }

    fn append(&mut self, day: &ClosedDay, drops: u64) -> bool {
        let line = match rollup::serialize_rollup(day, &self.prev, self.seq, drops) {
            Ok(line) => line,
            Err(err) => {
                tracing::warn!(error = %err, day = %day.day, "daily rollup not serialised");
                return false;
            }
        };
        let path = self.target_path(day.day);
        if let Err(err) = write_sealed(&path, &line) {
            tracing::warn!(error = %err, day = %day.day, "daily rollup not written, kept to retry");
            return false;
        }
        if let Some(hash) = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|v| v.get("hash")?.as_str().map(String::from))
        {
            self.prev = hash;
        }
        self.seq = self.seq.saturating_add(1);
        true
    }

    /// The month file for `day`, never behind the newest month on disk: a
    /// line filed into an older month after a clock step back would sit
    /// ahead of earlier lines in the reader's name order and break the chain.
    fn target_path(&self, day: NaiveDate) -> PathBuf {
        let month = (day.year(), day.month());
        let (year, m) = self
            .files()
            .last()
            .map_or(month, |(y, m, _)| month.max((*y, *m)));
        let first = NaiveDate::from_ymd_opt(year, m, 1).unwrap_or(day);
        self.dir.join(rollup::rollup_file_name(&self.stem, first))
    }

    fn snapshot(&mut self) {
        self.since_snapshot = 0;
        if let Some(open) = self.acc.peek() {
            self.write_snapshot(open);
        }
    }

    /// Overwrite the snapshot atomically: a temporary file, then a rename.
    fn write_snapshot(&self, day: &ClosedDay) {
        let Ok(text) = rollup::serialize_partial(day) else {
            return;
        };
        let target = self.partial_path();
        let tmp = target.with_extension("json.tmp");
        let written = super::archive::open_truncate(&tmp)
            .and_then(|mut file| {
                file.write_all(text.as_bytes())
                    .and_then(|()| file.sync_all())
            })
            .and_then(|()| std::fs::rename(&tmp, &target));
        if let Err(err) = written {
            tracing::warn!(error = %err, "daily rollup snapshot not written");
        }
    }

    /// Delete each month whose last day is more than `retention_days`
    /// before the day just closed, or before today when that day lies in
    /// the future: one future-stamped window must not age out the history.
    fn prune(&self, closed: NaiveDate) {
        let reference = closed.min(Utc::now().date_naive());
        let Some(cutoff) = reference.checked_sub_days(Days::new(u64::from(self.retention_days)))
        else {
            return;
        };
        for (year, month, path) in self.files() {
            if last_day_of_month(year, month).is_some_and(|last| last < cutoff)
                && let Err(err) = std::fs::remove_file(&path)
            {
                tracing::warn!(error = %err, path = %path.display(), "expired daily rollup file not removed");
            }
        }
    }

    fn refresh_horizon(&self, metrics: &MetricsState) {
        let days: BTreeSet<NaiveDate> = self
            .files()
            .iter()
            .flat_map(|(_, _, path)| rollup_values(path))
            .filter_map(|v| v.get("day")?.as_str()?.parse::<NaiveDate>().ok())
            .collect();
        let seconds = |day: &NaiveDate| {
            day.and_hms_opt(0, 0, 0)
                .map_or(0, |t| t.and_utc().timestamp())
        };
        metrics
            .archive_rollup_days
            .set(i64::try_from(days.len()).unwrap_or(i64::MAX));
        metrics
            .archive_oldest_day_seconds
            .set(days.first().map_or(0, seconds));
        metrics
            .archive_last_closed_day_seconds
            .set(days.last().map_or(0, seconds));
    }
}

/// The directory of the window archive, where the rollups live.
pub(super) fn rollup_dir(archive_path: &Path) -> PathBuf {
    archive_path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Append one line, first sealing any fragment a crash or a failed write
/// left on its own line, and cutting back whatever lands of a failed write,
/// the way the window sink does.
fn write_sealed(path: &Path, line: &str) -> std::io::Result<()> {
    let mut file =
        super::archive::open_append(path).map_err(|err| std::io::Error::other(err.to_string()))?;
    super::archive::terminate_incomplete_line(&mut file)?;
    let len = file.metadata()?.len();
    let written = super::archive::write_line(&mut file, line);
    if written.is_err() && file.set_len(len).is_err() {
        let _ = super::archive::terminate_incomplete_line(&mut file);
    }
    written
}

/// The rollup lines of one file. Unreadable files and lines yield nothing.
fn rollup_values(path: &Path) -> Vec<Value> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter(rollup::is_rollup_value)
        .collect()
}

fn value_ts(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("ts")?
        .as_str()
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|ts| ts.with_timezone(&Utc))
}

fn last_day_of_month(year: i32, month: u32) -> Option<NaiveDate> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)?.pred_opt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use tempfile::TempDir;

    fn at(m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, m, d, h, 0, 0).unwrap()
    }

    fn window() -> Report {
        serde_json::from_str(include_str!(
            "../../../../tests/fixtures/periodic/window-report.json"
        ))
        .unwrap()
    }

    fn lines(dir: &Path) -> Vec<serde_json::Value> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.to_string_lossy().contains("-daily-"))
            .collect();
        names.sort();
        names
            .iter()
            .flat_map(|p| {
                std::fs::read_to_string(p)
                    .unwrap()
                    .lines()
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .map(|l| serde_json::from_str(&l).unwrap())
            .collect()
    }

    #[test]
    fn a_day_closes_on_the_next_days_first_window_and_shutdown_writes_the_rest() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(1, 10, 9), &window(), 0, &metrics);
        w.observe(at(1, 11, 9), &window(), 0, &metrics);
        assert_eq!(lines(dir.path()).len(), 1);
        assert_eq!(metrics.archive_rollup_days.get(), 1);
        w.shutdown(0, &metrics);
        let all = lines(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[1]["seq"], 1);
        assert_eq!(all[1]["prev"], all[0]["hash"]);
        assert!(
            !dir.path()
                .join(rollup::partial_file_name("archive"))
                .exists()
        );
    }

    #[test]
    fn the_chain_resumes_across_a_restart_and_a_month() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(1, 31, 9), &window(), 0, &metrics);
        w.shutdown(0, &metrics);
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(2, 1, 9), &window(), 0, &metrics);
        w.shutdown(0, &metrics);
        let all = lines(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[1]["prev"], all[0]["hash"]);
        assert_eq!(all[1]["seq"], 1);
    }

    #[test]
    fn a_snapshot_left_by_a_crash_is_appended_exactly_once() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        for _ in 0..SNAPSHOT_EVERY {
            w.observe(at(1, 10, 9), &window(), 0, &metrics);
        }
        assert!(
            dir.path()
                .join(rollup::partial_file_name("archive"))
                .exists()
        );
        drop(w); // SIGKILL: no shutdown
        let w = RollupWriter::open(&archive, 400, 0, &metrics);
        drop(w); // killed again before any window
        let _w = RollupWriter::open(&archive, 400, 0, &metrics);
        let all = lines(dir.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0]["state"]["windows_aggregated"], SNAPSHOT_EVERY);
    }

    #[test]
    fn a_snapshot_the_last_line_covers_is_discarded() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        for _ in 0..SNAPSHOT_EVERY {
            w.observe(at(1, 10, 9), &window(), 0, &metrics);
        }
        let snapshot =
            std::fs::read(dir.path().join(rollup::partial_file_name("archive"))).unwrap();
        w.shutdown(0, &metrics);
        // A crash between the shutdown append and the snapshot removal.
        std::fs::write(
            dir.path().join(rollup::partial_file_name("archive")),
            snapshot,
        )
        .unwrap();
        let _w = RollupWriter::open(&archive, 400, 0, &metrics);
        assert_eq!(lines(dir.path()).len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_snapshot_behind_a_symlink_is_refused() {
        let dir = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut acc = DayAccumulator::default();
        acc.observe(at(1, 10, 9), &window());
        let planted = elsewhere.path().join("planted.json");
        std::fs::write(
            &planted,
            rollup::serialize_partial(acc.peek().unwrap()).unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            &planted,
            dir.path().join(rollup::partial_file_name("archive")),
        )
        .unwrap();
        let _w = RollupWriter::open(&archive, 400, 0, &metrics);
        assert_eq!(lines(dir.path()).len(), 0);
    }

    fn rollup_lines_in(path: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(rollup::is_rollup_value)
            .collect()
    }

    #[test]
    fn a_fragment_left_by_a_failed_write_does_not_swallow_the_next_day() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(1, 10, 9), &window(), 0, &metrics);
        w.observe(at(1, 11, 9), &window(), 0, &metrics);
        drop(w);
        let month = dir.path().join("archive-daily-2026-01.ndjson");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&month)
            .unwrap();
        file.write_all(b"{\"format\":\"perf-sent").unwrap();
        drop(file);
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(1, 12, 9), &window(), 0, &metrics);
        w.observe(at(1, 13, 9), &window(), 0, &metrics);
        let lines = rollup_lines_in(&month);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["prev"], lines[0]["hash"]);
    }

    #[test]
    fn a_window_stamped_far_ahead_does_not_prune_the_history() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let today = Utc::now();
        let past = today - Duration::days(30);
        let far = today + Duration::days(5 * 366);
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(past, &window(), 0, &metrics);
        w.observe(far, &window(), 0, &metrics);
        w.observe(past + Duration::days(1), &window(), 0, &metrics);
        let past_file = dir
            .path()
            .join(rollup::rollup_file_name("archive", past.date_naive()));
        assert!(
            past_file.exists(),
            "one future-stamped window pruned the history"
        );
    }

    #[test]
    fn a_huge_retention_never_panics() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, u32::MAX, 0, &metrics);
        w.observe(at(1, 10, 9), &window(), 0, &metrics);
        w.observe(at(1, 11, 9), &window(), 0, &metrics);
        w.shutdown(0, &metrics);
        assert_eq!(lines(dir.path()).len(), 2);
    }

    #[test]
    fn an_empty_newest_month_does_not_restart_the_chain() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(1, 10, 9), &window(), 0, &metrics);
        w.shutdown(0, &metrics);
        std::fs::write(dir.path().join("archive-daily-2026-02.ndjson"), b"").unwrap();
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(2, 1, 9), &window(), 0, &metrics);
        w.shutdown(0, &metrics);
        let all = lines(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[1]["prev"], all[0]["hash"]);
        assert_eq!(all[1]["seq"], 1);
    }

    #[test]
    fn a_day_whose_close_fails_at_shutdown_survives_as_the_snapshot() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let blocked = dir.path().join("archive-daily-2026-01.ndjson");
        std::fs::create_dir(&blocked).unwrap();
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(1, 10, 9), &window(), 0, &metrics);
        w.shutdown(0, &metrics);
        assert!(
            dir.path()
                .join(rollup::partial_file_name("archive"))
                .exists()
        );
        std::fs::remove_dir(&blocked).unwrap();
        let _w = RollupWriter::open(&archive, 400, 0, &metrics);
        let all = lines(dir.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0]["state"]["windows_aggregated"], 1);
    }

    #[test]
    fn a_day_whose_close_fails_is_retried_at_the_next_close() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let blocked = dir.path().join("archive-daily-2026-01.ndjson");
        std::fs::create_dir(&blocked).unwrap();
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(1, 10, 9), &window(), 0, &metrics);
        w.observe(at(1, 11, 9), &window(), 0, &metrics);
        std::fs::remove_dir(&blocked).unwrap();
        w.observe(at(1, 12, 9), &window(), 0, &metrics);
        let all = lines(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(
            (all[0]["day"].as_str(), all[1]["day"].as_str()),
            (Some("2026-01-10"), Some("2026-01-11"))
        );
        assert_eq!(all[1]["prev"], all[0]["hash"]);
    }

    #[test]
    fn a_clock_stepped_back_across_a_month_start_keeps_one_ordered_chain() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let oct = |h: u32, m: u32| Utc.with_ymd_and_hms(2026, 10, 1, h, m, 0).unwrap();
        let sep = Utc.with_ymd_and_hms(2026, 9, 30, 23, 50, 0).unwrap();
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(oct(0, 10), &window(), 0, &metrics);
        w.observe(sep, &window(), 0, &metrics);
        w.observe(oct(0, 20), &window(), 0, &metrics);
        w.shutdown(0, &metrics);
        assert!(!dir.path().join("archive-daily-2026-09.ndjson").exists());
        let period = crate::report::periodic::schema::Period {
            from_date: NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
            to_date: NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
            period_type: crate::report::periodic::schema::PeriodType::Custom,
            days_covered: 61,
        };
        let agg = crate::report::periodic::aggregator::aggregate_from_paths(
            &[dir.path().to_path_buf()],
            &period,
            false,
        )
        .unwrap();
        assert_eq!((agg.windows_aggregated, agg.chain_breaks), (3, 0));
    }

    #[test]
    fn retention_deletes_whole_months_past_the_horizon() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 30, 0, &metrics);
        w.observe(at(1, 10, 9), &window(), 0, &metrics);
        w.observe(at(2, 20, 9), &window(), 0, &metrics);
        w.observe(at(3, 25, 9), &window(), 0, &metrics);
        // The March window closes 2026-02-20: January's last day is 20
        // days back, kept. Shutdown closes 2026-03-25: January's last day
        // is then 53 days back, deleted, and February's 25, kept.
        w.shutdown(0, &metrics);
        assert!(!dir.path().join("archive-daily-2026-01.ndjson").exists());
        assert!(dir.path().join("archive-daily-2026-02.ndjson").exists());
        assert_eq!(metrics.archive_rollup_days.get(), 2);
        assert_eq!(
            metrics.archive_oldest_day_seconds.get(),
            at(2, 20, 0).timestamp()
        );
    }
}
