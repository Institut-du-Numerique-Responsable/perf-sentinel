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

/// A fresh key in `dir`: its path and fingerprint.
fn key_in(dir: &Path) -> (std::path::PathBuf, String) {
    let out = keygen(dir);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let fingerprint = stdout_json(&out)["fingerprint"]
        .as_str()
        .expect("fingerprint")
        .to_owned();
    (dir.join(format!("{fingerprint}.pk8")), fingerprint)
}

fn story_input() -> serde_json::Value {
    let text = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/card/story.json"
    ))
    .expect("story fixture");
    serde_json::from_str(&text).expect("story fixture is JSON")
}

/// `card render` of `input` with the key at `key`, into a fresh `out`
/// directory under `dir`.
fn render(dir: &Path, input: &[u8], key: &Path, format: &str) -> (Output, std::path::PathBuf) {
    let input_path = dir.join("card.json");
    fs::write(&input_path, input).expect("write input");
    let out_dir = dir.join("out");
    fs::create_dir_all(&out_dir).expect("out dir");
    let out = run(&[
        "render",
        "--input",
        input_path.to_str().expect("utf-8 path"),
        "--signing-key",
        key.to_str().expect("utf-8 path"),
        "--out-dir",
        out_dir.to_str().expect("utf-8 path"),
        "--format",
        format,
    ]);
    (out, out_dir)
}

/// `card render` of `input` under a fresh key: the output, the directory
/// holding `out/` (kept alive by the caller), and the key's fingerprint.
fn render_json(input: &serde_json::Value, format: &str) -> (Output, tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (key, fingerprint) = key_in(dir.path());
    let bytes = serde_json::to_vec(input).expect("input serializes");
    let (out, _) = render(dir.path(), &bytes, &key, format);
    (out, dir, fingerprint)
}

fn assert_refusal(out: &Output, out_dir: &Path, code: i32, result: &str, reason: &str) {
    assert_eq!(out.status.code(), Some(code), "{out:?}");
    let json = stdout_json(out);
    assert_eq!(json["result"], result, "{json}");
    assert_eq!(json["reason"], reason, "{json}");
    assert!(json.get("field").is_none(), "{json}");
    assert_eq!(fs::read_dir(out_dir).expect("out dir").count(), 0);
}

#[test]
fn card_render_writes_the_png_and_prints_the_c2_line() {
    let (out, dir, fingerprint) = render_json(&story_input(), "png");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let json = stdout_json(&out);
    let mut keys: Vec<&str> = json
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["compact_url", "files", "fingerprint", "result"]);
    assert_eq!(json["result"], "rendered");
    assert_eq!(json["files"], serde_json::json!(["card.png"]));
    assert_eq!(json["fingerprint"], fingerprint);
    assert!(
        json["compact_url"]
            .as_str()
            .expect("compact_url")
            .starts_with("https://perf-sentinel.dev/verifier#c="),
        "{json}"
    );
    let png = fs::read(dir.path().join("out/card.png")).expect("card.png exists");
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
}

#[test]
fn card_render_writes_the_pdf_carousel() {
    let (out, dir, _) = render_json(&story_input(), "pdf");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout_json(&out)["files"], serde_json::json!(["card.pdf"]));
    let pdf = fs::read(dir.path().join("out/card.pdf")).expect("card.pdf exists");
    assert!(pdf.starts_with(b"%PDF-"));
}

#[test]
fn card_render_refuses_a_qr_too_dense() {
    let mut input = story_input();
    input["org"]["domain"] =
        serde_json::json!("cooperative-des-transports-regionaux-du-grand-ouest.acme.com");
    let (out, dir, _) = render_json(&input, "png");
    assert_refusal(
        &out,
        &dir.path().join("out"),
        5,
        "unrenderable",
        "compact_too_large",
    );
}

#[test]
fn card_render_refuses_a_volume_on_a_slow_finding() {
    let mut input = story_input();
    input["story"]["finding_type"] = serde_json::json!("slow_sql");
    let (out, dir, _) = render_json(&input, "png");
    assert_refusal(
        &out,
        &dir.path().join("out"),
        1,
        "refused",
        "volume_not_avoidable",
    );
}

#[test]
fn card_render_names_the_field_it_cannot_draw() {
    let mut input = story_input();
    input["free"]["title"] = serde_json::json!("Fixed 🚀");
    let (out, dir, _) = render_json(&input, "png");
    assert_eq!(out.status.code(), Some(5), "{out:?}");
    assert_eq!(
        stdout_json(&out),
        serde_json::json!({"result":"unrenderable","reason":"unsupported_characters","field":"free.title"})
    );
    assert_eq!(
        fs::read_dir(dir.path().join("out"))
            .expect("out dir")
            .count(),
        0
    );
}

#[test]
fn card_render_refuses_malformed_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (key, _) = key_in(dir.path());
    let (out, out_dir) = render(dir.path(), b"{not json", &key, "png");
    assert_refusal(&out, &out_dir, 3, "input_error", "malformed_input");
}

#[test]
fn card_render_refuses_a_readable_signing_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (key, _) = key_in(dir.path());
    let input = serde_json::to_vec(&story_input()).expect("input serializes");

    let link = dir.path().join("link.pk8");
    std::os::unix::fs::symlink(&key, &link).expect("symlink");
    let (out, out_dir) = render(dir.path(), &input, &link, "png");
    assert_refusal(&out, &out_dir, 3, "input_error", "signing_key_unreadable");

    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).expect("chmod");
    let (out, out_dir) = render(dir.path(), &input, &key, "png");
    assert_refusal(&out, &out_dir, 3, "input_error", "signing_key_permissions");
}

#[test]
fn card_render_help_exits_zero() {
    let out = run(&["render", "--help"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}
