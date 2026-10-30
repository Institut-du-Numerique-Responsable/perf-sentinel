//! The card as a four-page PDF/A-3b carousel, the signed envelope attached
//! uncompressed so the site's drop zone reads it without inflating, and
//! that envelope read back.

use krilla::embed::AssociationKind;
use krilla::metadata::Metadata;

use super::copy::words;
use super::layout::{CARD_HEIGHT, CARD_WIDTH};
use super::payload::{FullPayload, Lang, canonical_bytes};
use super::raster::MAX_ENVELOPE_BYTES;
use super::{CardError, CardResult};
use crate::report::periodic::pdf::pages::Op;
use crate::report::periodic::pdf::{
    ExtractError, MAX_PDF_BYTES, add_pages, archival_document, embed, extract_named, pdf_date,
    sha256_hex,
};

/// The name of the attachment holding the signed envelope.
pub const CARD_ATTACHMENT_NAME: &str = "perf-sentinel-card.json";

fn input_error(reason: &'static str, detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::InputError, reason, detail)
}

fn export_failed(detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::Unrenderable, "pdf_export_failed", detail)
}

/// The carousel pages as a PDF/A-3b, 1080 x 1350 pt each, the envelope
/// attached as [`CARD_ATTACHMENT_NAME`]. The date and document id come
/// from the payload, never the clock, so a render is reproducible.
///
/// # Errors
///
/// `pdf_export_failed` when krilla refuses the document.
#[cfg_attr(not(test), expect(dead_code, reason = "called by render_card, Task 8"))]
pub(crate) fn render_carousel_pdf(
    pages: &[Vec<Op>],
    envelope: &str,
    payload: &FullPayload,
) -> Result<Vec<u8>, CardError> {
    let at = i64::try_from(payload.generated_at_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .ok_or_else(|| export_failed("generated_at_ms is out of range"))?;
    let date = pdf_date(at);
    let language = match payload.lang {
        Lang::En => "en",
        Lang::Fr => "fr",
    };
    let title = payload
        .free
        .title
        .clone()
        .unwrap_or_else(|| words(payload.lang).pdf_title.to_owned());
    let mut document = archival_document();
    document.set_metadata(
        Metadata::new()
            .title(title)
            .language(language.to_owned())
            .creator(format!("perf-sentinel {}", env!("CARGO_PKG_VERSION")))
            .creation_date(date)
            .document_id(sha256_hex(&canonical_bytes(payload))),
    );
    embed(
        &mut document,
        CARD_ATTACHMENT_NAME,
        "application/json",
        AssociationKind::Source,
        envelope.as_bytes(),
        date,
        "The signed card payload these pages present",
        false,
    );
    add_pages(&mut document, pages, CARD_WIDTH, CARD_HEIGHT);
    document
        .finish()
        .map_err(|e| export_failed(format!("{e:?}")))
}

/// The envelope a card PDF attaches, 64 KiB at most.
///
/// # Errors
///
/// `file_too_large`, `no_card_payload`, `duplicate_card_payload`, or
/// `malformed_input` for anything else that keeps the envelope unread.
pub fn read_pdf_envelope(pdf: &[u8]) -> Result<String, CardError> {
    if pdf.len() > MAX_PDF_BYTES {
        return Err(input_error(
            "file_too_large",
            format!("the PDF is over {MAX_PDF_BYTES} bytes"),
        ));
    }
    let malformed = |detail: String| input_error("malformed_input", detail);
    let bytes = match extract_named(pdf, CARD_ATTACHMENT_NAME, MAX_ENVELOPE_BYTES as u64) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return Err(input_error(
                "no_card_payload",
                "the PDF carries no card payload",
            ));
        }
        Err(ExtractError::Duplicate(_)) => {
            return Err(input_error(
                "duplicate_card_payload",
                "the PDF carries more than one card payload",
            ));
        }
        Err(e) => return Err(malformed(e.to_string())),
    };
    String::from_utf8(bytes).map_err(|e| malformed(format!("the card payload is not text: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::FindingType;
    use crate::report::card::keys::{b64url_encode, file_envelope};
    use crate::report::card::layout::layout_carousel;
    use crate::report::card::payload::canonical_bytes;
    use crate::report::card::test_fixtures::{compact_url_of, full_payload, story_input};
    use crate::report::periodic::pdf::tests::baked_report;
    use crate::report::periodic::pdf::{PdfInputs, render_disclosure_pdf};
    use serde_json::{Value, json};

    fn story() -> Value {
        story_input(FindingType::NPlusOneSql)
    }

    fn envelope(input: &Value) -> String {
        file_envelope(
            &canonical_bytes(&full_payload(input)),
            &b64url_encode(&[9u8; 64]),
        )
    }

    fn pdf_of(input: &Value) -> Vec<u8> {
        let payload = full_payload(input);
        let pages =
            layout_carousel(&payload, &compact_url_of(input)).expect("the carousel lays out");
        render_carousel_pdf(&pages, &envelope(input), &payload).expect("the carousel renders")
    }

    /// The catalog's `/Lang` and the info dictionary's `/Title`.
    fn language_and_title(pdf: &[u8]) -> (String, String) {
        let doc = lopdf::Document::load_mem(pdf).expect("lopdf reads the render");
        let lang = doc
            .catalog()
            .expect("catalog")
            .get(b"Lang")
            .and_then(lopdf::Object::as_str)
            .expect("Lang");
        let info = doc
            .trailer
            .get(b"Info")
            .and_then(|info| doc.dereference(info))
            .expect("Info")
            .1
            .as_dict()
            .expect("Info dict")
            .clone();
        let title = info.get(b"Title").expect("Title");
        (
            String::from_utf8_lossy(lang).into_owned(),
            lopdf::decode_text_string(title).expect("Title is text"),
        )
    }

    #[test]
    fn same_card_renders_the_same_pdf_bytes() {
        assert_eq!(pdf_of(&story()), pdf_of(&story()));
    }

    #[test]
    fn the_carousel_has_four_square_cut_pages() {
        let doc = lopdf::Document::load_mem(&pdf_of(&story())).expect("lopdf reads the render");
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 4);
        for id in pages.values() {
            let media_box: Vec<f32> = doc
                .get_dictionary(*id)
                .expect("page dict")
                .get(b"MediaBox")
                .and_then(lopdf::Object::as_array)
                .expect("MediaBox")
                .iter()
                .map(|n| n.as_float().expect("number"))
                .collect();
            assert_eq!(media_box, [0.0, 0.0, 1080.0, 1350.0]);
        }
    }

    #[test]
    fn the_pdf_carries_the_envelope() {
        let input = story();
        assert_eq!(
            read_pdf_envelope(&pdf_of(&input)).expect("the envelope reads back"),
            envelope(&input)
        );
    }

    #[test]
    fn the_envelope_is_stored_uncompressed() {
        let input = story();
        let pdf = pdf_of(&input);
        let needle = envelope(&input);
        assert!(
            pdf.windows(needle.len()).any(|w| w == needle.as_bytes()),
            "the raw PDF bytes hold the envelope verbatim"
        );
    }

    #[test]
    fn a_pdf_without_the_attachment_has_no_card_payload() {
        let (_, bytes) = baked_report();
        let disclosure = render_disclosure_pdf(&PdfInputs {
            report_bytes: &bytes,
            attestation: None,
            bundle: None,
            expected_identity: None,
            expected_issuer: None,
        })
        .expect("the disclosure renders");
        let error = read_pdf_envelope(&disclosure).expect_err("no card attachment");
        assert_eq!(error.reason, "no_card_payload");
        assert_eq!(error.result, CardResult::InputError);
        let error = read_pdf_envelope(b"GIF89a").expect_err("not a PDF");
        assert_eq!(error.reason, "malformed_input");
    }

    #[test]
    fn an_oversized_pdf_is_refused() {
        let mut pdf = b"%PDF-".to_vec();
        pdf.resize(MAX_PDF_BYTES + 1, b' ');
        let error = read_pdf_envelope(&pdf).expect_err("over the cap");
        assert_eq!(error.reason, "file_too_large");
    }

    #[test]
    fn the_metadata_follows_the_card_language() {
        let mut french = story();
        french["lang"] = json!("fr");
        assert_eq!(
            language_and_title(&pdf_of(&french)),
            ("fr".to_owned(), "Carte Perf Sentinel".to_owned())
        );
        let mut titled = story();
        titled["free"]["title"] = json!("Our cart fix");
        assert_eq!(
            language_and_title(&pdf_of(&titled)),
            ("en".to_owned(), "Our cart fix".to_owned())
        );
        assert_eq!(
            language_and_title(&pdf_of(&story())).1,
            "Perf Sentinel card"
        );
    }
}
