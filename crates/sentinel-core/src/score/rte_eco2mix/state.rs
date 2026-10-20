//! Shared RTE `éCO2mix` state: one national reading, read by the scoring
//! path for every configured region.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwapOption;

use crate::score::carbon::{RealTimeIntensityEntry, RealTimeProvider};

#[derive(Debug, Clone, Copy)]
struct Reading {
    gco2_per_kwh: f64,
    last_update_ms: u64,
}

/// Runtime state shared between the RTE scraper and the scoring path.
#[derive(Debug)]
pub struct RteEco2mixState {
    regions: Vec<String>,
    inner: ArcSwapOption<Reading>,
}

impl RteEco2mixState {
    /// Build an empty state serving `regions` (already lowercased).
    #[must_use]
    pub fn new(regions: Vec<String>) -> Arc<Self> {
        Arc::new(Self {
            regions,
            inner: ArcSwapOption::empty(),
        })
    }

    /// `cloud_region -> entry` for every configured region, empty when
    /// no reading is younger than `staleness_ms`.
    #[must_use]
    pub fn snapshot_with_metadata(
        &self,
        now_ms: u64,
        staleness_ms: u64,
    ) -> HashMap<String, RealTimeIntensityEntry> {
        let Some(reading) = self.inner.load_full() else {
            return HashMap::new();
        };
        if now_ms.saturating_sub(reading.last_update_ms) >= staleness_ms {
            return HashMap::new();
        }
        self.regions
            .iter()
            .map(|region| {
                (
                    region.clone(),
                    RealTimeIntensityEntry {
                        gco2_per_kwh: reading.gco2_per_kwh,
                        is_estimated: None,
                        estimation_method: None,
                        provider: RealTimeProvider::RteEco2mix,
                    },
                )
            })
            .collect()
    }

    pub(crate) fn publish(&self, gco2_per_kwh: f64, last_update_ms: u64) {
        self.inner.store(Some(Arc::new(Reading {
            gco2_per_kwh,
            last_update_ms,
        })));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_until_published() {
        let state = RteEco2mixState::new(vec!["eu-west-3".to_string()]);
        assert!(state.snapshot_with_metadata(1_000, 5_000).is_empty());
    }

    #[test]
    fn fresh_reading_is_served_to_every_region() {
        let state =
            RteEco2mixState::new(vec!["eu-west-3".to_string(), "francecentral".to_string()]);
        state.publish(21.0, 1_000);
        let snap = state.snapshot_with_metadata(2_000, 5_000);
        assert_eq!(snap.len(), 2);
        let entry = &snap["francecentral"];
        assert!((entry.gco2_per_kwh - 21.0).abs() < f64::EPSILON);
        assert_eq!(entry.provider, RealTimeProvider::RteEco2mix);
        assert_eq!(entry.is_estimated, None);
    }

    #[test]
    fn stale_reading_is_dropped() {
        let state = RteEco2mixState::new(vec!["eu-west-3".to_string()]);
        state.publish(21.0, 1_000);
        assert!(state.snapshot_with_metadata(6_000, 5_000).is_empty());
    }
}
