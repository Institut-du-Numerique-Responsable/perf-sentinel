//! Card keys: Ed25519 keys and signatures, their fingerprints, and the
//! two envelopes a signed payload travels in (the file one and the URL).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{CardError, CardResult};

/// Prefix of a key fingerprint, versioning how it is derived.
pub const FINGERPRINT_PREFIX: &str = "ps1:";
/// The verify page; the compact payload travels in its fragment, which
/// a browser never sends to the server.
pub const VERIFY_URL_PREFIX: &str = "https://perf-sentinel.dev/verifier#c=";

/// Base64url without padding.
#[must_use]
pub fn b64url_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Strict, canonical base64url: `None` on padding, a character outside
/// `[A-Za-z0-9_-]`, a length `% 4 == 1` or non-zero trailing bits, so one
/// changed character never decodes to the same bytes.
#[must_use]
pub fn b64url_decode(text: &str) -> Option<Vec<u8>> {
    // The engine refuses padding, foreign characters, `len % 4 == 1` and
    // non-zero trailing bits.
    URL_SAFE_NO_PAD.decode(text).ok()
}

/// `ps1:` then the base64url of the first 16 bytes of SHA-256 over the raw
/// 32-byte public key.
#[must_use]
pub fn fingerprint_of(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    format!("{FINGERPRINT_PREFIX}{}", b64url_encode(&digest[..16]))
}

/// A fresh key: its PKCS#8 v2 bytes to store, its public half to publish.
pub struct GeneratedKey {
    pub pkcs8: Vec<u8>,
    pub public_key: String,
    pub fingerprint: String,
}

/// Generate an Ed25519 key from the system random source.
///
/// # Errors
///
/// `signing_key_unreadable` when the system random source fails.
pub fn generate_key() -> Result<GeneratedKey, CardError> {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| key_error("the system random source failed"))?;
    let signer = CardSigner::from_pkcs8(document.as_ref())?;
    Ok(GeneratedKey {
        pkcs8: document.as_ref().to_vec(),
        public_key: signer.public_key_b64(),
        fingerprint: signer.fingerprint(),
    })
}

/// A loaded signing key.
pub struct CardSigner {
    pair: Ed25519KeyPair,
}

impl CardSigner {
    /// Load a PKCS#8 v2 key, the form `generate_key` writes.
    ///
    /// # Errors
    ///
    /// `signing_key_unreadable` for anything else, a v1 key included: v2
    /// carries the public half, which ring checks against the private one.
    pub fn from_pkcs8(der: &[u8]) -> Result<Self, CardError> {
        Ed25519KeyPair::from_pkcs8(der)
            .map(|pair| Self { pair })
            .map_err(|e| key_error(format!("not an Ed25519 PKCS#8 v2 key: {e}")))
    }

    #[must_use]
    pub fn public_key_b64(&self) -> String {
        b64url_encode(self.pair.public_key().as_ref())
    }

    #[must_use]
    pub fn fingerprint(&self) -> String {
        fingerprint_of(self.pair.public_key().as_ref())
    }

    /// Ed25519 is deterministic: one message, one signature.
    #[must_use]
    pub fn sign_b64(&self, message: &[u8]) -> String {
        b64url_encode(self.pair.sign(message).as_ref())
    }
}

/// Whether `signature` is an Ed25519 signature of `message` by `public_key`.
#[must_use]
pub fn verify_signature(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(message, signature)
        .is_ok()
}

/// The file envelope, keys sorted, no whitespace.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<'a> {
    payload: &'a str,
    signature: &'a str,
}

/// `{"payload":"<b64url>","signature":"<b64url>"}`, the envelope a PNG
/// or PDF card carries.
///
/// # Panics
///
/// Never: two strings always serialize.
#[must_use]
pub fn file_envelope(payload: &[u8], signature_b64: &str) -> String {
    let payload = b64url_encode(payload);
    serde_json::to_string(&Envelope {
        payload: &payload,
        signature: signature_b64,
    })
    .expect("two strings serialize")
}

/// The payload and signature bytes of a file envelope.
///
/// # Errors
///
/// `malformed_input` unless the text is an object with exactly `payload`
/// and `signature`, both canonical base64url, the signature 64 bytes.
pub fn parse_file_envelope(text: &str) -> Result<(Vec<u8>, Vec<u8>), CardError> {
    let envelope: Envelope<'_> =
        serde_json::from_str(text).map_err(|e| malformed(format!("card envelope: {e}")))?;
    decode_pair(envelope.payload, envelope.signature)
}

/// `VERIFY_URL_PREFIX` then `<b64url compact>.<b64url signature>`.
#[must_use]
pub fn compact_url(compact: &[u8], signature_b64: &str) -> String {
    format!(
        "{VERIFY_URL_PREFIX}{}.{signature_b64}",
        b64url_encode(compact)
    )
}

/// The compact payload and signature bytes of a verify URL: what follows
/// the first `#c=`, split on its last `.`.
///
/// # Errors
///
/// `malformed_input` when the URL has no `#c=`, no `.` after it, or either
/// half is not canonical base64url, or the signature is not 64 bytes.
pub fn parse_compact_url(url: &str) -> Result<(Vec<u8>, Vec<u8>), CardError> {
    let (_, fragment) = url
        .split_once("#c=")
        .ok_or_else(|| malformed("the URL carries no #c= fragment"))?;
    let (compact, signature) = fragment
        .rsplit_once('.')
        .ok_or_else(|| malformed("the #c= fragment has no signature"))?;
    decode_pair(compact, signature)
}

fn decode_pair(payload: &str, signature: &str) -> Result<(Vec<u8>, Vec<u8>), CardError> {
    let payload = b64url_decode(payload)
        .ok_or_else(|| malformed("the payload is not canonical base64url"))?;
    let signature = b64url_decode(signature)
        .filter(|bytes| bytes.len() == 64)
        .ok_or_else(|| malformed("the signature is not 64 bytes of canonical base64url"))?;
    Ok((payload, signature))
}

fn malformed(detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::InputError, "malformed_input", detail)
}

fn key_error(detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::InputError, "signing_key_unreadable", detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }

    #[test]
    fn a_generated_key_signs_and_verifies() {
        let key = generate_key().expect("key");
        let signer = CardSigner::from_pkcs8(&key.pkcs8).expect("signer");
        assert_eq!(key.pkcs8.len(), 83);
        let pk = b64url_decode(&key.public_key).expect("public key");
        assert_eq!(pk.len(), 32);
        assert_eq!(signer.public_key_b64(), key.public_key);
        assert_eq!(signer.fingerprint(), key.fingerprint);
        let sig = b64url_decode(&signer.sign_b64(b"abc")).expect("signature");
        assert!(verify_signature(&pk, b"abc", &sig));
        assert!(!verify_signature(&pk, b"abd", &sig));
    }

    #[test]
    fn signatures_are_deterministic() {
        let key = generate_key().expect("key");
        let signer = CardSigner::from_pkcs8(&key.pkcs8).expect("signer");
        assert_eq!(signer.sign_b64(b"message"), signer.sign_b64(b"message"));
    }

    #[test]
    fn the_fingerprint_is_ps1_and_22_characters() {
        let key = generate_key().expect("key");
        let pk = b64url_decode(&key.public_key).expect("public key");
        let fingerprint = fingerprint_of(&pk);
        assert!(fingerprint.starts_with(FINGERPRINT_PREFIX));
        assert_eq!(fingerprint.len(), 26);
        let digest = Sha256::digest(&pk);
        assert_eq!(fingerprint, format!("ps1:{}", b64url_encode(&digest[..16])));
    }

    #[test]
    fn a_pkcs8_v1_key_is_refused() {
        let v1 = hex(
            "302e020100300506032b657004220420d4ee72dbf913584ad5b6d8f1f769f8ad3afe7c28cbf1d4fbe097a88f44755842",
        );
        assert_eq!(v1.len(), 48);
        let Err(error) = CardSigner::from_pkcs8(&v1) else {
            panic!("a v1 key must be refused");
        };
        assert_eq!(error.reason, "signing_key_unreadable");
        assert_eq!(error.result, CardResult::InputError);
    }

    #[test]
    fn the_envelope_round_trips_and_is_canonical() {
        let sig = b64url_encode(&[7u8; 64]);
        let envelope = file_envelope(b"{}", &sig);
        assert!(envelope.starts_with(r#"{"payload":""#), "{envelope}");
        let (payload, signature) = parse_file_envelope(&envelope).expect("parses");
        assert_eq!(payload, b"{}");
        assert_eq!(signature, vec![7u8; 64]);

        let extra = format!(r#"{{"extra":1,"payload":"e30","signature":"{sig}"}}"#);
        let padded = format!(r#"{{"payload":"e30=","signature":"{sig}"}}"#);
        let short = format!(
            r#"{{"payload":"e30","signature":"{}"}}"#,
            b64url_encode(&[7u8; 63])
        );
        for bad in [extra, padded, short, String::from("not json")] {
            let error = parse_file_envelope(&bad).expect_err("refused");
            assert_eq!(error.reason, "malformed_input", "{bad}");
        }
    }

    #[test]
    fn the_compact_url_round_trips() {
        let sig = b64url_encode(&[9u8; 64]);
        let url = compact_url(br#"{"v":1}"#, &sig);
        assert!(url.starts_with(VERIFY_URL_PREFIX), "{url}");
        let (compact, signature) = parse_compact_url(&url).expect("parses");
        assert_eq!(compact, br#"{"v":1}"#);
        assert_eq!(signature, vec![9u8; 64]);

        let error = parse_compact_url("https://perf-sentinel.dev/verifier").expect_err("refused");
        assert_eq!(error.reason, "malformed_input");
    }

    #[test]
    fn base64url_is_canonical() {
        for len in 0..=70u8 {
            let bytes: Vec<u8> = (0..len)
                .map(|i| i.wrapping_mul(37).wrapping_add(len))
                .collect();
            assert_eq!(
                b64url_decode(&b64url_encode(&bytes)),
                Some(bytes),
                "length {len}"
            );
        }
        assert!(b64url_decode("AA").is_some());
        assert_eq!(b64url_decode("AB"), None);
        assert_eq!(b64url_decode("A"), None);
        assert_eq!(b64url_decode("AA=="), None);
    }
}
