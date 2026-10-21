//! `disclose-pdf`: the exit-code contract the Hub relies on.

use sentinel_core::report::periodic::schema::PeriodicReport;
use sentinel_core::report::periodic::{build_in_toto_statement_named, compute_content_hash};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const G2_EXAMPLE: &str = "../../docs/schemas/examples/example-official-public-G2.json";

fn example() -> PeriodicReport {
    let path = format!("{}/{G2_EXAMPLE}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_slice(&fs::read(path).expect("example")).expect("example parses")
}

/// Write `report` with its hash baked in, and return its path.
fn write_baked(dir: &Path, mut report: PeriodicReport) -> PathBuf {
    report.integrity.content_hash = compute_content_hash(&report).expect("hash");
    let path = dir.join("report.json");
    fs::write(&path, serde_json::to_vec_pretty(&report).expect("json")).expect("write");
    path
}

fn write_attestation(dir: &Path, report: &PeriodicReport, hex: &str) -> PathBuf {
    let statement = build_in_toto_statement_named(report, hex, "report.json");
    let path = dir.join("attestation.intoto.jsonl");
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&statement).expect("json")),
    )
    .expect("write");
    path
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .arg("disclose-pdf")
        .args(args)
        .output()
        .expect("spawn perf-sentinel disclose-pdf")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn disclose_pdf_renders_and_prints_rendered() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = example();
    let path = write_baked(dir.path(), report.clone());
    let baked: PeriodicReport = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let hex = baked
        .integrity
        .content_hash
        .trim_start_matches("sha256:")
        .to_string();
    let attestation = write_attestation(dir.path(), &baked, &hex);
    let output = dir.path().join("report.pdf");

    let out = run(&[
        "--report",
        path.to_str().unwrap(),
        "--attestation",
        attestation.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);

    assert_eq!(
        Some(0),
        out.status.code(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(r#"{"result":"rendered"}"#, stdout(&out));
    assert!(fs::read(&output).unwrap().starts_with(b"%PDF-"));
}

#[test]
fn disclose_pdf_refuses_edited_figures_with_exit_1() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_baked(dir.path(), example());
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["aggregate"]["total_requests"] = serde_json::json!(1);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let output = dir.path().join("report.pdf");

    let out = run(&[
        "--report",
        path.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);

    assert_eq!(Some(1), out.status.code());
    assert_eq!(
        r#"{"result":"refused","reason":"content_hash_mismatch"}"#,
        stdout(&out)
    );
    assert!(!output.exists());
}

#[test]
fn disclose_pdf_refuses_a_foreign_attestation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_baked(dir.path(), example());
    let attestation = write_attestation(dir.path(), &example(), &"ab".repeat(32));
    let output = dir.path().join("report.pdf");

    let out = run(&[
        "--report",
        path.to_str().unwrap(),
        "--attestation",
        attestation.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);

    assert_eq!(Some(1), out.status.code());
    assert_eq!(
        r#"{"result":"refused","reason":"binding_mismatch"}"#,
        stdout(&out)
    );
}

#[test]
fn disclose_pdf_oversized_attestation_is_exit_3_file_too_large() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_baked(dir.path(), example());
    let attestation = dir.path().join("attestation.intoto.jsonl");
    fs::write(&attestation, vec![b' '; 1024 * 1024 + 1]).expect("write");
    let output = dir.path().join("report.pdf");

    let out = run(&[
        "--report",
        path.to_str().unwrap(),
        "--attestation",
        attestation.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);

    assert_eq!(Some(3), out.status.code());
    assert_eq!(
        r#"{"result":"input_error","reason":"file_too_large"}"#,
        stdout(&out)
    );
}

#[test]
fn disclose_pdf_missing_report_is_exit_3_missing_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(&[
        "--report",
        dir.path().join("absent.json").to_str().unwrap(),
        "--output",
        dir.path().join("report.pdf").to_str().unwrap(),
    ]);

    assert_eq!(Some(3), out.status.code());
    assert_eq!(
        r#"{"result":"input_error","reason":"missing_file"}"#,
        stdout(&out)
    );
}

#[test]
fn disclose_pdf_newer_schema_is_exit_5() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_baked(dir.path(), example());
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["schema_version"] = "perf-sentinel-report/v1.9".into();
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

    let out = run(&[
        "--report",
        path.to_str().unwrap(),
        "--output",
        dir.path().join("r.pdf").to_str().unwrap(),
    ]);

    assert_eq!(Some(5), out.status.code());
    assert_eq!(
        r#"{"result":"unrenderable","reason":"unsupported_schema_version"}"#,
        stdout(&out)
    );
}

#[test]
fn disclose_pdf_cyrillic_is_exit_5_and_lists_the_characters_on_stderr() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut report = example();
    report.organisation.name = "Сбербанк".to_string();
    let path = write_baked(dir.path(), report);

    let out = run(&[
        "--report",
        path.to_str().unwrap(),
        "--output",
        dir.path().join("r.pdf").to_str().unwrap(),
    ]);

    assert_eq!(Some(5), out.status.code());
    assert_eq!(
        r#"{"result":"unrenderable","reason":"unsupported_characters"}"#,
        stdout(&out)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("U+0421"));
}

#[test]
fn disclose_pdf_help_lists_the_exit_codes() {
    let out = run(&["--help"]);
    let help = String::from_utf8_lossy(&out.stdout);
    for code in ["0 ", "1 ", "3 ", "5 "] {
        assert!(help.contains(&format!("  {code}")), "{help}");
    }
}
