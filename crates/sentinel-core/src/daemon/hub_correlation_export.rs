//! The daemon's cross-trace correlations on their way to the Hub, as
//! episodes: each period during which a pair kept co-occurring. The daemon
//! cuts them, since only it knows whether a pair still fires.
// Wired into the exporter task by a later change.
#![cfg_attr(not(test), allow(dead_code))]

use std::fmt::Write as _;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::hub_table::Keyed;
use crate::detect::correlate_cross::{CorrelationEndpoint, CrossTraceCorrelation};

/// The longest silence an episode's gap can reach, whatever the window.
const MAX_EPISODE_GAP_MS: u64 = 30 * 60 * 1_000;

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

#[cfg(test)]
pub(crate) mod tests;
