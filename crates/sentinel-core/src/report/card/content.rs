//! The second signature a card file carries, `content_sig`: over the file's
//! own bytes minus the envelope carrier, bound to the format and the payload.
//! The payload cannot name the file's hash itself: the file draws the QR
//! code, which holds the compact signature.

use super::jxl::jxl_envelope_range;
use super::keys::{CardSigner, file_envelope};
use super::raster::png_envelope_chunk;
use super::svg::svg_envelope_range;
use super::{CardError, CardFormat, CardResult};
use crate::report::periodic::pdf::sha256_hex;

/// The first line of the content message, which no canonical payload
/// (always `{...}`) can start with.
pub const CONTENT_SIG_CONTEXT: &str = "perf-sentinel-card-content-v1";
/// The draft's `content_sig`: the base64url of 64 zero bytes, as long as a
/// real signature, so the sealed file keeps the draft's length.
pub const CONTENT_SIG_PLACEHOLDER: &str =
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
/// The first 16 bytes of every file envelope, which the PDF slot search
/// needs to find exactly once.
pub const CONTENT_SIG_MARKER: &str = "{\"content_sig\":\"";
/// What follows the `content_sig` value in a file envelope.
const SLOT_END: &[u8] = b"\",\"payload\":\"";

/// The bytes `content_sig` signs: the context line, the format, the
/// SHA-256 of the canonical payload, the SHA-256 of the content, one per
/// line, lowercase hex, no trailing newline.
#[must_use]
pub fn content_message(format: CardFormat, payload: &[u8], content: &[u8]) -> Vec<u8> {
    format!(
        "{CONTENT_SIG_CONTEXT}\n{}\n{}\n{}",
        format.as_str(),
        sha256_hex(payload),
        sha256_hex(content)
    )
    .into_bytes()
}

/// The bytes of `file` that `content_sig` covers: the file without its
/// envelope carrier (PNG chunk, SVG element, JPEG XL box), or the PDF with
/// its slot's 86 bytes reset to the placeholder. `content_sig_b64` is read
/// for the PDF only.
///
/// # Errors
///
/// The carrier rules: `malformed_input` for a carrier out of the place the
/// engine writes it, and the readers' input errors. `Ok(None)` is a PDF
/// whose slot is not as written: a modified file, not a bad input.
pub fn card_content(
    format: CardFormat,
    file: &[u8],
    content_sig_b64: &str,
) -> Result<Option<Vec<u8>>, CardError> {
    let carrier = match format {
        CardFormat::Png => png_envelope_chunk(file)?,
        CardFormat::Svg => svg_envelope_range(file)?,
        CardFormat::Jxl => jxl_envelope_range(file)?,
        CardFormat::Pdf => {
            return Ok(pdf_slot(file, content_sig_b64).map(|at| {
                let mut content = file.to_vec();
                content[at..at + CONTENT_SIG_PLACEHOLDER.len()]
                    .copy_from_slice(CONTENT_SIG_PLACEHOLDER.as_bytes());
                content
            }));
        }
    };
    let mut content = file[..carrier.start].to_vec();
    content.extend_from_slice(&file[carrier.end..]);
    Ok(Some(content))
}

/// The offset of the `content_sig` value in a PDF: right after the only
/// [`CONTENT_SIG_MARKER`] of the raw bytes, equal to `content_sig_b64`
/// and followed by the `payload` key. `None` for anything else.
pub(crate) fn pdf_slot(pdf: &[u8], content_sig_b64: &str) -> Option<usize> {
    if content_sig_b64.len() != CONTENT_SIG_PLACEHOLDER.len() {
        return None;
    }
    let marker = CONTENT_SIG_MARKER.as_bytes();
    let mut found = pdf
        .windows(marker.len())
        .enumerate()
        .filter(|(_, window)| *window == marker)
        .map(|(at, _)| at + marker.len());
    let at = found.next()?;
    if found.next().is_some() {
        return None;
    }
    let end = at + content_sig_b64.len();
    let holds = pdf.get(at..end) == Some(content_sig_b64.as_bytes())
        && pdf
            .get(end..)
            .is_some_and(|rest| rest.starts_with(SLOT_END));
    holds.then_some(at)
}

/// Draft, hash, sign, assemble: `assemble` writes the file around an
/// envelope text, first the placeholder one, then (PNG, SVG, JPEG XL) the
/// real one. The PDF is exported once and its slot written in place: a
/// second export would rewrite its `/ID` and XMP `InstanceID`.
///
/// # Errors
///
/// What `assemble` returns, or the format's encode reason when the draft's
/// content cannot be read back.
pub(crate) fn seal_file(
    format: CardFormat,
    full_bytes: &[u8],
    payload_sig_b64: &str,
    signer: &CardSigner,
    assemble: impl Fn(&str) -> Result<Vec<u8>, CardError>,
) -> Result<Vec<u8>, CardError> {
    let draft = assemble(&file_envelope(
        full_bytes,
        payload_sig_b64,
        CONTENT_SIG_PLACEHOLDER,
    ))?;
    let unsealable = || {
        let reason = match format {
            CardFormat::Png => "png_encode_failed",
            CardFormat::Pdf => "pdf_export_failed",
            CardFormat::Svg => "svg_encode_failed",
            CardFormat::Jxl => "jxl_encode_failed",
        };
        CardError::new(
            CardResult::Unrenderable,
            reason,
            "the draft's content cannot be read back to sign it",
        )
    };
    let content = card_content(format, &draft, CONTENT_SIG_PLACEHOLDER)
        .ok()
        .flatten()
        .ok_or_else(unsealable)?;
    let content_sig = signer.sign_b64(&content_message(format, full_bytes, &content));
    if format == CardFormat::Pdf {
        let at = pdf_slot(&draft, CONTENT_SIG_PLACEHOLDER).ok_or_else(unsealable)?;
        let mut sealed = draft;
        sealed[at..at + content_sig.len()].copy_from_slice(content_sig.as_bytes());
        return Ok(sealed);
    }
    assemble(&file_envelope(full_bytes, payload_sig_b64, &content_sig))
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::detect::FindingType;
    use crate::report::card::jxl::{
        encode_codestream, jxl_boxes, jxl_container, read_jxl_envelope,
    };
    use crate::report::card::keys::{
        CardSigner, b64url_decode, b64url_encode, compact_url, file_envelope, generate_key,
        parse_file_envelope, verify_signature,
    };
    use crate::report::card::layout::{layout_carousel, layout_image};
    use crate::report::card::payload::{build_payloads, canonical_bytes, parse_card_input};
    use crate::report::card::pdf::{card_title, read_pdf_envelope, render_carousel_pdf};
    use crate::report::card::raster::{
        encode_png, opaque_rgb, png_envelope_chunk, rasterize, read_png_envelope,
    };
    use crate::report::card::svg::{read_svg_envelope, render_svg, svg_envelope_range};
    use crate::report::card::test_fixtures::{
        card_itxt, compact_url_of, full_payload, png_chunk, png_chunks, splice, story_input,
    };
    use crate::report::card::verify::{CardSource, ContentVerdict, check_integrity};
    use crate::report::card::{CardError, render_card};
    use serde_json::{Value, json};

    const FORMATS: [CardFormat; 4] = [
        CardFormat::Png,
        CardFormat::Svg,
        CardFormat::Jxl,
        CardFormat::Pdf,
    ];

    /// The story card in every format, under one key.
    struct Story {
        pkcs8: Vec<u8>,
        public_key: Vec<u8>,
        full_bytes: Vec<u8>,
        payload_sig: String,
        files: Vec<(CardFormat, Vec<u8>)>,
    }

    fn story_json() -> Value {
        story_input(FindingType::NPlusOneSql)
    }

    fn story() -> &'static Story {
        static STORY: OnceLock<Story> = OnceLock::new();
        STORY.get_or_init(|| {
            let key = generate_key().expect("key");
            let input = serde_json::to_vec(&story_json()).expect("json");
            let files: Vec<(CardFormat, Vec<u8>)> = FORMATS
                .iter()
                .map(|&format| {
                    let card = render_card(&input, &key.pkcs8, format).expect("renders");
                    (format, card.bytes)
                })
                .collect();
            let envelope = parse_file_envelope(&read(CardFormat::Png, &files[0].1).expect("reads"))
                .expect("parses");
            Story {
                pkcs8: key.pkcs8,
                public_key: b64url_decode(&key.public_key).expect("public key"),
                full_bytes: envelope.payload,
                payload_sig: b64url_encode(&envelope.signature),
                files,
            }
        })
    }

    fn file(format: CardFormat) -> &'static [u8] {
        &story()
            .files
            .iter()
            .find(|(f, _)| *f == format)
            .expect("rendered")
            .1
    }

    fn signer_of(pkcs8: &[u8]) -> CardSigner {
        CardSigner::from_pkcs8(pkcs8).expect("signer")
    }

    fn read(format: CardFormat, bytes: &[u8]) -> Result<String, CardError> {
        match format {
            CardFormat::Png => read_png_envelope(bytes),
            CardFormat::Svg => read_svg_envelope(bytes),
            CardFormat::Jxl => read_jxl_envelope(bytes),
            CardFormat::Pdf => read_pdf_envelope(bytes),
        }
    }

    fn source(format: CardFormat, bytes: &[u8]) -> CardSource<'_> {
        match format {
            CardFormat::Png => CardSource::Png(bytes),
            CardFormat::Svg => CardSource::Svg(bytes),
            CardFormat::Jxl => CardSource::Jxl(bytes),
            CardFormat::Pdf => CardSource::Pdf(bytes),
        }
    }

    fn content_of(format: CardFormat, bytes: &[u8]) -> Vec<u8> {
        let envelope = parse_file_envelope(&read(format, bytes).expect("reads")).expect("parses");
        card_content(format, bytes, &b64url_encode(&envelope.content_sig))
            .expect("the carrier is where it belongs")
            .expect("the PDF slot is as written")
    }

    fn verdict(format: CardFormat, bytes: &[u8]) -> ContentVerdict {
        check_integrity(source(format, bytes))
            .expect("checks")
            .content
    }

    fn refusal(format: CardFormat, bytes: &[u8]) -> &'static str {
        check_integrity(source(format, bytes))
            .expect_err("refused")
            .reason
    }

    type Assemble = Box<dyn Fn(&str) -> Vec<u8>>;

    /// The format's assembly of `input` under `signer`, as `render_card`
    /// does it: the drawing done once, the envelope written by the closure.
    fn assembler(format: CardFormat, input: &Value, signer: &CardSigner) -> Assemble {
        let input = parse_card_input(&serde_json::to_vec(input).expect("json")).expect("input");
        let (full, compact) =
            build_payloads(&input, &signer.public_key_b64(), &signer.fingerprint())
                .expect("payloads");
        let compact_bytes = canonical_bytes(&compact);
        let url = compact_url(&compact_bytes, &signer.sign_b64(&compact_bytes));
        match format {
            CardFormat::Png => {
                let pixmap =
                    rasterize(&layout_image(&full, &url).expect("lays out")).expect("raster");
                Box::new(move |envelope| encode_png(&pixmap, envelope).expect("png"))
            }
            CardFormat::Svg => {
                let ops = layout_image(&full, &url).expect("lays out");
                Box::new(move |envelope| {
                    render_svg(&ops, envelope, card_title(&full), full.lang).expect("svg")
                })
            }
            CardFormat::Jxl => {
                let pixmap =
                    rasterize(&layout_image(&full, &url).expect("lays out")).expect("raster");
                let codestream =
                    encode_codestream(&opaque_rgb(&pixmap), pixmap.width(), pixmap.height())
                        .expect("codestream");
                Box::new(move |envelope| jxl_container(&codestream, envelope).expect("jxl"))
            }
            CardFormat::Pdf => {
                let pages = layout_carousel(&full, &url).expect("lays out");
                Box::new(move |envelope| render_carousel_pdf(&pages, envelope, &full).expect("pdf"))
            }
        }
    }

    /// The story's file in `format` around another envelope text.
    fn story_with(format: CardFormat, envelope: &str) -> Vec<u8> {
        assembler(format, &story_json(), &signer_of(&story().pkcs8))(envelope)
    }

    /// The offset of the 86 slot bytes after the only marker of `pdf`.
    fn slot(pdf: &[u8]) -> usize {
        let marker = CONTENT_SIG_MARKER.as_bytes();
        let at: Vec<usize> = pdf
            .windows(marker.len())
            .enumerate()
            .filter(|(_, w)| *w == marker)
            .map(|(i, _)| i + marker.len())
            .collect();
        assert_eq!(at.len(), 1, "one slot");
        at[0]
    }

    fn sha(bytes: &[u8]) -> Vec<u8> {
        Sha256::digest(bytes).to_vec()
    }

    #[test]
    fn the_content_message_is_fixed() {
        let expected = "perf-sentinel-card-content-v1\npng\n\
            44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a\n\
            ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(
            content_message(CardFormat::Png, b"{}", b"abc"),
            expected.as_bytes()
        );
        assert_eq!(CONTENT_SIG_PLACEHOLDER, b64url_encode(&[0; 64]));
        assert_eq!(CONTENT_SIG_PLACEHOLDER.len(), 86);
        assert_eq!(CONTENT_SIG_MARKER.len(), 16);
    }

    #[test]
    fn each_format_signs_its_content() {
        let story = story();
        for format in FORMATS {
            let bytes = file(format);
            let envelope =
                parse_file_envelope(&read(format, bytes).expect("reads")).expect("parses");
            let content = content_of(format, bytes);
            let message = content_message(format, &envelope.payload, &content);
            assert!(
                verify_signature(&story.public_key, &message, &envelope.content_sig),
                "{format:?}"
            );
            let other = if format == CardFormat::Png {
                CardFormat::Svg
            } else {
                CardFormat::Png
            };
            let message = content_message(other, &envelope.payload, &content);
            assert!(!verify_signature(
                &story.public_key,
                &message,
                &envelope.content_sig
            ));
            assert_eq!(verdict(format, bytes), ContentVerdict::Intact, "{format:?}");
        }
    }

    #[test]
    fn the_pdf_is_sealed_in_place() {
        let story = story();
        let pdf = file(CardFormat::Pdf);
        let draft = story_with(
            CardFormat::Pdf,
            &file_envelope(
                &story.full_bytes,
                &story.payload_sig,
                CONTENT_SIG_PLACEHOLDER,
            ),
        );
        assert_eq!(draft.len(), pdf.len());
        let at = slot(pdf);
        let differ: Vec<usize> = (0..pdf.len()).filter(|&i| pdf[i] != draft[i]).collect();
        assert_ne!(pdf[at..at + 86], draft[at..at + 86]);
        assert!(
            differ.iter().all(|i| (at..at + 86).contains(i)),
            "{differ:?}"
        );
        assert_eq!(content_of(CardFormat::Pdf, pdf), draft);
    }

    #[test]
    fn the_content_hash_ignores_only_the_envelope() {
        let story = story();
        for format in FORMATS {
            let bytes = file(format);
            let envelope = read(format, bytes).expect("reads");
            let content = content_of(format, bytes);
            let carrier = carrier_range(format, bytes, &envelope);
            match format {
                CardFormat::Pdf => {
                    assert_eq!(content.len(), bytes.len());
                    assert_eq!(
                        content,
                        splice(bytes, carrier.clone(), CONTENT_SIG_PLACEHOLDER.as_bytes())
                    );
                }
                _ => assert_eq!(content, splice(bytes, carrier.clone(), b"")),
            }

            let middle = (bytes.len() / 2..bytes.len())
                .find(|i| !carrier.contains(i))
                .expect("content after the carrier");
            let mut changed = bytes.to_vec();
            changed[middle] ^= 1;
            let mut last = bytes.to_vec();
            *last.last_mut().expect("bytes") ^= 1;
            let mut appended = bytes.to_vec();
            appended.extend_from_slice(if format == CardFormat::Jxl {
                b"\0\0\0\x08free"
            } else {
                b"\n%retouched\n"
            });
            for edited in [changed, last, appended] {
                assert_ne!(
                    sha(&content_of(format, &edited)),
                    sha(&content),
                    "{format:?}"
                );
            }

            let other_sig = b64url_encode(&[5u8; 64]);
            let rewritten = if format == CardFormat::Pdf {
                splice(bytes, carrier, other_sig.as_bytes())
            } else {
                story_with(
                    format,
                    &file_envelope(&story.full_bytes, &story.payload_sig, &other_sig),
                )
            };
            assert_eq!(content_of(format, &rewritten), content, "{format:?}");
        }
    }

    /// The carrier `card_content` cuts: the PNG chunk, the SVG element, the
    /// JPEG XL box, or the PDF slot's 86 bytes.
    fn carrier_range(format: CardFormat, bytes: &[u8], envelope: &str) -> std::ops::Range<usize> {
        match format {
            CardFormat::Png => {
                let range = png_envelope_chunk(bytes).expect("walks");
                let len =
                    u32::from_be_bytes(bytes[range.start..range.start + 4].try_into().expect("4"));
                assert_eq!(&bytes[range.start + 4..range.start + 8], b"iTXt");
                assert_eq!(range.len(), len as usize + 12);
                range
            }
            CardFormat::Svg => {
                let element = format!("<metadata id=\"perf-sentinel-card\">{envelope}</metadata>");
                let text = std::str::from_utf8(bytes).expect("utf-8");
                assert_eq!(text.matches(&element).count(), 1);
                let at = text.find(&element).expect("the element");
                assert_eq!(
                    svg_envelope_range(bytes).expect("in place"),
                    at..at + element.len()
                );
                at..at + element.len()
            }
            CardFormat::Jxl => {
                jxl_boxes(bytes)
                    .expect("walks")
                    .into_iter()
                    .find(|(kind, _)| kind == b"pscd")
                    .expect("the pscd box")
                    .1
            }
            CardFormat::Pdf => {
                let at = slot(bytes);
                at..at + 86
            }
        }
    }

    #[test]
    fn a_retouched_png_is_content_modified() {
        let story = story();
        let signer = signer_of(&story.pkcs8);
        let input =
            parse_card_input(&serde_json::to_vec(&story_json()).expect("json")).expect("input");
        let (full, compact) =
            build_payloads(&input, &signer.public_key_b64(), &signer.fingerprint())
                .expect("payloads");
        let compact_bytes = canonical_bytes(&compact);
        let url = compact_url(&compact_bytes, &signer.sign_b64(&compact_bytes));
        let mut pixmap = rasterize(&layout_image(&full, &url).expect("lays out")).expect("raster");
        let envelope = read(CardFormat::Png, file(CardFormat::Png)).expect("reads");
        assert_eq!(
            &encode_png(&pixmap, &envelope).expect("png"),
            file(CardFormat::Png)
        );

        // One pixel of the card, black on white or white on ink.
        let pixel = &mut pixmap.pixels_mut()[540 * 1080 + 540];
        *pixel = if pixel.red() > 128 {
            tiny_skia::PremultipliedColorU8::from_rgba(0, 0, 0, 255).expect("opaque")
        } else {
            tiny_skia::PremultipliedColorU8::from_rgba(255, 255, 255, 255).expect("opaque")
        };
        let retouched = encode_png(&pixmap, &envelope).expect("png");
        let integrity = check_integrity(CardSource::Png(&retouched)).expect("checks");
        assert!(integrity.ok);
        assert_eq!(integrity.content, ContentVerdict::Modified);
        assert_eq!(integrity.reason, "content_modified");
        assert_eq!(integrity.figures, None);
        assert_eq!(integrity.domain, None);
        assert_eq!(integrity.fingerprint, None);
        assert_eq!(integrity.public_key, None);
        assert_eq!(integrity.signed_day, None);
    }

    #[test]
    fn a_copied_envelope_on_another_image_is_modified() {
        let key = generate_key().expect("key");
        let signer = signer_of(&key.pkcs8);
        let mut team_a = story_json();
        team_a["free"]["team"] = json!("Team A");
        let mut team_b = story_json();
        team_b["free"]["team"] = json!("Team B");
        let json_a = serde_json::to_vec(&team_a).expect("json");
        let json_b = serde_json::to_vec(&team_b).expect("json");
        for format in FORMATS {
            let a = render_card(&json_a, &key.pkcs8, format)
                .expect("renders")
                .bytes;
            let envelope_a = read(format, &a).expect("reads");
            let mixed = if format == CardFormat::Pdf {
                let b = render_card(&json_b, &key.pkcs8, format)
                    .expect("renders")
                    .bytes;
                let envelope_b = read(format, &b).expect("reads");
                assert_eq!(envelope_a.len(), envelope_b.len());
                let at = b
                    .windows(envelope_b.len())
                    .position(|w| w == envelope_b.as_bytes())
                    .expect("B's envelope");
                splice(&b, at..at + envelope_b.len(), envelope_a.as_bytes())
            } else {
                assembler(format, &team_b, &signer)(&envelope_a)
            };
            assert_eq!(
                verdict(format, &mixed),
                ContentVerdict::Modified,
                "{format:?}"
            );
            if format == CardFormat::Png {
                let svg_a = assembler(CardFormat::Svg, &team_a, &signer)(&envelope_a);
                assert_eq!(verdict(CardFormat::Svg, &svg_a), ContentVerdict::Modified);
            }
        }
    }

    #[test]
    fn a_non_canonical_envelope_never_reads_intact() {
        let envelope = read(CardFormat::Png, file(CardFormat::Png)).expect("reads");
        let value: Value = serde_json::from_str(&envelope).expect("JSON");
        let reordered = format!(
            r#"{{"payload":{},"content_sig":{},"signature":{}}}"#,
            value["payload"], value["content_sig"], value["signature"]
        );
        assert_eq!(reordered.len(), envelope.len());
        let spaced = envelope.replacen(':', ": ", 1);

        let png = file(CardFormat::Png);
        let chunk = png_envelope_chunk(png).expect("walks");
        let svg = file(CardFormat::Svg);
        let svg_text = std::str::from_utf8(svg).expect("utf-8");
        let cases = [
            (
                CardFormat::Png,
                splice(png, chunk.clone(), &card_itxt(&reordered)),
            ),
            (CardFormat::Png, splice(png, chunk, &card_itxt(&spaced))),
            (
                CardFormat::Svg,
                svg_text
                    .replacen(&envelope_of(svg), &reordered_svg(svg), 1)
                    .into_bytes(),
            ),
            (CardFormat::Jxl, story_with(CardFormat::Jxl, &spaced_jxl())),
        ];
        for (format, bytes) in cases {
            assert_eq!(refusal(format, &bytes), "malformed_input", "{format:?}");
            let error = parse_file_envelope(&read(format, &bytes).expect("the carrier reads"))
                .expect_err("refused");
            assert_eq!(error.reason, "malformed_input");
            let sig = b64url_encode(&[0; 64]);
            assert_eq!(
                card_content(format, &bytes, &sig)
                    .expect("walks")
                    .expect("content"),
                content_of(format, file(format)),
                "{format:?}"
            );
        }
    }

    fn envelope_of(svg: &[u8]) -> String {
        read_svg_envelope(svg).expect("reads")
    }

    /// The SVG envelope's keys reordered: same length, same byte set.
    fn reordered_svg(svg: &[u8]) -> String {
        let value: Value = serde_json::from_str(&envelope_of(svg)).expect("JSON");
        format!(
            r#"{{"payload":{},"content_sig":{},"signature":{}}}"#,
            value["payload"], value["content_sig"], value["signature"]
        )
    }

    fn spaced_jxl() -> String {
        read_jxl_envelope(file(CardFormat::Jxl))
            .expect("reads")
            .replacen(':', ": ", 1)
    }

    #[test]
    fn a_payload_signature_is_not_a_content_signature() {
        let envelope = read(CardFormat::Png, file(CardFormat::Png)).expect("reads");
        let value: Value = serde_json::from_str(&envelope).expect("JSON");
        let swapped = format!(
            r#"{{"content_sig":{},"payload":{},"signature":{}}}"#,
            value["signature"], value["payload"], value["content_sig"]
        );
        let png = story_with(CardFormat::Png, &swapped);
        let integrity = check_integrity(CardSource::Png(&png)).expect("checks");
        assert!(!integrity.ok);
        assert_eq!(integrity.reason, "signature_invalid");
    }

    #[test]
    fn png_envelope_chunks_are_walked_whole() {
        let png = file(CardFormat::Png);
        let chunks = png_chunks(png);
        let range_of = |kind: &[u8; 4]| {
            chunks
                .iter()
                .find(|(k, _)| k == kind)
                .expect("the chunk")
                .1
                .clone()
        };
        let card = range_of(b"iTXt");
        let idat = range_of(b"IDAT");
        let iend = range_of(b"IEND");
        let card_bytes = &png[card.clone()];

        let after_idat = splice(png, iend.start..iend.start, card_bytes);
        assert_eq!(
            refusal(CardFormat::Png, &after_idat),
            "duplicate_card_payload"
        );
        let before_idat = splice(png, idat.start..idat.start, card_bytes);
        assert_eq!(
            refusal(CardFormat::Png, &before_idat),
            "duplicate_card_payload"
        );
        let moved = splice(
            &splice(png, iend.start..iend.start, card_bytes),
            card.clone(),
            b"",
        );
        assert_eq!(refusal(CardFormat::Png, &moved), "no_card_payload");

        let text = png_chunk(*b"tEXt", b"Comment\0hello");
        let before = splice(png, card.start..card.start, &text);
        let after = splice(png, card.end..card.end, &text);
        assert_eq!(
            content_of(CardFormat::Png, &before),
            content_of(CardFormat::Png, &after)
        );
        assert_eq!(verdict(CardFormat::Png, &before), ContentVerdict::Modified);
        assert_eq!(verdict(CardFormat::Png, &after), ContentVerdict::Modified);

        let mut past = png.to_vec();
        let too_long = u32::try_from(png.len()).expect("small file");
        past[idat.start..idat.start + 4].copy_from_slice(&too_long.to_be_bytes());
        let mut huge = png.to_vec();
        huge[idat.start..idat.start + 4].copy_from_slice(&0x8000_0000u32.to_be_bytes());
        let cut = png[..idat.start + 6].to_vec();
        for bytes in [past, huge, cut] {
            assert_eq!(refusal(CardFormat::Png, &bytes), "malformed_input");
        }
    }

    #[test]
    fn the_carrier_sits_where_the_engine_writes_it() {
        let svg = std::str::from_utf8(file(CardFormat::Svg)).expect("utf-8");
        let envelope = envelope_of(svg.as_bytes());
        let element = format!("<metadata id=\"perf-sentinel-card\">{envelope}</metadata>");
        let without = svg.replacen(&format!("{element}\n"), "", 1);
        let in_title = without.replacen("</title>", &format!("{element}</title>"), 1);
        let in_path = {
            let at = without.find(" d=\"M").expect("a path") + 8;
            format!("{}{element}{}", &without[..at], &without[at..])
        };
        let after_svg = format!("{without}{element}\n");
        for moved in [in_title, in_path, after_svg] {
            assert_eq!(
                refusal(CardFormat::Svg, moved.as_bytes()),
                "malformed_input"
            );
            let error = read_svg_envelope(moved.as_bytes()).expect_err("refused");
            assert_eq!(error.reason, "malformed_input");
        }
        let bom = format!("\u{feff}\n{svg}");
        assert_eq!(envelope_of(bom.as_bytes()), envelope);
        let range = svg_envelope_range(svg.as_bytes()).expect("in place");
        assert_eq!(
            svg_envelope_range(bom.as_bytes()).expect("in place"),
            range.start + 4..range.end + 4
        );
        let integrity = check_integrity(CardSource::Svg(bom.as_bytes())).expect("checks");
        assert_eq!(integrity.content, ContentVerdict::Modified);
        assert_eq!(integrity.reason, "content_modified");

        let jxl = file(CardFormat::Jxl);
        let boxes = jxl_boxes(jxl).expect("walks");
        let range = |kind: &[u8; 4]| {
            boxes
                .iter()
                .find(|(k, _)| k == kind)
                .expect("box")
                .1
                .clone()
        };
        let (ftyp, pscd, jxlc) = (range(b"ftyp"), range(b"pscd"), range(b"jxlc"));
        let pscd_bytes = &jxl[pscd.clone()];
        let after_jxlc = splice(
            &splice(jxl, jxlc.end..jxlc.end, pscd_bytes),
            pscd.clone(),
            b"",
        );
        assert_eq!(
            read_jxl_envelope(&after_jxlc).expect("reads"),
            read_jxl_envelope(jxl).expect("reads")
        );
        assert_eq!(refusal(CardFormat::Jxl, &after_jxlc), "malformed_input");
        let before_ftyp = splice(
            &splice(jxl, pscd.clone(), b""),
            ftyp.start..ftyp.start,
            pscd_bytes,
        );
        assert_eq!(refusal(CardFormat::Jxl, &before_ftyp), "malformed_input");
        let mut large = 1u32.to_be_bytes().to_vec();
        large.extend_from_slice(b"pscd");
        large.extend_from_slice(&(pscd.len() as u64 + 8).to_be_bytes());
        large.extend_from_slice(&jxl[pscd.start + 8..pscd.end]);
        let large = splice(jxl, pscd, &large);
        assert_eq!(
            read_jxl_envelope(&large).expect("reads"),
            read_jxl_envelope(jxl).expect("reads")
        );
        assert_eq!(refusal(CardFormat::Jxl, &large), "malformed_input");
    }

    #[test]
    fn a_pdf_with_its_slot_twice_or_appended_bytes_is_modified() {
        let pdf = file(CardFormat::Pdf);
        let mut appended = pdf.to_vec();
        appended.extend_from_slice(b"\n%retouched\n");
        let mut twice = pdf.to_vec();
        twice.extend_from_slice(CONTENT_SIG_MARKER.as_bytes());
        twice.extend_from_slice(CONTENT_SIG_PLACEHOLDER.as_bytes());
        let at = slot(pdf);
        let other = splice(pdf, at..at + 86, b64url_encode(&[5u8; 64]).as_bytes());
        for bytes in [appended, twice, other] {
            assert_eq!(verdict(CardFormat::Pdf, &bytes), ContentVerdict::Modified);
        }
    }

    #[test]
    fn a_title_holding_the_marker_would_break_the_slot() {
        let input = story_json();
        let mut full = full_payload(&input);
        full.free.title = Some("Q3 {\"content_sig\":\" review".to_owned());
        let pages = layout_carousel(&full, &compact_url_of(&input)).expect("lays out");
        let envelope = file_envelope(
            &canonical_bytes(&full),
            &b64url_encode(&[9u8; 64]),
            CONTENT_SIG_PLACEHOLDER,
        );
        let draft = render_carousel_pdf(&pages, &envelope, &full).expect("renders");
        let marker = CONTENT_SIG_MARKER.as_bytes();
        let count = draft.windows(marker.len()).filter(|w| *w == marker).count();
        assert_eq!(count, 2, "krilla writes the title's marker raw");
        assert_eq!(pdf_slot(&draft, CONTENT_SIG_PLACEHOLDER), None);
        assert_eq!(
            card_content(CardFormat::Pdf, &draft, CONTENT_SIG_PLACEHOLDER).expect("no walk"),
            None
        );
    }
}
