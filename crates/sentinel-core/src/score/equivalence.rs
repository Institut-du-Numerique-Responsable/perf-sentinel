//! Everyday CO2 equivalences for share cards, one table per country. Rows
//! come from three sources: the ADEME Base Carbone (France), the DESNZ
//! GHG conversion factors (UK) and the US EPA GHG Equivalencies
//! Calculator (US). Rows and vintages are generated into
//! `equivalence_data.rs` by `scripts/refresh-equivalences.py`, selection
//! rules in `docs/SHARE-CARD.md`.

use serde::{Deserialize, Serialize};

use super::equivalence_data::GENERATED_EQUIVALENCES;

/// The country whose everyday equivalences a card picks from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EquivalenceCountry {
    #[default]
    Fr,
    Gb,
    Us,
}

/// One emission factor, in µg of CO2 equivalent per `unit`.
#[derive(Debug, PartialEq, Eq)]
pub struct Equivalence {
    pub id: &'static str,
    pub countries: &'static [EquivalenceCountry],
    /// Whether `pick_equivalence` may choose this row. A row that is not
    /// auto is reachable by id only.
    pub auto: bool,
    pub source: &'static str,
    pub source_ref: &'static str,
    pub vintage: &'static str,
    pub per_unit_ug: u64,
    pub unit: &'static str,
    pub label: &'static str,
}

/// Below two, "1 meal" reads as a rounding artefact rather than a figure.
pub const MIN_EQUIVALENCE_COUNT: u64 = 2;
/// Upper bound on the count of an equivalence the caller chooses by id.
pub const MAX_CHOSEN_EQUIVALENCE_COUNT: u64 = 1000;

/// Every row, by ascending `per_unit_ug`.
#[must_use]
pub fn equivalences() -> &'static [Equivalence] {
    GENERATED_EQUIVALENCES
}

/// The rows listing `country`, by ascending `per_unit_ug`.
pub fn equivalences_for(country: EquivalenceCountry) -> impl Iterator<Item = &'static Equivalence> {
    GENERATED_EQUIVALENCES
        .iter()
        .filter(move |e| e.countries.contains(&country))
}

#[must_use]
pub fn find_equivalence(id: &str) -> Option<&'static Equivalence> {
    GENERATED_EQUIVALENCES.iter().find(|e| e.id == id)
}

#[must_use]
pub fn find_equivalence_for(country: EquivalenceCountry, id: &str) -> Option<&'static Equivalence> {
    equivalences_for(country).find(|e| e.id == id)
}

/// The CO2 a card prints: `co2_ug` rounded down to two significant
/// figures, so the printed figure never exceeds the measure.
#[must_use]
pub fn printed_co2_ug(co2_ug: u64) -> u64 {
    let mut scale = 1;
    while co2_ug / scale >= 100 {
        scale *= 10;
    }
    co2_ug / scale * scale
}

/// Floor division: the count is an "at least". Pass
/// `printed_co2_ug(co2)`, never the raw measure.
#[must_use]
pub fn equivalence_count(e: &Equivalence, printed_co2_ug: u64) -> u64 {
    printed_co2_ug / e.per_unit_ug
}

/// The largest auto row of `country` whose count is at least
/// `MIN_EQUIVALENCE_COUNT`, `None` below twice its smallest auto factor.
#[must_use]
pub fn pick_equivalence(
    country: EquivalenceCountry,
    printed_co2_ug: u64,
) -> Option<&'static Equivalence> {
    // Rows ascend, so the last match is the largest.
    equivalences_for(country)
        .filter(|e| e.auto && equivalence_count(e, printed_co2_ug) >= MIN_EQUIVALENCE_COUNT)
        .last()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::equivalence_data::{BASE_CARBONE_VINTAGE, DESNZ_VINTAGE, EPA_VINTAGE};

    // Relation-based: a Base Carbone refresh may move every factor, these
    // must still hold.
    #[test]
    fn the_table_is_sorted_and_scale_coherent() {
        let table = equivalences();
        assert_ne!(table, []);
        let mut ids: Vec<&str> = table.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), table.len(), "ids must be unique");
        for e in table {
            assert!(e.per_unit_ug > 0, "{}", e.id);
            assert!(e.per_unit_ug <= 9_007_199_254_740_991, "{}", e.id);
        }
        for pair in table.windows(2) {
            assert!(
                pair[0].per_unit_ug < pair[1].per_unit_ug,
                "{} then {}",
                pair[0].id,
                pair[1].id
            );
        }
    }

    #[test]
    fn each_country_has_a_readable_ladder_of_auto_rows() {
        for country in [
            EquivalenceCountry::Fr,
            EquivalenceCountry::Gb,
            EquivalenceCountry::Us,
        ] {
            let auto: Vec<_> = equivalences_for(country).filter(|e| e.auto).collect();
            assert!(!auto.is_empty(), "{country:?}");
            for pair in auto.windows(2) {
                assert!(pair[0].per_unit_ug < pair[1].per_unit_ug, "{country:?}");
                assert!(
                    pair[1].per_unit_ug <= 200 * pair[0].per_unit_ug,
                    "{} then {}",
                    pair[0].id,
                    pair[1].id
                );
            }
        }
    }

    #[test]
    fn the_pick_is_the_largest_auto_row_of_the_country() {
        for country in [
            EquivalenceCountry::Fr,
            EquivalenceCountry::Gb,
            EquivalenceCountry::Us,
        ] {
            for e in equivalences_for(country).filter(|e| e.auto) {
                assert_eq!(
                    pick_equivalence(country, 2 * e.per_unit_ug),
                    Some(e),
                    "{}",
                    e.id
                );
            }
        }
        let printed = printed_co2_ug(12_400_000_000);
        assert_eq!(
            pick_equivalence(EquivalenceCountry::Fr, printed).map(|e| e.id),
            Some("meal")
        );
        assert_eq!(
            pick_equivalence(EquivalenceCountry::Gb, printed).map(|e| e.id),
            Some("gb_car_km")
        );
        assert_eq!(
            pick_equivalence(EquivalenceCountry::Us, printed).map(|e| e.id),
            Some("us_car_mile")
        );
        assert_eq!(pick_equivalence(EquivalenceCountry::Fr, 500_000_000), None);
    }

    #[test]
    fn the_tgv_is_never_picked() {
        let tgv = find_equivalence("tgv_km").expect("tgv row");
        assert!(!tgv.auto);
        let mut co2 = 1u64;
        while co2 < 1_000_000_000_000_000 {
            assert_ne!(
                pick_equivalence(EquivalenceCountry::Fr, co2).map(|e| e.id),
                Some("tgv_km")
            );
            co2 *= 3;
        }
    }

    #[test]
    fn a_row_belongs_only_to_its_countries() {
        assert!(find_equivalence_for(EquivalenceCountry::Fr, "gb_car_km").is_none());
        assert!(find_equivalence_for(EquivalenceCountry::Us, "meal").is_none());
        for country in [
            EquivalenceCountry::Fr,
            EquivalenceCountry::Gb,
            EquivalenceCountry::Us,
        ] {
            assert!(find_equivalence_for(country, "smartphone").is_some());
        }
    }

    #[test]
    fn every_row_names_its_source_and_vintage() {
        for e in equivalences() {
            assert!(
                !e.source.is_empty() && !e.source_ref.is_empty() && !e.vintage.is_empty(),
                "{}",
                e.id
            );
            assert!(!e.countries.is_empty(), "{}", e.id);
        }
        assert_eq!(
            find_equivalence("meal").map(|e| e.vintage),
            Some(BASE_CARBONE_VINTAGE)
        );
        assert_eq!(
            find_equivalence("gb_car_km").map(|e| e.vintage),
            Some(DESNZ_VINTAGE)
        );
        assert_eq!(
            find_equivalence("us_car_mile").map(|e| e.vintage),
            Some(EPA_VINTAGE)
        );
    }

    #[test]
    fn the_country_reads_as_its_lowercase_code() {
        assert_eq!(
            serde_json::to_value(EquivalenceCountry::Gb).expect("serializes"),
            serde_json::json!("gb")
        );
        assert_eq!(EquivalenceCountry::default(), EquivalenceCountry::Fr);
        assert!(serde_json::from_value::<EquivalenceCountry>(serde_json::json!("uk")).is_err());
    }

    #[test]
    fn counts_round_down() {
        let first = &equivalences()[0];
        let f = first.per_unit_ug;
        for co2 in [2 * f, 2 * f + 1, 3 * f - 1] {
            let count = equivalence_count(first, co2);
            assert!(count * f <= co2 && co2 < (count + 1) * f, "{co2}");
        }
    }

    #[test]
    fn printed_co2_keeps_two_significant_figures_rounded_down() {
        for (raw, printed) in [
            (12_400_000_000, 12_000_000_000),
            (19_800_000_000, 19_000_000_000),
            (999_999, 990_000),
            (1_500_000_000_000, 1_500_000_000_000),
            (41, 41),
            (0, 0),
        ] {
            assert_eq!(printed_co2_ug(raw), printed, "{raw}");
        }
    }

    #[test]
    fn the_count_uses_the_printed_co2() {
        let meal = find_equivalence("meal").expect("meal row");
        assert_eq!(equivalence_count(meal, printed_co2_ug(12_400_000_000)), 5);
        assert_eq!(equivalence_count(meal, printed_co2_ug(15_800_000_000)), 7);
        assert_eq!(equivalence_count(meal, printed_co2_ug(19_800_000_000)), 9);
        let smartphone = find_equivalence("smartphone").expect("smartphone row");
        assert_eq!(
            equivalence_count(smartphone, printed_co2_ug(140_000_000_000)),
            3
        );
        for e in equivalences() {
            let mut co2 = 1u64;
            while co2 < 1_000_000_000_000_000 {
                for raw in [co2, co2 * 3 + 7, co2 * 7 - 1] {
                    let printed = printed_co2_ug(raw);
                    assert!(
                        equivalence_count(e, printed) * e.per_unit_ug <= printed,
                        "{} {raw}",
                        e.id
                    );
                }
                co2 *= 10;
            }
        }
    }

    #[test]
    fn find_returns_each_row_by_id() {
        for e in equivalences() {
            assert_eq!(find_equivalence(e.id), Some(e));
        }
        assert_eq!(find_equivalence("flight"), None);
    }

    /// `scripts/refresh-equivalences.py` rewrites the docs table with the
    /// generated rows, so a refresh keeps this green and a hand edit to
    /// the data alone breaks it.
    #[test]
    fn the_docs_carry_the_vintage_and_every_row() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        for relative in ["SHARE-CARD.md", "FR/SHARE-CARD-FR.md"] {
            let doc = std::fs::read_to_string(root.join(relative)).expect("read share card docs");
            for vintage in [BASE_CARBONE_VINTAGE, DESNZ_VINTAGE, EPA_VINTAGE] {
                assert!(doc.contains(vintage), "{relative}: {vintage}");
            }
            for e in equivalences() {
                assert!(doc.contains(&format!("`{}`", e.id)), "{relative}: {}", e.id);
                assert!(doc.contains(e.source_ref), "{relative}: {}", e.source_ref);
            }
        }
    }
}
