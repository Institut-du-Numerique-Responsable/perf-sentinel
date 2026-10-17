use std::collections::BTreeSet;

use serde_json::json;

use super::*;

const SCHEMA: &str = include_str!("../../schema.rs");

/// Where each struct of `schema.rs` sits in the serialised report. A struct
/// missing here fails `the_annex_lists_every_field_of_the_schema`, so a new
/// one cannot slip past the annex either.
const ROOTS: &[(&str, &[&str])] = &[
    ("PeriodicReport", &[""]),
    ("ReportMetadata", &["report_metadata"]),
    ("Organisation", &["organisation"]),
    ("OrgIdentifiers", &["organisation.identifiers"]),
    ("Period", &["period"]),
    ("ScopeManifest", &["scope_manifest"]),
    ("CoverageBasis", &["scope_manifest.coverage_basis"]),
    ("ExcludedApp", &["scope_manifest.applications_excluded[]"]),
    ("ExcludedEnv", &["scope_manifest.environments_excluded[]"]),
    ("Methodology", &["methodology"]),
    ("DisabledPattern", &["methodology.disabled_patterns[]"]),
    ("CalibrationInputs", &["methodology.calibration_inputs"]),
    ("StandardCrosswalk", &["methodology.standard_crosswalk"]),
    (
        "CrosswalkEntry",
        &["methodology.standard_crosswalk.mappings[]"],
    ),
    ("Aggregate", &["aggregate"]),
    ("CarbonBreakdown", &["aggregate.carbon_breakdown"]),
    (
        "WasteTier",
        &["aggregate.canonical_waste", "aggregate.operational_waste"],
    ),
    (
        "DatabaseWasteAggregate",
        &["aggregate.database_waste", "aggregate.messaging_waste"],
    ),
    ("TemporalCoverage", &["aggregate.temporal_coverage"]),
    ("ApplicationG1", &["applications[]"]),
    ("ApplicationG2", &["applications[]"]),
    ("AntiPatternDetail", &["applications[].anti_patterns[]"]),
    ("Integrity", &["integrity"]),
    ("SourceChain", &["integrity.trace_integrity_chain"]),
    ("SignatureMetadata", &["integrity.signature"]),
    (
        "BinaryAttestationMetadata",
        &["integrity.binary_attestation"],
    ),
    ("CrossPeriodLogRef", &["integrity.cross_period_log"]),
    ("Notes", &["notes"]),
];

/// `(struct, [(json name, type)])` for every `pub struct` of `schema.rs`.
fn schema_structs() -> Vec<(String, Vec<(String, String)>)> {
    parse_structs(SCHEMA.split("#[cfg(test)]").next().unwrap_or(SCHEMA))
}

/// The structs of `source` and their serialised field names. A struct-level
/// `rename_all` would change every name, so it is refused rather than
/// misread.
fn parse_structs(source: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut structs = Vec::new();
    let mut current: Option<(String, Vec<(String, String)>)> = None;
    let mut rename: Option<String> = None;
    let mut renames_all = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if current.is_none() {
            renames_all |= trimmed.starts_with("#[serde(") && trimmed.contains("rename_all");
            if trimmed.starts_with("pub enum ") {
                renames_all = false;
            }
            if let Some(rest) = trimmed.strip_prefix("pub struct ") {
                assert!(!renames_all, "a struct-level rename_all: {trimmed}");
                let name = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if rest.trim_end().ends_with('{') {
                    current = Some((name, Vec::new()));
                }
            }
            continue;
        }
        let Some((_, fields)) = current.as_mut() else {
            continue;
        };
        if line == "}" {
            structs.push(current.take().expect("open struct"));
            continue;
        }
        if let Some(start) = trimmed
            .find("rename = \"")
            .filter(|_| trimmed.starts_with("#[serde("))
        {
            rename = trimmed[start + 10..].split('"').next().map(str::to_string);
            continue;
        }
        if let Some((name, ty)) = trimmed
            .strip_prefix("pub(crate) ")
            .or_else(|| trimmed.strip_prefix("pub "))
            .and_then(|rest| rest.split_once(':'))
        {
            let name = name.trim_start_matches("r#").trim().to_string();
            fields.push((
                rename.take().unwrap_or(name),
                ty.trim().trim_end_matches(',').to_string(),
            ));
        }
    }
    structs
}

fn bare(path: &str) -> String {
    path.replace("[]", "")
}

#[test]
fn the_annex_lists_every_field_of_the_schema() {
    let structs = schema_structs();
    assert!(
        structs.len() > 20,
        "the schema parser found {} structs",
        structs.len()
    );
    let containers: BTreeSet<String> = ROOTS
        .iter()
        .flat_map(|(_, roots)| roots.iter().map(|root| bare(root)))
        .collect();
    let mut expected = BTreeSet::new();
    for (name, fields) in &structs {
        let roots = ROOTS
            .iter()
            .find(|(root, _)| root == name)
            .unwrap_or_else(|| panic!("{name} is in schema.rs but not in ROOTS"))
            .1;
        for root in roots {
            for (field, ty) in fields {
                let path = if root.is_empty() {
                    field.clone()
                } else {
                    format!("{root}.{field}")
                };
                if containers.contains(&bare(&path)) {
                    continue;
                }
                let leaf = if ty.starts_with("BTreeMap<") {
                    format!("{path}.*")
                } else {
                    path
                };
                expected.insert(bare(&leaf));
            }
        }
    }
    let listed: BTreeSet<String> = FIELDS.iter().map(|field| bare(field)).collect();
    let missing: Vec<&String> = expected.difference(&listed).collect();
    let extra: Vec<&String> = listed.difference(&expected).collect();
    assert!(
        missing.is_empty(),
        "fields of schema.rs absent from FIELDS: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "FIELDS names fields schema.rs no longer has: {extra:?}"
    );
}

fn find<'a>(sections: &'a [(String, Vec<Row>)], path: &str) -> &'a Row {
    sections
        .iter()
        .flat_map(|(_, rows)| rows)
        .find(|row| row.path == path)
        .unwrap_or_else(|| panic!("no row {path}"))
}

#[test]
fn values_print_and_absent_fields_say_so() {
    let report = json!({
        "schema_version": "perf-sentinel-report/v1.7",
        "organisation": {"name": "Example SAS", "identifiers": {"siren": "123456789"}, "sector": ""},
        "methodology": {"enabled_patterns": ["n_plus_one_sql", "slow_sql"], "calibration_inputs": {"scaphandre_used": false}},
        "aggregate": {"total_energy_kwh": 18.42, "per_service_measured_ratio": {"billing": 0.5}},
        "integrity": {"signature": null}
    });
    let sections = rows(&report);
    assert_eq!(
        find(&sections, "schema_version").value.as_deref(),
        Some("perf-sentinel-report/v1.7")
    );
    assert_eq!(
        find(&sections, "organisation.identifiers.siren")
            .value
            .as_deref(),
        Some("123456789")
    );
    assert_eq!(find(&sections, "organisation.identifiers.lei").value, None);
    assert_eq!(
        find(&sections, "organisation.sector").value,
        None,
        "an empty string is not a value"
    );
    assert_eq!(
        find(&sections, "methodology.enabled_patterns[1]")
            .value
            .as_deref(),
        Some("slow_sql")
    );
    assert_eq!(
        find(&sections, "methodology.core_patterns_required").value,
        None
    );
    assert_eq!(
        find(&sections, "methodology.calibration_inputs.scaphandre_used")
            .value
            .as_deref(),
        Some("false")
    );
    assert_eq!(
        find(&sections, "aggregate.total_energy_kwh")
            .value
            .as_deref(),
        Some("18.42")
    );
    assert_eq!(
        find(&sections, "aggregate.per_service_measured_ratio.billing")
            .value
            .as_deref(),
        Some("0.5")
    );
    assert_eq!(
        find(&sections, "aggregate.per_service_energy_models").value,
        None
    );
    assert_eq!(
        find(&sections, "integrity.signature.signer_identity").value,
        None
    );
    assert_eq!(find(&sections, "report_metadata.report_uuid").value, None);
}

#[test]
fn arrays_are_walked_item_by_item_and_an_empty_one_is_absent() {
    let report = json!({
        "applications": [
            {"service_name": "checkout", "anti_patterns": [{"type": "n_plus_one_sql", "occurrences": 12}]},
            {"service_name": "billing", "anti_patterns_detected_count": 5}
        ],
        "scope_manifest": {"environments_excluded": []}
    });
    let sections = rows(&report);
    assert_eq!(
        find(&sections, "applications[0].service_name")
            .value
            .as_deref(),
        Some("checkout")
    );
    assert_eq!(
        find(&sections, "applications[0].anti_patterns[0].occurrences")
            .value
            .as_deref(),
        Some("12")
    );
    assert_eq!(
        find(&sections, "applications[1].anti_patterns[].type").value,
        None
    );
    assert_eq!(
        find(&sections, "applications[1].anti_patterns_detected_count")
            .value
            .as_deref(),
        Some("5")
    );
    assert_eq!(
        find(&sections, "scope_manifest.environments_excluded[].name").value,
        None
    );
}

#[test]
fn sections_follow_the_first_segment_in_schema_order() {
    let names: Vec<String> = rows(&json!({})).into_iter().map(|(name, _)| name).collect();
    assert_eq!(
        names,
        [
            "schema_version",
            "report_metadata",
            "organisation",
            "period",
            "scope_manifest",
            "methodology",
            "aggregate",
            "applications",
            "integrity",
            "notes"
        ]
    );
}

#[test]
fn an_array_prints_item_by_item() {
    let report = json!({
        "applications": [
            {"service_name": "checkout", "display_name": "Checkout"},
            {"service_name": "billing", "display_name": "Billing"}
        ]
    });
    let sections = rows(&report);
    let paths: Vec<&str> = sections
        .iter()
        .find(|(name, _)| name == "applications")
        .expect("applications")
        .1
        .iter()
        .map(|row| row.path.as_str())
        .collect();
    let at = |path: &str| {
        paths
            .iter()
            .position(|p| *p == path)
            .unwrap_or_else(|| panic!("{path}"))
    };
    assert!(
        at("applications[0].display_name") < at("applications[1].service_name"),
        "{paths:?}"
    );
}

#[test]
fn a_whitespace_only_value_is_absent() {
    let sections = rows(&json!({"organisation": {"sector": "  "}}));
    assert_eq!(find(&sections, "organisation.sector").value, None);
}

#[test]
fn the_schema_parser_reads_crate_visible_fields() {
    let parsed =
        parse_structs("pub struct Probe {\n    pub(crate) hidden: u64,\n    pub shown: u64,\n}\n");
    assert_eq!(
        parsed[0]
            .1
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["hidden", "shown"]
    );
}

#[test]
#[should_panic(expected = "rename_all")]
fn the_schema_parser_refuses_a_struct_level_rename_all() {
    parse_structs(
        "#[serde(rename_all = \"camelCase\")]\npub struct Probe {\n    pub one: u64,\n}\n",
    );
}
