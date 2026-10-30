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

fn verify(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .arg("verify-card")
        .args(args)
        .output()
        .expect("spawn perf-sentinel verify-card")
}

fn assert_line(out: &Output, code: i32, expected: &serde_json::Value) {
    assert_eq!(out.status.code(), Some(code), "{out:?}");
    assert_eq!(&stdout_json(out), expected);
}

fn verified(level: u8, domain: Option<&str>, key: &str, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "integrity": "ok",
        "level": level,
        "domain": domain,
        "key": key,
        "reason": reason,
    })
}

/// A story card PNG of `input` under a fresh key: the directory holding
/// it (kept alive by the caller), its path, the key's fingerprint and the
/// card's verify URL.
fn rendered_png(
    input: &serde_json::Value,
) -> (tempfile::TempDir, std::path::PathBuf, String, String) {
    let (out, dir, fingerprint) = render_json(input, "png");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let url = stdout_json(&out)["compact_url"]
        .as_str()
        .expect("compact_url")
        .to_owned();
    let png = dir.path().join("out/card.png");
    (dir, png, fingerprint, url)
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

/// A `--keys` file listing `keys` under `dir`.
fn key_list(dir: &Path, keys: &serde_json::Value) -> std::path::PathBuf {
    let path = dir.join("published.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({ "keys": keys })).expect("json"),
    )
    .expect("write key list");
    path
}

#[test]
fn verify_card_on_its_own_png_is_ok_level_1() {
    let (_dir, png, _, _) = rendered_png(&story_input());
    assert_line(
        &verify(&[path_str(&png)]),
        0,
        &verified(1, Some("acme.fr"), "unknown", "no_key_list"),
    );
}

#[test]
fn verify_card_with_the_published_key_is_level_2() {
    let (dir, png, fingerprint, _) = rendered_png(&story_input());
    let cases = [
        (
            serde_json::json!({"fingerprint": fingerprint, "from": "2026-01-01"}),
            0,
            verified(2, Some("acme.fr"), "current", "key_published"),
        ),
        (
            serde_json::json!({"fingerprint": fingerprint, "from": "2026-01-01", "to": "2026-12-31"}),
            0,
            verified(2, Some("acme.fr"), "retired", "key_published"),
        ),
        (
            serde_json::json!({"fingerprint": fingerprint, "from": "2026-01-01", "revoked": "2026-09-15"}),
            1,
            verified(1, Some("acme.fr"), "revoked", "signed_after_revocation"),
        ),
    ];
    for (entry, code, expected) in cases {
        let keys = key_list(dir.path(), &serde_json::json!([entry]));
        let out = verify(&[path_str(&png), "--keys", path_str(&keys)]);
        assert_line(&out, code, &expected);
    }
}

#[test]
fn verify_card_after_a_rotation_keeps_old_cards() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (key_a, fingerprint_a) = key_in(dir.path());
    let (_, fingerprint_b) = key_in(dir.path());
    let input = serde_json::to_vec(&story_input()).expect("input serializes");
    let (out, out_dir) = render(dir.path(), &input, &key_a, "png");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let keys = key_list(
        dir.path(),
        &serde_json::json!([
            {"fingerprint": fingerprint_a, "from": "2026-01-01", "to": "2026-10-15"},
            {"fingerprint": fingerprint_b, "from": "2026-10-15"},
        ]),
    );
    let out = verify(&[
        path_str(&out_dir.join("card.png")),
        "--keys",
        path_str(&keys),
    ]);
    assert_line(
        &out,
        0,
        &verified(2, Some("acme.fr"), "retired", "key_published"),
    );
}

#[test]
fn verify_card_on_the_compact_url_is_ok() {
    use sentinel_core::report::card::keys::{b64url_decode, b64url_encode};

    let (_dir, _, _, url) = rendered_png(&story_input());
    assert_line(
        &verify(&[&url]),
        0,
        &verified(1, Some("acme.fr"), "unknown", "no_key_list"),
    );

    // One payload character changed, the signature kept: the day it was
    // generated, 2026-10-01, moved to the next.
    let (prefix, fragment) = url.split_once("#c=").expect("fragment");
    let (compact, signature) = fragment.rsplit_once('.').expect("signature");
    let compact = String::from_utf8(b64url_decode(compact).expect("base64url")).expect("utf-8");
    let changed = compact.replacen("\"g\":20727", "\"g\":20728", 1);
    assert_ne!(changed, compact);
    let tampered = format!(
        "{prefix}#c={}.{signature}",
        b64url_encode(changed.as_bytes())
    );
    assert_line(
        &verify(&[&tampered]),
        1,
        &serde_json::json!({
            "integrity": "tampered",
            "level": 1,
            "domain": null,
            "key": "unknown",
            "reason": "signature_invalid",
        }),
    );
}

/// `png` without its text chunks, every other chunk kept as it was.
fn without_text_chunks(png: &[u8]) -> Vec<u8> {
    let mut out = png[..8].to_vec();
    let mut rest = &png[8..];
    while rest.len() >= 12 {
        let len = u32::from_be_bytes(rest[..4].try_into().expect("4 bytes")) as usize;
        let (chunk, tail) = rest.split_at(12 + len);
        if !matches!(&chunk[4..8], b"iTXt" | b"tEXt" | b"zTXt") {
            out.extend_from_slice(chunk);
        }
        rest = tail;
    }
    out
}

#[test]
fn verify_card_on_a_stripped_png_says_no_card_payload() {
    let (dir, png, _, _) = rendered_png(&story_input());
    let stripped = dir.path().join("stripped.png");
    let bytes = without_text_chunks(&fs::read(&png).expect("card.png"));
    assert!(bytes.len() < fs::metadata(&png).expect("card.png").len() as usize);
    fs::write(&stripped, bytes).expect("write stripped png");
    assert_line(
        &verify(&[path_str(&stripped)]),
        3,
        &serde_json::json!({"result": "input_error", "reason": "no_card_payload"}),
    );
}

#[test]
fn verify_card_on_a_file_that_is_no_card_is_an_input_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let text = dir.path().join("card.txt");
    fs::write(&text, b"not a card").expect("write");
    assert_line(
        &verify(&[path_str(&text)]),
        3,
        &serde_json::json!({"result": "input_error", "reason": "malformed_input"}),
    );
    assert_line(
        &verify(&[path_str(&dir.path().join("missing.png"))]),
        3,
        &serde_json::json!({"result": "input_error", "reason": "missing_file"}),
    );
}

#[test]
fn verify_card_on_a_card_without_a_domain_prints_null() {
    let mut input = story_input();
    input["org"] = serde_json::json!({"name": "Acme"});
    let (_dir, png, _, url) = rendered_png(&input);
    let expected = verified(1, None, "unknown", "no_key_list");
    assert_line(&verify(&[path_str(&png)]), 0, &expected);
    assert_line(&verify(&[&url]), 0, &expected);
}
