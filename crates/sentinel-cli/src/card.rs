//! `perf-sentinel card`: the exit codes and the JSON line are a contract
//! the Hub reads, never its stderr.

use std::path::Path;

use sentinel_core::report::card::{CardError, CardFormat, CardResult, RenderedCard, render_card};
use serde::Serialize;

use crate::limits::{CappedRead, read_capped};

/// The largest `card.json` read: the Hub's are a few hundred bytes.
const MAX_INPUT_BYTES: u64 = 64 * 1024;
/// The largest signing key read: a PKCS#8 v2 Ed25519 key is 83 bytes.
const MAX_KEY_BYTES: u64 = 1024;

#[derive(Serialize)]
struct ErrorLine {
    result: &'static str,
    reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<&'static str>,
}

pub(crate) fn input_error(reason: &'static str, detail: String) -> CardError {
    CardError {
        result: CardResult::InputError,
        reason,
        detail,
        field: None,
    }
}

/// Print the error line and return its exit code.
pub(crate) fn finish_error(error: &CardError) -> i32 {
    eprintln!("Error: {error}");
    let line = ErrorLine {
        result: error.result.as_str(),
        reason: error.reason,
        field: error.field,
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
pub fn cmd_card_keygen(out_dir: &Path) -> i32 {
    use sentinel_core::report::card::keys;
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
        return finish_error(&input_error(
            reason,
            format!("write {}: {e}", path.display()),
        ));
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

/// What `card render` reads and where it writes.
pub struct RenderArgs<'a> {
    pub input: &'a Path,
    pub signing_key: &'a Path,
    pub out_dir: &'a Path,
    pub format: CardFormat,
}

#[derive(Serialize)]
struct Rendered<'a> {
    result: &'static str,
    files: [&'static str; 1],
    fingerprint: &'a str,
    compact_url: &'a str,
}

/// Render and sign the card of `--input` into `<out_dir>/card.png` or
/// `card.pdf`. Nothing is written unless the card renders.
pub fn cmd_card_render(args: &RenderArgs<'_>) -> i32 {
    let card = match render(args) {
        Ok(card) => card,
        Err(e) => return finish_error(&e),
    };
    let path = args.out_dir.join(card.file_name);
    if let Err(e) = crate::write_file_no_follow(&path, &card.bytes) {
        return finish_error(&input_error(
            "output_unwritable",
            format!("write {}: {e}", path.display()),
        ));
    }
    println!(
        "{}",
        serde_json::to_string(&Rendered {
            result: "rendered",
            files: [card.file_name],
            fingerprint: &card.fingerprint,
            compact_url: &card.compact_url,
        })
        .expect("plain struct serialises")
    );
    0
}

fn render(args: &RenderArgs<'_>) -> Result<RenderedCard, CardError> {
    let input = read_file(args.input, MAX_INPUT_BYTES)?;
    let key = read_signing_key(args.signing_key)?;
    render_card(&input, &key, args.format)
}

/// `path` whole, `max` bytes at most.
pub(crate) fn read_file(path: &Path, max: u64) -> Result<Vec<u8>, CardError> {
    match read_capped(path, max) {
        Ok(CappedRead::Bytes(bytes)) => Ok(bytes),
        Ok(CappedRead::TooLarge(len)) => Err(input_error(
            "file_too_large",
            format!(
                "{} is {len} bytes, exceeds the {max}-byte cap",
                path.display()
            ),
        )),
        Err(e) => Err(input_error(
            "missing_file",
            format!("read {}: {e}", path.display()),
        )),
    }
}

/// The key file, never through a symlink and, on Unix, readable by its
/// owner only. The detail names the path, never the bytes.
fn read_signing_key(path: &Path) -> Result<Vec<u8>, CardError> {
    use std::io::Read as _;

    let unreadable = |detail: String| {
        input_error(
            "signing_key_unreadable",
            format!("{}: {detail}", path.display()),
        )
    };
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|e| unreadable(e.to_string()))?;
    let meta = file.metadata().map_err(|e| unreadable(e.to_string()))?;
    if !meta.is_file() {
        return Err(unreadable("not a regular file".to_owned()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(input_error(
                "signing_key_permissions",
                format!(
                    "{} has mode {mode:o}, others can read it; chmod 600 it",
                    path.display()
                ),
            ));
        }
    }
    let mut bytes = Vec::new();
    file.take(MAX_KEY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| unreadable(e.to_string()))?;
    if bytes.len() as u64 > MAX_KEY_BYTES {
        return Err(unreadable(format!("over {MAX_KEY_BYTES} bytes")));
    }
    Ok(bytes)
}
