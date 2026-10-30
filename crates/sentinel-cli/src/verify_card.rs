//! `perf-sentinel verify-card`: the JSON line and the exit code are the
//! contract (C2), stderr carries the detail.

use std::path::Path;

use sentinel_core::report::card::CardError;
use sentinel_core::report::card::binding::{
    Binding, KeyStatus, PublishedKey, judge, parse_key_list,
};
use sentinel_core::report::card::raster::MAX_CARD_PNG_BYTES;
use sentinel_core::report::card::verify::{CardSource, Integrity, check_integrity};
use serde::Serialize;

use crate::card::{finish_error, input_error, read_file};

/// The largest `--keys` file read: a published list holds a few keys.
const MAX_KEY_LIST_BYTES: u64 = 64 * 1024;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const PDF_MAGIC: &[u8] = b"%PDF-";

#[derive(Serialize)]
struct VerifyLine<'a> {
    integrity: &'static str,
    level: u8,
    domain: Option<&'a str>,
    key: KeyStatus,
    reason: &'static str,
}

/// Verify the card at `target` (a PNG, a PDF or its verify URL), then
/// bind its key through `--keys`. `--online` has no HTTP lookup yet.
#[expect(clippy::unused_async, reason = "the --online lookups of Task 10 await")]
pub async fn cmd_verify_card(
    target: &str,
    keys: Option<&Path>,
    online: bool,
    _doh: Option<&str>,
) -> i32 {
    let published = match keys.map(read_key_list).transpose() {
        Ok(published) => published,
        Err(e) => return finish_error(&e),
    };
    let integrity = match integrity_of(target) {
        Ok(integrity) => integrity,
        Err(e) => return finish_error(&e),
    };
    let binding = match (
        &integrity.fingerprint,
        &integrity.public_key,
        integrity.signed_day,
    ) {
        (Some(fingerprint), Some(public_key), Some(day)) if integrity.ok => {
            if online {
                eprintln!("Error: this build cannot read the domain's key list online");
                return print_line(&integrity, unbound("no_http_client"), 4);
            }
            judge(fingerprint, public_key, day, published.as_deref())
        }
        _ => unbound(integrity.reason),
    };
    let untrusted = !integrity.ok
        || matches!(
            binding.reason,
            "signed_after_revocation" | "signed_outside_validity" | "public_key_mismatch"
        );
    print_line(&integrity, binding, i32::from(untrusted))
}

fn unbound(reason: &'static str) -> Binding {
    Binding {
        level: 1,
        key: KeyStatus::Unknown,
        reason,
    }
}

fn print_line(integrity: &Integrity, binding: Binding, code: i32) -> i32 {
    let line = VerifyLine {
        integrity: if integrity.ok { "ok" } else { "tampered" },
        level: binding.level,
        domain: integrity.domain.as_deref(),
        key: binding.key,
        reason: binding.reason,
    };
    println!(
        "{}",
        serde_json::to_string(&line).expect("plain struct serialises")
    );
    code
}

fn read_key_list(path: &Path) -> Result<Vec<PublishedKey>, CardError> {
    parse_key_list(&read_file(path, MAX_KEY_LIST_BYTES)?)
}

/// A target starting with `https://` or `http://` is a verify URL; any
/// other is a file, a PNG or a PDF by its first bytes.
fn integrity_of(target: &str) -> Result<Integrity, CardError> {
    if target.starts_with("https://") || target.starts_with("http://") {
        return check_integrity(CardSource::Url(target));
    }
    let bytes = read_file(Path::new(target), MAX_CARD_PNG_BYTES as u64)?;
    let source = if bytes.starts_with(PNG_MAGIC) {
        CardSource::Png(&bytes)
    } else if bytes.starts_with(PDF_MAGIC) {
        CardSource::Pdf(&bytes)
    } else {
        return Err(input_error(
            "malformed_input",
            format!("{target} is not a PNG, a PDF or a verify URL"),
        ));
    };
    check_integrity(source)
}
