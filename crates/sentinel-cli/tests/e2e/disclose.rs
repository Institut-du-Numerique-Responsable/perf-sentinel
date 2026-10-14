//! `disclose` subcommand flag contracts.

use crate::helpers::ORG_CONFIG_EXAMPLE;
use std::fs;
use std::process::Command;

#[test]
fn cli_disclose_tui_relaxes_period_flags_and_requires_terminal() {
    // `--tui` makes --intent/--confidentiality/--period-type/--from/--to/--output
    // optional. With stdout piped (no TTY) the preview exits via the terminal
    // guard (code 1), not a clap missing-argument error (code 2). Reaching the
    // guard proves clap accepted the omitted flags. --input and --org-config
    // stay required.
    let dir = tempfile::tempdir().expect("temp dir");
    let archive = dir.path().join("archive.ndjson");
    fs::write(
        &archive,
        "{\"ts\":\"2026-03-15T00:00:00Z\",\"report\":{}}\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .arg("disclose")
        .arg("--tui")
        .arg("--input")
        .arg(&archive)
        .arg("--org-config")
        .arg(ORG_CONFIG_EXAMPLE)
        .output()
        .expect("failed to execute perf-sentinel");

    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requires a terminal"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn cli_disclose_without_tui_requires_period_flags() {
    // Canonical (non-TUI) disclose still demands the period/intent flags.
    // Omitting them is a clap usage error (exit code 2).
    let dir = tempfile::tempdir().expect("temp dir");
    let archive = dir.path().join("archive.ndjson");
    fs::write(
        &archive,
        "{\"ts\":\"2026-03-15T00:00:00Z\",\"report\":{}}\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .arg("disclose")
        .arg("--input")
        .arg(&archive)
        .arg("--org-config")
        .arg(ORG_CONFIG_EXAMPLE)
        .output()
        .expect("failed to execute perf-sentinel");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected a clap usage error, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--intent") || stderr.contains("required"),
        "unexpected stderr: {stderr}"
    );
}

/// One archive line per timestamp, each a real `analyze` report, the shape
/// the daemon archives.
fn write_archive(dir: &std::path::Path, stamps: &[&str]) -> std::path::PathBuf {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/n_plus_one_sql.json");
    let analyzed = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["analyze", "--format", "json", "--input"])
        .arg(&fixture)
        .output()
        .expect("run analyze");
    let report: serde_json::Value = serde_json::from_slice(&analyzed.stdout).expect("report");
    let archive = dir.join("archive.ndjson");
    let lines: String = stamps
        .iter()
        .map(|ts| format!("{}\n", serde_json::json!({ "ts": ts, "report": report })))
        .collect();
    fs::write(&archive, lines).unwrap();
    archive
}

fn disclose_q1(
    archive: &std::path::Path,
    intent: &str,
    out: &std::path::Path,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "disclose",
            "--intent",
            intent,
            "--confidentiality",
            "public",
        ])
        .args([
            "--period-type",
            "calendar-quarter",
            "--from",
            "2026-01-01",
            "--to",
            "2026-03-31",
        ])
        .arg("--input")
        .arg(archive)
        .arg("--output")
        .arg(out)
        .arg("--org-config")
        .arg(ORG_CONFIG_EXAMPLE)
        .output()
        .expect("run disclose")
}

#[test]
fn cli_disclose_official_refuses_an_archive_that_starts_after_the_period() {
    // What rotation leaves of a busy quarter: its last weeks only.
    let dir = tempfile::tempdir().expect("temp dir");
    let archive = write_archive(
        dir.path(),
        &["2026-03-10T09:00:00Z", "2026-03-30T09:00:00Z"],
    );
    let out = dir.path().join("report.json");

    let official = disclose_q1(&archive, "official", &out);
    let stderr = String::from_utf8_lossy(&official.stderr);
    assert_eq!(official.status.code(), Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains(
            "Error: the archive starts on 2026-03-10, after the period's first day 2026-01-01"
        ),
        "stderr: {stderr}"
    );
    assert!(!out.exists(), "an official report must not be written");

    let internal = disclose_q1(&archive, "internal", &out);
    let stderr = String::from_utf8_lossy(&internal.stderr);
    assert_eq!(internal.status.code(), Some(0), "stderr: {stderr}");
    assert!(
        stderr.contains("Warning: the archive starts on 2026-03-10"),
        "stderr: {stderr}"
    );
}

#[test]
fn cli_disclose_does_not_flag_an_archive_that_precedes_the_period() {
    // A quiet start: nothing inside the period before March, but December
    // lines prove the archive was not pruned.
    let dir = tempfile::tempdir().expect("temp dir");
    let archive = write_archive(
        dir.path(),
        &["2025-12-20T09:00:00Z", "2026-03-10T09:00:00Z"],
    );
    let out = dir.path().join("report.json");
    let internal = disclose_q1(&archive, "internal", &out);
    let stderr = String::from_utf8_lossy(&internal.stderr);
    assert_eq!(internal.status.code(), Some(0), "stderr: {stderr}");
    assert!(
        !stderr.contains("the archive starts on"),
        "stderr: {stderr}"
    );
}
