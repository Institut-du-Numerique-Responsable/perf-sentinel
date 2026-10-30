//! `perf-sentinel verify-card`: the JSON line and the exit code are the
//! contract (C2), stderr carries the detail.

use std::path::Path;

use sentinel_core::report::card::CardError;
use sentinel_core::report::card::binding::{
    Binding, DEFAULT_DOH_URL, KeyStatus, PublishedKey, judge, parse_key_list,
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
/// bind its key through `--keys`, or through `--online` the key list its
/// domain publishes.
pub async fn cmd_verify_card(
    target: &str,
    keys: Option<&Path>,
    online: bool,
    doh: Option<&str>,
) -> i32 {
    let doh = doh.unwrap_or(DEFAULT_DOH_URL);
    if online && !doh.starts_with("https://") {
        return finish_error(&input_error(
            "malformed_input",
            format!("--doh {doh} is not an https:// URL"),
        ));
    }
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
            let published = if online {
                match read_online(&integrity, doh).await {
                    Ok(published) => published,
                    Err(code) => return code,
                }
            } else {
                published
            };
            judge(fingerprint, public_key, day, published.as_deref())
        }
        _ => unbound(integrity.reason),
    };
    let untrusted = !integrity.ok
        || matches!(
            binding.reason,
            "key_revoked" | "signed_outside_validity" | "public_key_mismatch"
        );
    print_line(&integrity, binding, i32::from(untrusted))
}

/// The key list the card's domain publishes, or the exit code of the line
/// already printed: an input error, or level 1 when it cannot be read.
async fn read_online(integrity: &Integrity, doh: &str) -> Result<Option<Vec<PublishedKey>>, i32> {
    let Some(domain) = integrity.domain.as_deref() else {
        return Err(finish_error(&input_error(
            "no_domain",
            "the card names no domain to read a key list from".to_owned(),
        )));
    };
    match fetch_published(domain, doh).await {
        Ok(Ok(published)) => Ok(published),
        Ok(Err(reason)) => {
            eprintln!("Error: the key list of {domain} could not be read ({reason})");
            Err(print_line(integrity, unbound(reason), 4))
        }
        Err(e) => Err(finish_error(&e)),
    }
}

/// An input error, or the merged lookups: a key list, or the reason none
/// could be read.
type Fetched = Result<Result<Option<Vec<PublishedKey>>, &'static str>, CardError>;

/// Ask the DNS-over-HTTPS service `doh` for the domain's TXT record and
/// fetch its well-known file, both at once.
#[cfg(any(feature = "daemon", feature = "tempo", feature = "jaeger-query"))]
async fn fetch_published(domain: &str, doh: &str) -> Fetched {
    use sentinel_core::http_client::{Uri, build_client};
    use sentinel_core::report::card::binding::merge_lookups;
    use sentinel_core::report::card::online::{lookup_txt, lookup_well_known};

    let doh: Uri = doh
        .parse()
        .map_err(|e| input_error("malformed_input", format!("--doh {doh}: {e}")))?;
    let well_known = well_known_url(domain)?;
    let client = build_client();
    let (dns, listed) = tokio::join!(
        lookup_txt(&client, &doh, domain),
        lookup_well_known(&client, &well_known)
    );
    Ok(merge_lookups(dns, listed))
}

/// A build without the `daemon`, `tempo` or `jaeger-query` feature has no
/// HTTP client, so `--online` cannot read anything.
#[cfg(not(any(feature = "daemon", feature = "tempo", feature = "jaeger-query")))]
fn fetch_published(_domain: &str, _doh: &str) -> std::future::Ready<Fetched> {
    std::future::ready(Ok(Err("no_http_client")))
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

/// The domain's `/.well-known/perf-sentinel-key`, refused unless its
/// authority is exactly the domain: no userinfo, no port.
#[cfg(any(feature = "daemon", feature = "tempo", feature = "jaeger-query"))]
fn well_known_url(domain: &str) -> Result<sentinel_core::http_client::Uri, CardError> {
    let refused = || {
        input_error(
            "malformed_input",
            format!("{domain:?} does not make a well-known URL on that host alone"),
        )
    };
    let uri: sentinel_core::http_client::Uri =
        format!("https://{domain}/.well-known/perf-sentinel-key")
            .parse()
            .map_err(|_| refused())?;
    // The host is the domain, and the authority is that host alone: no
    // userinfo, no port.
    if uri.host() == Some(domain) && uri.authority().is_some_and(|a| a.as_str() == domain) {
        Ok(uri)
    } else {
        Err(refused())
    }
}

#[cfg(all(
    test,
    any(feature = "daemon", feature = "tempo", feature = "jaeger-query")
))]
mod tests {
    use super::*;

    #[test]
    fn the_well_known_url_is_on_the_card_domain() {
        let uri = well_known_url("acme.fr").expect("a url");
        assert_eq!(uri.scheme_str(), Some("https"));
        assert_eq!(uri.host(), Some("acme.fr"));
        assert_eq!(uri.port(), None);
        assert_eq!(uri.path(), "/.well-known/perf-sentinel-key");
        for bad in [
            "evil.example@acme.fr",
            "acme.fr:8443",
            "acme.fr/x",
            "acme.fr?x",
        ] {
            let error = well_known_url(bad).expect_err(bad);
            assert_eq!(error.reason, "malformed_input", "{bad}");
        }
    }
}
