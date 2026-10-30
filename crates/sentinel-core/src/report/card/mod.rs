//! Share cards: the signed payloads a card carries, its keys, and their
//! rendering. Contract C2/C3 in `docs/SHARE-CARD.md`.

pub mod binding;
pub mod content;
mod copy;
pub mod jxl;
pub mod keys;
mod layout;
#[cfg(any(feature = "daemon", feature = "tempo", feature = "jaeger-query"))]
pub mod online;
pub mod payload;
pub mod pdf;
pub mod raster;
pub mod svg;
#[cfg(test)]
mod test_fixtures;
pub mod verify;

/// The family a card failure belongs to, which fixes the exit code and
/// the `result` key of the error line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardResult {
    InputError,
    Refused,
    Unrenderable,
}

impl CardResult {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InputError => "input_error",
            Self::Refused => "refused",
            Self::Unrenderable => "unrenderable",
        }
    }

    #[must_use]
    pub fn exit_code(self) -> i32 {
        match self {
            Self::InputError => 3,
            Self::Refused => 1,
            Self::Unrenderable => 5,
        }
    }
}

/// A card failure. `reason` comes from the closed list of C2; `field` is
/// the `card.json` path of the text at fault, set on
/// `unsupported_characters` (the first text the fonts cannot draw) and on
/// the `malformed_input` of a `free.title` holding the PDF slot marker.
#[derive(Debug, thiserror::Error)]
#[error("{reason}: {detail}")]
pub struct CardError {
    pub result: CardResult,
    pub reason: &'static str,
    pub detail: String,
    pub field: Option<&'static str>,
}

impl CardError {
    pub(crate) fn new(result: CardResult, reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            result,
            reason,
            detail: detail.into(),
            field: None,
        }
    }
}

/// The file a card renders to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardFormat {
    /// The single 1080 x 1350 image.
    Png,
    /// The four-page carousel.
    Pdf,
    /// The single image as an SVG, its text drawn as outlines.
    Svg,
    /// The single image as a lossless JPEG XL, the PNG's pixels.
    Jxl,
}

impl CardFormat {
    /// The format's name in the content signature's message.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Pdf => "pdf",
            Self::Svg => "svg",
            Self::Jxl => "jxl",
        }
    }
}

/// A signed card, ready to write.
#[derive(Debug)]
pub struct RenderedCard {
    pub bytes: Vec<u8>,
    /// `card.png`, `card.pdf`, `card.svg` or `card.jxl`.
    pub file_name: &'static str,
    pub fingerprint: String,
    pub compact_url: String,
}

/// Parse a `card.json`, sign its full and compact payloads with `pkcs8`,
/// draw the card carrying both, and seal the file with its content
/// signature. Same input and key, same bytes.
///
/// # Errors
///
/// Any reason of C2: an input error, a refusal or an unrenderable card.
pub fn render_card(
    input_json: &[u8],
    pkcs8: &[u8],
    format: CardFormat,
) -> Result<RenderedCard, CardError> {
    let input = payload::parse_card_input(input_json)?;
    let signer = keys::CardSigner::from_pkcs8(pkcs8)?;
    let fingerprint = signer.fingerprint();
    let (full, compact) = payload::build_payloads(&input, &signer.public_key_b64(), &fingerprint)?;
    let full_bytes = payload::canonical_bytes(&full);
    let compact_bytes = payload::canonical_bytes(&compact);
    let payload_sig = signer.sign_b64(&full_bytes);
    let compact_url = keys::compact_url(&compact_bytes, &signer.sign_b64(&compact_bytes));
    let seal = |assemble: &dyn Fn(&str) -> Result<Vec<u8>, CardError>| {
        content::seal_file(format, &full_bytes, &payload_sig, &signer, assemble)
    };
    let (bytes, file_name) = match format {
        CardFormat::Png => {
            let pixmap = raster::rasterize(&layout::layout_image(&full, &compact_url)?)?;
            (
                seal(&|envelope| raster::encode_png(&pixmap, envelope))?,
                "card.png",
            )
        }
        CardFormat::Pdf => {
            let pages = layout::layout_carousel(&full, &compact_url)?;
            (
                seal(&|envelope| pdf::render_carousel_pdf(&pages, envelope, &full))?,
                "card.pdf",
            )
        }
        CardFormat::Svg => {
            let ops = layout::layout_image(&full, &compact_url)?;
            let title = pdf::card_title(&full);
            (
                seal(&|envelope| svg::render_svg(&ops, envelope, title, full.lang))?,
                "card.svg",
            )
        }
        CardFormat::Jxl => {
            let pixmap = raster::rasterize(&layout::layout_image(&full, &compact_url)?)?;
            let codestream = jxl::encode_codestream(
                &raster::opaque_rgb(&pixmap),
                pixmap.width(),
                pixmap.height(),
            )?;
            (
                seal(&|envelope| jxl::jxl_container(&codestream, envelope))?,
                "card.jxl",
            )
        }
    };
    Ok(RenderedCard {
        bytes,
        file_name,
        fingerprint,
        compact_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::FindingType;
    use keys::{
        b64url_decode, generate_key, parse_compact_url, parse_file_envelope, verify_signature,
    };
    use payload::parse_full_payload;
    use raster::read_png_envelope;
    use test_fixtures::story_input;
    use verify::{CardSource, ContentVerdict, check_integrity};

    #[test]
    fn render_card_signs_what_it_draws() {
        let key = generate_key().expect("key");
        let input = serde_json::to_vec(&story_input(FindingType::NPlusOneSql)).expect("json");
        let card = render_card(&input, &key.pkcs8, CardFormat::Png).expect("renders");
        assert_eq!(card.file_name, "card.png");

        let envelope = read_png_envelope(&card.bytes).expect("envelope");
        let envelope = parse_file_envelope(&envelope).expect("file envelope");
        let full = parse_full_payload(&envelope.payload).expect("full payload");
        let public_key = b64url_decode(&full.public_key).expect("public key");
        assert!(verify_signature(
            &public_key,
            &envelope.payload,
            &envelope.signature
        ));
        assert_eq!(full.key, card.fingerprint);
        assert_eq!(card.fingerprint, key.fingerprint);

        let (compact, compact_signature) = parse_compact_url(&card.compact_url).expect("url");
        assert!(verify_signature(&public_key, &compact, &compact_signature));

        let integrity = check_integrity(CardSource::Png(&card.bytes)).expect("checks");
        assert_eq!(integrity.content, ContentVerdict::Intact);
    }

    #[test]
    fn render_card_signs_what_it_draws_as_svg() {
        let key = generate_key().expect("key");
        let input = serde_json::to_vec(&story_input(FindingType::NPlusOneSql)).expect("json");
        let card = render_card(&input, &key.pkcs8, CardFormat::Svg).expect("renders");
        assert_eq!(card.file_name, "card.svg");

        let envelope = svg::read_svg_envelope(&card.bytes).expect("envelope");
        let envelope = parse_file_envelope(&envelope).expect("file envelope");
        let full = parse_full_payload(&envelope.payload).expect("full payload");
        let public_key = b64url_decode(&full.public_key).expect("public key");
        assert!(verify_signature(
            &public_key,
            &envelope.payload,
            &envelope.signature
        ));

        let png = render_card(&input, &key.pkcs8, CardFormat::Png).expect("renders");
        assert_eq!(card.compact_url, png.compact_url);
        assert_eq!(card.fingerprint, png.fingerprint);

        let integrity = check_integrity(CardSource::Svg(&card.bytes)).expect("checks");
        assert_eq!(integrity.content, ContentVerdict::Intact);
    }

    #[test]
    fn render_card_signs_what_it_draws_as_jxl() {
        let key = generate_key().expect("key");
        let input = serde_json::to_vec(&story_input(FindingType::NPlusOneSql)).expect("json");
        let card = render_card(&input, &key.pkcs8, CardFormat::Jxl).expect("renders");
        assert_eq!(card.file_name, "card.jxl");

        let envelope = jxl::read_jxl_envelope(&card.bytes).expect("envelope");
        let envelope = parse_file_envelope(&envelope).expect("file envelope");
        let full = parse_full_payload(&envelope.payload).expect("full payload");
        let public_key = b64url_decode(&full.public_key).expect("public key");
        assert!(verify_signature(
            &public_key,
            &envelope.payload,
            &envelope.signature
        ));

        let png = render_card(&input, &key.pkcs8, CardFormat::Png).expect("renders");
        assert_eq!(card.compact_url, png.compact_url);
        assert_eq!(card.fingerprint, png.fingerprint);

        let integrity = check_integrity(CardSource::Jxl(&card.bytes)).expect("checks");
        assert_eq!(integrity.content, ContentVerdict::Intact);
    }

    #[test]
    fn same_card_renders_the_same_bytes_in_every_format() {
        let key = generate_key().expect("key");
        let input = serde_json::to_vec(&story_input(FindingType::NPlusOneSql)).expect("json");
        for format in [
            CardFormat::Png,
            CardFormat::Pdf,
            CardFormat::Svg,
            CardFormat::Jxl,
        ] {
            let once = render_card(&input, &key.pkcs8, format).expect("renders");
            let twice = render_card(&input, &key.pkcs8, format).expect("renders");
            assert!(once.bytes == twice.bytes, "{format:?}");
            assert_eq!(format.as_str(), &once.file_name[5..]);
        }
    }
}
