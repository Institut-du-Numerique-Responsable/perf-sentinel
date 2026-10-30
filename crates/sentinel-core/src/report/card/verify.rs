//! A card's integrity: the signature over its payload, then, for a file,
//! the signature over its content, then the payload's own rules. A bad
//! signature is a tamper whatever the bytes say; a valid signature over a
//! payload that breaks the rules is a malformed input.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use super::content::{card_content, content_message};
use super::jxl::read_jxl_envelope;
use super::keys::{
    FileEnvelope, b64url_decode, b64url_encode, fingerprint_of, parse_compact_url,
    parse_file_envelope, verify_signature,
};
use super::payload::{
    ANGLE_LETTERS, Angle, CompactPayload, DayRange, KIND_LETTERS, MS_PER_DAY, Period, compact_of,
    parse_compact_payload, parse_full_payload,
};
use super::pdf::read_pdf_envelope;
use super::raster::read_png_envelope;
use super::svg::read_svg_envelope;
use super::{CardError, CardFormat, CardResult};
use crate::time::epoch_day_to_iso_date;

/// What `verify-card` reads: a card file, or the verify URL of its QR code.
#[derive(Debug, Clone, Copy)]
pub enum CardSource<'a> {
    Png(&'a [u8]),
    Pdf(&'a [u8]),
    Svg(&'a [u8]),
    Jxl(&'a [u8]),
    Url(&'a str),
}

/// Whether a card file is the one its key signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentVerdict {
    /// The content signature holds over the file.
    Intact,
    /// The content signature does not hold, or the PDF slot is not as written.
    Modified,
    /// A verify URL has no file, or the check stopped before the content.
    NotChecked,
}

/// The card's big figures as signed, so a reader compares them with the
/// image: the compact payload's fields under their full names, unrounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignedFigures {
    pub angle: Angle,
    pub period: Period,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_fix_period: Option<DayRange>,
    #[serde(flatten)]
    pub values: BTreeMap<&'static str, u64>,
}

/// The compact `f` letters and the names the verify page gives them.
const FIGURE_NAMES: [(&str, &str); 9] = [
    ("o", "cited_ops"),
    ("m", "cited_ms"),
    ("b", "before_fix_ops"),
    ("t", "threshold_ops"),
    ("u", "threshold_ms"),
    ("w", "last_waste_permille"),
    ("x", "previous_waste_permille"),
    ("n", "confirmed_fixes_count"),
    ("e", "services_count"),
];

/// The figures a compact payload signs.
///
/// # Panics
///
/// On an angle or period letter outside the tables, which a payload that
/// passed `parse_compact_payload` or came from `compact_of` never holds.
#[must_use]
pub fn signed_figures(compact: &CompactPayload) -> SignedFigures {
    let angle = ANGLE_LETTERS
        .iter()
        .find(|(_, letter)| *letter == compact.a)
        .expect("a checked angle letter")
        .0;
    let (kind, from, to) = &compact.p;
    let kind = KIND_LETTERS
        .iter()
        .find(|(_, letter)| letter == kind)
        .expect("a checked period letter")
        .0;
    SignedFigures {
        angle,
        period: Period {
            kind,
            from: epoch_day_to_iso_date(*from),
            to: epoch_day_to_iso_date(*to),
        },
        before_fix_period: compact.r.map(|(from, to)| DayRange {
            from: epoch_day_to_iso_date(from),
            to: epoch_day_to_iso_date(to),
        }),
        values: compact
            .f
            .iter()
            .filter_map(|(letter, value)| {
                let (_, name) = FIGURE_NAMES.iter().find(|(l, _)| l == letter)?;
                Some((*name, *value))
            })
            .collect(),
    }
}

/// The integrity verdict. `ok` is the payload's signature holding over a
/// payload whose `key` is its public key's fingerprint. `reason` is
/// `signature_valid`, `signature_invalid`, `fingerprint_mismatch` or
/// `content_modified`; the key, domain, day and figures are set only on
/// `signature_valid`, so nothing is left to judge on a modified file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integrity {
    pub ok: bool,
    pub content: ContentVerdict,
    pub domain: Option<String>,
    pub fingerprint: Option<String>,
    pub public_key: Option<String>,
    /// The epoch day the card was generated.
    pub signed_day: Option<u32>,
    pub figures: Option<SignedFigures>,
    pub reason: &'static str,
}

/// Check a card: the full payload and the content of a PNG, PDF, SVG or
/// JPEG XL, the compact payload of a verify URL.
///
/// # Errors
///
/// The input errors of the envelope readers and the carrier rules
/// (`file_too_large`, `no_card_payload`, `duplicate_card_payload`,
/// `malformed_input`), `malformed_input` for a compact payload in a full
/// envelope or a payload that breaks a rule under a valid signature, and
/// `unsupported_version`. A payload with no usable public key is a tamper,
/// not an error.
pub fn check_integrity(source: CardSource<'_>) -> Result<Integrity, CardError> {
    match source {
        CardSource::Png(png) => check_file(CardFormat::Png, png, &read_png_envelope(png)?),
        CardSource::Pdf(pdf) => check_file(CardFormat::Pdf, pdf, &read_pdf_envelope(pdf)?),
        CardSource::Svg(svg) => check_file(CardFormat::Svg, svg, &read_svg_envelope(svg)?),
        CardSource::Jxl(jxl) => check_file(CardFormat::Jxl, jxl, &read_jxl_envelope(jxl)?),
        CardSource::Url(url) => check_compact(url),
    }
}

/// A full payload's verified key: its base64url text, raw bytes and
/// fingerprint, or the verdict that stopped the check.
type SignedKey = Result<(String, Vec<u8>, String), Integrity>;

/// The order of R7, extended: the envelope and its carrier, the payload
/// signature, the fingerprint, the content signature, the payload's rules.
fn check_file(format: CardFormat, file: &[u8], text: &str) -> Result<Integrity, CardError> {
    let envelope = parse_file_envelope(text)?;
    let content = card_content(format, file, &b64url_encode(&envelope.content_sig))?;
    let (public_key, raw_key, fingerprint) = match signed_full_key(&envelope)? {
        Ok(key) => key,
        Err(verdict) => return Ok(verdict),
    };
    let intact = content.is_some_and(|content| {
        let message = content_message(format, &envelope.payload, &content);
        verify_signature(&raw_key, &message, &envelope.content_sig)
    });
    if !intact {
        return Ok(Integrity {
            ok: true,
            content: ContentVerdict::Modified,
            ..tampered("content_modified")
        });
    }
    let full = parse_full_payload(&envelope.payload).map_err(signed_rules_error)?;
    let signed_day = u32::try_from(full.generated_at_ms / MS_PER_DAY)
        .map_err(|_| malformed("generated_at_ms is out of range"))?;
    let figures = signed_figures(&compact_of(&full).map_err(signed_rules_error)?);
    Ok(verified(
        ContentVerdict::Intact,
        full.org.domain,
        (fingerprint, public_key, signed_day),
        figures,
    ))
}

/// The payload's signature and fingerprint checks.
fn signed_full_key(envelope: &FileEnvelope) -> Result<SignedKey, CardError> {
    let payload = &envelope.payload;
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return Ok(Err(unreadable_payload()));
    };
    let Some((public_key, raw_key)) = named_key(&value, "public_key") else {
        // A compact payload under a full envelope is the wrong card, not an edit.
        return if parse_compact_payload(payload).is_ok() {
            Err(malformed("a compact payload is not a full card"))
        } else {
            Ok(Err(unreadable_payload()))
        };
    };
    if !verify_signature(&raw_key, payload, &envelope.signature) {
        return Ok(Err(tampered("signature_invalid")));
    }
    let fingerprint = fingerprint_of(&raw_key);
    if let Some(key) = value.get("key").and_then(Value::as_str)
        && key != fingerprint
    {
        return Ok(Err(tampered("fingerprint_mismatch")));
    }
    Ok(Ok((public_key, raw_key, fingerprint)))
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
    let figures = signed_figures(&compact);
    Ok(verified(
        ContentVerdict::NotChecked,
        compact.d,
        (fingerprint_of(&raw_key), public_key, compact.g),
        figures,
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

/// A verified card: its fingerprint, public key and signing day, the
/// figures it signs.
fn verified(
    content: ContentVerdict,
    domain: Option<String>,
    (fingerprint, public_key, signed_day): (String, String, u32),
    figures: SignedFigures,
) -> Integrity {
    Integrity {
        ok: true,
        content,
        domain,
        fingerprint: Some(fingerprint),
        public_key: Some(public_key),
        signed_day: Some(signed_day),
        figures: Some(figures),
        reason: "signature_valid",
    }
}

fn tampered(reason: &'static str) -> Integrity {
    Integrity {
        ok: false,
        content: ContentVerdict::NotChecked,
        domain: None,
        fingerprint: None,
        public_key: None,
        signed_day: None,
        figures: None,
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
    use crate::report::card::content::{CONTENT_SIG_PLACEHOLDER, seal_file};
    use crate::report::card::keys::{
        CardSigner, b64url_encode, compact_url, file_envelope, generate_key,
    };
    use crate::report::card::layout::layout_image;
    use crate::report::card::payload::{build_payloads, canonical_bytes, parse_card_input};
    use crate::report::card::raster::encode_png;
    use crate::report::card::svg::render_svg;
    use crate::report::card::test_fixtures::{
        collective_input, compact_url_of, curve_input, full_payload, story_input,
        story_input_with_threshold,
    };
    use crate::report::card::{CardFormat, render_card};
    use serde_json::json;

    fn story_bytes() -> Vec<u8> {
        serde_json::to_vec(&story_input(FindingType::NPlusOneSql)).expect("json")
    }

    /// A 1x1 PNG carrying `envelope`, for a check that stops before the
    /// content step.
    fn png_with(envelope: &str) -> Vec<u8> {
        let pixmap = tiny_skia::Pixmap::new(1, 1).expect("pixmap");
        encode_png(&pixmap, envelope).expect("png")
    }

    /// The placeholder envelope of `payload` under `signature_b64`.
    fn unsealed(payload: &[u8], signature_b64: &str) -> String {
        file_envelope(payload, signature_b64, CONTENT_SIG_PLACEHOLDER)
    }

    /// A 1x1 PNG of `payload`, signed and sealed by `signer` as a render
    /// would, for a check that reaches the payload's rules.
    fn sealed_png(payload: &[u8], signer: &CardSigner) -> Vec<u8> {
        let pixmap = tiny_skia::Pixmap::new(1, 1).expect("pixmap");
        seal_file(
            CardFormat::Png,
            payload,
            &signer.sign_b64(payload),
            signer,
            |envelope| encode_png(&pixmap, envelope),
        )
        .expect("sealed")
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

    fn assert_ok(integrity: &Integrity, fingerprint: &str, content: ContentVerdict) {
        assert!(integrity.ok, "{integrity:?}");
        assert_eq!(integrity.reason, "signature_valid");
        assert_eq!(integrity.content, content);
        assert!(integrity.figures.is_some());
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
        assert_ok(&integrity, &key.fingerprint, ContentVerdict::Intact);
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
            ContentVerdict::Intact,
        );
    }

    #[test]
    fn its_compact_url_verifies() {
        let key = generate_key().expect("key");
        let card = render_card(&story_bytes(), &key.pkcs8, CardFormat::Png).expect("renders");
        assert_ok(
            &check_integrity(CardSource::Url(&card.compact_url)).expect("checks"),
            &key.fingerprint,
            ContentVerdict::NotChecked,
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
        let png = png_with(&unsealed(changed.as_bytes(), &signer.sign_b64(&full)));
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
        let png = png_with(&unsealed(&compact, &signer.sign_b64(&compact)));
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
        let png = png_with(&unsealed(&full, &signer.sign_b64(&full)));
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
        let png = sealed_png(&v2, &signer);
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
        let png = sealed_png(&slow, &signer);
        let error = check_integrity(CardSource::Png(&png)).expect_err("refused");
        assert_eq!(error.reason, "malformed_input");
        assert_eq!(error.result, CardResult::InputError);

        // A public key that is not 32 bytes leaves nothing to verify with:
        // the signature cannot hold, so the card reads as tampered.
        let short = resigned(&full, |v| {
            v["public_key"] = json!(b64url_encode(&[7u8; 31]));
        });
        let png = png_with(&unsealed(&short, &signer.sign_b64(&short)));
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
        let png = sealed_png(&retired, &signer);
        assert_ok(
            &check_integrity(CardSource::Png(&png)).expect("checks"),
            &signer.fingerprint(),
            ContentVerdict::Intact,
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
            let envelope = format!(
                r#"{{"content_sig":"{CONTENT_SIG_PLACEHOLDER}","payload":"{}","signature":"{}"}}"#,
                flip(&full_text, at),
                signer.sign_b64(&full)
            );
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

    #[test]
    fn a_rendered_svg_verifies() {
        let key = generate_key().expect("key");
        let card = render_card(&story_bytes(), &key.pkcs8, CardFormat::Svg).expect("renders");
        assert_ok(
            &check_integrity(CardSource::Svg(&card.bytes)).expect("checks"),
            &key.fingerprint,
            ContentVerdict::Intact,
        );
    }

    #[test]
    fn one_changed_payload_byte_in_the_svg_is_tampered() {
        let signer = signer();
        let input = story_input(FindingType::NPlusOneSql);
        let (full, _) = canonical(&input, &signer, &signer.fingerprint());
        let changed = String::from_utf8(full.clone()).expect("utf-8").replacen(
            "\"cited_ops\":41",
            "\"cited_ops\":42",
            1,
        );
        assert_ne!(changed.as_bytes(), full.as_slice());
        let payload = full_payload(&input);
        let ops = layout_image(&payload, &compact_url_of(&input)).expect("lays out");
        let envelope = unsealed(changed.as_bytes(), &signer.sign_b64(&full));
        let svg = render_svg(&ops, &envelope, "Perf Sentinel card", payload.lang).expect("svg");
        let integrity = check_integrity(CardSource::Svg(&svg)).expect("checks");
        assert_eq!(integrity, tampered("signature_invalid"));
    }

    #[test]
    fn a_rendered_jxl_verifies() {
        let key = generate_key().expect("key");
        let card = render_card(&story_bytes(), &key.pkcs8, CardFormat::Jxl).expect("renders");
        assert_ok(
            &check_integrity(CardSource::Jxl(&card.bytes)).expect("checks"),
            &key.fingerprint,
            ContentVerdict::Intact,
        );
    }

    #[test]
    fn one_changed_payload_byte_in_the_jxl_is_tampered() {
        let key = generate_key().expect("key");
        let card = render_card(&story_bytes(), &key.pkcs8, CardFormat::Jxl).expect("renders");
        let envelope = read_jxl_envelope(&card.bytes).expect("envelope");
        let parsed = parse_file_envelope(&envelope).expect("file envelope");
        let full = parsed.payload;
        let changed = String::from_utf8(full.clone()).expect("utf-8").replacen(
            "\"cited_ops\":41",
            "\"cited_ops\":42",
            1,
        );
        assert_ne!(changed.as_bytes(), full.as_slice());
        let forged = file_envelope(
            changed.as_bytes(),
            &b64url_encode(&parsed.signature),
            &b64url_encode(&parsed.content_sig),
        );
        // One digit for another: the pscd box keeps its size.
        assert_eq!(forged.len(), envelope.len());
        let at = card
            .bytes
            .windows(envelope.len())
            .position(|w| w == envelope.as_bytes())
            .expect("the envelope sits in the pscd box");
        let mut jxl = card.bytes.clone();
        jxl[at..at + envelope.len()].copy_from_slice(forged.as_bytes());
        let integrity = check_integrity(CardSource::Jxl(&jxl)).expect("checks");
        assert_eq!(integrity, tampered("signature_invalid"));
    }

    #[test]
    fn a_url_is_not_checked() {
        let key = generate_key().expect("key");
        let card = render_card(&story_bytes(), &key.pkcs8, CardFormat::Png).expect("renders");
        let integrity = check_integrity(CardSource::Url(&card.compact_url)).expect("checks");
        assert_eq!(integrity.content, ContentVerdict::NotChecked);
        assert!(integrity.figures.is_some());
    }

    /// The figures `verify-card` prints for the PNG of `input`, and for its
    /// verify URL: both must agree.
    fn figures_of(input: &serde_json::Value, pkcs8: &[u8]) -> serde_json::Value {
        let json = serde_json::to_vec(input).expect("json");
        let card = render_card(&json, pkcs8, CardFormat::Png).expect("renders");
        let file = check_integrity(CardSource::Png(&card.bytes)).expect("checks");
        let url = check_integrity(CardSource::Url(&card.compact_url)).expect("checks");
        assert_eq!(file.figures, url.figures);
        serde_json::to_value(file.figures.expect("figures")).expect("serializes")
    }

    #[test]
    fn verify_card_prints_the_signed_figures() {
        let key = generate_key().expect("key");
        assert_eq!(
            figures_of(&story_input(FindingType::NPlusOneSql), &key.pkcs8),
            json!({
                "angle": "story",
                "period": {"kind": "month", "from": "2026-09-01", "to": "2026-09-30"},
                "before_fix_period": {"from": "2026-07-01", "to": "2026-07-31"},
                "before_fix_ops": 1_937_000,
                "cited_ops": 41,
            })
        );
        let curve = figures_of(&curve_input(), &key.pkcs8);
        assert_eq!(curve["angle"], "curve");
        assert_eq!(curve["last_waste_permille"], 31);
        assert_eq!(curve["previous_waste_permille"], 48);
        assert!(curve.get("before_fix_period").is_none());
        let collective = figures_of(&collective_input(), &key.pkcs8);
        assert_eq!(collective["confirmed_fixes_count"], 7);
        assert_eq!(collective["services_count"], 3);
        assert_eq!(
            collective["before_fix_period"],
            json!({"from": "2026-08-01", "to": "2026-08-31"})
        );
        let slow = figures_of(
            &story_input_with_threshold(FindingType::SlowSql),
            &key.pkcs8,
        );
        assert_eq!(slow["cited_ms"], 2349);
        assert_eq!(slow["threshold_ms"], 500);
        assert_eq!(
            slow["before_fix_period"],
            json!({"from": "2026-08-01", "to": "2026-08-31"})
        );

        let signer = CardSigner::from_pkcs8(&key.pkcs8).expect("signer");
        let (full, _) = canonical(
            &story_input(FindingType::NPlusOneSql),
            &signer,
            &signer.fingerprint(),
        );
        let changed = String::from_utf8(full.clone()).expect("utf-8").replacen(
            "\"cited_ops\":41",
            "\"cited_ops\":42",
            1,
        );
        let tampered = png_with(&unsealed(changed.as_bytes(), &signer.sign_b64(&full)));
        let integrity = check_integrity(CardSource::Png(&tampered)).expect("checks");
        assert!(!integrity.ok);
        assert_eq!(integrity.figures, None);
    }
}
