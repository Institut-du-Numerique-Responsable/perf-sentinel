// GENERATED FILE - DO NOT EDIT BY HAND.
// Regenerate with: python3 scripts/refresh-equivalences.py
//
// Everyday equivalences for share cards, in µg CO2e per unit. The rows
// and their order are chosen by hand in the script, the factors come
// from the sources.
//
// Sources, selection rules in docs/SHARE-CARD.md:
// - ADEME, Base Carbone (Licence Ouverte 2.0),
//   https://data.ademe.fr/datasets/base-carboner
// - DESNZ, GHG conversion factors for company reporting (Open
//   Government Licence v3.0),
//   https://www.gov.uk/government/collections/government-conversion-factors-for-company-reporting
// - US EPA, GHG Equivalencies Calculator (US federal public domain),
//   https://www.epa.gov/energy/greenhouse-gases-equivalencies-calculator-calculations-and-references

use super::equivalence::Equivalence;
use super::equivalence::EquivalenceCountry::{Fr, Gb, Us};

/// Grep-audited by release procedure step 2.5, like `PUE_VINTAGE`. The
/// date is the Licence Ouverte "date de dernière mise à jour".
pub(crate) const BASE_CARBONE_VINTAGE: &str = "V23.6 (2025-07-03)";
/// The DESNZ publication year and version.
pub(crate) const DESNZ_VINTAGE: &str = "2026, v1.2";
/// The EPA calculator revision, year and month.
pub(crate) const EPA_VINTAGE: &str = "2024-10";

pub(super) static GENERATED_EQUIVALENCES: &[Equivalence] = &[
    Equivalence {
        id: "tgv_km",
        countries: &[Fr],
        auto: false,
        source: "ADEME, Base Carbone",
        source_ref: "43256",
        vintage: BASE_CARBONE_VINTAGE,
        per_unit_ug: 2_930_000,
        unit: "passenger_km",
        label: "km by high-speed train in France",
    }, // TGV, 2022
    Equivalence {
        id: "gb_rail_km",
        countries: &[Gb],
        auto: true,
        source: "DESNZ, GHG conversion factors",
        source_ref: "25_315_3147_11_1 + 26_911_3147_11_1",
        vintage: DESNZ_VINTAGE,
        per_unit_ug: 39_890_000,
        unit: "passenger_km",
        label: "km by train in the UK",
    }, // National rail, tailpipe + WTT
    Equivalence {
        id: "gb_car_km",
        countries: &[Gb],
        auto: true,
        source: "DESNZ, GHG conversion factors",
        source_ref: "4_301_3074_4_1 + 26_904_3074_4_1",
        vintage: DESNZ_VINTAGE,
        per_unit_ug: 209_900_000,
        unit: "vehicle_km",
        label: "km driven by an average car in the UK",
    }, // Average car, Unknown, tailpipe + WTT
    Equivalence {
        id: "car_km",
        countries: &[Fr],
        auto: true,
        source: "ADEME, Base Carbone",
        source_ref: "43791",
        vintage: BASE_CARBONE_VINTAGE,
        per_unit_ug: 256_000_000,
        unit: "vehicle_km",
        label: "km driven by an average car in France",
    }, // Voiture, Motorisation moyenne
    Equivalence {
        id: "us_car_mile",
        countries: &[Us],
        auto: true,
        source: "US EPA, GHG Equivalencies Calculator",
        source_ref: "Miles driven by the average gasoline-powered passenger vehicle",
        vintage: EPA_VINTAGE,
        per_unit_ug: 393_000_000,
        unit: "vehicle_mile",
        label: "miles driven by an average US car, tailpipe only",
    }, // EPA calculator
    Equivalence {
        id: "meal",
        countries: &[Fr],
        auto: true,
        source: "ADEME, Base Carbone",
        source_ref: "20682",
        vintage: BASE_CARBONE_VINTAGE,
        per_unit_ug: 2_040_000_000,
        unit: "meal",
        label: "average meals in France",
    }, // Repas
    Equivalence {
        id: "smartphone",
        countries: &[Fr, Gb, Us],
        auto: true,
        source: "ADEME, Base Carbone",
        source_ref: "27013",
        vintage: BASE_CARBONE_VINTAGE,
        per_unit_ug: 39_100_000_000,
        unit: "unit",
        label: "smartphones",
    }, // Smartphone, de plus de 5,5 pouces
];
