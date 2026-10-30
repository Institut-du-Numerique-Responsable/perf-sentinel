//! Share cards: the signed payloads a card carries, its keys, and their
//! rendering. Contract C2/C3 in `docs/SHARE-CARD.md`.

mod copy;
pub mod keys;
mod layout;
pub mod payload;
pub mod pdf;
pub mod raster;
#[cfg(test)]
mod test_fixtures;

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
/// set only on `unsupported_characters`, to the `card.json` path of the
/// first text the fonts cannot draw.
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
}

/// A signed card, ready to write.
#[derive(Debug)]
pub struct RenderedCard {
    pub bytes: Vec<u8>,
    /// `card.png` or `card.pdf`.
    pub file_name: &'static str,
    pub fingerprint: String,
    pub compact_url: String,
}

/// Parse a `card.json`, sign its full and compact payloads with `pkcs8`,
/// and draw the card carrying both. Same input and key, same bytes.
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
    let envelope = keys::file_envelope(&full_bytes, &signer.sign_b64(&full_bytes));
    let compact_url = keys::compact_url(&compact_bytes, &signer.sign_b64(&compact_bytes));
    let (bytes, file_name) = match format {
        CardFormat::Png => {
            let pixmap = raster::rasterize(&layout::layout_image(&full, &compact_url)?)?;
            (raster::encode_png(&pixmap, &envelope)?, "card.png")
        }
        CardFormat::Pdf => {
            let pages = layout::layout_carousel(&full, &compact_url)?;
            (
                pdf::render_carousel_pdf(&pages, &envelope, &full)?,
                "card.pdf",
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

    #[test]
    fn render_card_signs_what_it_draws() {
        let key = generate_key().expect("key");
        let input = serde_json::to_vec(&story_input(FindingType::NPlusOneSql)).expect("json");
        let card = render_card(&input, &key.pkcs8, CardFormat::Png).expect("renders");
        assert_eq!(card.file_name, "card.png");

        let envelope = read_png_envelope(&card.bytes).expect("envelope");
        let (payload, signature) = parse_file_envelope(&envelope).expect("file envelope");
        let full = parse_full_payload(&payload).expect("full payload");
        let public_key = b64url_decode(&full.public_key).expect("public key");
        assert!(verify_signature(&public_key, &payload, &signature));
        assert_eq!(full.key, card.fingerprint);
        assert_eq!(card.fingerprint, key.fingerprint);

        let (compact, compact_signature) = parse_compact_url(&card.compact_url).expect("url");
        assert!(verify_signature(&public_key, &compact, &compact_signature));

        let again = render_card(&input, &key.pkcs8, CardFormat::Png).expect("renders");
        assert_eq!(again.bytes, card.bytes);
    }
}
