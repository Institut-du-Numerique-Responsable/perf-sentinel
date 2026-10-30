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
/// Level 2: the domain publishes the key, not revoked, valid on the day the
/// card states it was signed.
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
    // The signing day is the signer's own claim: a stolen key could date a
    // card before any revocation day, so a revoked key vouches for nothing.
    if entry.revoked.is_some() {
        return binding(1, KeyStatus::Revoked, "key_revoked");
    }
    let status = if entry.to.is_some() {
        KeyStatus::Retired
    } else {
        KeyStatus::Current
    };
    if signed_day < entry.from || entry.to.is_some_and(|day| signed_day > day) {
        return binding(1, status, "signed_outside_validity");
    }
    binding(2, status, "key_published")
}

/// What one place a domain publishes its keys in said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// The keys it lists.
    Found(Vec<PublishedKey>),
    /// It answers that it publishes no key.
    Absent,
    /// It could not be read: no answer, an error status, a redirect, a
    /// body that is too large or not the expected shape.
    Failed,
}

/// Read a `_perf-sentinel` TXT record, `v=ps1; k=<fingerprint>;
/// from=YYYY-MM-DD[; to=YYYY-MM-DD][; revoked=YYYY-MM-DD]`, as a resolver
/// hands it over: bare, in one level of quotes, or split in quoted strings
/// (`"a" "b"`). Unknown parameters are ignored; any other record, a bad
/// date or a parameter given twice is `None`.
#[must_use]
pub fn parse_txt_record(data: &str) -> Option<PublishedKey> {
    let data = data.trim();
    let text: String = match data.strip_prefix('"').and_then(|d| d.strip_suffix('"')) {
        // `a" "b` splits on its quotes into a, the gap, b: keep the strings.
        Some(inner) => inner.split('"').step_by(2).collect(),
        None => data.to_owned(),
    };
    let mut fields = text.split(';').map(str::trim).filter(|f| !f.is_empty());
    if fields.next()? != "v=ps1" {
        return None;
    }
    let (mut k, mut from, mut to, mut revoked) = (None, None, None, None);
    for field in fields {
        let (name, value) = field.split_once('=')?;
        let slot = match name.trim() {
            "k" => &mut k,
            "from" => &mut from,
            "to" => &mut to,
            "revoked" => &mut revoked,
            _ => continue,
        };
        if slot.replace(value.trim()).is_some() {
            return None;
        }
    }
    // Outer `None`: a bad date; inner: the parameter is absent.
    let day = |v: Option<&str>| {
        v.map(|t| parse_iso_date_to_epoch_day(t).ok_or(()))
            .transpose()
            .ok()
    };
    Some(PublishedKey {
        fingerprint: k?.to_owned(),
        public_key: None,
        from: day(from)??,
        to: day(to)?,
        revoked: day(revoked)?,
    })
}

/// Read a DNS-over-HTTPS JSON answer (`application/dns-json`) for the TXT
/// records of `_perf-sentinel.<domain>`: NXDOMAIN or no `v=ps1` record is
/// `Absent`, any other failure `Failed`.
#[must_use]
pub fn parse_doh_answer(json: &[u8]) -> Lookup {
    const NOERROR: u32 = 0;
    const NXDOMAIN: u32 = 3;
    const TXT: u16 = 16;
    match serde_json::from_slice::<DohAnswer>(json) {
        Ok(answer) if answer.status == NXDOMAIN => Lookup::Absent,
        Ok(answer) if answer.status == NOERROR => {
            let keys: Vec<PublishedKey> = answer
                .answer
                .iter()
                .filter(|record| record.kind == TXT)
                .filter_map(|record| parse_txt_record(&record.data))
                .collect();
            if keys.is_empty() {
                Lookup::Absent
            } else {
                Lookup::Found(keys)
            }
        }
        _ => Lookup::Failed,
    }
}

/// The fields of a DNS-over-HTTPS JSON answer this reads; resolvers add
/// others.
#[derive(Deserialize)]
struct DohAnswer {
    #[serde(rename = "Status")]
    status: u32,
    #[serde(rename = "Answer", default)]
    answer: Vec<DohRecord>,
}

#[derive(Deserialize)]
struct DohRecord {
    #[serde(rename = "type")]
    kind: u16,
    data: String,
}

/// The key list of a domain from its DNS record and its well-known file:
/// the union by fingerprint, an entry listed in both keeping every limit
/// either sets (so revoked wins over retired, retired over current).
/// Nothing published anywhere is an empty list.
///
/// # Errors
///
/// `binding_check_failed` when a lookup failed and none found a key (an
/// empty list counts as none), since the missing one might have revoked
/// the key.
pub fn merge_lookups(
    dns: Lookup,
    well_known: Lookup,
) -> Result<Option<Vec<PublishedKey>>, &'static str> {
    let failed = dns == Lookup::Failed || well_known == Lookup::Failed;
    let mut found = false;
    let mut merged: Vec<PublishedKey> = Vec::new();
    for lookup in [dns, well_known] {
        let Lookup::Found(keys) = lookup else {
            continue;
        };
        // An empty list proves nothing when the other lookup failed.
        found |= !keys.is_empty();
        for key in keys {
            match merged.iter_mut().find(|m| m.fingerprint == key.fingerprint) {
                Some(entry) => restrict(entry, key),
                None => merged.push(key),
            }
        }
    }
    if failed && !found {
        return Err("binding_check_failed");
    }
    Ok(Some(merged))
}

/// Narrow `entry` by every limit `other` sets for the same key.
fn restrict(entry: &mut PublishedKey, other: PublishedKey) {
    let earliest = |a: Option<u32>, b: Option<u32>| match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    entry.from = entry.from.max(other.from);
    entry.to = earliest(entry.to, other.to);
    entry.revoked = earliest(entry.revoked, other.revoked);
    if entry.public_key.is_none() {
        entry.public_key = other.public_key;
    }
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
                (1, KeyStatus::Revoked, "key_revoked"),
                "{revoked}"
            );
        }
    }

    #[test]
    fn revoked_key_never_reaches_level_2() {
        // The signing day is the signer's own claim: a stolen key can date a
        // card before any revocation day, so no revocation day vouches.
        for revoked in ["2026-10-02", "2026-12-31", "2026-10-01", "2026-09-15"] {
            assert_eq!(
                judged(&[key("2026-01-01", Some("2026-12-31"), Some(revoked))]),
                (1, KeyStatus::Revoked, "key_revoked"),
                "{revoked}"
            );
        }
        assert_eq!(
            judged(&[key("2026-10-02", None, Some("2026-10-05"))]),
            (1, KeyStatus::Revoked, "key_revoked"),
            "revocation is checked before the validity window"
        );
        let mut other = key("2026-01-01", None, Some("2026-10-02"));
        other.public_key = Some("CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg".to_owned());
        assert_eq!(
            judged(&[other]),
            (1, KeyStatus::Unknown, "public_key_mismatch")
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

    #[test]
    fn txt_records_read_in_both_resolver_styles() {
        let published = PublishedKey {
            fingerprint: FP.to_owned(),
            public_key: None,
            from: day("2026-10-30"),
            to: None,
            revoked: None,
        };
        // Cloudflare quotes the string, Google does not.
        for data in [
            "\"v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; from=2026-10-30\"",
            "v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; from=2026-10-30",
            "v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; from=2026-10-30; note=ours",
        ] {
            assert_eq!(parse_txt_record(data), Some(published.clone()), "{data}");
        }
        let split = parse_txt_record(
            "\"v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA\" \"; from=2026-10-30; revoked=2026-11-02\"",
        );
        assert_eq!(
            split,
            Some(PublishedKey {
                revoked: Some(day("2026-11-02")),
                ..published.clone()
            })
        );
        assert_eq!(
            parse_txt_record("v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; from=2026-10-30; to=2026-12-31"),
            Some(PublishedKey {
                to: Some(day("2026-12-31")),
                ..published
            })
        );
        for other in [
            "v=spf1 -all",
            "",
            "k=ps1:AAAAAAAAAAAAAAAAAAAAAA; v=ps1; from=2026-10-30",
            "v=ps1; from=2026-10-30",
            "v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA",
            "v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; from=2026-10-3",
            "v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; from=2026-10-30; revoked=soon",
            "v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; k=ps1:BBBBBBBBBBBBBBBBBBBBBB; from=2026-10-30",
        ] {
            assert_eq!(parse_txt_record(other), None, "{other}");
        }
    }

    #[test]
    fn doh_answers_tell_absent_from_found() {
        assert_eq!(parse_doh_answer(br#"{"Status":3}"#), Lookup::Absent);
        assert_eq!(parse_doh_answer(br#"{"Status":0}"#), Lookup::Absent);
        assert_eq!(
            parse_doh_answer(
                br#"{"Status":0,"Answer":[{"name":"_perf-sentinel.acme.fr","type":16,"TTL":300,"data":"\"v=spf1 -all\""}]}"#
            ),
            Lookup::Absent
        );
        let found = parse_doh_answer(
            br#"{"Status":0,"TC":false,"Question":[{"name":"_perf-sentinel.acme.fr","type":16}],"Answer":[
                {"name":"_perf-sentinel.acme.fr","type":5,"TTL":300,"data":"v=ps1; k=ps1:CCCCCCCCCCCCCCCCCCCCCC; from=2026-01-01"},
                {"name":"_perf-sentinel.acme.fr","type":16,"TTL":300,"data":"\"v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; from=2026-10-30\""},
                {"name":"_perf-sentinel.acme.fr","type":16,"TTL":300,"data":"\"v=spf1 -all\""}
            ]}"#,
        );
        assert_eq!(found, Lookup::Found(vec![key("2026-10-30", None, None)]));
        for failed in [
            &b"garbage"[..],
            br#"{"Status":2}"#,
            br#"{"Answer":[]}"#,
            br#"{"Status":0,"Answer":{}}"#,
        ] {
            assert_eq!(
                parse_doh_answer(failed),
                Lookup::Failed,
                "{}",
                String::from_utf8_lossy(failed)
            );
        }
    }

    #[test]
    fn merge_prefers_the_most_restrictive_entry() {
        let current = key("2026-01-01", None, None);
        let retired = key("2026-01-01", Some("2026-12-31"), None);
        let revoked = key("2026-01-01", None, Some("2026-09-15"));
        let mut listed = current.clone();
        listed.public_key = Some(PK.to_owned());
        let mut other = key("2025-01-01", None, None);
        other.fingerprint = "ps1:BBBBBBBBBBBBBBBBBBBBBB".to_owned();

        for (dns, well_known, expected) in [
            (&revoked, &current, &revoked),
            (&current, &revoked, &revoked),
            (&retired, &current, &retired),
            (&current, &retired, &retired),
            (
                &retired,
                &revoked,
                &key("2026-01-01", Some("2026-12-31"), Some("2026-09-15")),
            ),
        ] {
            let merged = merge_lookups(
                Lookup::Found(vec![dns.clone()]),
                Lookup::Found(vec![well_known.clone()]),
            );
            assert_eq!(merged, Ok(Some(vec![expected.clone()])));
            let keys = merged.expect("merged").expect("a list");
            assert_eq!(
                judge(FP, PK, SIGNED, Some(&keys)).key,
                judge(FP, PK, SIGNED, Some(std::slice::from_ref(expected))).key
            );
        }

        // The well-known public key survives a DNS entry without one, and a
        // key listed in one place only is kept.
        let merged = merge_lookups(
            Lookup::Found(vec![revoked.clone(), other.clone()]),
            Lookup::Found(vec![listed]),
        );
        let mut revoked_with_key = revoked;
        revoked_with_key.public_key = Some(PK.to_owned());
        assert_eq!(merged, Ok(Some(vec![revoked_with_key, other])));
    }

    #[test]
    fn merge_absent_everywhere_is_not_published() {
        assert_eq!(
            merge_lookups(Lookup::Absent, Lookup::Absent),
            Ok(Some(vec![]))
        );
        let only = key("2026-01-01", None, None);
        for (dns, well_known) in [
            (Lookup::Found(vec![only.clone()]), Lookup::Absent),
            (Lookup::Absent, Lookup::Found(vec![only.clone()])),
            (Lookup::Found(vec![only.clone()]), Lookup::Failed),
            (Lookup::Failed, Lookup::Found(vec![only.clone()])),
        ] {
            assert_eq!(merge_lookups(dns, well_known), Ok(Some(vec![only.clone()])));
        }
        let keys = merge_lookups(Lookup::Absent, Lookup::Absent)
            .expect("merged")
            .expect("a list");
        assert_eq!(
            judge(FP, PK, SIGNED, Some(&keys)).reason,
            "key_not_published"
        );
    }

    #[test]
    fn merge_with_one_failure_and_nothing_found_cannot_conclude() {
        for (dns, well_known) in [
            (Lookup::Failed, Lookup::Absent),
            (Lookup::Absent, Lookup::Failed),
            (Lookup::Failed, Lookup::Failed),
            (Lookup::Failed, Lookup::Found(vec![])),
            (Lookup::Found(vec![]), Lookup::Failed),
        ] {
            assert_eq!(merge_lookups(dns, well_known), Err("binding_check_failed"));
        }
    }
}
