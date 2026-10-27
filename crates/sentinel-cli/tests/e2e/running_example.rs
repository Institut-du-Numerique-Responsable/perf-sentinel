//! The running example of the engine synoptic (<https://perf-sentinel.dev/synoptique/>):
//! every report figure the synoptic shows for each file, pinned against the binary.
//! Synthetic teaching data like `demo.json`, never an ingest regression fixture.
//! A deliberate change to one of these figures, a renormalized eu-west-3 hourly
//! profile included, updates the tables here, then the synoptic is republished.

use std::process::Command;

use serde_json::{Value, json};

use crate::helpers::{fixture_path, write_ack_file};

const ABC: &str = "abc1235f0e7d41a9c2b8e60d3f71c4b2";
const SEVEN: &str = "7f2e90c3b1a4d58e6f2079b1c4d3e2f5";
const BOTH: &[&str] = &["n_plus_one_sql_critical_max", "io_waste_ratio_max"];
const ROUTES: [&str; 3] = ["/checkout", "/customers/{id}", "/orders/{id}"];
const GATE: [(&str, f64); 4] = [
    ("n_plus_one_sql_critical_max", 0.0),
    ("n_plus_one_http_warning_max", 3.0),
    ("n_plus_one_messaging_warning_max", 3.0),
    ("io_waste_ratio_max", 0.3),
];
/// The Report panel shows these, in report order, for `running_example.json`.
const SIGNATURES: [&str; 4] = [
    "n_plus_one_sql:order-svc:_orders_{id}:309cae97e89846acd8fea7999a01d16a",
    "redundant_sql:order-svc:_customers_{id}:001296e11583d1f19e9a31279edfaaf2",
    "serialized_calls:order-svc:_customers_{id}:6459abf68dbe8c9da0159f81a91cc21e",
    "serialized_calls:order-svc:_orders_{id}:8113ed0b617b313ce515607f3a08ed64",
];

/// `type`, `severity`, `trace_id`, `green_impact.estimated_extra_io_ops`.
type Finding = (&'static str, &'static str, &'static str, u64);
const N1: Finding = ("n_plus_one_sql", "critical", ABC, 11);
const RED: Finding = ("redundant_sql", "info", SEVEN, 2);
const SER_CUSTOMERS: Finding = ("serialized_calls", "info", SEVEN, 0);
const SER_ORDERS: Finding = ("serialized_calls", "info", ABC, 0);

struct Case {
    file: &'static str,
    spans: u64,
    io: u64,
    sql: u64,
    avoidable: u64,
    waste: f64,
    kwh: f64,
    operational_g: f64,
    total_g: f64,
    top: (f64, &'static str),
    /// I/O per route, in `ROUTES` order.
    routes: [u64; 3],
    findings: &'static [Finding],
    failed: &'static [&'static str],
    exit: i32,
}

const CASES: [Case; 4] = [
    Case {
        file: "running_example.json",
        spans: 28,
        io: 20,
        sql: 18,
        avoidable: 13,
        waste: 0.65,
        kwh: 1.2e-6,
        operational_g: 8.376_6e-5,
        total_g: 3.083_766e-3,
        top: (14.0, "/orders/{id}"),
        routes: [2, 4, 14],
        findings: &[N1, RED, SER_CUSTOMERS, SER_ORDERS],
        failed: BOTH,
        exit: 1,
    },
    Case {
        file: "running_example_n1_fixed.json",
        spans: 17,
        io: 9,
        sql: 7,
        avoidable: 2,
        waste: 2.0 / 9.0,
        kwh: 6.5e-7,
        operational_g: 4.537_325e-5,
        total_g: 3.045_373_25e-3,
        top: (4.0, "/customers/{id}"),
        routes: [2, 4, 3],
        findings: &[RED, SER_CUSTOMERS, SER_ORDERS],
        failed: &[],
        exit: 0,
    },
    Case {
        file: "running_example_redundant_fixed.json",
        spans: 26,
        io: 18,
        sql: 16,
        avoidable: 11,
        waste: 11.0 / 18.0,
        kwh: 1.1e-6,
        operational_g: 7.678_55e-5,
        total_g: 3.076_785_5e-3,
        top: (14.0, "/orders/{id}"),
        routes: [2, 2, 14],
        findings: &[N1, SER_ORDERS],
        failed: BOTH,
        exit: 1,
    },
    Case {
        file: "running_example_fixed.json",
        spans: 15,
        io: 7,
        sql: 5,
        avoidable: 0,
        waste: 0.0,
        kwh: 5.5e-7,
        operational_g: 3.839_275e-5,
        total_g: 3.038_392_75e-3,
        top: (3.0, "/orders/{id}"),
        routes: [2, 2, 3],
        findings: &[SER_ORDERS],
        failed: &[],
        exit: 0,
    },
];

fn assert_close(got: &Value, want: f64, what: &str) {
    assert!(
        got.as_f64()
            .is_some_and(|v| (v - want).abs() <= 1e-9 * want.abs().max(1e-9)),
        "{what}: {got} != {want}"
    );
}

/// The synoptic's command, typed from the workspace root: `--ci` prints JSON.
fn run(file: &str, extra: &[&str]) -> (Option<i32>, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .current_dir(fixture_path("../.."))
        .args(["analyze", "--ci"])
        .args(extra)
        .args(["--input", &format!("tests/fixtures/{file}")])
        .env("RUST_LOG", "error")
        .output()
        .expect("spawn perf-sentinel");
    let report = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{file}: --ci stdout is not JSON: {e}\nstderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code(), report)
}

fn failed_rules(r: &Value) -> Vec<&str> {
    r["quality_gate"]["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|rule| rule["passed"].as_bool() == Some(false))
        .filter_map(|rule| rule["rule"].as_str())
        .collect()
}

#[test]
fn running_example_variants_match_the_synoptic() {
    for c in &CASES {
        let (code, r) = run(c.file, &["--no-acknowledgments"]);
        let (a, g) = (&r["analysis"], &r["green_summary"]);
        assert_eq!(a["traces_analyzed"], 3, "{}", c.file);
        assert_eq!(a["ingest"]["spans_received"], c.spans, "{}", c.file);
        assert_eq!(a["ingest"]["spans_filtered"], 8, "{}", c.file);
        assert_eq!(a["events_processed"], c.io, "{}", c.file);
        assert_eq!(g["total_io_ops"], c.io, "{}", c.file);
        assert_eq!(g["total_sql_io_ops"], c.sql, "{}", c.file);
        assert_eq!(g["total_messaging_io_ops"], 0, "{}", c.file);
        assert_eq!(g["avoidable_io_ops"], c.avoidable, "{}", c.file);
        assert_close(&g["io_waste_ratio"], c.waste, c.file);
        assert_close(&g["energy_kwh"], c.kwh, c.file);
        assert_close(&g["co2"]["operational_gco2"], c.operational_g, c.file);
        assert_close(&g["co2"]["total"]["mid"], c.total_g, c.file);
        assert_close(
            &g["top_offenders"][0]["io_intensity_score"],
            c.top.0,
            c.file,
        );
        assert_eq!(g["top_offenders"][0]["endpoint"], c.top.1, "{}", c.file);
        let routes: Vec<Value> = r["per_endpoint_io_ops"]
            .as_array()
            .expect("per_endpoint_io_ops array")
            .iter()
            .map(|e| json!([e["endpoint"], e["io_ops"]]))
            .collect();
        let want: Vec<_> = ROUTES.into_iter().zip(c.routes).collect();
        assert_eq!(json!(routes), json!(want), "{}", c.file);
        let findings: Vec<Value> = r["findings"]
            .as_array()
            .expect("findings array")
            .iter()
            .map(|f| {
                json!([
                    f["type"],
                    f["severity"],
                    f["trace_id"],
                    f["green_impact"]["estimated_extra_io_ops"]
                ])
            })
            .collect();
        assert_eq!(json!(findings), json!(c.findings), "{}", c.file);
        let rules = r["quality_gate"]["rules"].as_array().expect("gate rules");
        assert_eq!(rules.len(), GATE.len(), "{}", c.file);
        for (rule, (name, threshold)) in rules.iter().zip(GATE) {
            assert_eq!(rule["rule"], name, "{}", c.file);
            assert_close(&rule["threshold"], threshold, c.file);
        }
        assert_eq!(failed_rules(&r), c.failed, "{}", c.file);
        assert_eq!(code, Some(c.exit), "{}", c.file);
    }
}

#[test]
fn running_example_signatures_and_exit_without_ci() {
    let (_, r) = run("running_example.json", &["--no-acknowledgments"]);
    let signatures: Vec<&Value> = r["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .map(|f| &f["signature"])
        .collect();
    assert_eq!(json!(signatures), json!(SIGNATURES));
    // Without --ci the gate does not set the exit code.
    let text = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .current_dir(fixture_path("../.."))
        .args(["analyze", "--no-acknowledgments"])
        .args(["--input", "tests/fixtures/running_example.json"])
        .env("RUST_LOG", "error")
        .output()
        .expect("spawn perf-sentinel");
    assert_eq!(text.status.code(), Some(0));
}

#[test]
fn running_example_acknowledged_n_plus_one_keeps_its_waste() {
    let dir = tempfile::tempdir().expect("tempdir");
    let acks = write_ack_file(dir.path(), SIGNATURES[0]);
    let acks = acks.to_str().expect("utf-8 path");
    let (code, r) = run("running_example.json", &["--acknowledgments", acks]);
    assert_eq!(r["findings"].as_array().map(Vec::len), Some(3));
    assert_close(&r["green_summary"]["io_waste_ratio"], 0.65, "acknowledged");
    assert_eq!(failed_rules(&r), ["io_waste_ratio_max"]);
    assert_eq!(code, Some(1));
}
