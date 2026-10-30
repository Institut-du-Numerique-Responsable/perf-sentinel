//! The binding of a card's key to a domain: the key list an organization
//! publishes (contract C3, `/.well-known/perf-sentinel-key`) and the level
//! it gives a card signed on a given day.

use serde::{Deserialize, Serialize};

use super::{CardError, CardResult};
use crate::time::parse_iso_date_to_epoch_day;

/// The DNS-over-HTTPS service `verify-card --online` asks unless `--doh`
/// names another.
pub const DEFAULT_DOH_URL: &str = "https://cloudflare-dns.com/dns-query";

/// One published key; the days are epoch days.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedKey {
    pub fingerprint: String,
    pub public_key: Option<String>,
    pub from: u32,
    pub to: Option<u32>,
    pub revoked: Option<u32>,
}

/// What the published list says of the card's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStatus {
    Current,
    Retired,
    Revoked,
    Unknown,
}

/// Level 1: the signature holds, nothing ties the key to the domain.
/// Level 2: the domain publishes the key for the day the card was signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub level: u8,
    pub key: KeyStatus,
    pub reason: &'static str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyList {
    keys: Vec<RawKey>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKey {
    fingerprint: String,
    #[serde(default)]
    public_key: Option<String>,
    from: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    revoked: Option<String>,
}

/// Parse `{"keys":[{"fingerprint","public_key","from","to","revoked"}]}`,
/// dates `YYYY-MM-DD`, `public_key`, `to` and `revoked` absent or null.
///
/// # Errors
///
/// `malformed_input` on any other shape, a bad date, or a fingerprint
/// listed twice, which would leave the binding ambiguous.
pub fn parse_key_list(json: &[u8]) -> Result<Vec<PublishedKey>, CardError> {
    let list: KeyList =
        serde_json::from_slice(json).map_err(|e| malformed(format!("key list: {e}")))?;
    let mut keys: Vec<PublishedKey> = Vec::with_capacity(list.keys.len());
    for raw in list.keys {
        if keys.iter().any(|k| k.fingerprint == raw.fingerprint) {
            return Err(malformed(format!(
                "key list: {} is listed twice",
                raw.fingerprint
            )));
        }
        keys.push(PublishedKey {
            from: date(&raw.from)?,
            to: raw.to.as_deref().map(date).transpose()?,
            revoked: raw.revoked.as_deref().map(date).transpose()?,
            fingerprint: raw.fingerprint,
            public_key: raw.public_key,
        });
    }
    Ok(keys)
}

/// The binding of a key, signed on `signed_day`, to the list its domain
/// publishes (`None` when no list was read).
#[must_use]
pub fn judge(
    fingerprint: &str,
    public_key_b64: &str,
    signed_day: u32,
    published: Option<&[PublishedKey]>,
) -> Binding {
    let binding = |level, key, reason| Binding { level, key, reason };
    let Some(published) = published else {
        return binding(1, KeyStatus::Unknown, "no_key_list");
    };
    let Some(entry) = published.iter().find(|k| k.fingerprint == fingerprint) else {
        return binding(1, KeyStatus::Unknown, "key_not_published");
    };
    if entry
        .public_key
        .as_deref()
        .is_some_and(|pk| pk != public_key_b64)
    {
        return binding(1, KeyStatus::Unknown, "public_key_mismatch");
    }
    if entry.revoked.is_some_and(|day| signed_day >= day) {
        return binding(1, KeyStatus::Revoked, "signed_after_revocation");
    }
    let status = if entry.to.is_some() {
        KeyStatus::Retired
    } else {
        KeyStatus::Current
    };
    if signed_day < entry.from || entry.to.is_some_and(|day| signed_day > day) {
        return binding(1, status, "signed_outside_validity");
    }
    if entry.revoked.is_some() {
        return binding(2, KeyStatus::Revoked, "signed_before_revocation");
    }
    binding(2, status, "key_published")
}

fn date(text: &str) -> Result<u32, CardError> {
    parse_iso_date_to_epoch_day(text)
        .ok_or_else(|| malformed(format!("key list: {text:?} is not a YYYY-MM-DD date")))
}

fn malformed(detail: impl Into<String>) -> CardError {
    CardError::new(CardResult::InputError, "malformed_input", detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str = "ps1:AAAAAAAAAAAAAAAAAAAAAA";
    const PK: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc";
    /// 2026-10-01, the day the fixtures are generated.
    const SIGNED: u32 = 20727;

    fn day(text: &str) -> u32 {
        parse_iso_date_to_epoch_day(text).expect("date")
    }

    fn key(from: &str, to: Option<&str>, revoked: Option<&str>) -> PublishedKey {
        PublishedKey {
            fingerprint: FP.to_owned(),
            public_key: None,
            from: day(from),
            to: to.map(day),
            revoked: revoked.map(day),
        }
    }

    fn judged(keys: &[PublishedKey]) -> (u8, KeyStatus, &'static str) {
        let b = judge(FP, PK, SIGNED, Some(keys));
        (b.level, b.key, b.reason)
    }

    #[test]
    fn judge_without_a_list_is_level_1() {
        let b = judge(FP, PK, SIGNED, None);
        assert_eq!(
            (b.level, b.key, b.reason),
            (1, KeyStatus::Unknown, "no_key_list")
        );
    }

    #[test]
    fn judge_unpublished_key() {
        let mut other = key("2026-01-01", None, None);
        other.fingerprint = "ps1:BBBBBBBBBBBBBBBBBBBBBB".to_owned();
        assert_eq!(
            judged(&[other]),
            (1, KeyStatus::Unknown, "key_not_published")
        );
        assert_eq!(judged(&[]), (1, KeyStatus::Unknown, "key_not_published"));
    }

    #[test]
    fn judge_public_key_mismatch() {
        let mut listed = key("2026-01-01", None, None);
        listed.public_key = Some("CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg".to_owned());
        assert_eq!(
            judged(&[listed.clone()]),
            (1, KeyStatus::Unknown, "public_key_mismatch")
        );
        listed.public_key = Some(PK.to_owned());
        assert_eq!(judged(&[listed]), (2, KeyStatus::Current, "key_published"));
    }

    #[test]
    fn judge_after_revocation_is_untrusted() {
        for revoked in ["2026-09-15", "2026-10-01"] {
            assert_eq!(
                judged(&[key("2026-01-01", None, Some(revoked))]),
                (1, KeyStatus::Revoked, "signed_after_revocation"),
                "{revoked}"
            );
        }
    }

    #[test]
    fn judge_before_revocation_stays_attached() {
        assert_eq!(
            judged(&[key("2026-01-01", Some("2026-12-31"), Some("2026-10-02"))]),
            (2, KeyStatus::Revoked, "signed_before_revocation")
        );
    }

    #[test]
    fn judge_outside_validity() {
        assert_eq!(
            judged(&[key("2026-10-02", None, None)]),
            (1, KeyStatus::Current, "signed_outside_validity")
        );
        assert_eq!(
            judged(&[key("2026-01-01", Some("2026-09-30"), None)]),
            (1, KeyStatus::Retired, "signed_outside_validity")
        );
    }

    #[test]
    fn judge_retired_key_still_attaches_old_cards() {
        assert_eq!(
            judged(&[key("2026-01-01", Some("2026-10-01"), None)]),
            (2, KeyStatus::Retired, "key_published")
        );
    }

    #[test]
    fn judge_current_key() {
        assert_eq!(
            judged(&[key("2026-10-01", None, None)]),
            (2, KeyStatus::Current, "key_published")
        );
    }

    #[test]
    fn parse_key_list_reads_the_well_known_shape() {
        let keys = parse_key_list(
            format!(
                r#"{{"keys":[
                    {{"fingerprint":"{FP}","public_key":"{PK}","from":"2026-01-01","to":null,"revoked":null}},
                    {{"fingerprint":"ps1:BBBBBBBBBBBBBBBBBBBBBB","from":"2025-01-01","to":"2025-12-31"}}
                ]}}"#
            )
            .as_bytes(),
        )
        .expect("parses");
        assert_eq!(
            keys,
            [
                PublishedKey {
                    fingerprint: FP.to_owned(),
                    public_key: Some(PK.to_owned()),
                    from: day("2026-01-01"),
                    to: None,
                    revoked: None,
                },
                PublishedKey {
                    fingerprint: "ps1:BBBBBBBBBBBBBBBBBBBBBB".to_owned(),
                    public_key: None,
                    from: day("2025-01-01"),
                    to: Some(day("2025-12-31")),
                    revoked: None,
                },
            ]
        );

        for bad in [
            format!(r#"{{"keys":[{{"fingerprint":"{FP}","from":"2026-1-01"}}]}}"#),
            format!(r#"{{"keys":[{{"fingerprint":"{FP}","from":"2026-01-01","note":"x"}}]}}"#),
            String::from(r#"{"keys":[],"extra":1}"#),
            format!(
                r#"{{"keys":[{{"fingerprint":"{FP}","from":"2026-01-01","revoked":"soon"}}]}}"#
            ),
            format!(
                r#"{{"keys":[{{"fingerprint":"{FP}","from":"2026-01-01"}},{{"fingerprint":"{FP}","from":"2026-02-01"}}]}}"#
            ),
            String::from("not json"),
        ] {
            let error = parse_key_list(bad.as_bytes()).expect_err("refused");
            assert_eq!(error.reason, "malformed_input", "{bad}");
            assert_eq!(error.result, CardResult::InputError, "{bad}");
        }
    }
}
