//! Card keys: canonical base64url.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

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

#[cfg(test)]
mod tests {
    use super::*;

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
