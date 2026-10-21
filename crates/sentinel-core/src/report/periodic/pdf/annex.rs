//! The annex: every field the schema defines, read from the report as
//! serialised, so a reader sees what the report carries and what it leaves
//! out. A field the report does not carry prints as not reported rather than
//! disappearing.

use serde_json::Value;

/// What a field the report does not carry, or carries empty, prints as.
pub(super) const ABSENT: &str = "not reported";

/// Every leaf of `PeriodicReport`, in the order the report serialises it.
/// `[]` walks an array, `*` the keys of a map. `tests` derives the same list
/// from `schema.rs`, so a field added there and not here fails the build.
pub(super) const FIELDS: &[&str] = &[
    "schema_version",
    "report_metadata.intent",
    "report_metadata.confidentiality_level",
    "report_metadata.integrity_level",
    "report_metadata.generated_at",
    "report_metadata.generated_by",
    "report_metadata.perf_sentinel_version",
    "report_metadata.report_uuid",
    "report_metadata.binary_version",
    "organisation.name",
    "organisation.country",
    "organisation.identifiers.siren",
    "organisation.identifiers.vat",
    "organisation.identifiers.lei",
    "organisation.identifiers.opencorporates_url",
    "organisation.identifiers.domain",
    "organisation.sector",
    "period.from_date",
    "period.to_date",
    "period.period_type",
    "period.days_covered",
    "scope_manifest.total_applications_declared",
    "scope_manifest.applications_measured",
    "scope_manifest.applications_excluded[].service_name",
    "scope_manifest.applications_excluded[].reason",
    "scope_manifest.environments_measured",
    "scope_manifest.environments_excluded[].name",
    "scope_manifest.environments_excluded[].reason",
    "scope_manifest.total_requests_in_period",
    "scope_manifest.requests_measured",
    "scope_manifest.coverage_percentage",
    "scope_manifest.coverage_basis.operator_declared",
    "scope_manifest.coverage_basis.machine_derived",
    "methodology.sci_specification",
    "methodology.perf_sentinel_version",
    "methodology.enabled_patterns",
    "methodology.disabled_patterns[].name",
    "methodology.disabled_patterns[].reason",
    "methodology.core_patterns_required",
    "methodology.conformance",
    "methodology.calibration_inputs.cloud_regions",
    "methodology.calibration_inputs.carbon_intensity_source",
    "methodology.calibration_inputs.specpower_table_version",
    "methodology.calibration_inputs.binary_specpower_vintage",
    "methodology.calibration_inputs.scaphandre_used",
    "methodology.calibration_inputs.energy_source_models",
    "methodology.calibration_inputs.calibration_applied",
    "methodology.calibration_inputs.scoring_coefficients",
    "methodology.calibration_inputs.carbon_methodologies",
    "methodology.calibration_inputs.embodied_gco2_total",
    "methodology.calibration_inputs.embodied_gco2_per_request",
    "methodology.standard_crosswalk.standard",
    "methodology.standard_crosswalk.mappings[].report_field",
    "methodology.standard_crosswalk.mappings[].datapoint",
    "methodology.standard_crosswalk.mappings[].note",
    "methodology.standard_crosswalk.caveats",
    "aggregate.total_requests",
    "aggregate.total_energy_kwh",
    "aggregate.total_carbon_kgco2eq",
    "aggregate.carbon_breakdown.operational_kgco2eq",
    "aggregate.carbon_breakdown.embodied_kgco2eq",
    "aggregate.carbon_breakdown.database_kgco2eq",
    "aggregate.carbon_breakdown.messaging_kgco2eq",
    "aggregate.carbon_breakdown.database_kgco2eq_out_of_total",
    "aggregate.carbon_breakdown.messaging_kgco2eq_out_of_total",
    "aggregate.carbon_breakdown.transport_kgco2eq",
    "aggregate.carbon_breakdown.transport_kgco2eq_low",
    "aggregate.carbon_breakdown.transport_kgco2eq_high",
    "aggregate.aggregate_efficiency_score",
    "aggregate.aggregate_waste_ratio",
    "aggregate.anti_patterns_detected_count",
    "aggregate.estimated_optimization_potential_kgco2eq",
    "aggregate.canonical_waste.n_plus_one_threshold",
    "aggregate.canonical_waste.energy_kwh",
    "aggregate.canonical_waste.carbon_kgco2eq",
    "aggregate.canonical_waste.waste_ratio",
    "aggregate.canonical_waste.efficiency_score",
    "aggregate.operational_waste.n_plus_one_threshold",
    "aggregate.operational_waste.energy_kwh",
    "aggregate.operational_waste.carbon_kgco2eq",
    "aggregate.operational_waste.waste_ratio",
    "aggregate.operational_waste.efficiency_score",
    "aggregate.period_coverage",
    "aggregate.binary_versions",
    "aggregate.runtime_windows_count",
    "aggregate.fallback_windows_count",
    "aggregate.database_waste.energy_kwh",
    "aggregate.database_waste.measured_energy_kwh",
    "aggregate.database_waste.declared_energy_kwh",
    "aggregate.database_waste.models",
    "aggregate.database_waste.windows_with_figure",
    "aggregate.database_waste.measured_windows",
    "aggregate.database_waste.declared_windows",
    "aggregate.database_waste.estimated_windows",
    "aggregate.database_waste.windows_with_carbon",
    "aggregate.database_waste.operational_waste_kwh",
    "aggregate.database_waste.operational_waste_kgco2eq",
    "aggregate.database_waste.canonical_waste_kwh",
    "aggregate.database_waste.canonical_waste_kgco2eq",
    "aggregate.messaging_waste.energy_kwh",
    "aggregate.messaging_waste.measured_energy_kwh",
    "aggregate.messaging_waste.declared_energy_kwh",
    "aggregate.messaging_waste.models",
    "aggregate.messaging_waste.windows_with_figure",
    "aggregate.messaging_waste.measured_windows",
    "aggregate.messaging_waste.declared_windows",
    "aggregate.messaging_waste.estimated_windows",
    "aggregate.messaging_waste.windows_with_carbon",
    "aggregate.messaging_waste.operational_waste_kwh",
    "aggregate.messaging_waste.operational_waste_kgco2eq",
    "aggregate.messaging_waste.canonical_waste_kwh",
    "aggregate.messaging_waste.canonical_waste_kgco2eq",
    "aggregate.per_service_energy_models.*",
    "aggregate.per_service_measured_ratio.*",
    "aggregate.temporal_coverage.temporal_coverage",
    "aggregate.temporal_coverage.observed_days",
    "aggregate.temporal_coverage.days_in_period",
    "aggregate.temporal_coverage.largest_gap_days",
    "applications[].service_name",
    "applications[].display_name",
    "applications[].service_version",
    "applications[].endpoints_observed",
    "applications[].total_requests",
    "applications[].energy_kwh",
    "applications[].carbon_kgco2eq",
    "applications[].efficiency_score",
    "applications[].anti_patterns[].type",
    "applications[].anti_patterns[].occurrences",
    "applications[].anti_patterns[].estimated_waste_kwh",
    "applications[].anti_patterns[].estimated_waste_kgco2eq",
    "applications[].anti_patterns[].first_seen",
    "applications[].anti_patterns[].last_seen",
    "applications[].anti_patterns[].rgesn_criteria",
    "applications[].anti_patterns_detected_count",
    "integrity.content_hash",
    "integrity.binary_hash",
    "integrity.binary_verification_url",
    "integrity.trace_integrity_chain.windows_verified",
    "integrity.trace_integrity_chain.windows_unchained",
    "integrity.trace_integrity_chain.breaks",
    "integrity.trace_integrity_chain.breaks_outside_period",
    "integrity.trace_integrity_chain.windows_dropped",
    "integrity.trace_integrity_chain.drop_counter_resets",
    "integrity.signature.format",
    "integrity.signature.bundle_url",
    "integrity.signature.signer_identity",
    "integrity.signature.signer_issuer",
    "integrity.signature.rekor_url",
    "integrity.signature.rekor_log_index",
    "integrity.signature.signed_at",
    "integrity.binary_attestation.format",
    "integrity.binary_attestation.attestation_url",
    "integrity.binary_attestation.builder_id",
    "integrity.binary_attestation.git_tag",
    "integrity.binary_attestation.git_commit",
    "integrity.binary_attestation.slsa_level",
    "integrity.cross_period_log.format",
    "integrity.cross_period_log.log_url",
    "integrity.cross_period_log.previous_report_hash",
    "notes.disclaimers",
    "notes.reference_urls.*",
];

/// One printed line: the field's path, with array indices filled in, and
/// its value, or [`ABSENT`].
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Row {
    pub path: String,
    pub value: Option<String>,
}

/// The rows of the annex, grouped by the first segment of their path.
pub(super) fn rows(report: &Value) -> Vec<(String, Vec<Row>)> {
    runs(FIELDS)
        .into_iter()
        .map(|(head, patterns)| {
            let section = head.trim_end_matches("[]").to_string();
            let mut found = Vec::new();
            walk(report, &patterns, "", &mut found);
            (section, found)
        })
        .collect()
}

/// Consecutive patterns grouped by their first segment, each paired with
/// the whole pattern, so an array is entered once and printed item by item.
fn runs<'a>(patterns: &[&'a str]) -> Vec<(&'a str, Vec<&'a str>)> {
    let mut runs: Vec<(&str, Vec<&str>)> = Vec::new();
    for pattern in patterns {
        let head = pattern.split('.').next().unwrap_or(pattern);
        match runs.last_mut() {
            Some((last, members)) if *last == head => members.push(pattern),
            _ => runs.push((head, vec![pattern])),
        }
    }
    runs
}

/// Prints every pattern from `node`, a run of patterns at a time.
fn walk(node: &Value, patterns: &[&str], prefix: &str, rows: &mut Vec<Row>) {
    for (head, members) in runs(patterns) {
        if head == "*" {
            map_rows(node, prefix, rows);
        } else {
            follow(node, head, &members, prefix, rows);
        }
    }
}

/// One run: the patterns that start with `head`, followed into `node`.
fn follow(node: &Value, head: &str, members: &[&str], prefix: &str, rows: &mut Vec<Row>) {
    let rests: Vec<&str> = members
        .iter()
        .filter_map(|pattern| pattern.split_once('.').map(|(_, rest)| rest))
        .collect();
    let (name, is_array) = head
        .strip_suffix("[]")
        .map_or((head, false), |name| (name, true));
    let here = join(prefix, name);
    let Some(child) = node.get(name).filter(|value| !value.is_null()) else {
        absent_all(prefix, members, rows);
        return;
    };
    if !is_array {
        descend(child, &rests, here, rows);
        return;
    }
    match child.as_array().filter(|items| !items.is_empty()) {
        Some(items) => {
            for (index, item) in items.iter().enumerate() {
                descend(item, &rests, format!("{here}[{index}]"), rows);
            }
        }
        None => absent_all(prefix, members, rows),
    }
}

/// A leaf when nothing is left to follow, the remaining patterns otherwise.
fn descend(node: &Value, rests: &[&str], path: String, rows: &mut Vec<Row>) {
    if rests.is_empty() {
        leaf(node, path, rows);
    } else {
        walk(node, rests, &path, rows);
    }
}

/// The entries of a map, or one absent row for the map itself.
fn map_rows(node: &Value, prefix: &str, rows: &mut Vec<Row>) {
    match node.as_object().filter(|map| !map.is_empty()) {
        Some(map) => {
            for (key, value) in map {
                leaf(value, join(prefix, key), rows);
            }
        }
        None => rows.push(Row {
            path: prefix.to_string(),
            value: None,
        }),
    }
}

/// One absent row per pattern the walk could not follow.
fn absent_all(prefix: &str, patterns: &[&str], rows: &mut Vec<Row>) {
    for pattern in patterns {
        rows.push(Row {
            path: join(prefix, pattern.trim_end_matches(".*")),
            value: None,
        });
    }
}

/// A leaf: one row, or one row per entry when it is a list.
fn leaf(value: &Value, path: String, rows: &mut Vec<Row>) {
    match value.as_array() {
        Some(items) if !items.is_empty() => {
            for (index, item) in items.iter().enumerate() {
                leaf(item, format!("{path}[{index}]"), rows);
            }
        }
        _ => rows.push(Row {
            path,
            value: scalar(value),
        }),
    }
}

/// A value as printed: strings bare, empty, blank or null as absent.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => (!text.trim().is_empty()).then(|| text.clone()),
        Value::Array(items) if items.is_empty() => None,
        Value::Object(map) if map.is_empty() => None,
        other => Some(other.to_string()),
    }
}

fn join(prefix: &str, segment: &str) -> String {
    if prefix.is_empty() {
        segment.to_string()
    } else {
        format!("{prefix}.{segment}")
    }
}

#[cfg(test)]
mod tests;
