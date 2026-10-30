//! `perf-sentinel card`: the exit codes and the JSON line are a contract
//! the Hub reads, never its stderr.

use sentinel_core::report::card::CardError;
use serde::Serialize;

#[derive(Serialize)]
struct ErrorLine {
    result: &'static str,
    reason: &'static str,
}

/// Print the error line and return its exit code.
#[cfg_attr(
    not(unix),
    allow(dead_code, reason = "only keygen calls it until card render")
)]
fn finish_error(error: &CardError) -> i32 {
    eprintln!("Error: {error}");
    let line = ErrorLine {
        result: error.result.as_str(),
        reason: error.reason,
    };
    println!(
        "{}",
        serde_json::to_string(&line).expect("plain struct serialises")
    );
    error.result.exit_code()
}

/// Write a fresh key to `<out_dir>/<fingerprint>.pk8` and print its
/// public half. Never overwrites: an earlier key is someone's identity.
#[cfg(unix)]
pub fn cmd_card_keygen(out_dir: &std::path::Path) -> i32 {
    use sentinel_core::report::card::{CardResult, keys};
    use std::fs::OpenOptions;
    use std::io::{ErrorKind, Write as _};
    use std::os::unix::fs::OpenOptionsExt as _;

    #[derive(Serialize)]
    struct Generated<'a> {
        fingerprint: &'a str,
        public_key: &'a str,
    }

    let key = match keys::generate_key() {
        Ok(key) => key,
        Err(e) => return finish_error(&e),
    };
    let path = out_dir.join(format!("{}.pk8", key.fingerprint));
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .and_then(|mut file| {
            file.write_all(&key.pkcs8).inspect_err(|_| {
                // A truncated key must not pass for one.
                let _ = std::fs::remove_file(&path);
            })
        });
    if let Err(e) = written {
        let reason = if e.kind() == ErrorKind::AlreadyExists {
            "key_exists"
        } else {
            "output_unwritable"
        };
        let detail = format!("write {}: {e}", path.display());
        return finish_error(&CardError {
            result: CardResult::InputError,
            reason,
            detail,
            field: None,
        });
    }
    println!(
        "{}",
        serde_json::to_string(&Generated {
            fingerprint: &key.fingerprint,
            public_key: &key.public_key,
        })
        .expect("plain struct serialises")
    );
    0
}
