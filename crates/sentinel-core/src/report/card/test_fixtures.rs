//! The three `card.json` fixtures every card test starts from, shaped per
//! finding type.

use super::keys::{b64url_encode, compact_url};
use super::payload::{
    CompactPayload, FullPayload, build_payloads, canonical_bytes, parse_card_input,
};
use crate::detect::FindingType;
use serde_json::{Value, json};

fn fixture(text: &str) -> Value {
    serde_json::from_str(text).expect("card fixture is JSON")
}

fn type_name(finding_type: FindingType) -> String {
    serde_json::to_value(finding_type)
        .expect("finding type serializes")
        .as_str()
        .expect("finding type is a string")
        .to_owned()
}

fn is_avoidable(name: &str) -> bool {
    name.starts_with("n_plus_one_") || name.starts_with("redundant_")
}

/// The story fixture for `finding_type`: an avoidable type keeps its
/// volume, CO2 and before-fix period; a threshold type cites one trace
/// (`cited_ms` for `slow_*`, `cited_ops` otherwise) with nothing else.
pub(crate) fn story_input(finding_type: FindingType) -> Value {
    let name = type_name(finding_type);
    let mut input = fixture(include_str!(
        "../../../../../tests/fixtures/card/story.json"
    ));
    input["story"]["finding_type"] = json!(name);
    if !is_avoidable(&name) {
        input["figures"] = if name.starts_with("slow_") {
            json!({"cited_ms": 2349})
        } else {
            json!({"cited_ops": 64})
        };
        input
            .as_object_mut()
            .expect("fixture is an object")
            .remove("before_fix_period");
    }
    input
}

/// [`story_input`] plus a threshold and the before-fix period it names:
/// August 2026, the month of `observed_at`.
pub(crate) fn story_input_with_threshold(finding_type: FindingType) -> Value {
    let name = type_name(finding_type.clone());
    assert!(!is_avoidable(&name), "{name} takes no threshold");
    let mut input = story_input(finding_type);
    if name.starts_with("slow_") {
        input["figures"]["threshold_ms"] = json!(500);
    } else {
        input["figures"]["threshold_ops"] = json!(50);
    }
    input["before_fix_period"] = json!({"from": "2026-08-01", "to": "2026-08-31"});
    input
}

pub(crate) fn curve_input() -> Value {
    fixture(include_str!(
        "../../../../../tests/fixtures/card/curve.json"
    ))
}

pub(crate) fn collective_input() -> Value {
    fixture(include_str!(
        "../../../../../tests/fixtures/card/collective.json"
    ))
}

/// The full payload `build_payloads` makes of `input`, under a dummy key.
pub(crate) fn full_payload(input: &Value) -> FullPayload {
    payloads(input).0
}

/// The verify URL of `input`'s compact payload, with a dummy signature of
/// the real length.
pub(crate) fn compact_url_of(input: &Value) -> String {
    compact_url(
        &canonical_bytes(&payloads(input).1),
        &b64url_encode(&[9u8; 64]),
    )
}

fn payloads(input: &Value) -> (FullPayload, CompactPayload) {
    let bytes = serde_json::to_vec(input).expect("fixture serializes");
    let input = parse_card_input(&bytes).expect("fixture is accepted");
    build_payloads(
        &input,
        &b64url_encode(&[7u8; 32]),
        "ps1:AAAAAAAAAAAAAAAAAAAAAA",
    )
    .expect("fixture builds")
}

/// The chunks of a PNG after its signature, each as its type and its whole
/// byte range (length field through CRC). Test-side walker, no checks.
pub(crate) fn png_chunks(png: &[u8]) -> Vec<([u8; 4], std::ops::Range<usize>)> {
    let mut at = 8;
    let mut out = Vec::new();
    while at + 12 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().expect("4 bytes")) as usize;
        let end = at + 12 + len;
        out.push((png[at + 4..at + 8].try_into().expect("4 bytes"), at..end));
        at = end;
    }
    out
}

/// A whole PNG chunk: length, type, data and its CRC.
pub(crate) fn png_chunk(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(data.len())
        .expect("small chunk")
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let crc = super::raster::png_crc(&out[4..]);
    out.extend_from_slice(&crc.to_be_bytes());
    out
}

/// The card's `iTXt` chunk holding `text`, written as the engine writes it:
/// no compression, empty language tag and translated keyword.
pub(crate) fn card_itxt(text: &str) -> Vec<u8> {
    let mut data = b"perf-sentinel-card\0\0\0\0\0".to_vec();
    data.extend_from_slice(text.as_bytes());
    png_chunk(*b"iTXt", &data)
}

/// `bytes` with `range` replaced by `with`.
pub(crate) fn splice(bytes: &[u8], range: std::ops::Range<usize>, with: &[u8]) -> Vec<u8> {
    let mut out = bytes[..range.start].to_vec();
    out.extend_from_slice(with);
    out.extend_from_slice(&bytes[range.end..]);
    out
}
