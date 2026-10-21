//! End-to-end integration tests for the `verify-hash` subcommand.
//!
//! These tests exercise the CLI binary directly so they cover the full
//! parse + dispatch + verification path. Cosign delegation is not
//! mocked end to end (a PATH-override stub is fragile across runners).
//! The content hash branch is exercised in full. The signature and
//! binary attestation branches are exercised via metadata-present /
//! absent permutations.

use sentinel_core::report::periodic::compute_content_hash;
use sentinel_core::report::periodic::schema::PeriodicReport;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn workspace_doc(rel: &str) -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("..").join("..").join(rel)
}

fn run_verify(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .arg("verify-hash")
        .args(args)
        .output()
        .expect("spawn perf-sentinel verify-hash")
}

#[test]
fn verify_hash_on_a_zeroed_hash_fails_content_check() {
    // A zeroed content_hash does not match the recompute. Exit code must
    // be 1 (UNTRUSTED), not 0.
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = write_example_with_zeroed_hash(tmp.path());
    let out = run_verify(&["--report", path.to_str().unwrap()]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[FAIL] Content hash"),
        "expected content-hash FAIL: {stdout}"
    );
    assert!(stdout.contains("UNTRUSTED"), "expected UNTRUSTED: {stdout}");
}

fn write_example_with_zeroed_hash(dir: &std::path::Path) -> PathBuf {
    let example = workspace_doc("docs/schemas/examples/example-official-public-G2.json");
    let mut report: PeriodicReport = serde_json::from_slice(&fs::read(&example).unwrap()).unwrap();
    report.integrity.content_hash = format!("sha256:{}", "0".repeat(64));
    let path = dir.join("zeroed.json");
    fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    path
}

fn write_example_with_fixed_hash(dir: &std::path::Path) -> PathBuf {
    // Load the example, recompute the canonical hash, write a fresh
    // copy with the real hash baked in. Bypasses the disclose pipeline
    // entirely and exercises just the verify-hash side.
    let example = workspace_doc("docs/schemas/examples/example-official-public-G2.json");
    let mut report: PeriodicReport = serde_json::from_slice(&fs::read(&example).unwrap()).unwrap();
    let hash = compute_content_hash(&report).unwrap();
    report.integrity.content_hash = hash;
    let path = dir.join("report.json");
    fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    path
}

#[test]
fn verify_hash_returns_partial_with_exit_2_when_signature_absent() {
    // A hash-only report (no Sigstore signature) yields PARTIAL.
    // Exit code 2 distinguishes PARTIAL (verification could not
    // complete) from UNTRUSTED (exit 1, a check actively failed).
    // A scripted `verify-hash && deploy` still blocks because exit is
    // non-zero. The content hash matches but the signature was never
    // verified.
    let tmp = tempfile::tempdir().expect("tempdir");
    let report_path = write_example_with_fixed_hash(tmp.path());
    let v = run_verify(&["--report", report_path.to_str().unwrap()]);
    assert_eq!(
        v.status.code(),
        Some(2),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&v.stdout),
        String::from_utf8_lossy(&v.stderr)
    );
    let stdout = String::from_utf8_lossy(&v.stdout);
    assert!(stdout.contains("[OK] Content hash"), "{stdout}");
    assert!(stdout.contains("PARTIAL"), "{stdout}");
}

#[test]
fn verify_hash_signed_without_identity_flags_is_untrusted() {
    // A report carrying a Sigstore signature block but verified WITHOUT
    // --expected-identity/--expected-issuer must fail closed. Any OIDC
    // account holder can forge an unconstrained Sigstore bundle, so the
    // signature check returns FAIL and the verdict is UNTRUSTED (exit 1),
    // never a silent pass. The identity gate short-circuits before cosign
    // is invoked, so this needs no cosign binary on PATH. Content hash
    // still matches because the signature is a post-sign (blanked) field.
    use sentinel_core::report::periodic::schema::SignatureMetadata;

    let tmp = tempfile::tempdir().expect("tempdir");
    let example = workspace_doc("docs/schemas/examples/example-official-public-G2.json");
    let mut report: PeriodicReport = serde_json::from_slice(&fs::read(&example).unwrap()).unwrap();
    report.integrity.signature = Some(SignatureMetadata {
        format: "sigstore-cosign-intoto-v1".to_string(),
        bundle_url: "https://example.fr/report.bundle".to_string(),
        signer_identity: "ci@example.fr".to_string(),
        signer_issuer: "https://token.actions.githubusercontent.com".to_string(),
        rekor_url: "https://rekor.sigstore.dev".to_string(),
        rekor_log_index: 1,
        signed_at: "2026-01-01T00:00:00Z".to_string(),
    });
    let hash = compute_content_hash(&report).unwrap();
    report.integrity.content_hash = hash;
    let report_path = tmp.path().join("report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();

    let v = run_verify(&["--report", report_path.to_str().unwrap()]);
    assert_eq!(
        v.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&v.stdout),
        String::from_utf8_lossy(&v.stderr)
    );
    let stdout = String::from_utf8_lossy(&v.stdout);
    assert!(stdout.contains("[OK] Content hash"), "{stdout}");
    assert!(stdout.contains("[FAIL] Signature"), "{stdout}");
    assert!(stdout.contains("UNTRUSTED"), "{stdout}");
}

fn write_attestation_for(report: &PeriodicReport, path: &std::path::Path) {
    use sentinel_core::report::periodic::build_in_toto_statement_named;

    let hash = compute_content_hash(report).unwrap();
    let hex = hash.trim_start_matches("sha256:");
    let statement = build_in_toto_statement_named(report, hex, "report.json");
    let mut line = serde_json::to_string(&statement).unwrap();
    line.push('\n');
    fs::write(path, line).unwrap();
}

#[test]
fn verify_hash_binds_an_attestation_naming_the_content_hash() {
    // Hash-only report (no signature block) with its attestation: the
    // binding check passes and the verdict stays PARTIAL, since no
    // signature was verified.
    let tmp = tempfile::tempdir().expect("tempdir");
    let report_path = write_example_with_fixed_hash(tmp.path());
    let report: PeriodicReport = serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    let attestation_path = tmp.path().join("attestation.intoto.jsonl");
    write_attestation_for(&report, &attestation_path);

    let v = run_verify(&[
        "--report",
        report_path.to_str().unwrap(),
        "--attestation",
        attestation_path.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&v.stdout);
    assert_eq!(v.status.code(), Some(2), "{stdout}");
    assert!(stdout.contains("[OK] Attestation binding"), "{stdout}");
    assert!(stdout.contains("PARTIAL"), "{stdout}");
}

#[test]
fn verify_hash_refuses_an_attestation_that_names_other_figures() {
    // Figures edited after the attestation was signed, hash re-baked as
    // `hash-bake --allow-signed` would: the content hash matches the edited
    // file, but the signed statement names the original figures, so a valid
    // signature over it must not vouch for these.
    let tmp = tempfile::tempdir().expect("tempdir");
    let report_path = write_example_with_fixed_hash(tmp.path());
    let original: PeriodicReport =
        serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    let attestation_path = tmp.path().join("attestation.intoto.jsonl");
    write_attestation_for(&original, &attestation_path);

    let mut edited = original.clone();
    edited.aggregate.total_carbon_kgco2eq /= 2.0;
    edited.integrity.content_hash = compute_content_hash(&edited).unwrap();
    fs::write(&report_path, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();

    let v = run_verify(&[
        "--report",
        report_path.to_str().unwrap(),
        "--attestation",
        attestation_path.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&v.stdout);
    assert_eq!(v.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("[OK] Content hash"), "{stdout}");
    assert!(stdout.contains("[FAIL] Attestation binding"), "{stdout}");
    assert!(stdout.contains("UNTRUSTED"), "{stdout}");
}

#[test]
fn verify_hash_after_content_tamper_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let report_path = write_example_with_fixed_hash(tmp.path());
    let bytes = fs::read_to_string(&report_path).unwrap();
    let mutated = bytes.replace("\"Example SAS\"", "\"Tampered SAS\"");
    fs::write(&report_path, mutated).unwrap();
    let v = run_verify(&["--report", report_path.to_str().unwrap()]);
    assert_eq!(v.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&v.stdout);
    assert!(stdout.contains("[FAIL] Content hash"), "{stdout}");
}

#[test]
fn verify_hash_missing_report_returns_input_error() {
    // Missing file is exit 3 (INPUT_ERROR), distinct from UNTRUSTED
    // (1) and PARTIAL (2) so a wrapper script can react differently
    // to a wrong path than to a tamper attempt.
    let v = run_verify(&["--report", "/nonexistent/path/to/missing.json"]);
    assert_eq!(v.status.code(), Some(3));
}

#[test]
fn verify_hash_json_format_emits_structured_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = write_example_with_zeroed_hash(tmp.path());
    let v = run_verify(&["--report", path.to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&v.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON output");
    assert_eq!(parsed["overall"], "UNTRUSTED");
    assert_eq!(parsed["verifications"]["content_hash"]["status"], "fail");
    assert_eq!(
        parsed["verifications"]["signature"]["status"],
        "not_provided"
    );
}

#[test]
fn verify_hash_url_mode_rejects_http_scheme() {
    // Hardening: only HTTPS is accepted. http:// must fail with a
    // network-class exit code 4 (NETWORK_ERROR), distinct from
    // INPUT_ERROR (3, e.g. file missing) and UNTRUSTED (1).
    let v = run_verify(&["--url", "http://example.fr/report.json"]);
    assert_eq!(v.status.code(), Some(4));
}

#[test]
fn verify_hash_local_report_over_size_cap_returns_input_error() {
    // 64 MiB cap on `--report <local>`. Sparse `set_len` extends without
    // writing the bytes so the test runs fast on every filesystem.
    // Source of truth: `crates/sentinel-cli/src/limits.rs::MAX_LOCAL_REPORT_BYTES`.
    // Kept as a literal here because `perf-sentinel` is a pure binary
    // crate without a `lib.rs` re-export. Bump in lockstep when the cap
    // changes.
    let tmp = tempfile::tempdir().expect("tempdir");
    let huge = tmp.path().join("huge.json");
    let file = fs::File::create(&huge).unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    drop(file);
    let v = run_verify(&["--report", huge.to_str().unwrap()]);
    assert_eq!(v.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&v.stderr);
    assert!(stderr.contains("exceeds"), "stderr: {stderr}");
}

#[test]
fn verify_hash_refuses_a_newer_schema_with_exit_5() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_example_with_fixed_hash(dir.path());
    let mut report: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    report["schema_version"] = "perf-sentinel-report/v1.9".into();
    fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();

    let out = run_verify(&["--report", path.to_str().unwrap()]);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(5), "stderr: {stderr}");
    assert!(stderr.contains("perf-sentinel-report/v1.9"), "{stderr}");
    assert!(stderr.contains("upgrade perf-sentinel"), "{stderr}");
}

/// A recipient holding only the PDF gets the same verdict as one holding
/// the JSON and the attestation.
#[cfg(feature = "pdf")]
#[test]
fn verify_hash_on_the_pdf_matches_verify_hash_on_the_json() {
    let dir = tempfile::tempdir().unwrap();
    let report_path = write_example_with_fixed_hash(dir.path());
    let report: PeriodicReport = serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    let attestation = dir.path().join("attestation.intoto.jsonl");
    write_attestation_for(&report, &attestation);
    let pdf = dir.path().join("report.pdf");
    let render = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["disclose-pdf", "--report"])
        .arg(&report_path)
        .arg("--attestation")
        .arg(&attestation)
        .arg("--output")
        .arg(&pdf)
        .output()
        .unwrap();
    assert_eq!(
        Some(0),
        render.status.code(),
        "{}",
        String::from_utf8_lossy(&render.stderr)
    );

    let from_json = run_verify(&[
        "--report",
        report_path.to_str().unwrap(),
        "--attestation",
        attestation.to_str().unwrap(),
        "--format",
        "json",
    ]);
    let from_pdf = run_verify(&["--report", pdf.to_str().unwrap(), "--format", "json"]);

    let verdicts = |out: &std::process::Output| {
        let value: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
        (
            value["verifications"].clone(),
            value["overall"].clone(),
            out.status.code(),
        )
    };
    assert_eq!(verdicts(&from_json), verdicts(&from_pdf));
    let text = run_verify(&["--report", pdf.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("attached report.json"), "{stdout}");
    assert!(stdout.contains("not its printed pages"), "{stdout}");
    assert_eq!(
        "ok",
        verdicts(&from_pdf).0["attestation_binding"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
    );
}

/// The attestation and bundle read out of a PDF go to a private directory
/// that is gone when the command returns.
#[cfg(feature = "pdf")]
#[test]
fn verify_hash_on_a_pdf_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let report_path = write_example_with_fixed_hash(dir.path());
    let report: PeriodicReport = serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    let attestation = dir.path().join("attestation.intoto.jsonl");
    write_attestation_for(&report, &attestation);
    let pdf = dir.path().join("report.pdf");
    let render = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["disclose-pdf", "--report"])
        .arg(&report_path)
        .arg("--attestation")
        .arg(&attestation)
        .arg("--output")
        .arg(&pdf)
        .output()
        .unwrap();
    assert_eq!(Some(0), render.status.code());

    let out = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .env("TMPDIR", tmp.path())
        .args(["verify-hash", "--report"])
        .arg(&pdf)
        .output()
        .unwrap();

    assert_eq!(
        Some(2),
        out.status.code(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let left: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}

/// The command page 5 prints must parse: placeholders swapped for real
/// values, it reaches a verdict instead of a usage error.
#[cfg(feature = "pdf")]
#[test]
fn the_printed_verify_command_parses() {
    let dir = tempfile::tempdir().unwrap();
    let report_path = write_example_with_fixed_hash(dir.path());
    let report: PeriodicReport = serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    assert!(
        report.integrity.binary_hash.is_some(),
        "the example declares a binary hash"
    );
    let pdf = dir.path().join("report.pdf");
    let render = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["disclose-pdf", "--report"])
        .arg(&report_path)
        .arg("--output")
        .arg(&pdf)
        .output()
        .unwrap();
    assert_eq!(Some(0), render.status.code());
    let words = sentinel_core::report::periodic::pdf::verify_command_words(
        &report,
        Some("ci@example.org"),
        Some("https://issuer.example"),
    );
    assert_eq!(Some("perf-sentinel"), words.first().map(String::as_str));
    let args: Vec<String> = words
        .iter()
        .skip(1)
        .map(|word| match word.as_str() {
            "report.pdf" => pdf.to_str().unwrap().to_string(),
            w if w.starts_with('<') => env!("CARGO_BIN_EXE_perf-sentinel").to_string(),
            w => w.to_string(),
        })
        .collect();
    let out = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(&args)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("Overall:"),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(!stderr.contains("Usage:"), "{stderr}");
}
