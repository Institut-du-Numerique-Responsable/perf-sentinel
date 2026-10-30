//! A card's integrity: the signature over its payload, then the payload's
//! own rules. A bad signature is a tamper whatever the bytes say; a valid
//! signature over a payload that breaks the rules is a malformed input.

use serde_json::Value;

use super::keys::{
    b64url_decode, fingerprint_of, parse_compact_url, parse_file_envelope, verify_signature,
};
use super::payload::{MS_PER_DAY, parse_compact_payload, parse_full_payload};
use super::pdf::read_pdf_envelope;
use super::raster::read_png_envelope;
use super::{CardError, CardResult};

/// What `verify-card` reads: a card file, or the verify URL of its QR code.
#[derive(Debug, Clone, Copy)]
pub enum CardSource<'a> {
    Png(&'a [u8]),
    Pdf(&'a [u8]),
    Url(&'a str),
}

/// The integrity verdict. `ok` is the signature holding over a payload
/// whose `key` is its public key's fingerprint; the other fields are set
/// only then. `reason` is `signature_valid`, `signature_invalid` or
/// `fingerprint_mismatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integrity {
    pub ok: bool,
    pub domain: Option<String>,
    pub fingerprint: Option<String>,
    pub public_key: Option<String>,
    /// The epoch day the card was generated.
    pub signed_day: Option<u32>,
    pub reason: &'static str,
}

/// Check a card: the full payload of a PNG or PDF, the compact payload of
/// a verify URL.
///
/// # Errors
///
/// The input errors of the envelope readers (`file_too_large`,
/// `no_card_payload`, `duplicate_card_payload`, `malformed_input`),
/// `malformed_input` for a compact payload in a full envelope or a payload
/// that breaks a rule under a valid signature, and `unsupported_version`. A
/// payload with no usable public key is a tamper, not an error.
pub fn check_integrity(source: CardSource<'_>) -> Result<Integrity, CardError> {
    match source {
        CardSource::Png(png) => check_full(&read_png_envelope(png)?),
        CardSource::Pdf(pdf) => check_full(&read_pdf_envelope(pdf)?),
        CardSource::Url(url) => check_compact(url),
    }
}

fn check_full(envelope: &str) -> Result<Integrity, CardError> {
    let (payload, signature) = parse_file_envelope(envelope)?;
    let Ok(value) = serde_json::from_slice::<Value>(&payload) else {
        return Ok(unreadable_payload());
    };
    let Some((public_key, raw_key)) = named_key(&value, "public_key") else {
        // A compact payload under a full envelope is the wrong card, not an edit.
        return if parse_compact_payload(&payload).is_ok() {
            Err(malformed("a compact payload is not a full card"))
        } else {
            Ok(unreadable_payload())
        };
    };
    if !verify_signature(&raw_key, &payload, &signature) {
        return Ok(tampered("signature_invalid"));
    }
    let fingerprint = fingerprint_of(&raw_key);
    if let Some(key) = value.get("key").and_then(Value::as_str)
        && key != fingerprint
    {
        return Ok(tampered("fingerprint_mismatch"));
    }
    let full = parse_full_payload(&payload).map_err(signed_rules_error)?;
    let signed_day = u32::try_from(full.generated_at_ms / MS_PER_DAY)
        .map_err(|_| malformed("generated_at_ms is out of range"))?;
    Ok(verified(
        full.org.domain,
        fingerprint,
        public_key,
        signed_day,
    ))
}

fn check_compact(url: &str) -> Result<Integrity, CardError> {
    let (payload, signature) = parse_compact_url(url)?;
    let Ok(value) = serde_json::from_slice::<Value>(&payload) else {
        return Ok(unreadable_payload());
    };
    let Some((public_key, raw_key)) = named_key(&value, "k") else {
        return Ok(unreadable_payload());
    };
    if !verify_signature(&raw_key, &payload, &signature) {
        return Ok(tampered("signature_invalid"));
    }
    let compact = parse_compact_payload(&payload).map_err(signed_rules_error)?;
    Ok(verified(
        compact.d,
        fingerprint_of(&raw_key),
        public_key,
        compact.g,
    ))
}

/// The base64url public key under `field` and its 32 raw bytes.
fn named_key(value: &Value, field: &str) -> Option<(String, Vec<u8>)> {
    let text = value.get(field)?.as_str()?;
    let raw = b64url_decode(text).filter(|bytes| bytes.len() == 32)?;
    Some((text.to_owned(), raw))
}

/// Every rule a payload breaks reads as a malformed input, but for a
/// format this binary does not read.
fn signed_rules_error(error: CardError) -> CardError {
    if error.reason == "unsupported_version" || error.reason == "malformed_input" {
        error
    } else {
        malformed(format!("{}: {}", error.reason, error.detail))
    }
}

fn verified(
    domain: Option<String>,
    fingerprint: String,
    public_key: String,
    signed_day: u32,
) -> Integrity {
    Integrity {
        ok: true,
        domain,
        fingerprint: Some(fingerprint),
        public_key: Some(public_key),
        signed_day: Some(signed_day),
        reason: "signature_valid",
    }
}

fn tampered(reason: &'static str) -> Integrity {
    Integrity {
        ok: false,
        domain: None,
        fingerprint: None,
        public_key: None,
        signed_day: None,
        reason,
    }
}

/// Bytes with no usable public key (not JSON, or the key field gone or
/// broken) cannot be what the signer signed: a card edited (one flipped
/// character is enough) reads as tampered, not as a bad input, even though
/// no key is left to check the signature with.
fn unreadable_payload() -> Integrity {
    tampered("signature_invalid")
}

fn malformed(detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::InputError, "malformed_input", detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::FindingType;
    use crate::report::card::keys::{
        CardSigner, b64url_encode, compact_url, file_envelope, generate_key,
    };
    use crate::report::card::payload::{build_payloads, canonical_bytes, parse_card_input};
    use crate::report::card::raster::encode_png;
    use crate::report::card::test_fixtures::story_input;
    use crate::report::card::{CardFormat, render_card};
    use serde_json::json;

    fn story_bytes() -> Vec<u8> {
        serde_json::to_vec(&story_input(FindingType::NPlusOneSql)).expect("json")
    }

    /// A 1x1 PNG carrying `envelope`: the pixels play no part in the check.
    fn png_with(envelope: &str) -> Vec<u8> {
        let pixmap = tiny_skia::Pixmap::new(1, 1).expect("pixmap");
        encode_png(&pixmap, envelope).expect("png")
    }

    /// The full and compact canonical bytes of `input` under `signer`,
    /// with `key` as the full payload's fingerprint.
    fn canonical(input: &serde_json::Value, signer: &CardSigner, key: &str) -> (Vec<u8>, Vec<u8>) {
        let input = parse_card_input(&serde_json::to_vec(input).expect("json")).expect("input");
        let (full, compact) = build_payloads(&input, &signer.public_key_b64(), key).expect("build");
        (canonical_bytes(&full), canonical_bytes(&compact))
    }

    fn signer() -> CardSigner {
        CardSigner::from_pkcs8(&generate_key().expect("key").pkcs8).expect("signer")
    }

    fn assert_ok(integrity: &Integrity, fingerprint: &str) {
        assert!(integrity.ok, "{integrity:?}");
        assert_eq!(integrity.reason, "signature_valid");
        assert_eq!(integrity.domain.as_deref(), Some("acme.fr"));
        assert_eq!(integrity.fingerprint.as_deref(), Some(fingerprint));
        // 2026-10-01 12:00 UTC.
        assert_eq!(integrity.signed_day, Some(20727));
    }

    #[test]
    fn a_rendered_png_verifies() {
        let key = generate_key().expect("key");
        let card = render_card(&story_bytes(), &key.pkcs8, CardFormat::Png).expect("renders");
        let integrity = check_integrity(CardSource::Png(&card.bytes)).expect("checks");
        assert_ok(&integrity, &key.fingerprint);
        assert_eq!(
            integrity.public_key.as_deref(),
            Some(key.public_key.as_str())
        );
    }

    #[test]
    fn a_rendered_pdf_verifies() {
        let key = generate_key().expect("key");
        let card = render_card(&story_bytes(), &key.pkcs8, CardFormat::Pdf).expect("renders");
        assert_ok(
            &check_integrity(CardSource::Pdf(&card.bytes)).expect("checks"),
            &key.fingerprint,
        );
    }

    #[test]
    fn its_compact_url_verifies() {
        let key = generate_key().expect("key");
        let card = render_card(&story_bytes(), &key.pkcs8, CardFormat::Png).expect("renders");
        assert_ok(
            &check_integrity(CardSource::Url(&card.compact_url)).expect("checks"),
            &key.fingerprint,
        );
    }

    #[test]
    fn one_changed_payload_byte_is_tampered() {
        let signer = signer();
        let (full, compact) = canonical(
            &story_input(FindingType::NPlusOneSql),
            &signer,
            &signer.fingerprint(),
        );
        let changed = String::from_utf8(full.clone()).expect("utf-8").replacen(
            "\"cited_ops\":41",
            "\"cited_ops\":42",
            1,
        );
        assert_ne!(changed.as_bytes(), full.as_slice());
        let png = png_with(&file_envelope(changed.as_bytes(), &signer.sign_b64(&full)));
        let integrity = check_integrity(CardSource::Png(&png)).expect("checks");
        assert_eq!(integrity, tampered("signature_invalid"));

        let changed = String::from_utf8(compact.clone())
            .expect("utf-8")
            .replacen("\"o\":41", "\"o\":42", 1);
        assert_ne!(changed.as_bytes(), compact.as_slice());
        let url = compact_url(changed.as_bytes(), &signer.sign_b64(&compact));
        let integrity = check_integrity(CardSource::Url(&url)).expect("checks");
        assert_eq!(integrity, tampered("signature_invalid"));
    }

    #[test]
    fn a_compact_signature_does_not_pass_as_a_full_payload() {
        let signer = signer();
        let (_, compact) = canonical(
            &story_input(FindingType::NPlusOneSql),
            &signer,
            &signer.fingerprint(),
        );
        let png = png_with(&file_envelope(&compact, &signer.sign_b64(&compact)));
        let error = check_integrity(CardSource::Png(&png)).expect_err("refused");
        assert_eq!(error.reason, "malformed_input");
        assert_eq!(error.result, CardResult::InputError);
    }

    #[test]
    fn a_payload_whose_key_is_not_its_public_key_is_tampered() {
        let signer = signer();
        let (full, _) = canonical(
            &story_input(FindingType::NPlusOneSql),
            &signer,
            "ps1:AAAAAAAAAAAAAAAAAAAAAA",
        );
        let png = png_with(&file_envelope(&full, &signer.sign_b64(&full)));
        let integrity = check_integrity(CardSource::Png(&png)).expect("checks");
        assert_eq!(integrity, tampered("fingerprint_mismatch"));
    }

    /// The canonical bytes of `payload` after `edit` changes its JSON.
    fn resigned(payload: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let mut value: serde_json::Value = serde_json::from_slice(payload).expect("json");
        edit(&mut value);
        canonical_bytes(&value)
    }

    #[test]
    fn a_newer_format_is_unsupported_and_a_broken_rule_malformed() {
        let signer = signer();
        let (full, compact) = canonical(
            &story_input(FindingType::NPlusOneSql),
            &signer,
            &signer.fingerprint(),
        );
        let v2 = resigned(&full, |v| v["v"] = json!(2));
        let png = png_with(&file_envelope(&v2, &signer.sign_b64(&v2)));
        let error = check_integrity(CardSource::Png(&png)).expect_err("refused");
        assert_eq!(error.reason, "unsupported_version");
        assert_eq!(error.result.exit_code(), 5);

        let v2 = resigned(&compact, |v| v["v"] = json!(2));
        let url = compact_url(&v2, &signer.sign_b64(&v2));
        let error = check_integrity(CardSource::Url(&url)).expect_err("refused");
        assert_eq!(error.reason, "unsupported_version");

        // A refusal under a valid signature: a slow finding with a volume.
        let slow = resigned(&full, |v| {
            v["story"]["finding_type"] = json!("slow_sql");
        });
        let png = png_with(&file_envelope(&slow, &signer.sign_b64(&slow)));
        let error = check_integrity(CardSource::Png(&png)).expect_err("refused");
        assert_eq!(error.reason, "malformed_input");
        assert_eq!(error.result, CardResult::InputError);

        // A public key that is not 32 bytes leaves nothing to verify with:
        // the signature cannot hold, so the card reads as tampered.
        let short = resigned(&full, |v| {
            v["public_key"] = json!(b64url_encode(&[7u8; 31]));
        });
        let png = png_with(&file_envelope(&short, &signer.sign_b64(&short)));
        assert_eq!(
            check_integrity(CardSource::Png(&png)).expect("checks"),
            tampered("signature_invalid")
        );
    }

    #[test]
    fn a_signed_equivalence_the_table_lacks_still_verifies() {
        let signer = signer();
        let (full, _) = canonical(
            &story_input(FindingType::NPlusOneSql),
            &signer,
            &signer.fingerprint(),
        );
        let retired = resigned(&full, |v| {
            v["equivalence"]["id"] = json!("a-row-a-later-table-dropped");
            v["equivalence"]["vintage"] = json!("Base Carbone v1.0");
        });
        let png = png_with(&file_envelope(&retired, &signer.sign_b64(&retired)));
        assert_ok(
            &check_integrity(CardSource::Png(&png)).expect("checks"),
            &signer.fingerprint(),
        );
    }

    /// A payload character changed (here: A and B swapped), the signature
    /// kept, is a tamper wherever the edit lands, the key field included.
    #[test]
    fn a_flipped_payload_character_is_tampered_not_malformed() {
        let signer = signer();
        let (full, compact) = canonical(
            &story_input(FindingType::NPlusOneSql),
            &signer,
            &signer.fingerprint(),
        );
        let flip = |text: &str, at: usize| {
            let mut bytes = text.as_bytes().to_vec();
            bytes[at] = if bytes[at] == b'A' { b'B' } else { b'A' };
            String::from_utf8(bytes).expect("ascii")
        };
        let prefix = compact_url(&[], "")
            .split_once("#c=")
            .expect("fragment")
            .0
            .to_owned()
            + "#c=";
        let full_text = b64url_encode(&full);
        let compact_text = b64url_encode(&compact);
        for at in 0..full_text.len() {
            let envelope =
                json!({"payload": flip(&full_text, at), "signature": signer.sign_b64(&full)})
                    .to_string();
            let png = png_with(&envelope);
            match check_integrity(CardSource::Png(&png)) {
                Ok(integrity) => assert!(!integrity.ok, "flip {at} verified"),
                Err(error) => panic!("flip {at} read as {}", error.reason),
            }
        }
        for at in 0..compact_text.len() {
            let url = format!(
                "{prefix}{}.{}",
                flip(&compact_text, at),
                signer.sign_b64(&compact)
            );
            match check_integrity(CardSource::Url(&url)) {
                Ok(integrity) => assert!(!integrity.ok, "flip {at} verified"),
                Err(error) => panic!("flip {at} read as {}", error.reason),
            }
        }
    }
}
