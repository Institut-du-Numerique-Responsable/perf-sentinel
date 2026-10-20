//! Shared `ArcSwap`-backed storage for per-service energy coefficients.
//!
//! See `docs/design/05-GREENOPS-AND-CARBON.md` § "Energy state cache
//! coherency" for the read-heavy / write-rare design rationale and the
//! ArcSwap-vs-RwLock tradeoff.
//!
//! It also holds the one-source split: a measured or interpolated
//! source is counted once, and the services reading it share its
//! energy in proportion to the ops each emitted this tick
//! ([`services_by_key`], [`split_by_ops`]).

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

/// One row in the shared state: a measured coefficient with a
/// freshness timestamp.
///
/// `last_update_ms` is monotonic milliseconds since process start,
/// produced by [`crate::score::scaphandre::state::monotonic_ms`]. The
/// scoring snapshot uses the `staleness_ms` parameter to discard
/// entries older than `3 × scrape_interval` (so a hung scraper does
/// not silently return increasingly stale data).
#[derive(Debug, Clone, Copy)]
pub(crate) struct EnergyRow {
    pub(crate) energy_per_op_kwh: f64,
    pub(crate) last_update_ms: u64,
}

/// Shared storage for per-service energy coefficients with staleness
/// filtering.
///
/// Constructors and method signatures mirror the `ScaphandreState` /
/// `CloudEnergyState` public surface so the wrapping newtypes can
/// delegate line-for-line.
#[derive(Debug, Default)]
pub(crate) struct AgedEnergyMap {
    inner: ArcSwap<HashMap<String, EnergyRow>>,
}

impl AgedEnergyMap {
    /// Synchronous snapshot of per-service coefficients, filtering out
    /// rows whose age is `>= staleness_ms`.
    ///
    /// The returned `HashMap<String, f64>` is owned: the scoring path
    /// hands this directly to `CarbonContext.energy_snapshot`. Keys are
    /// cloned once per fresh row, typically single digits of services.
    #[must_use]
    pub(crate) fn snapshot(&self, now_ms: u64, staleness_ms: u64) -> HashMap<String, f64> {
        let current = self.inner.load_full();
        current
            .iter()
            .filter_map(|(service, energy)| {
                // Saturating sub so a clock skew or monotonic-reset
                // event does not accidentally mark fresh rows as stale.
                let age = now_ms.saturating_sub(energy.last_update_ms);
                if age < staleness_ms {
                    Some((service.clone(), energy.energy_per_op_kwh))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Publish a fresh table, atomically replacing the previous one.
    /// Called by the scraper task after each successful scrape cycle.
    pub(crate) fn publish(&self, new_table: HashMap<String, EnergyRow>) {
        self.inner.store(Arc::new(new_table));
    }

    /// Produce an owned copy of the current table so the scraper can
    /// merge-update it before publishing the new version.
    pub(crate) fn current_owned(&self) -> HashMap<String, EnergyRow> {
        (*self.inner.load_full()).clone()
    }

    /// Test-only helper: insert an entry directly without running the
    /// full scrape loop.
    #[cfg(test)]
    pub(crate) fn insert_for_test(
        &self,
        service: String,
        energy_per_op_kwh: f64,
        last_update_ms: u64,
    ) {
        let mut current = self.current_owned();
        current.insert(
            service,
            EnergyRow {
                energy_per_op_kwh,
                last_update_ms,
            },
        );
        self.publish(current);
    }
}

/// Insert or overwrite one service's row without re-cloning the key in
/// the steady state (the common case: the service already has an
/// entry, only its coefficient moves). Used by [`split_by_ops`].
pub(crate) fn upsert_row(map: &mut HashMap<String, EnergyRow>, service: &str, row: EnergyRow) {
    if let Some(slot) = map.get_mut(service) {
        *slot = row;
    } else {
        map.insert(service.to_owned(), row);
    }
}

/// Build the `source key -> Vec<service>` reverse index from a
/// `service -> source key` mapping. Built once at scraper startup, so a
/// tick does not walk the full mapping per source, an O(K x S) cost.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub(crate) fn services_by_key(map: &HashMap<String, String>) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (svc, key) in map {
        out.entry(key.clone()).or_default().push(svc.clone());
    }
    out
}

/// Split one source's energy between the services reading it, by the
/// ops each emitted this tick. Returns whether `next` was modified.
///
/// Contributors are the services with ops > 0. A zero sum leaves the
/// table untouched, so a service with no ops keeps its previous row.
/// Otherwise `energy_per_op` runs once with the summed ops, and a
/// finite `Some(e)` is written for every contributor, so `e x ops`
/// sums back to the source's energy. Each backend keeps its own guard
/// (sign, zero) inside the closure.
#[allow(clippy::implicit_hasher)]
pub(crate) fn split_by_ops(
    next: &mut HashMap<String, EnergyRow>,
    services: &[String],
    op_deltas: &HashMap<String, u64>,
    now_ms: u64,
    energy_per_op: impl FnOnce(u64) -> Option<f64>,
) -> bool {
    let mut contributors: Vec<&str> = Vec::with_capacity(services.len());
    let mut total_ops: u64 = 0;
    for svc in services {
        let ops = op_deltas.get(svc.as_str()).copied().unwrap_or(0);
        if ops > 0 {
            contributors.push(svc.as_str());
            total_ops = total_ops.saturating_add(ops);
        }
    }
    if total_ops == 0 {
        return false;
    }
    let Some(energy_per_op_kwh) = energy_per_op(total_ops).filter(|e| e.is_finite()) else {
        return false;
    };
    let row = EnergyRow {
        energy_per_op_kwh,
        last_update_ms: now_ms,
    };
    for svc in contributors {
        upsert_row(next, svc, row);
    }
    true
}

/// Generate a nominally distinct energy-state wrapper around
/// [`AgedEnergyMap`]. Each invocation creates a new type that delegates
/// `new`, `snapshot`, `publish`, `current_owned`, and `insert_for_test`
/// to the shared storage. The types are not unified, so the daemon
/// cannot accidentally swap a Scaphandre state for a cloud-energy state
/// (or vice versa) when tagging the energy model.
macro_rules! impl_energy_state {
    (
        $(#[$meta:meta])*
        $vis:vis struct $Name:ident;
    ) => {
        $(#[$meta])*
        $vis struct $Name {
            inner: $crate::score::energy_state::AgedEnergyMap,
        }

        impl $Name {
            #[must_use]
            $vis fn new() -> std::sync::Arc<Self> {
                std::sync::Arc::new(Self::default())
            }

            #[must_use]
            $vis fn snapshot(
                &self,
                now_ms: u64,
                staleness_ms: u64,
            ) -> std::collections::HashMap<String, f64> {
                self.inner.snapshot(now_ms, staleness_ms)
            }

            pub(super) fn publish(
                &self,
                new_table: std::collections::HashMap<String, ServiceEnergy>,
            ) {
                self.inner.publish(new_table);
            }

            pub(super) fn current_owned(
                &self,
            ) -> std::collections::HashMap<String, ServiceEnergy> {
                self.inner.current_owned()
            }

            #[cfg(test)]
            pub(crate) fn insert_for_test(
                &self,
                service: String,
                energy_per_op_kwh: f64,
                last_update_ms: u64,
            ) {
                self.inner
                    .insert_for_test(service, energy_per_op_kwh, last_update_ms);
            }
        }
    };
}

pub(crate) use impl_energy_state;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_returns_empty_snapshot() {
        let state = AgedEnergyMap::default();
        assert!(state.snapshot(1000, 5000).is_empty());
    }

    #[test]
    fn fresh_entry_appears_in_snapshot() {
        let state = AgedEnergyMap::default();
        state.insert_for_test("svc-a".into(), 1e-7, 100);
        let snap = state.snapshot(200, 500);
        assert_eq!(snap.len(), 1);
        assert!((snap["svc-a"] - 1e-7).abs() < 1e-15);
    }

    #[test]
    fn stale_entry_filtered_out() {
        let state = AgedEnergyMap::default();
        state.insert_for_test("svc-a".into(), 1e-7, 100);
        // now=700, staleness=500: age 600 >= 500, so the row is stale.
        assert!(state.snapshot(700, 500).is_empty());
    }

    #[test]
    fn mixed_fresh_and_stale() {
        let state = AgedEnergyMap::default();
        state.insert_for_test("fresh".into(), 2e-7, 500);
        state.insert_for_test("stale".into(), 3e-7, 100);
        let snap = state.snapshot(600, 200);
        assert_eq!(snap.len(), 1);
        assert!(snap.contains_key("fresh"));
        assert!(!snap.contains_key("stale"));
    }

    #[test]
    fn saturating_sub_protects_against_clock_skew() {
        let state = AgedEnergyMap::default();
        // Row at t=1000, read at t=500 (time went backwards).
        // saturating_sub gives age 0 < staleness, so the row is fresh.
        state.insert_for_test("svc".into(), 5e-7, 1000);
        let snap = state.snapshot(500, 200);
        assert_eq!(snap.len(), 1);
    }

    fn ops(pairs: &[(&str, u64)]) -> HashMap<String, u64> {
        pairs.iter().map(|(s, n)| ((*s).to_string(), *n)).collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn split_by_ops_sums_back_to_the_source_energy() {
        let source_kwh = 1e-3;
        let deltas = ops(&[("a", 1), ("b", 2), ("c", 7)]);
        let mut next = HashMap::new();
        let changed = split_by_ops(&mut next, &names(&["a", "b", "c"]), &deltas, 42, |total| {
            Some(source_kwh / total as f64)
        });
        assert!(changed);
        let summed: f64 = deltas
            .iter()
            .map(|(svc, n)| next[svc].energy_per_op_kwh * *n as f64)
            .sum();
        assert!((summed - source_kwh).abs() < 1e-15);
        assert!(next.values().all(|row| row.last_update_ms == 42));
    }

    #[test]
    fn split_by_ops_keeps_a_zero_ops_member_row() {
        let old = EnergyRow {
            energy_per_op_kwh: 9e-9,
            last_update_ms: 5,
        };
        let mut next = HashMap::from([("idle".to_string(), old)]);
        let deltas = ops(&[("busy", 4), ("idle", 0)]);
        assert!(split_by_ops(
            &mut next,
            &names(&["busy", "idle"]),
            &deltas,
            100,
            |total| { Some(1.0 / total as f64) }
        ));
        assert!((next["busy"].energy_per_op_kwh - 0.25).abs() < 1e-15);
        assert_eq!(next["idle"].last_update_ms, 5);
        assert!((next["idle"].energy_per_op_kwh - 9e-9).abs() < 1e-20);
    }

    #[test]
    fn split_by_ops_all_zero_ops_leaves_the_table_unchanged() {
        let mut next = HashMap::new();
        let deltas = ops(&[("a", 0)]);
        let mut called = false;
        let changed = split_by_ops(&mut next, &names(&["a", "b"]), &deltas, 1, |_| {
            called = true;
            Some(1.0)
        });
        assert!(!changed);
        assert!(!called);
        assert!(next.is_empty());
    }

    #[test]
    fn split_by_ops_rejects_none_and_nan() {
        let deltas = ops(&[("a", 3)]);
        let services = names(&["a"]);
        for value in [None, Some(f64::NAN), Some(f64::INFINITY)] {
            let mut next = HashMap::new();
            assert!(!split_by_ops(&mut next, &services, &deltas, 1, |_| value));
            assert!(next.is_empty());
        }
    }

    #[test]
    fn current_owned_returns_independent_copy() {
        let state = AgedEnergyMap::default();
        state.insert_for_test("svc".into(), 1e-7, 100);
        let mut owned = state.current_owned();
        owned.clear();
        // Original state must be unaffected.
        assert_eq!(state.snapshot(200, 500).len(), 1);
    }
}
