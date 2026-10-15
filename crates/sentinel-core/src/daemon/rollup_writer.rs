//! Daily rollup files beside the window archive. One chained line per
//! UTC day in `<stem>-daily-YYYY-MM.ndjson`, the chain carried across
//! months and restarts, a partial snapshot so a crash costs at most
//! [`SNAPSHOT_EVERY`] windows, and retention by age. Runs on the archive
//! writer's blocking thread. See `docs/design/08-PERIODIC-DISCLOSURE.md`.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::report::Report;
use crate::report::metrics::MetricsState;
use crate::report::periodic::hasher::ARCHIVE_CHAIN_SEED;
use crate::report::periodic::rollup::{self, ClosedDay, DayAccumulator};

/// Windows between two snapshots of the open day: about five minutes at
/// the default 15-second window.
pub(super) const SNAPSHOT_EVERY: u64 = 20;

pub(super) struct RollupWriter {
    dir: PathBuf,
    stem: String,
    retention_days: u32,
    acc: DayAccumulator,
    prev: String,
    seq: u64,
    since_snapshot: u64,
}

impl RollupWriter {
    pub(super) fn open(
        archive_path: &Path,
        retention_days: u32,
        drops: u64,
        metrics: &MetricsState,
    ) -> Self {
        let dir = archive_path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let stem = archive_path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("archive")
            .to_string();
        let mut writer = Self {
            dir,
            stem,
            retention_days,
            acc: DayAccumulator::default(),
            prev: ARCHIVE_CHAIN_SEED.to_string(),
            seq: 0,
            since_snapshot: 0,
        };
        let last_ts = writer.resume();
        writer.recover_snapshot(last_ts, drops);
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
            self.close(&day, drops, metrics);
        }
        self.since_snapshot += 1;
        if self.since_snapshot >= SNAPSHOT_EVERY {
            self.snapshot();
        }
    }

    /// Graceful stop: the day so far becomes a line, and the snapshot it
    /// supersedes goes. A restart the same day opens a second line for it.
    pub(super) fn shutdown(mut self, drops: u64, metrics: &MetricsState) {
        if let Some(day) = self.acc.take() {
            self.close(&day, drops, metrics);
        }
        let _ = std::fs::remove_file(self.partial_path());
    }

    fn close(&mut self, day: &ClosedDay, drops: u64, metrics: &MetricsState) {
        self.append(day, drops);
        self.prune(day.day);
        self.refresh_horizon(metrics);
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

    /// Continue the chain from the newest line on disk. Returns that
    /// line's `ts`, which identifies the day state it closed.
    fn resume(&mut self) -> Option<DateTime<Utc>> {
        let (_, _, newest) = self.files().pop()?;
        let file = std::fs::File::open(newest).ok()?;
        let last = BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
            .filter(rollup::is_rollup_value)
            .last()?;
        let hash = last.get("hash")?.as_str()?.to_string();
        let seq = last.get("seq")?.as_u64()?;
        (self.prev, self.seq) = (hash, seq.saturating_add(1));
        last.get("ts")?
            .as_str()
            .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
            .map(|ts| ts.with_timezone(&Utc))
    }

    /// A snapshot whose day state the last line already closed (same
    /// first window) is stale. Any other is the only copy of its windows.
    fn recover_snapshot(&mut self, last_ts: Option<DateTime<Utc>>, drops: u64) {
        let path = self.partial_path();
        if super::archive::is_symlink(&path) {
            tracing::warn!(path = %path.display(), "daily rollup snapshot is a symlink, refusing to follow it");
            return;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        if let Some(day) = rollup::parse_partial(&text)
            && Some(day.first_ts) != last_ts
        {
            self.append(&day, drops);
        }
        let _ = std::fs::remove_file(path);
    }

    fn append(&mut self, day: &ClosedDay, drops: u64) {
        let line = match rollup::serialize_rollup(day, &self.prev, self.seq, drops) {
            Ok(line) => line,
            Err(err) => {
                tracing::warn!(error = %err, day = %day.day, "daily rollup not serialised, the day falls back to its window lines");
                return;
            }
        };
        let path = self.dir.join(rollup::rollup_file_name(&self.stem, day.day));
        let written = super::archive::open_append(&path)
            .map_err(|err| std::io::Error::other(err.to_string()))
            .and_then(|mut file| super::archive::write_line(&mut file, &line));
        match written {
            Ok(()) => {
                if let Some(hash) = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v.get("hash")?.as_str().map(String::from))
                {
                    self.prev = hash;
                }
                self.seq = self.seq.saturating_add(1);
            }
            Err(err) => {
                tracing::warn!(error = %err, day = %day.day, "daily rollup not written, the day falls back to its window lines");
            }
        }
    }

    /// Overwrite the snapshot atomically: a temporary file, then a rename.
    fn snapshot(&mut self) {
        self.since_snapshot = 0;
        let Some(open) = self.acc.peek() else { return };
        let Ok(text) = rollup::serialize_partial(open) else {
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
    /// before `newest`.
    fn prune(&self, newest: NaiveDate) {
        let cutoff = newest - Duration::days(i64::from(self.retention_days));
        for (year, month, path) in self.files() {
            if last_day_of_month(year, month).is_some_and(|last| last < cutoff)
                && let Err(err) = std::fs::remove_file(&path)
            {
                tracing::warn!(error = %err, path = %path.display(), "expired daily rollup file not removed");
            }
        }
    }

    fn refresh_horizon(&self, metrics: &MetricsState) {
        let mut days = std::collections::BTreeSet::new();
        for (_, _, path) in self.files() {
            let Ok(file) = std::fs::File::open(path) else {
                continue;
            };
            days.extend(
                BufReader::new(file)
                    .lines()
                    .map_while(Result::ok)
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
                    .filter(rollup::is_rollup_value)
                    .filter_map(|v| v.get("day")?.as_str()?.parse::<NaiveDate>().ok()),
            );
        }
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
    use chrono::TimeZone;
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
