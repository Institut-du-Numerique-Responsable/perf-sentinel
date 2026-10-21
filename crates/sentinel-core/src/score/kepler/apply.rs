//! Kepler joules-counter delta math + state update.
//!
//! Methodology (counter-reset semantics, scrape-mode trade-offs) lives in
//! `docs/design/05-GREENOPS-AND-CARBON.md` "Kepler and Redfish attribution
//! notes".

use std::collections::HashMap;

use super::config::KeplerConfig;
use super::state::KeplerState;
use crate::score::energy_state::{services_by_key, split_by_ops};
use crate::score::prom_parser::{PromSample, sum_by_label};

/// Convert a joules delta + ops delta into a kWh-per-op coefficient.
/// Returns `None` when the math is meaningless (zero ops, non-finite
/// or negative joules) so the caller can keep the previous entry.
#[must_use]
pub fn compute_energy_per_op_kwh(joules_delta: f64, ops: u64) -> Option<f64> {
    if ops == 0 || !joules_delta.is_finite() || joules_delta < 0.0 {
        return None;
    }
    // 1 kWh = 3.6e6 J.
    let kwh = joules_delta / 3_600_000.0;
    Some(kwh / ops as f64)
}

/// Compute per-label joule deltas vs the previous scrape, advance the
/// per-label `last_raw_joules` table in place, and return only labels
/// with a strictly positive delta, along with how many mapped services
/// found their label on the wire (fed to the no-match warn). The
/// counter is tracked once per label value, however many services map
/// to it. Counter-reset and first-observation semantics are documented
/// in design doc 05.
#[allow(clippy::implicit_hasher)]
pub fn joules_deltas(
    samples: &[PromSample],
    label_services: &HashMap<String, Vec<String>>,
    last_raw_joules: &mut HashMap<String, f64>,
) -> (HashMap<String, f64>, usize) {
    // O(N) index over samples so the label loop stays O(N + M)
    // instead of O(N × M) on Kepler endpoints exposing hundreds of
    // containers per node. Summing and per-row validation semantics
    // live in [`sum_by_label`]. On cumulative counters the sum stays
    // monotonic while the series set is stable. A vanishing series
    // drops the sum (negative delta, filtered below, next tick
    // re-baselines) and a newly discovered series joins with its
    // near-zero counter.
    let by_label = sum_by_label(samples);
    let mut out = HashMap::with_capacity(label_services.len());
    let mut matched = 0usize;
    for (label_value, services) in label_services {
        let Some(&current) = by_label.get(label_value.as_str()) else {
            continue;
        };
        matched += services.len();
        // Update raw-counter table without cloning the key in the
        // steady state.
        let previous = if let Some(slot) = last_raw_joules.get_mut(label_value) {
            let prev = *slot;
            *slot = current;
            Some(prev)
        } else {
            last_raw_joules.insert(label_value.clone(), current);
            None
        };
        if let Some(prev) = previous {
            let delta = current - prev;
            // Counter resets and non-finite values are filtered here.
            if delta > 0.0 && delta.is_finite() {
                out.insert(label_value.clone(), delta);
            }
        }
    }
    (out, matched)
}

/// Apply a freshly-scraped Kepler batch to a [`KeplerState`]. Each
/// label's joules delta is split between the services mapped to it,
/// by the ops each emitted this window. Services with no ops, or whose
/// label has no delta this window, keep their previous entry.
#[allow(clippy::implicit_hasher)]
pub fn apply_scrape(
    state: &KeplerState,
    label_deltas: &HashMap<String, f64>,
    label_services: &HashMap<String, Vec<String>>,
    op_deltas: &HashMap<String, u64>,
    now_ms: u64,
) {
    let mut next = state.current_owned();
    let mut any_change = false;
    for (label_value, &joules_delta) in label_deltas {
        let Some(services) = label_services.get(label_value) else {
            continue;
        };
        any_change |= split_by_ops(&mut next, services, op_deltas, now_ms, |ops| {
            compute_energy_per_op_kwh(joules_delta, ops)
        });
    }
    if any_change {
        state.publish(next);
    }
}

/// Convenience wrapper: compute deltas and apply in one call. Used by
/// the scraper loop. Returns how many mapped services found their
/// label on the wire, independent of whether a delta was published.
#[allow(clippy::implicit_hasher)]
pub fn process_scrape(
    state: &KeplerState,
    samples: &[PromSample],
    op_deltas: &HashMap<String, u64>,
    cfg: &KeplerConfig,
    last_raw_joules: &mut HashMap<String, f64>,
    now_ms: u64,
) -> usize {
    // Rebuilt per tick: `service_mappings` holds at most 1024 entries.
    let label_services = services_by_key(&cfg.service_mappings);
    let (label_deltas, matched) = joules_deltas(samples, &label_services, last_raw_joules);
    apply_scrape(state, &label_deltas, &label_services, op_deltas, now_ms);
    matched
}
