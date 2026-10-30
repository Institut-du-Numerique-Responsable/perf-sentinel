//! `card`: the exit-code and JSON-line contract the Hub relies on.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .arg("card")
        .args(args)
        .output()
        .expect("spawn perf-sentinel card")
}

fn stdout_json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is one JSON line ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn keygen(dir: &Path) -> Output {
    run(&["keygen", "--out-dir", dir.to_str().expect("utf-8 path")])
}

#[test]
fn card_keygen_writes_a_private_key_file_and_prints_its_public_half() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = keygen(dir.path());
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let json = stdout_json(&out);
    let object = json.as_object().expect("an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["fingerprint", "public_key"]);
    let fingerprint = json["fingerprint"].as_str().expect("fingerprint");
    assert!(fingerprint.starts_with("ps1:"), "{fingerprint}");
    let file = dir.path().join(format!("{fingerprint}.pk8"));
    let meta = fs::metadata(&file).expect("the key file exists");
    assert_eq!(meta.len(), 83);
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn card_keygen_leaves_earlier_keys_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = stdout_json(&keygen(dir.path()));
    let first_path = dir.path().join(format!(
        "{}.pk8",
        first["fingerprint"].as_str().expect("fp")
    ));
    let first_bytes = fs::read(&first_path).expect("first key");
    let second = keygen(dir.path());
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    assert_ne!(first["fingerprint"], stdout_json(&second)["fingerprint"]);
    assert_eq!(fs::read(&first_path).expect("first key"), first_bytes);
}

#[test]
fn card_keygen_into_a_missing_directory_is_an_input_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = keygen(&dir.path().join("missing"));
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let json = stdout_json(&out);
    assert_eq!(json["result"], "input_error");
    assert_eq!(json["reason"], "output_unwritable");
}
