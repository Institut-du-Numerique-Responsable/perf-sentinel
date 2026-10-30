//! In-memory per-day ledger of the finding signatures the daemon detected.
//!
//! Counts are keyed by UTC epoch day then signature. A periodic tick seals
//! past days, carries the shed and reject deltas, and drops days past
//! retention. A restart starts a new ledger (`ledger_id`), so the first day
//! is always a gap. The lock is never held across an `.await`.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;

use crate::config::DaemonDetectionsConfig;
use crate::detect::below_threshold::WatchIndex;
use crate::detect::{DetectConfig, Finding};
use crate::report::metrics::MetricsState;

pub(crate) const DAY_MS: u64 = 86_400_000;
/// Longer signatures are counted in the day's overflow, never tracked.
pub(crate) const MAX_TRACKED_SIGNATURE_BYTES: usize = 1024;
/// Estimated JSON bytes one page may carry.
pub(crate) const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;
/// Estimated JSON bytes of a `CountRow` beside its signature.
/// Room for `below_threshold_passes`.
const ROW_OVERHEAD_BYTES: usize = 128;

/// The JSON length of a tracked signature. Control characters never reach a
/// key (they go to the overflow), so `"` and `\` are the only escapes.
fn json_string_len(s: &str) -> usize {
    s.len() + s.bytes().filter(|b| matches!(b, b'"' | b'\\')).count()
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Tally {
    pub(crate) detections: u64,
    pub(crate) extra_io_ops: u64,
    pub(crate) below_threshold_passes: u64,
    /// The day's `rev` at this row's last change.
    pub(crate) rev: u64,
}

impl Tally {
    fn add(&mut self, extra_io_ops: u64, rev: u64) {
        self.detections += 1;
        self.extra_io_ops += extra_io_ops;
        self.rev = rev;
    }
}

/// One batch's (or one day's summed) operational avoidable carbon and I/O.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct DayCarbon {
    pub(crate) gco2_mid: f64,
    pub(crate) gco2_low: f64,
    pub(crate) avoidable_io_ops: u64,
    pub(crate) total_io_ops: u64,
}

#[derive(Debug)]
pub(crate) struct DayBucket {
    pub(crate) counts: BTreeMap<Arc<str>, Tally>,
    /// Rows opened by below-passes alone (`detections == 0`), capped apart
    /// so they never take a detection slot.
    pub(crate) watch_rows: usize,
    pub(crate) overflow: Tally,
    pub(crate) gap: bool,
    pub(crate) sealed: bool,
    pub(crate) shed_traces: u64,
    pub(crate) ingest_rejected: u64,
    pub(crate) sampling_rate: f64,
    /// The day's sums, `None` until a scored batch brings them.
    pub(crate) avoidable: Option<DayCarbon>,
    /// Bumped on every mutation, compared to `acked_rev` by the Hub push,
    /// which sends only the rows whose own `rev` passed `acked_rev`.
    pub(crate) rev: u64,
    pub(crate) acked_rev: u64,
}

impl DayBucket {
    fn new(sampling_rate: f64) -> Self {
        Self {
            counts: BTreeMap::new(),
            watch_rows: 0,
            overflow: Tally::default(),
            gap: false,
            sealed: false,
            shed_traces: 0,
            ingest_rejected: 0,
            sampling_rate,
            avoidable: None,
            // A new day is unsent, so it starts ahead of `acked_rev`.
            rev: 1,
            acked_rev: 0,
        }
    }
}

#[derive(Debug)]
pub(crate) struct LedgerState {
    pub(crate) days: BTreeMap<u32, DayBucket>,
    last_shed: u64,
    last_rejected: u64,
    last_tick_day: u32,
    /// The signatures the Hub asked to watch, replaced on each reply.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the Hub export and the event loop wire it next")
    )]
    watch: Arc<WatchIndex>,
}

impl LedgerState {
    /// `max(day(now), first unsealed day)`: a late batch or a clock step
    /// back never reopens a sealed day.
    fn effective_day(&self, now_ms: u64) -> u32 {
        let floor = self
            .days
            .iter()
            .find(|(_, b)| !b.sealed)
            .map(|(d, _)| *d)
            .or_else(|| self.sealed_through().map(|d| d.saturating_add(1)))
            .unwrap_or(0);
        day_of(now_ms).max(floor)
    }

    fn sealed_through(&self) -> Option<u32> {
        self.days
            .iter()
            .rev()
            .find(|(_, b)| b.sealed)
            .map(|(d, _)| *d)
    }

    fn bucket(&mut self, day: u32, sampling_rate: f64) -> &mut DayBucket {
        self.days
            .entry(day)
            .or_insert_with(|| DayBucket::new(sampling_rate))
    }

    /// Adds shed traces and ingest rejects to `day` unless it is sealed.
    fn add_losses(&mut self, day: u32, shed: u64, rejected: u64) {
        if shed == 0 && rejected == 0 {
            return;
        }
        if let Some(b) = self.days.get_mut(&day).filter(|b| !b.sealed) {
            b.shed_traces += shed;
            b.ingest_rejected += rejected;
            b.rev += 1;
        }
    }

    /// Reuses the key another day already holds, so days share one `Arc`.
    fn shared_key(&self, signature: &str) -> Arc<str> {
        self.days
            .values()
            .find_map(|b| b.counts.get_key_value(signature))
            .map_or_else(|| Arc::from(signature), |(k, _)| Arc::clone(k))
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct DetectionLedgerStatus {
    pub(crate) ledger_id: String,
    pub(crate) started_ms: u64,
    pub(crate) retained_days: u32,
    pub(crate) oldest_day: u32,
    pub(crate) current_day: u32,
    pub(crate) sealed_through: Option<u32>,
}

/// One page of `GET /api/detections`, the body of a Hub push page too.
#[derive(Debug, Serialize)]
pub(crate) struct DetectionsPage {
    pub(crate) ledger_id: String,
    pub(crate) started_ms: u64,
    pub(crate) current_day: u32,
    pub(crate) oldest_day: u32,
    pub(crate) sealed_through: Option<u32>,
    pub(crate) counts: Vec<CountRow>,
    pub(crate) days: Vec<DayRow>,
    pub(crate) next_after: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CountRow {
    pub(crate) day: u32,
    pub(crate) signature: String,
    pub(crate) detections: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) extra_io_ops: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) below_threshold_passes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DayRow {
    pub(crate) day: u32,
    pub(crate) sealed: bool,
    pub(crate) gap: bool,
    pub(crate) overflow_detections: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) overflow_extra_io_ops: Option<u64>,
    pub(crate) shed_traces: u64,
    pub(crate) ingest_rejected: u64,
    pub(crate) sampling_rate: f64,
    pub(crate) thresholds: DayThresholds,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) avoidable_gco2: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) avoidable_gco2_low: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) avoidable_io_ops: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total_io_ops: Option<u64>,
}

/// The detection thresholds the daemon applies, sent with every day so the
/// Hub reads a pass against the threshold it was measured under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct DayThresholds {
    pub(crate) slow_query_threshold_ms: u64,
    pub(crate) slow_http_threshold_ms: u64,
    pub(crate) max_fanout: u32,
    pub(crate) chatty_threshold: u32,
    pub(crate) pool_saturation_threshold: u32,
    pub(crate) serialized_min_calls: u32,
}

impl From<&DetectConfig> for DayThresholds {
    fn from(cfg: &DetectConfig) -> Self {
        Self {
            // The engine has one slow knob, both slow keys carry it.
            slow_query_threshold_ms: cfg.slow_threshold_ms,
            slow_http_threshold_ms: cfg.slow_threshold_ms,
            max_fanout: cfg.max_fanout,
            chatty_threshold: cfg.chatty_service_min_calls,
            pool_saturation_threshold: cfg.pool_saturation_concurrent_threshold,
            serialized_min_calls: cfg.serialized_min_sequential,
        }
    }
}

impl Default for DayThresholds {
    fn default() -> Self {
        Self::from(&DetectConfig::default())
    }
}

/// One day as the Hub push sends it, copied under the lock.
#[derive(Debug)]
pub(crate) struct DaySnapshot {
    pub(crate) day: u32,
    pub(crate) rev: u64,
    pub(crate) rows: Vec<CountRow>,
    pub(crate) meta: DayRow,
}

#[derive(Debug)]
pub(crate) struct DetectionLedger {
    ledger_id: String,
    started_ms: u64,
    retained_days: u32,
    max_signatures_per_day: usize,
    io_tracked: bool,
    sampling_rate: f64,
    thresholds: DayThresholds,
    state: Mutex<LedgerState>,
}

fn day_of(now_ms: u64) -> u32 {
    u32::try_from(now_ms / DAY_MS).unwrap_or(u32::MAX)
}

fn is_trackable(signature: &str) -> bool {
    signature.len() <= MAX_TRACKED_SIGNATURE_BYTES && !signature.chars().any(char::is_control)
}

impl DetectionLedger {
    pub(crate) fn new(
        cfg: &DaemonDetectionsConfig,
        io_tracked: bool,
        sampling_rate: f64,
        thresholds: DayThresholds,
        now_ms: u64,
    ) -> Self {
        let today = day_of(now_ms);
        let mut first = DayBucket::new(sampling_rate);
        // The daemon was not running since midnight.
        first.gap = true;
        Self {
            ledger_id: uuid::Uuid::new_v4().hyphenated().to_string(),
            started_ms: now_ms,
            retained_days: cfg.retained_days,
            max_signatures_per_day: cfg.max_signatures_per_day,
            io_tracked,
            sampling_rate,
            thresholds,
            state: Mutex::new(LedgerState {
                days: BTreeMap::from([(today, first)]),
                last_shed: 0,
                last_rejected: 0,
                last_tick_day: today,
                watch: Arc::default(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, LedgerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Counts one batch of findings, returns how many went to overflow.
    pub(crate) fn record(&self, findings: &[Finding], now_ms: u64) -> u64 {
        let mut state = self.lock();
        let day = state.effective_day(now_ms);
        let rev = state.bucket(day, self.sampling_rate).rev + 1;
        let mut overflowed = 0;
        let mut changed = false;
        for finding in findings.iter().filter(|f| !f.signature.is_empty()) {
            let io = if self.io_tracked {
                finding
                    .green_impact
                    .as_ref()
                    .map_or(0, |g| g.estimated_extra_io_ops as u64)
            } else {
                0
            };
            if !self.count(&mut state, day, &finding.signature, io, rev) {
                overflowed += 1;
            }
            changed = true;
        }
        if changed {
            state.bucket(day, self.sampling_rate).rev = rev;
        }
        overflowed
    }

    /// Replaces the watch list, keeping the trackable thresholded
    /// signatures up to `max_signatures_per_day`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the Hub export and the event loop wire it next")
    )]
    pub(crate) fn set_watch(&self, signatures: &[String]) {
        let watch = WatchIndex::from_signatures(
            signatures
                .iter()
                .map(String::as_str)
                .filter(|s| is_trackable(s)),
            self.max_signatures_per_day,
        );
        self.lock().watch = Arc::new(watch);
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the Hub export and the event loop wire it next")
    )]
    pub(crate) fn watch(&self) -> Arc<WatchIndex> {
        Arc::clone(&self.lock().watch)
    }

    /// Adds one batch's below-threshold passes to their day. A signature
    /// without a row gets one only while the day has watch-row room, else
    /// its passes are dropped, never sent to overflow.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the Hub export and the event loop wire it next")
    )]
    pub(crate) fn record_below(&self, passes: &HashMap<Arc<str>, u64>, now_ms: u64) {
        // Sorted so the rows kept at the cap do not depend on hash order.
        let mut passes: Vec<_> = passes.iter().filter(|(_, n)| **n > 0).collect();
        passes.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let cap = self.max_signatures_per_day;
        let mut state = self.lock();
        let day = state.effective_day(now_ms);
        let bucket = state.bucket(day, self.sampling_rate);
        let rev = bucket.rev + 1;
        let mut changed = false;
        for (signature, &n) in passes {
            if let Some(tally) = bucket.counts.get_mut(&**signature) {
                tally.below_threshold_passes += n;
                tally.rev = rev;
            } else if bucket.watch_rows < cap && bucket.counts.len() < cap.saturating_mul(2) {
                let tally = Tally {
                    below_threshold_passes: n,
                    rev,
                    ..Tally::default()
                };
                bucket.counts.insert(Arc::clone(signature), tally);
                bucket.watch_rows += 1;
            } else {
                continue;
            }
            changed = true;
        }
        if changed {
            bucket.rev = rev;
        }
    }

    /// Adds one scored batch's avoidable carbon to its day, which the push
    /// then resends.
    pub(crate) fn record_avoidable(&self, batch: DayCarbon, now_ms: u64) {
        // NaN serializes as JSON `null` and the sums must only grow.
        if !(batch.gco2_mid.is_finite() && batch.gco2_low.is_finite())
            || batch.gco2_mid < 0.0
            || batch.gco2_low < 0.0
        {
            return;
        }
        let mut state = self.lock();
        let day = state.effective_day(now_ms);
        let bucket = state.bucket(day, self.sampling_rate);
        let sum = bucket.avoidable.unwrap_or_default();
        bucket.avoidable = Some(DayCarbon {
            gco2_mid: sum.gco2_mid + batch.gco2_mid,
            gco2_low: sum.gco2_low + batch.gco2_low,
            avoidable_io_ops: sum.avoidable_io_ops + batch.avoidable_io_ops,
            total_io_ops: sum.total_io_ops + batch.total_io_ops,
        });
        bucket.rev += 1;
    }

    /// Adds one detection to `day`, `false` when it went to overflow.
    fn count(&self, state: &mut LedgerState, day: u32, signature: &str, io: u64, rev: u64) -> bool {
        let bucket = state.bucket(day, self.sampling_rate);
        if let Some(tally) = bucket.counts.get_mut(signature) {
            if tally.detections == 0 {
                bucket.watch_rows -= 1;
            }
            tally.add(io, rev);
            return true;
        }
        // Watch rows never take a detection slot.
        if bucket.counts.len() - bucket.watch_rows >= self.max_signatures_per_day
            || !is_trackable(signature)
        {
            bucket.overflow.add(io, rev);
            return false;
        }
        let key = state.shared_key(signature);
        state
            .bucket(day, self.sampling_rate)
            .counts
            .entry(key)
            .or_default()
            .add(io, rev);
        true
    }

    /// Periodic pass: carries the shed and reject deltas, seals past days,
    /// drops days past retention. Returns the tracked signature keys left.
    pub(crate) fn tick(&self, now_ms: u64, shed_total: u64, rejected_total: u64) -> usize {
        let mut state = self.lock();
        let today = state.effective_day(now_ms);
        state.bucket(today, self.sampling_rate);
        let shed = shed_total.saturating_sub(state.last_shed);
        let rejected = rejected_total.saturating_sub(state.last_rejected);
        state.last_shed = shed_total;
        state.last_rejected = rejected_total;
        state.add_losses(today, shed, rejected);
        // A delta spanning midnight may belong to either day: flag both.
        let previous = std::mem::replace(&mut state.last_tick_day, today);
        if previous != today {
            state.add_losses(previous, shed, rejected);
        }
        for bucket in state.days.range_mut(..today).map(|(_, b)| b) {
            if !bucket.sealed {
                bucket.sealed = true;
                bucket.rev += 1;
            }
        }
        let oldest = today.saturating_add(1).saturating_sub(self.retained_days);
        state.days = state.days.split_off(&oldest);
        state.days.values().map(|b| b.counts.len()).sum()
    }

    /// Shutdown drain: today's counts stop before midnight.
    pub(crate) fn mark_drain_gap(&self, now_ms: u64) {
        let mut state = self.lock();
        let today = state.effective_day(now_ms);
        let bucket = state.bucket(today, self.sampling_rate);
        if !bucket.gap {
            bucket.gap = true;
            bucket.rev += 1;
        }
    }

    pub(crate) fn status(&self) -> DetectionLedgerStatus {
        self.status_of(&self.lock())
    }

    fn status_of(&self, state: &LedgerState) -> DetectionLedgerStatus {
        let oldest_day = state.days.keys().next().copied();
        let current_day = state.days.keys().next_back().copied();
        DetectionLedgerStatus {
            ledger_id: self.ledger_id.clone(),
            started_ms: self.started_ms,
            retained_days: self.retained_days,
            oldest_day: oldest_day.unwrap_or(state.last_tick_day),
            current_day: current_day.unwrap_or(state.last_tick_day),
            sealed_through: state.sealed_through(),
        }
    }

    /// Rows strictly after `after`, from `since_day` on, in (day, signature)
    /// order. A day's `DayRow` rides with its last count row, or with the
    /// page that passes it when it has none.
    pub(crate) fn page(
        &self,
        since_day: Option<u32>,
        after: Option<(u32, &str)>,
        limit: usize,
    ) -> DetectionsPage {
        let state = self.lock();
        let status = self.status_of(&state);
        let start = since_day.unwrap_or(0).max(after.map_or(0, |(day, _)| day));
        let mut page = DetectionsPage {
            ledger_id: status.ledger_id,
            started_ms: status.started_ms,
            current_day: status.current_day,
            oldest_day: status.oldest_day,
            sealed_through: status.sealed_through,
            counts: Vec::new(),
            days: Vec::new(),
            next_after: None,
        };
        let mut bytes = 0;
        for (&day, bucket) in state.days.range(start..) {
            let cursor = after.filter(|(d, _)| *d == day).map(|(_, sig)| sig);
            let from = cursor.map_or(Bound::Unbounded, Bound::Excluded);
            let rows = bucket.counts.range::<str, _>((from, Bound::Unbounded));
            let before = page.counts.len();
            for (signature, tally) in rows {
                let size = json_string_len(signature) + ROW_OVERHEAD_BYTES;
                if page.counts.len() >= limit || bytes + size > MAX_PAGE_BYTES {
                    if let Some(last) = page.counts.last() {
                        // Empty days past the cursor ride the next page, not both.
                        let last_day = last.day;
                        page.days.retain(|d| d.day <= last_day);
                        page.next_after = Some(format!("{}:{}", last.day, last.signature));
                    }
                    return page;
                }
                bytes += size;
                page.counts.push(self.count_row(day, signature, tally));
            }
            // The cursor's own day went out with the page holding its last row.
            if cursor.is_none() || page.counts.len() > before {
                page.days.push(self.day_row(day, bucket));
            }
        }
        page
    }

    /// Every day whose `rev` moved past `acked_rev`, with only the rows
    /// that changed since then: the Hub keeps the larger count per row, so
    /// the others need not travel again.
    pub(crate) fn dirty_days(&self) -> Vec<DaySnapshot> {
        let state = self.lock();
        state
            .days
            .iter()
            .filter(|(_, bucket)| bucket.rev > bucket.acked_rev)
            .map(|(&day, bucket)| DaySnapshot {
                day,
                rev: bucket.rev,
                rows: bucket
                    .counts
                    .iter()
                    .filter(|(_, tally)| tally.rev > bucket.acked_rev)
                    .map(|(signature, tally)| self.count_row(day, signature, tally))
                    .collect(),
                meta: self.day_row(day, bucket),
            })
            .collect()
    }

    /// The Hub holds `day` as of `rev`.
    pub(crate) fn ack_day(&self, day: u32, rev: u64) {
        if let Some(bucket) = self.lock().days.get_mut(&day) {
            bucket.acked_rev = bucket.acked_rev.max(rev);
        }
    }

    /// Flags `day` incomplete without making it dirty, so a page the Hub
    /// refused is not sent again and again.
    pub(crate) fn mark_gap_quiet(&self, day: u32) {
        if let Some(bucket) = self.lock().days.get_mut(&day) {
            bucket.gap = true;
        }
    }

    fn count_row(&self, day: u32, signature: &str, tally: &Tally) -> CountRow {
        CountRow {
            day,
            signature: signature.to_string(),
            detections: tally.detections,
            extra_io_ops: self.io_tracked.then_some(tally.extra_io_ops),
            below_threshold_passes: (tally.below_threshold_passes > 0)
                .then_some(tally.below_threshold_passes),
        }
    }

    fn day_row(&self, day: u32, bucket: &DayBucket) -> DayRow {
        DayRow {
            day,
            sealed: bucket.sealed,
            gap: bucket.gap,
            overflow_detections: bucket.overflow.detections,
            overflow_extra_io_ops: self.io_tracked.then_some(bucket.overflow.extra_io_ops),
            shed_traces: bucket.shed_traces,
            ingest_rejected: bucket.ingest_rejected,
            sampling_rate: bucket.sampling_rate,
            thresholds: self.thresholds,
            avoidable_gco2: bucket.avoidable.map(|c| c.gco2_mid),
            avoidable_gco2_low: bucket.avoidable.map(|c| c.gco2_low),
            avoidable_io_ops: bucket.avoidable.map(|c| c.avoidable_io_ops),
            total_io_ops: bucket.avoidable.map(|c| c.total_io_ops),
        }
    }

    #[cfg(test)]
    pub(crate) fn tally(&self, day: u32, signature: &str) -> Option<(u64, Option<u64>)> {
        let state = self.lock();
        let tally = state.days.get(&day)?.counts.get(signature)?;
        Some((
            tally.detections,
            self.io_tracked.then_some(tally.extra_io_ops),
        ))
    }

    #[cfg(test)]
    pub(crate) fn overflow(&self, day: u32) -> u64 {
        self.lock()
            .days
            .get(&day)
            .map_or(0, |b| b.overflow.detections)
    }
}

/// Ticks the ledger once a minute and publishes the tracked-key gauge.
pub(super) fn spawn_ticker(
    ledger: Arc<DetectionLedger>,
    metrics: Arc<MetricsState>,
) -> super::task_guard::AbortOnDrop {
    super::task_guard::AbortOnDrop(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_mins(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // the first tick fires immediately, skip it
        loop {
            ticker.tick().await;
            tick_once(&ledger, &metrics, super::event_loop::current_time_ms());
        }
    }))
}

fn tick_once(ledger: &DetectionLedger, metrics: &MetricsState, now_ms: u64) {
    let rejected =
        metrics.otlp_rejected_channel_full.get() + metrics.otlp_rejected_memory_pressure.get();
    let tracked = ledger.tick(now_ms, metrics.analysis_shed_traces_total.get(), rejected);
    metrics
        .detection_ledger_signatures
        .set(i64::try_from(tracked).unwrap_or(i64::MAX));
}

#[cfg(test)]
mod tests;
