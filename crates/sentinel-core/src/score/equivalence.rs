//! Everyday CO2 equivalences for share cards, from the ADEME Base
//! Carbone. Rows and vintage are generated into `equivalence_data.rs` by
//! `scripts/refresh-equivalences.py`, selection rules in
//! `docs/SHARE-CARD.md`.

use super::equivalence_data::{EQUIVALENCE_TABLE_VINTAGE, GENERATED_EQUIVALENCES};

/// One Base Carbone factor, in µg of CO2 equivalent per `unit`.
#[derive(Debug, PartialEq, Eq)]
pub struct Equivalence {
    pub id: &'static str,
    pub base_carbone_id: u32,
    pub per_unit_ug: u64,
    pub unit: &'static str,
    pub label: &'static str,
}

pub const EQUIVALENCE_SOURCE: &str = "ADEME, Base Carbone";
/// Below two, "1 meal" reads as a rounding artefact rather than a figure.
pub const MIN_EQUIVALENCE_COUNT: u64 = 2;
/// Upper bound on the count of an equivalence the caller chooses by id.
pub const MAX_CHOSEN_EQUIVALENCE_COUNT: u64 = 1000;

#[must_use]
pub fn embedded_equivalence_vintage() -> &'static str {
    EQUIVALENCE_TABLE_VINTAGE
}

/// Every row, by ascending `per_unit_ug`.
#[must_use]
pub fn equivalences() -> &'static [Equivalence] {
    GENERATED_EQUIVALENCES
}

#[must_use]
pub fn find_equivalence(id: &str) -> Option<&'static Equivalence> {
    GENERATED_EQUIVALENCES.iter().find(|e| e.id == id)
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

/// The largest factor whose count is at least `MIN_EQUIVALENCE_COUNT`,
/// `None` below twice the smallest factor.
#[must_use]
pub fn pick_equivalence(printed_co2_ug: u64) -> Option<&'static Equivalence> {
    GENERATED_EQUIVALENCES
        .iter()
        .rev()
        .find(|e| equivalence_count(e, printed_co2_ug) >= MIN_EQUIVALENCE_COUNT)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(
                pair[1].per_unit_ug <= 100 * pair[0].per_unit_ug,
                "{} then {}",
                pair[0].id,
                pair[1].id
            );
        }
    }

    #[test]
    fn the_pick_is_the_largest_factor_counting_at_least_two() {
        let table = equivalences();
        for e in table {
            assert_eq!(pick_equivalence(2 * e.per_unit_ug), Some(e), "{}", e.id);
        }
        assert_eq!(pick_equivalence(2 * table[0].per_unit_ug - 1), None);
        assert_eq!(pick_equivalence(u64::MAX / 2), table.last());
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

    #[test]
    fn the_vintage_accessor_is_the_generated_constant() {
        let vintage = embedded_equivalence_vintage();
        assert_eq!(vintage, EQUIVALENCE_TABLE_VINTAGE);
        assert!(vintage.contains('V'), "{vintage}");
        let has_date = vintage.as_bytes().windows(10).any(|w| {
            w.iter().enumerate().all(|(i, b)| match i {
                4 | 7 => *b == b'-',
                _ => b.is_ascii_digit(),
            })
        });
        assert!(has_date, "{vintage}");
    }

    /// `scripts/refresh-equivalences.py` rewrites the docs table with the
    /// generated rows, so a refresh keeps this green and a hand edit to
    /// the data alone breaks it.
    #[test]
    fn the_docs_carry_the_vintage_and_every_row() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        for relative in ["SHARE-CARD.md", "FR/SHARE-CARD-FR.md"] {
            let doc = std::fs::read_to_string(root.join(relative)).expect("read share card docs");
            assert!(
                doc.contains(embedded_equivalence_vintage()),
                "{relative}: vintage"
            );
            for e in equivalences() {
                assert!(doc.contains(&format!("`{}`", e.id)), "{relative}: {}", e.id);
                assert!(
                    doc.contains(&e.base_carbone_id.to_string()),
                    "{relative}: {}",
                    e.base_carbone_id
                );
            }
        }
    }
}
