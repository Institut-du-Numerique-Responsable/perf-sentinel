//! Redfish node-level power to per-service energy-per-op attribution.
//! Formula and node-level granularity trade-offs documented in design
//! doc 05 "Kepler and Redfish attribution notes" and
//! `docs/LIMITATIONS.md` "Redfish BMC precision bounds".

use std::collections::HashMap;

use super::state::ServiceEnergy;
use crate::score::energy_state::split_by_ops;

/// Apply one chassis observation to an in-progress next-state table
/// (caller hoists `state.current_owned()` and `state.publish()` so
/// multi-chassis ticks pay them once, not C times). Returns whether
/// the table was modified.
///
/// Services with zero ops, or chassis with zero total ops, leave the
/// table unchanged (no division by zero, no flapping).
#[allow(clippy::implicit_hasher)]
pub(crate) fn apply_chassis_scrape(
    next: &mut HashMap<String, ServiceEnergy>,
    chassis_services: &[String],
    chassis_watts: f64,
    scrape_interval_secs: f64,
    op_deltas: &HashMap<String, u64>,
    now_ms: u64,
) -> bool {
    if !chassis_watts.is_finite()
        || chassis_watts <= 0.0
        || !scrape_interval_secs.is_finite()
        || scrape_interval_secs <= 0.0
        || chassis_services.is_empty()
    {
        return false;
    }
    // chassis_joules = watts x seconds, kWh = J / 3.6e6. The shared
    // coefficient x service_ops sums back to chassis_joules.
    let chassis_kwh = (chassis_watts * scrape_interval_secs) / 3_600_000.0;
    split_by_ops(next, chassis_services, op_deltas, now_ms, |total_ops| {
        let energy_per_op = chassis_kwh / total_ops as f64;
        (energy_per_op.is_finite() && energy_per_op > 0.0).then_some(energy_per_op)
    })
}
