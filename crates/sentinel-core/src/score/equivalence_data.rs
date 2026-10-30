// GENERATED FILE - DO NOT EDIT BY HAND.
// Regenerate with: python3 scripts/refresh-equivalences.py
//
// Everyday equivalences for share cards, in µg CO2e per unit. The rows
// and their order are chosen by hand in the script, the factors come
// from the source.
//
// Source: ADEME, Base Carbone (Licence Ouverte 2.0),
// https://data.ademe.fr/datasets/base-carboner - selection rules in
// docs/SHARE-CARD.md.

use super::equivalence::Equivalence;

/// Grep-audited by release procedure step 2.5, like `PUE_VINTAGE`. The
/// date is the Licence Ouverte "date de dernière mise à jour".
pub(crate) const EQUIVALENCE_TABLE_VINTAGE: &str = "V23.6 (2025-07-03)";

pub(super) static GENERATED_EQUIVALENCES: &[Equivalence] = &[
    Equivalence {
        id: "tgv_km",
        base_carbone_id: 43256,
        per_unit_ug: 2_930_000,
        unit: "passenger_km",
        label: "km by high-speed train",
    }, // TGV, 2022
    Equivalence {
        id: "car_km",
        base_carbone_id: 43791,
        per_unit_ug: 256_000_000,
        unit: "vehicle_km",
        label: "km driven by an average car",
    }, // Voiture, Motorisation moyenne
    Equivalence {
        id: "meal",
        base_carbone_id: 20682,
        per_unit_ug: 2_040_000_000,
        unit: "meal",
        label: "average meals",
    }, // Repas
    Equivalence {
        id: "smartphone",
        base_carbone_id: 27013,
        per_unit_ug: 39_100_000_000,
        unit: "unit",
        label: "smartphones",
    }, // Smartphone, de plus de 5,5 pouces
];
