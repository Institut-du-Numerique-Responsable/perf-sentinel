//! Validates the SARIF output against the OASIS SARIF 2.1.0 JSON schema.
//!
//! The schema is vendored at `tests/fixtures/sarif-schema-2.1.0.json` (workspace
//! root), copied from
//! <https://docs.oasis-open.org/sarif/sarif/v2.1.0/errata01/os/schemas/sarif-schema-2.1.0.json>.
//! The validator below is a minimal subset of JSON Schema draft-04: it follows
//! `$ref` to `#/definitions/...` and checks `required`, `additionalProperties:
//! false`, `items`, `minItems` and `enum`. Other keywords are ignored.

use serde_json::{Map, Value};

use super::*;
use crate::acknowledgments::Acknowledgment;
use crate::detect::suggestions::SuggestedFix;
use crate::event::CodeLocation;
use crate::report::AcknowledgedFinding;

fn load_schema() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sarif-schema-2.1.0.json"
    );
    let raw = std::fs::read_to_string(path).expect("read vendored SARIF schema");
    serde_json::from_str(&raw).expect("parse vendored SARIF schema")
}

fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => {
            let name = reference
                .strip_prefix("#/definitions/")
                .unwrap_or_else(|| panic!("unsupported $ref {reference}"));
            resolve(root, &root["definitions"][name])
        }
        None => schema,
    }
}

fn validate(root: &Value, schema: &Value, value: &Value, path: &str, errors: &mut Vec<String>) {
    let schema = resolve(root, schema);
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        errors.push(format!("{path}: {value} is not one of {allowed:?}"));
    }
    if let Some(obj) = value.as_object() {
        check_object(root, schema, obj, path, errors);
    }
    if let Some(arr) = value.as_array() {
        check_array(root, schema, arr, path, errors);
    }
}

fn check_object(
    root: &Value,
    schema: &Value,
    obj: &Map<String, Value>,
    path: &str,
    errors: &mut Vec<String>,
) {
    let required = schema.get("required").and_then(Value::as_array);
    for key in required.into_iter().flatten().filter_map(Value::as_str) {
        if !obj.contains_key(key) {
            errors.push(format!("{path}: missing required property {key}"));
        }
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));
    for (key, child) in obj {
        match properties.and_then(|p| p.get(key)) {
            Some(sub) => validate(root, sub, child, &format!("{path}/{key}"), errors),
            None if closed => errors.push(format!("{path}: unexpected property {key}")),
            None => {}
        }
    }
}

fn check_array(root: &Value, schema: &Value, arr: &[Value], path: &str, errors: &mut Vec<String>) {
    if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
        && (arr.len() as u64) < min
    {
        errors.push(format!("{path}: {} items, minimum is {min}", arr.len()));
    }
    if let Some(items) = schema.get("items") {
        for (i, item) in arr.iter().enumerate() {
            validate(root, items, item, &format!("{path}/{i}"), errors);
        }
    }
}

fn schema_errors(sarif: &Value) -> Vec<String> {
    let schema = load_schema();
    let mut errors = Vec::new();
    validate(&schema, &schema, sarif, "", &mut errors);
    errors
}

fn finding_with_code_location() -> Finding {
    let mut f = crate::test_helpers::make_finding(FindingType::NPlusOneSql, Severity::Critical);
    f.signature =
        "n_plus_one_sql:order-svc:POST_/api/orders:abc12345abc12345abc12345abc12345".to_string();
    f.code_location = Some(CodeLocation {
        function: Some("processItems".to_string()),
        filepath: Some("src/Order.java".to_string()),
        lineno: Some(42),
        namespace: Some("com.example.Order".to_string()),
    });
    f.suggested_fix = Some(SuggestedFix {
        pattern: "n_plus_one_sql".to_string(),
        framework: "java_jpa".to_string(),
        recommendation: "Use `JOIN FETCH`".to_string(),
        reference_url: Some("https://example.com/fetch".to_string()),
    });
    f
}

fn acknowledged_finding() -> AcknowledgedFinding {
    let mut finding = crate::test_helpers::make_finding(FindingType::SlowHttp, Severity::Info);
    finding.signature =
        "slow_http:order-svc:GET_/api/items:0123456789abcdef0123456789abcdef".to_string();
    finding.code_location = Some(CodeLocation {
        function: None,
        filepath: Some("src/items.rs".to_string()),
        lineno: None,
        namespace: None,
    });
    AcknowledgedFinding {
        acknowledgment: Acknowledgment {
            signature: finding.signature.clone(),
            acknowledged_by: "alice@example.com".to_string(),
            acknowledged_at: "2026-05-03".to_string(),
            reason: "intentional cache invalidation".to_string(),
            expires_at: Some("2026-12-31".to_string()),
            service: Some("order-svc".to_string()),
            source_endpoint: Some("GET /api/items".to_string()),
        },
        finding,
    }
}

fn full_report() -> Report {
    // No code location, no suggested fix, empty signature.
    let bare = crate::test_helpers::make_finding(FindingType::RedundantSql, Severity::Warning);
    let mut report = crate::test_helpers::empty_report();
    report.findings = vec![finding_with_code_location(), bare];
    report.acknowledged_findings = vec![acknowledged_finding()];
    report
}

#[test]
fn sarif_with_every_optional_field_matches_schema() {
    let sarif = serde_json::to_value(report_to_sarif(&full_report())).unwrap();
    assert_eq!(sarif["runs"][0]["results"].as_array().unwrap().len(), 3);
    let errors = schema_errors(&sarif);
    assert!(errors.is_empty(), "schema errors: {errors:#?}");
}

#[test]
fn findings_to_sarif_matches_schema() {
    let findings = vec![finding_with_code_location(), acknowledged_finding().finding];
    let sarif = serde_json::to_value(findings_to_sarif(&findings)).unwrap();
    let errors = schema_errors(&sarif);
    assert!(errors.is_empty(), "schema errors: {errors:#?}");
}

#[test]
fn validator_rejects_the_old_result_shape() {
    let mut sarif = serde_json::to_value(report_to_sarif(&full_report())).unwrap();
    let result = &mut sarif["runs"][0]["results"][0];
    result["logicalLocations"] = serde_json::json!([{ "name": "order-svc", "kind": "module" }]);
    result["fixes"] = serde_json::json!([{ "description": { "text": "Use JOIN FETCH" } }]);
    let errors = schema_errors(&sarif);
    assert!(
        errors
            .iter()
            .any(|e| e.ends_with("unexpected property logicalLocations")),
        "result-level logicalLocations not caught: {errors:#?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.ends_with("missing required property artifactChanges")),
        "fix without artifactChanges not caught: {errors:#?}"
    );
}
