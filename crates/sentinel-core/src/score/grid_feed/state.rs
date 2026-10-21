//! Shared state of one grid operator feed: the latest value per operator
//! zone, served to every cloud region mapped to that zone.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::score::carbon::{RealTimeIntensityEntry, RealTimeProvider};

#[derive(Debug, Clone, Copy)]
struct Reading {
    gco2_per_kwh: f64,
    last_update_ms: u64,
}

/// Runtime state shared between a feed's scraper and the scoring path.
#[derive(Debug)]
pub struct GridFeedState {
    provider: RealTimeProvider,
    /// Lowercased cloud region -> operator zone.
    regions: BTreeMap<String, String>,
    readings: ArcSwap<HashMap<String, Reading>>,
}

impl GridFeedState {
    #[must_use]
    pub fn new(provider: RealTimeProvider, regions: BTreeMap<String, String>) -> Arc<Self> {
        Arc::new(Self {
            provider,
            regions,
            readings: ArcSwap::default(),
        })
    }

    /// `cloud_region -> entry` for every region whose zone has a reading
    /// younger than `staleness_ms`.
    #[must_use]
    pub fn snapshot_with_metadata(
        &self,
        now_ms: u64,
        staleness_ms: u64,
    ) -> HashMap<String, RealTimeIntensityEntry> {
        let readings = self.readings.load();
        self.regions
            .iter()
            .filter_map(|(region, zone)| {
                let reading = readings.get(zone)?;
                (now_ms.saturating_sub(reading.last_update_ms) < staleness_ms).then(|| {
                    (
                        region.clone(),
                        RealTimeIntensityEntry {
                            gco2_per_kwh: reading.gco2_per_kwh,
                            is_estimated: None,
                            estimation_method: None,
                            provider: self.provider,
                        },
                    )
                })
            })
            .collect()
    }

    /// Record fresh zone values, keeping the last good value of a zone the
    /// tick did not return.
    pub(crate) fn publish(&self, zones: &[(String, f64)], last_update_ms: u64) {
        let mut table = (**self.readings.load()).clone();
        for (zone, gco2_per_kwh) in zones {
            table.insert(
                zone.clone(),
                Reading {
                    gco2_per_kwh: *gco2_per_kwh,
                    last_update_ms,
                },
            );
        }
        self.readings.store(Arc::new(table));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dk_state() -> Arc<GridFeedState> {
        GridFeedState::new(
            RealTimeProvider::Energinet,
            BTreeMap::from([
                ("denmarkeast".to_string(), "DK2".to_string()),
                ("dk-west".to_string(), "DK1".to_string()),
            ]),
        )
    }

    #[test]
    fn empty_until_published() {
        assert!(dk_state().snapshot_with_metadata(1_000, 5_000).is_empty());
    }

    #[test]
    fn each_region_gets_its_zone_value() {
        let state = dk_state();
        state.publish(
            &[("DK1".to_string(), 120.0), ("DK2".to_string(), 80.0)],
            1_000,
        );
        let snap = state.snapshot_with_metadata(2_000, 5_000);
        assert!((snap["dk-west"].gco2_per_kwh - 120.0).abs() < f64::EPSILON);
        assert!((snap["denmarkeast"].gco2_per_kwh - 80.0).abs() < f64::EPSILON);
        assert_eq!(snap["denmarkeast"].provider, RealTimeProvider::Energinet);
        assert_eq!(snap["denmarkeast"].is_estimated, None);
    }

    #[test]
    fn a_zone_missing_from_a_tick_keeps_its_last_value_until_stale() {
        let state = dk_state();
        state.publish(
            &[("DK1".to_string(), 120.0), ("DK2".to_string(), 80.0)],
            1_000,
        );
        state.publish(&[("DK1".to_string(), 110.0)], 4_000);
        let snap = state.snapshot_with_metadata(5_500, 5_000);
        assert!((snap["dk-west"].gco2_per_kwh - 110.0).abs() < f64::EPSILON);
        assert!(snap["denmarkeast"].gco2_per_kwh > 0.0);
        let later = state.snapshot_with_metadata(6_500, 5_000);
        assert!(!later.contains_key("denmarkeast"), "DK2 went stale");
        assert!(later.contains_key("dk-west"));
    }
}
