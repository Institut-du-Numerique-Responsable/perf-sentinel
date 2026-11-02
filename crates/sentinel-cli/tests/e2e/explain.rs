//! `explain` subcommand.

use serde_json::Value;
use std::fs;
use std::process::{Command, Stdio};

#[test]
fn cli_explain_text_output() {
    let fixture_path = format!(
        "{}/../../tests/fixtures/n_plus_one_sql.json",
        env!("CARGO_MANIFEST_DIR")
    );

    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "explain",
            "--input",
            &fixture_path,
            "--trace-id",
            "trace-n1-sql",
        ])
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(
        output.status.success(),
        "explain should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("trace-n1-sql"), "should show trace ID");
}

#[test]
fn cli_explain_json_output() {
    let fixture_path = format!(
        "{}/../../tests/fixtures/n_plus_one_sql.json",
        env!("CARGO_MANIFEST_DIR")
    );

    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "explain",
            "--input",
            &fixture_path,
            "--trace-id",
            "trace-n1-sql",
            "--format",
            "json",
        ])
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(
        output.status.success(),
        "explain --format json should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let tree: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("explain JSON should be valid: {e}\nstdout: {stdout}"));
    assert_eq!(tree["trace_id"], "trace-n1-sql");
}

#[test]
fn cli_explain_unknown_trace_id_fails() {
    let fixture_path = format!(
        "{}/../../tests/fixtures/n_plus_one_sql.json",
        env!("CARGO_MANIFEST_DIR")
    );

    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "explain",
            "--input",
            &fixture_path,
            "--trace-id",
            "nonexistent-trace",
        ])
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(
        !output.status.success(),
        "should fail with unknown trace ID"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "should mention trace not found"
    );
    // explain has no quality gate: a missing trace is a tooling error, so
    // its exit code must match the rest of explain's failure paths (input
    // read/parse) rather than the gate code 1. See docs/CI.md "Exit codes".
    assert_eq!(
        output.status.code(),
        Some(75),
        "unknown trace must exit EXIT_TOOLING_ERROR (75), not 1"
    );
}

#[test]
fn cli_help_shows_explain() {
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .arg("--help")
        .output()
        .expect("failed to execute perf-sentinel");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("explain"),
        "help should list explain subcommand"
    );
}

/// On an unknown --trace-id, explain lists the available ids so the
/// operator can recover from a typo. This exercises the shared
/// `trace_not_found_exit` helper that `explain --tui` reuses (the --tui
/// path is gated on a TTY, so the recovery hint is asserted here on the
/// pipe-safe non-interactive command).
#[test]
fn cli_explain_unknown_trace_lists_available_ids() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let file_path = dir.path().join("traces.json");
    fs::write(&file_path, b"[]").expect("failed to write fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "explain",
            "--input",
            file_path.to_str().unwrap(),
            "--trace-id",
            "nope",
        ])
        .output()
        .expect("failed to execute perf-sentinel");
    assert!(
        !output.status.success(),
        "explain with an unknown trace id must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("trace ID 'nope' not found"),
        "got: {stderr}"
    );
    assert!(
        stderr.contains("Available trace IDs"),
        "expected the available-ids recovery hint, got: {stderr}"
    );
}

#[test]
fn cli_explain_reads_a_snapshot_report_with_embedded_traces() {
    // A daemon snapshot has no raw events. Its masked span trees are
    // what explain must draw from, like the dashboard and the TUI.
    let fixture = format!(
        "{}/../../tests/fixtures/report_with_embedded_traces.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "explain",
            "--input",
            &fixture,
            "--trace-id",
            "trace-demo-nplus-sql",
        ])
        .output()
        .expect("failed to run perf-sentinel");
    assert!(
        output.status.success(),
        "explain on a snapshot failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Trace trace-demo-nplus-sql"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("update flux_cdar"),
        "the masked template must be the tree's operation text"
    );
}

/// Run `explain` on `input` and return its stderr bytes.
fn explain_stderr(input: &[u8], trace_id: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let file_path = dir.path().join("traces.json");
    fs::write(&file_path, input).expect("failed to write fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "explain",
            "--input",
            file_path.to_str().unwrap(),
            "--trace-id",
            trace_id,
        ])
        // Keep the colored tracing lines out of the byte check.
        .env("RUST_LOG", "error")
        .output()
        .expect("failed to execute perf-sentinel");
    assert!(!output.status.success());
    output.stderr
}

#[test]
fn cli_explain_diagnostics_strip_escape_sequences_from_input() {
    // Trace ids listed in the not-found hint come from the trace file.
    let trace = br#"[{"timestamp":"2025-07-10T14:32:01.123Z","trace_id":"t\u001b]0;pwned\u0007","span_id":"s1","service":"svc","type":"sql","operation":"SELECT","target":"SELECT 1","duration_us":100,"source":{"endpoint":"GET /test","method":"m"}}]"#;
    let stderr = explain_stderr(trace, "nope\u{1b}[2J");
    assert!(stderr.contains(&b'n'), "expected a diagnostic");
    assert!(
        !stderr.contains(&0x1b),
        "{}",
        String::from_utf8_lossy(&stderr)
    );
    // serde echoes an unknown variant in its parse error.
    let bad_type = br#"[{"timestamp":"2025-07-10T14:32:01.123Z","trace_id":"t1","span_id":"s1","service":"svc","type":"x\u001b[2J","operation":"SELECT","target":"SELECT 1","duration_us":100,"source":{"endpoint":"GET /test","method":"m"}}]"#;
    let stderr = explain_stderr(bad_type, "t1");
    assert!(
        !stderr.contains(&0x1b),
        "{}",
        String::from_utf8_lossy(&stderr)
    );
}
