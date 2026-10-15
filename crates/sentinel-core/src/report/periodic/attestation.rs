//! In-toto v1 statement builder for periodic disclosure reports.
//!
//! Produces a sidecar attestation document whose subject is the
//! report's canonical content hash, plus a lean methodology summary. The output is a
//! complete in-toto v1 Statement (`_type`, `predicateType`, `subject`,
//! `predicate`), not just a predicate. Operators sign it with
//! `cosign sign-blob <statement> --bundle <bundle.sig>
//! --new-bundle-format`. Do not pass the statement to
//! `cosign attest-blob --predicate`: that would wrap it in another
//! statement and produce a permanent double-wrapped entry in the
//! Rekor public log.
//!
//! Wire format reference: `docs/design/10-SIGSTORE-ATTESTATION.md`
//! and <https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md>.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::errors::HashError;
use super::hasher::{compute_content_hash, format_sha256};
use super::schema::{PeriodicReport, ReportIntent};

/// in-toto v1 statement type URL.
pub const IN_TOTO_STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";

/// perf-sentinel custom predicate type URL.
///
/// The `perf-sentinel.io` host is a namespace convention (the domain is
/// not formally owned by the project, see design doc 10 for the
/// rationale). Verifiers identify the predicate by exact string match.
pub const PERF_SENTINEL_PREDICATE_TYPE: &str = "https://perf-sentinel.io/attestation/v1";

/// Conventional subject name for the report file, used when the caller
/// does not pass an explicit name to [`build_in_toto_statement`].
pub const DEFAULT_SUBJECT_NAME: &str = "perf-sentinel-report.json";

/// Value of [`PerfSentinelPredicate::subject_binding`] when the subject
/// digest is the report's canonical content hash.
pub const SUBJECT_BINDING_CONTENT_HASH: &str = "content_hash";

// Only `PartialEq` (no `Eq`) on the statement, predicate, and methodology
// summary: `MethodologySummary.period_coverage: f64` is not `Eq` because
// NaN breaks reflexivity. `PartialEq` is sufficient for `assert_eq!` and
// the serde round-trip tests, and matches Rust's stdlib stance on f64.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InTotoStatement {
    #[serde(rename = "_type")]
    pub statement_type: String,
    #[serde(rename = "predicateType")]
    pub predicate_type: String,
    pub subject: Vec<InTotoSubject>,
    pub predicate: PerfSentinelPredicate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InTotoSubject {
    pub name: String,
    pub digest: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerfSentinelPredicate {
    pub perf_sentinel_version: String,
    pub report_uuid: String,
    pub period: PeriodSummary,
    pub intent: String,
    pub confidentiality_level: String,
    pub organisation: OrganisationSummary,
    pub methodology_summary: MethodologySummary,
    /// What the subject digest names. [`SUBJECT_BINDING_CONTENT_HASH`]
    /// on every statement disclose writes: the canonical content hash,
    /// which stays put when the signature locator is patched into the
    /// report after signing. Absent on statements written before, whose
    /// subject pinned the report file bytes at disclose time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_binding: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodSummary {
    pub from_date: String,
    pub to_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganisationSummary {
    pub name: String,
    pub country: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub identifiers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodologySummary {
    pub sci_specification: String,
    pub conformance: String,
    pub calibration_applied: bool,
    pub period_coverage: f64,
    /// Count of patterns flagged as core-required by the validator at
    /// the time of signing. A consumer can compare this to the live
    /// `core_patterns_required` set defined by the perf-sentinel
    /// version recorded in `predicate.perf_sentinel_version`. A
    /// mismatch suggests the report was generated with a non-standard
    /// core set.
    pub core_patterns_count: u32,
    /// Count of patterns actively enabled at the time of report
    /// generation. Combined with `core_patterns_count`, lets a consumer
    /// detect reports where the enabled set is a strict subset of core
    /// (core patterns dropped post-hoc).
    pub enabled_patterns_count: u32,
    /// Count of patterns explicitly disabled at the time of report
    /// generation. Lets a consumer flag reports that disabled too many
    /// patterns or disabled core patterns.
    pub disabled_patterns_count: u32,
    /// SHA-256 over the sorted, colon-joined names of the core pattern
    /// set declared in `methodology.core_patterns_required`. Lets a
    /// consumer detect substitution as well as shrinkage (covered by
    /// `core_patterns_count`): replacing a canonical core pattern with
    /// another keeps the count constant yet changes the hash. The
    /// reference value for a given `predicate.perf_sentinel_version` is
    /// the hash of the canonical `core_patterns_required()` slice in that
    /// version.
    pub core_patterns_hash: String,
}

/// Build an in-toto v1 statement whose subject is `content_hash_hex`
/// and fold in a lean projection of the disclosure's methodology block.
/// `content_hash_hex` must be the report's canonical content hash
/// without its `sha256:` prefix: unlike the file bytes, it survives the
/// signature locator being patched in after signing, so the signed
/// statement keeps naming the published figures.
#[must_use]
pub fn build_in_toto_statement(report: &PeriodicReport, content_hash_hex: &str) -> InTotoStatement {
    build_in_toto_statement_named(report, content_hash_hex, DEFAULT_SUBJECT_NAME)
}

/// Same as [`build_in_toto_statement`] but lets the caller pick the
/// subject name. Useful when the report file is published under a
/// non-default name (e.g. `2026-Q1-disclosure.json`).
#[must_use]
pub fn build_in_toto_statement_named(
    report: &PeriodicReport,
    content_hash_hex: &str,
    subject_name: &str,
) -> InTotoStatement {
    let mut digest = BTreeMap::new();
    digest.insert("sha256".to_string(), content_hash_hex.to_string());

    let mut identifiers = BTreeMap::new();
    let ids = &report.organisation.identifiers;
    if let Some(v) = &ids.siren {
        identifiers.insert("siren".to_string(), v.clone());
    }
    if let Some(v) = &ids.vat {
        identifiers.insert("vat".to_string(), v.clone());
    }
    if let Some(v) = &ids.lei {
        identifiers.insert("lei".to_string(), v.clone());
    }
    if let Some(v) = &ids.opencorporates_url {
        identifiers.insert("opencorporates_url".to_string(), v.clone());
    }
    if let Some(v) = &ids.domain {
        identifiers.insert("domain".to_string(), v.clone());
    }

    InTotoStatement {
        statement_type: IN_TOTO_STATEMENT_TYPE.to_string(),
        predicate_type: PERF_SENTINEL_PREDICATE_TYPE.to_string(),
        subject: vec![InTotoSubject {
            name: subject_name.to_string(),
            digest,
        }],
        predicate: PerfSentinelPredicate {
            perf_sentinel_version: report.report_metadata.perf_sentinel_version.clone(),
            report_uuid: report.report_metadata.report_uuid.to_string(),
            period: PeriodSummary {
                from_date: report.period.from_date.to_string(),
                to_date: report.period.to_date.to_string(),
            },
            intent: intent_str(report.report_metadata.intent).to_string(),
            confidentiality_level: confidentiality_str(
                report.report_metadata.confidentiality_level,
            )
            .to_string(),
            organisation: OrganisationSummary {
                name: report.organisation.name.clone(),
                country: report.organisation.country.clone(),
                identifiers,
            },
            methodology_summary: MethodologySummary {
                sci_specification: report.methodology.sci_specification.clone(),
                conformance: conformance_str(report.methodology.conformance).to_string(),
                calibration_applied: report.methodology.calibration_inputs.calibration_applied,
                period_coverage: report.aggregate.period_coverage,
                core_patterns_count: u32::try_from(report.methodology.core_patterns_required.len())
                    .unwrap_or(u32::MAX),
                enabled_patterns_count: u32::try_from(report.methodology.enabled_patterns.len())
                    .unwrap_or(u32::MAX),
                disabled_patterns_count: u32::try_from(report.methodology.disabled_patterns.len())
                    .unwrap_or(u32::MAX),
                core_patterns_hash: hash_core_patterns(&report.methodology.core_patterns_required),
            },
            subject_binding: Some(SUBJECT_BINDING_CONTENT_HASH.to_string()),
        },
    }
}

/// How an in-toto statement ties to the report it travels with.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AttestationBinding {
    /// A subject digest is the report's recomputed content hash.
    ContentHash,
    /// Statement written before content-hash subjects, whose subject is
    /// the SHA-256 of these exact report bytes.
    FileBytes,
    /// Statement written before content-hash subjects, whose subject
    /// pins report bytes that changed since, typically when
    /// `integrity.signature` was patched in after signing. Nothing ties
    /// it to these figures.
    Unbindable,
    /// The statement names another report. The string says how.
    Mismatch(String),
}

/// Check that `statement` names `report`. `report_bytes` are the bytes
/// the report was read from, compared only for statements written
/// before content-hash subjects.
///
/// # Errors
///
/// Returns [`HashError`] when the report cannot be serialised to
/// recompute its content hash.
pub fn check_attestation_binding(
    report: &PeriodicReport,
    report_bytes: &[u8],
    statement: &InTotoStatement,
) -> Result<AttestationBinding, HashError> {
    if let Some(reason) = header_mismatch(report, statement) {
        return Ok(AttestationBinding::Mismatch(reason));
    }
    let content_hash = compute_content_hash(report)?;
    if names_digest(statement, &content_hash) {
        return Ok(AttestationBinding::ContentHash);
    }
    if let Some(binding) = &statement.predicate.subject_binding {
        return Ok(AttestationBinding::Mismatch(
            if binding == SUBJECT_BINDING_CONTENT_HASH {
                format!(
                    "the statement names {}, these figures hash to {content_hash}",
                    first_subject_digest(statement)
                )
            } else {
                format!("unknown subject_binding {binding:?}")
            },
        ));
    }
    if names_digest(statement, &format_sha256(report_bytes)) {
        return Ok(AttestationBinding::FileBytes);
    }
    Ok(AttestationBinding::Unbindable)
}

fn header_mismatch(report: &PeriodicReport, statement: &InTotoStatement) -> Option<String> {
    if statement.statement_type != IN_TOTO_STATEMENT_TYPE {
        return Some(format!("_type is {:?}", statement.statement_type));
    }
    if statement.predicate_type != PERF_SENTINEL_PREDICATE_TYPE {
        return Some(format!("predicateType is {:?}", statement.predicate_type));
    }
    let report_uuid = report.report_metadata.report_uuid.to_string();
    if statement.predicate.report_uuid != report_uuid {
        return Some(format!(
            "the statement names report_uuid {}, the report is {report_uuid}",
            statement.predicate.report_uuid
        ));
    }
    None
}

/// Whether a subject's SHA-256 equals `prefixed`, a `sha256:<hex>` value.
fn names_digest(statement: &InTotoStatement, prefixed: &str) -> bool {
    let hex = prefixed.trim_start_matches("sha256:");
    statement
        .subject
        .iter()
        .filter_map(|s| s.digest.get("sha256"))
        .any(|d| d.eq_ignore_ascii_case(hex))
}

fn first_subject_digest(statement: &InTotoStatement) -> String {
    statement
        .subject
        .iter()
        .find_map(|s| s.digest.get("sha256"))
        .map_or_else(
            || "no sha256 subject".to_string(),
            |d| format!("sha256:{d}"),
        )
}

/// SHA-256 over the sorted, colon-joined core pattern names. Sorting
/// makes the hash independent of input order. 64-hex output, no `sha256:`
/// prefix, to match the in-toto subject digest format.
/// Exposed so `verify-hash` can recompute the canonical value from
/// the local binary and cross-check against the predicate.
#[must_use]
pub fn hash_core_patterns(patterns: &[String]) -> String {
    let mut sorted: Vec<&str> = patterns.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let joined = sorted.join(":");
    let digest = Sha256::digest(joined.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

const fn intent_str(intent: ReportIntent) -> &'static str {
    match intent {
        ReportIntent::Internal => "internal",
        ReportIntent::Official => "official",
        ReportIntent::Audited => "audited",
    }
}

const fn confidentiality_str(c: super::schema::Confidentiality) -> &'static str {
    match c {
        super::schema::Confidentiality::Internal => "internal",
        super::schema::Confidentiality::Public => "public",
    }
}

const fn conformance_str(c: super::schema::Conformance) -> &'static str {
    match c {
        super::schema::Conformance::CoreRequired => "core-required",
        super::schema::Conformance::Extended => "extended",
        super::schema::Conformance::Partial => "partial",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::periodic::schema::PeriodicReport;
    use std::path::PathBuf;

    fn load_g2_example() -> PeriodicReport {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = PathBuf::from(manifest_dir)
            .join("..")
            .join("..")
            .join("docs/schemas/examples/example-official-public-G2.json");
        let raw = std::fs::read_to_string(&path).expect("read G2 example");
        serde_json::from_str(&raw).expect("parse G2 example")
    }

    const DIGEST_64: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

    #[test]
    fn statement_carries_expected_top_level_fields() {
        let r = load_g2_example();
        let s = build_in_toto_statement(&r, DIGEST_64);
        assert_eq!(s.statement_type, IN_TOTO_STATEMENT_TYPE);
        assert_eq!(s.predicate_type, PERF_SENTINEL_PREDICATE_TYPE);
        assert_eq!(s.subject.len(), 1);
        assert_eq!(s.subject[0].name, DEFAULT_SUBJECT_NAME);
        assert_eq!(s.subject[0].digest.get("sha256").unwrap(), DIGEST_64);
    }

    #[test]
    fn predicate_projects_methodology_summary() {
        let r = load_g2_example();
        let s = build_in_toto_statement(&r, DIGEST_64);
        assert_eq!(s.predicate.intent, "official");
        assert_eq!(s.predicate.confidentiality_level, "public");
        assert_eq!(
            s.predicate.methodology_summary.sci_specification,
            "ISO/IEC 21031:2024"
        );
        assert_eq!(s.predicate.methodology_summary.conformance, "core-required");
        assert!(
            (s.predicate.methodology_summary.period_coverage - r.aggregate.period_coverage).abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn predicate_pattern_counts_match_g2_methodology() {
        let r = load_g2_example();
        let s = build_in_toto_statement(&r, DIGEST_64);
        let m = &s.predicate.methodology_summary;
        // G2 ships the four canonical core patterns, the ten enabled
        // patterns from FindingType, and no disabled patterns.
        assert_eq!(m.core_patterns_count, 4);
        assert_eq!(m.enabled_patterns_count, 10);
        assert_eq!(m.disabled_patterns_count, 0);
    }

    #[test]
    fn predicate_pattern_counts_reflect_disabled_overrides() {
        use crate::report::periodic::schema::DisabledPattern;
        let mut r = load_g2_example();
        r.methodology.enabled_patterns.truncate(8);
        r.methodology.disabled_patterns = vec![
            DisabledPattern {
                name: "pool_saturation".to_string(),
                reason: "noisy on this stack".to_string(),
            },
            DisabledPattern {
                name: "serialized_calls".to_string(),
                reason: "false positives in batch jobs".to_string(),
            },
        ];
        let s = build_in_toto_statement(&r, DIGEST_64);
        let m = &s.predicate.methodology_summary;
        assert_eq!(m.enabled_patterns_count, 8);
        assert_eq!(m.disabled_patterns_count, 2);
    }

    #[test]
    fn predicate_enabled_count_is_at_least_core_count() {
        // Audit invariant: every core pattern must be enabled, so
        // enabled_patterns_count >= core_patterns_count. A consumer
        // can fail an audit on the reverse.
        let r = load_g2_example();
        let s = build_in_toto_statement(&r, DIGEST_64);
        let m = &s.predicate.methodology_summary;
        assert!(m.enabled_patterns_count >= m.core_patterns_count);
    }

    #[test]
    fn predicate_core_patterns_hash_is_64_hex_and_stable() {
        let r = load_g2_example();
        let s1 = build_in_toto_statement(&r, DIGEST_64);
        let s2 = build_in_toto_statement(&r, DIGEST_64);
        let h1 = &s1.predicate.methodology_summary.core_patterns_hash;
        let h2 = &s2.predicate.methodology_summary.core_patterns_hash;
        assert_eq!(h1, h2, "hash must be deterministic");
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn core_patterns_hash_is_invariant_under_input_order() {
        let a = hash_core_patterns(&[
            "n_plus_one_sql".to_string(),
            "n_plus_one_http".to_string(),
            "redundant_sql".to_string(),
            "redundant_http".to_string(),
        ]);
        let b = hash_core_patterns(&[
            "redundant_http".to_string(),
            "n_plus_one_http".to_string(),
            "redundant_sql".to_string(),
            "n_plus_one_sql".to_string(),
        ]);
        assert_eq!(a, b);
    }

    #[test]
    fn core_patterns_hash_changes_on_substitution() {
        // Substituting a core pattern with another (same count) must
        // change the hash. Defence in depth beyond core_patterns_count.
        let baseline = hash_core_patterns(&[
            "n_plus_one_sql".to_string(),
            "n_plus_one_http".to_string(),
            "redundant_sql".to_string(),
            "redundant_http".to_string(),
        ]);
        let substituted = hash_core_patterns(&[
            "n_plus_one_sql".to_string(),
            "n_plus_one_http".to_string(),
            "redundant_sql".to_string(),
            "slow_sql".to_string(),
        ]);
        assert_ne!(baseline, substituted);
    }

    #[test]
    fn core_patterns_hash_matches_canonical_set_for_g2() {
        // Anchor: the G2 example carries the canonical 4-pattern core
        // set, so the predicate hash equals the hash of the canonical
        // reference list a consumer would hardcode.
        let r = load_g2_example();
        let s = build_in_toto_statement(&r, DIGEST_64);
        let canonical =
            hash_core_patterns(&crate::report::periodic::schema::core_patterns_required());
        assert_eq!(
            s.predicate.methodology_summary.core_patterns_hash,
            canonical
        );
    }

    #[test]
    fn statement_serde_roundtrip() {
        let r = load_g2_example();
        let s = build_in_toto_statement(&r, DIGEST_64);
        let json = serde_json::to_string(&s).unwrap();
        let back: InTotoStatement = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        // in-toto wire spec uses `_type` and `predicateType`, not the
        // Rust struct field names.
        assert!(json.contains("\"_type\""));
        assert!(json.contains("\"predicateType\""));
    }

    #[test]
    fn custom_subject_name_overrides_default() {
        let r = load_g2_example();
        let s = build_in_toto_statement_named(&r, DIGEST_64, "2026-Q1-disclosure.json");
        assert_eq!(s.subject[0].name, "2026-Q1-disclosure.json");
    }

    #[test]
    fn statement_declares_a_content_hash_subject() {
        let s = build_in_toto_statement(&load_g2_example(), DIGEST_64);
        assert_eq!(
            s.predicate.subject_binding.as_deref(),
            Some(SUBJECT_BINDING_CONTENT_HASH)
        );
        assert!(
            serde_json::to_string(&s)
                .unwrap()
                .contains("\"subject_binding\":\"content_hash\"")
        );
    }

    #[test]
    fn statement_written_before_subject_binding_still_parses() {
        let mut value =
            serde_json::to_value(build_in_toto_statement(&load_g2_example(), DIGEST_64)).unwrap();
        value["predicate"]
            .as_object_mut()
            .unwrap()
            .remove("subject_binding");
        let back: InTotoStatement = serde_json::from_value(value).unwrap();
        assert_eq!(back.predicate.subject_binding, None);
    }

    fn hashed_g2() -> PeriodicReport {
        let mut r = load_g2_example();
        r.integrity.content_hash = compute_content_hash(&r).unwrap();
        r
    }

    fn statement_for(r: &PeriodicReport) -> InTotoStatement {
        let hash = compute_content_hash(r).unwrap();
        build_in_toto_statement(r, hash.trim_start_matches("sha256:"))
    }

    fn legacy_statement(r: &PeriodicReport, file_bytes: &[u8]) -> InTotoStatement {
        let digest = format_sha256(file_bytes);
        let mut s = build_in_toto_statement(r, digest.trim_start_matches("sha256:"));
        s.predicate.subject_binding = None;
        s
    }

    #[test]
    fn binding_holds_when_the_subject_is_the_content_hash() {
        let r = hashed_g2();
        let s = statement_for(&r);
        let binding = check_attestation_binding(&r, b"any bytes", &s).unwrap();
        assert_eq!(binding, AttestationBinding::ContentHash);
    }

    #[test]
    fn binding_survives_the_signature_locator_patch() {
        use crate::report::periodic::schema::{IntegrityLevel, SignatureMetadata};
        let mut r = hashed_g2();
        let s = statement_for(&r);
        r.integrity.signature = Some(SignatureMetadata {
            format: "sigstore-cosign-intoto-v1".to_string(),
            bundle_url: "https://example.fr/bundle.sig".to_string(),
            signer_identity: "ci@example.fr".to_string(),
            signer_issuer: "https://token.actions.githubusercontent.com".to_string(),
            rekor_url: "https://rekor.sigstore.dev".to_string(),
            rekor_log_index: 1,
            signed_at: "2026-04-05T12:04:00Z".to_string(),
        });
        r.report_metadata.integrity_level = IntegrityLevel::Signed;
        let binding = check_attestation_binding(&r, b"patched bytes", &s).unwrap();
        assert_eq!(binding, AttestationBinding::ContentHash);
    }

    #[test]
    fn binding_fails_when_figures_changed_after_signing() {
        let original = hashed_g2();
        let s = statement_for(&original);
        let mut edited = original.clone();
        edited.aggregate.total_carbon_kgco2eq /= 2.0;
        edited.integrity.content_hash = compute_content_hash(&edited).unwrap();
        let binding = check_attestation_binding(&edited, b"edited bytes", &s).unwrap();
        assert!(
            matches!(&binding, AttestationBinding::Mismatch(reason) if reason.contains("these figures hash to")),
            "{binding:?}"
        );
    }

    #[test]
    fn binding_fails_on_another_report_uuid() {
        let r = hashed_g2();
        let mut s = statement_for(&r);
        s.predicate.report_uuid = "00000000-0000-4000-8000-000000000099".to_string();
        let binding = check_attestation_binding(&r, b"", &s).unwrap();
        assert!(
            matches!(&binding, AttestationBinding::Mismatch(reason) if reason.contains("report_uuid")),
            "{binding:?}"
        );
    }

    #[test]
    fn binding_fails_on_a_foreign_predicate_type() {
        let r = hashed_g2();
        let mut s = statement_for(&r);
        s.predicate_type = "https://slsa.dev/provenance/v1".to_string();
        let binding = check_attestation_binding(&r, b"", &s).unwrap();
        assert!(
            matches!(binding, AttestationBinding::Mismatch(_)),
            "{binding:?}"
        );
    }

    #[test]
    fn binding_fails_on_an_unknown_subject_binding() {
        let r = hashed_g2();
        let mut s = statement_for(&r);
        s.subject[0]
            .digest
            .insert("sha256".to_string(), DIGEST_64.to_string());
        s.predicate.subject_binding = Some("file_bytes".to_string());
        let binding = check_attestation_binding(&r, b"", &s).unwrap();
        assert!(
            matches!(&binding, AttestationBinding::Mismatch(reason) if reason.contains("unknown subject_binding")),
            "{binding:?}"
        );
    }

    #[test]
    fn legacy_statement_binds_the_untouched_file_bytes() {
        let r = hashed_g2();
        let bytes = serde_json::to_vec_pretty(&r).unwrap();
        let s = legacy_statement(&r, &bytes);
        let binding = check_attestation_binding(&r, &bytes, &s).unwrap();
        assert_eq!(binding, AttestationBinding::FileBytes);
    }

    #[test]
    fn legacy_statement_over_patched_bytes_is_unbindable() {
        let r = hashed_g2();
        let before = serde_json::to_vec_pretty(&r).unwrap();
        let s = legacy_statement(&r, &before);
        let after = serde_json::to_vec(&r).unwrap();
        let binding = check_attestation_binding(&r, &after, &s).unwrap();
        assert_eq!(binding, AttestationBinding::Unbindable);
    }
}
