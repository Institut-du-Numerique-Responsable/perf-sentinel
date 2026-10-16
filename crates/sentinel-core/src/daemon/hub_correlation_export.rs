//! The daemon's cross-trace correlations on their way to the Hub, as
//! episodes: each period during which a pair kept co-occurring. The daemon
//! cuts them, since only it knows whether a pair still fires.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use prometheus::{Gauge, IntCounter};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::event_loop::current_time_ms;
use super::hub_table::{Keyed, LatestTable, Pending, Route, RouteExport, import_route};
use crate::detect::correlate_cross::{
    CorrelationConfig, CorrelationEndpoint, CrossTraceCorrelation, CrossTraceCorrelator,
};
use crate::http_client;
use crate::report::metrics::MetricsState;
use crate::time::parse_iso8601_utc_to_ms;

/// The longest silence an episode's gap can reach, whatever the window.
const MAX_EPISODE_GAP_MS: u64 = 30 * 60 * 1_000;

/// An open episode is re-sent once its `last_seen` moved this far.
const REQUEUE_AFTER_MS: u64 = 60_000;

/// Episodes per request.
pub(super) const MAX_EPISODE_BATCH: usize = 100;
/// The Hub's cap on a correlation import.
pub(super) const MAX_EPISODE_BODY_BYTES: usize = 1024 * 1024;

/// How long a pair's `last_seen` may stand still before its episode
/// closes: the correlator's window, at most 30 minutes.
pub(super) fn episode_gap_ms(window_ms: u64) -> u64 {
    window_ms.min(MAX_EPISODE_GAP_MS)
}

/// One period of continuous activity of a pair, as the Hub stores it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Episode {
    pub(crate) id: String,
    pub(crate) pair_key: String,
    pub(crate) source: CorrelationEndpoint,
    pub(crate) target: CorrelationEndpoint,
    pub(crate) started_at_ms: u64,
    pub(crate) last_seen_ms: u64,
    pub(crate) ended_at_ms: Option<u64>,
    pub(crate) peak_confidence: f64,
    pub(crate) peak_co_occurrences: u32,
    pub(crate) last_confidence: f64,
    pub(crate) last_co_occurrences: u32,
    pub(crate) median_lag_ms: f64,
    pub(crate) sample_trace_id: Option<String>,
    pub(crate) source_sample_trace_id: Option<String>,
    /// Lets the Hub tell an open episode from one whose daemon died
    /// before closing it.
    pub(crate) gap_ms: u64,
}

impl Keyed for Episode {
    fn key(&self) -> &str {
        &self.id
    }
}

impl Episode {
    fn open(
        correlation: &CrossTraceCorrelation,
        pair_key: String,
        started_at_ms: u64,
        last_seen_ms: u64,
        gap_ms: u64,
    ) -> Self {
        Self {
            id: episode_id(&pair_key, started_at_ms),
            pair_key,
            source: correlation.source.clone(),
            target: correlation.target.clone(),
            started_at_ms,
            last_seen_ms,
            ended_at_ms: None,
            peak_confidence: correlation.confidence,
            peak_co_occurrences: correlation.co_occurrence_count,
            last_confidence: correlation.confidence,
            last_co_occurrences: correlation.co_occurrence_count,
            median_lag_ms: correlation.median_lag_ms,
            sample_trace_id: correlation.sample_trace_id.clone(),
            source_sample_trace_id: correlation.source_sample_trace_id.clone(),
            gap_ms,
        }
    }
}

/// 32 hex characters of a SHA-256 over both endpoints, source first. Every
/// field the correlator keys a pair on goes in, `grouping_value` included,
/// so two namespaces never share a pair.
pub(crate) fn pair_key(source: &CorrelationEndpoint, target: &CorrelationEndpoint) -> String {
    let mut hasher = Sha256::new();
    for side in [source, target] {
        for field in [
            Some(side.finding_type.as_str()),
            Some(side.service.as_str()),
            Some(side.template.as_str()),
            side.grouping_key.as_deref(),
            side.grouping_value.as_deref(),
        ] {
            feed(&mut hasher, field);
        }
    }
    hex32(&hasher.finalize())
}

/// 32 hex characters of a SHA-256 over the pair and the episode's start, so
/// every copy of one episode carries the same id.
pub(crate) fn episode_id(pair_key: &str, started_at_ms: u64) -> String {
    let mut hasher = Sha256::new();
    feed(&mut hasher, Some(pair_key));
    hasher.update(started_at_ms.to_be_bytes());
    hex32(&hasher.finalize())
}

/// Length-prefixed, so no two field lists hash the same bytes.
fn feed(hasher: &mut Sha256, field: Option<&str>) {
    match field {
        Some(value) => {
            hasher.update([1_u8]);
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        None => hasher.update([0_u8]),
    }
}

fn hex32(digest: &[u8]) -> String {
    let mut out = String::with_capacity(32);
    for byte in &digest[..16] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

struct OpenEpisode {
    episode: Episode,
    /// `last_seen_ms` of the copy last queued, for the one-minute throttle.
    queued_seen_ms: u64,
}

impl OpenEpisode {
    fn new(episode: Episode) -> Self {
        let queued_seen_ms = episode.last_seen_ms;
        Self {
            episode,
            queued_seen_ms,
        }
    }

    /// Fold a snapshot row in when its `last_seen` advanced. The copy to
    /// queue when a peak rose or `last_seen` moved a minute past the last
    /// copy queued, otherwise nothing: rolling drift is not sent.
    fn update(
        &mut self,
        correlation: &CrossTraceCorrelation,
        last_seen_ms: u64,
    ) -> Option<Episode> {
        let episode = &mut self.episode;
        if last_seen_ms <= episode.last_seen_ms {
            return None;
        }
        let peaked = correlation.confidence > episode.peak_confidence
            || correlation.co_occurrence_count > episode.peak_co_occurrences;
        episode.last_seen_ms = last_seen_ms;
        episode.peak_confidence = episode.peak_confidence.max(correlation.confidence);
        episode.peak_co_occurrences = episode
            .peak_co_occurrences
            .max(correlation.co_occurrence_count);
        episode.last_confidence = correlation.confidence;
        episode.last_co_occurrences = correlation.co_occurrence_count;
        episode.median_lag_ms = correlation.median_lag_ms;
        episode
            .sample_trace_id
            .clone_from(&correlation.sample_trace_id);
        episode
            .source_sample_trace_id
            .clone_from(&correlation.source_sample_trace_id);
        if !peaked && last_seen_ms - self.queued_seen_ms < REQUEUE_AFTER_MS {
            return None;
        }
        self.queued_seen_ms = last_seen_ms;
        Some(self.episode.clone())
    }
}

#[derive(Default)]
struct Track {
    open: Option<OpenEpisode>,
    /// `last_seen_ms` of the latest episode closed for this pair.
    closed_seen_ms: Option<u64>,
}

impl Track {
    fn close(&mut self) -> Option<Episode> {
        let OpenEpisode { mut episode, .. } = self.open.take()?;
        episode.ended_at_ms = Some(episode.last_seen_ms);
        self.closed_seen_ms = Some(episode.last_seen_ms);
        Some(episode)
    }
}

/// Cuts the correlator's pairs into episodes. Owned by the exporter task,
/// fed one `active_correlations()` snapshot per round.
pub(crate) struct EpisodeTracker {
    window_ms: u64,
    gap_ms: u64,
    // Ordered, so the copies of one round queue in the same order every run.
    pairs: BTreeMap<String, Track>,
    /// Closed pairs remembered so a returning one starts a new episode: as
    /// many as the correlator may still hold.
    max_closed: usize,
}

impl EpisodeTracker {
    pub(crate) fn new(window_ms: u64, max_closed: usize) -> Self {
        Self {
            window_ms,
            gap_ms: episode_gap_ms(window_ms),
            pairs: BTreeMap::new(),
            max_closed,
        }
    }

    /// Fold one snapshot in, close what went quiet, and return the copies
    /// to queue.
    pub(crate) fn observe(
        &mut self,
        snapshot: &[CrossTraceCorrelation],
        now_ms: u64,
    ) -> Vec<Episode> {
        let mut queue: Vec<Episode> = snapshot
            .iter()
            .filter_map(|correlation| self.step(correlation, now_ms))
            .collect();
        let gap_ms = self.gap_ms;
        queue.extend(
            self.pairs
                .values_mut()
                .filter(|track| {
                    track.open.as_ref().is_some_and(|open| {
                        now_ms.saturating_sub(open.episode.last_seen_ms) >= gap_ms
                    })
                })
                .filter_map(Track::close),
        );
        // A closed pair is remembered by count, not by time: below the
        // thresholds it can keep co-occurring, and the correlator its old
        // `first_seen`, long after the episode closed.
        self.pairs
            .retain(|_, track| track.open.is_some() || track.closed_seen_ms.is_some());
        self.forget_oldest_closed();
        queue
    }

    fn forget_oldest_closed(&mut self) {
        let mut closed: Vec<(u64, String)> = self
            .pairs
            .iter()
            .filter(|(_, track)| track.open.is_none())
            .filter_map(|(key, track)| Some((track.closed_seen_ms?, key.clone())))
            .collect();
        let Some(excess) = closed.len().checked_sub(self.max_closed) else {
            return;
        };
        closed.sort_unstable();
        for (_, key) in closed.into_iter().take(excess) {
            self.pairs.remove(&key);
        }
    }

    /// Close every open episode at its `last_seen`, for a graceful shutdown.
    pub(crate) fn close_all(&mut self) -> Vec<Episode> {
        self.pairs.values_mut().filter_map(Track::close).collect()
    }

    fn step(&mut self, correlation: &CrossTraceCorrelation, now_ms: u64) -> Option<Episode> {
        let last_seen_ms = parse_iso8601_utc_to_ms(&correlation.last_seen).ok()?;
        let key = pair_key(&correlation.source, &correlation.target);
        let track = self.pairs.entry(key.clone()).or_default();
        if let Some(open) = track.open.as_mut() {
            return open.update(correlation, last_seen_ms);
        }
        // Active means `last_seen` advancing: a pair quiet for the gap, or
        // not newer than the episode already closed for it, opens nothing.
        let quiet = now_ms.saturating_sub(last_seen_ms) >= self.gap_ms;
        if quiet
            || track
                .closed_seen_ms
                .is_some_and(|closed| last_seen_ms <= closed)
        {
            return None;
        }
        // A pair never met starts at its `first_seen`, but no earlier than
        // the window its counters cover: older activity is not evidenced.
        let started_at_ms = match track.closed_seen_ms {
            Some(_) => last_seen_ms,
            None => parse_iso8601_utc_to_ms(&correlation.first_seen)
                .unwrap_or(last_seen_ms)
                .max(now_ms.saturating_sub(self.window_ms))
                .min(last_seen_ms),
        };
        let episode = Episode::open(correlation, key, started_at_ms, last_seen_ms, self.gap_ms);
        track.open = Some(OpenEpisode::new(episode.clone()));
        Some(episode)
    }
}

pub(crate) type CorrelationExportBuffer = LatestTable<Episode>;

#[derive(Serialize)]
struct EpisodePayload<'a> {
    producer_version: &'static str,
    episodes: Vec<&'a Episode>,
}

fn episode_body(pending: &[Pending<Episode>]) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&EpisodePayload {
        producer_version: env!("CARGO_PKG_VERSION"),
        episodes: pending.iter().map(|entry| &*entry.value).collect(),
    })
}

fn episodes_pending(metrics: &MetricsState) -> &Gauge {
    &metrics.hub_export_correlations_pending
}

fn episodes_dropped(metrics: &MetricsState) -> &IntCounter {
    &metrics.hub_export_correlations_dropped_total
}

static CORRELATION_ROUTE: Route<Episode> = Route {
    max_batch: MAX_EPISODE_BATCH,
    max_body: MAX_EPISODE_BODY_BYTES,
    body: episode_body,
    pending: episodes_pending,
    dropped: episodes_dropped,
    rejected: "PerfSentinelHub rejected the correlation batch permanently; dropping it",
    retrying: "PerfSentinelHub correlation export failed; retaining the batch",
    missing: "PerfSentinelHub has no correlation import route; correlations are not pushed until restart, findings and incidents are",
};

/// The correlation import beside the configured findings import.
pub(super) fn correlations_endpoint(findings_endpoint: &str) -> String {
    import_route(findings_endpoint, "/api/import/correlations")
}

/// The exporter task's correlation half: it reads the correlator, cuts
/// episodes, and sends them on its own route.
pub(crate) struct CorrelationExport {
    correlator: Arc<tokio::sync::Mutex<CrossTraceCorrelator>>,
    tracker: EpisodeTracker,
    buffer: Arc<CorrelationExportBuffer>,
    send: RouteExport<Episode>,
    /// When the correlator is next read. Its own clock, so a send backoff
    /// does not blur when episodes start and end.
    observe_due: tokio::time::Instant,
    /// Set once shutdown closed every episode, so they are closed once.
    closed: bool,
}

impl CorrelationExport {
    pub(crate) fn new(
        correlator: Arc<tokio::sync::Mutex<CrossTraceCorrelator>>,
        config: &CorrelationConfig,
        buffer: Arc<CorrelationExportBuffer>,
        uri: http_client::Uri,
    ) -> Self {
        Self {
            correlator,
            tracker: EpisodeTracker::new(config.window_ms, config.max_tracked_pairs),
            send: RouteExport::with_route(&CORRELATION_ROUTE, buffer.clone(), uri),
            buffer,
            observe_due: tokio::time::Instant::now(),
            closed: false,
        }
    }

    pub(super) fn buffer(&self) -> Arc<CorrelationExportBuffer> {
        self.buffer.clone()
    }

    /// The earlier of the next reading and the next send.
    pub(super) fn due(&self) -> tokio::time::Instant {
        self.observe_due.min(self.send.due)
    }

    pub(super) fn start_at(&mut self, at: tokio::time::Instant) {
        self.observe_due = at;
        self.send.due = at;
    }

    /// Read the correlator when due, then send one batch when due. While
    /// draining, both happen every call, and the first one closes every
    /// open episode. True when a batch went out.
    pub(crate) async fn round(
        &mut self,
        client: &http_client::HttpClientWithBody,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
        draining: bool,
    ) -> bool {
        let now = tokio::time::Instant::now();
        if draining || self.observe_due <= now {
            self.observe(metrics, draining).await;
            self.observe_due = now + flush_interval;
        }
        if draining || self.send.due <= now {
            return self
                .send
                .round(client, api_key, metrics, flush_interval)
                .await;
        }
        false
    }

    async fn observe(&mut self, metrics: &MetricsState, draining: bool) {
        if self.closed || self.buffer.is_unsupported() {
            return;
        }
        let snapshot = self.correlator.lock().await.active_correlations();
        let mut episodes = self.tracker.observe(&snapshot, current_time_ms());
        if draining {
            episodes.extend(self.tracker.close_all());
            self.closed = true;
        }
        let evicted: u64 = episodes
            .iter()
            .map(|episode| self.buffer.push(episode))
            .sum();
        metrics
            .hub_export_correlations_dropped_total
            .inc_by(evicted);
        metrics
            .hub_export_correlations_pending
            .set(self.buffer.len() as f64);
    }
}

#[cfg(test)]
pub(crate) mod tests;
