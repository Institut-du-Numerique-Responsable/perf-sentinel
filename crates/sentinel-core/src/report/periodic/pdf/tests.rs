use super::*;
use crate::report::periodic::{
    PeriodicReport, build_in_toto_statement_named, compute_content_hash,
};

const G2_EXAMPLE: &str =
    include_str!("../../../../../../docs/schemas/examples/example-official-public-G2.json");

/// The official G2 example with its content hash baked in.
pub(crate) fn baked_report() -> (PeriodicReport, Vec<u8>) {
    let mut report: PeriodicReport = serde_json::from_str(G2_EXAMPLE).expect("example parses");
    report.integrity.content_hash = compute_content_hash(&report).expect("hash");
    let bytes = serde_json::to_vec_pretty(&report).expect("serialise");
    (report, bytes)
}

/// An attestation naming `hex`, as `disclose --emit-attestation` writes it.
pub(crate) fn attestation_naming(report: &PeriodicReport, hex: &str) -> Vec<u8> {
    let statement = build_in_toto_statement_named(report, hex, "report.json");
    let mut line = serde_json::to_vec(&statement).expect("statement");
    line.push(b'\n');
    line
}

pub(crate) fn attestation_for(report: &PeriodicReport) -> Vec<u8> {
    let hash = compute_content_hash(report).expect("hash");
    attestation_naming(report, hash.trim_start_matches("sha256:"))
}

fn inputs<'a>(
    report: &'a [u8],
    attestation: Option<&'a [u8]>,
    bundle: Option<&'a [u8]>,
) -> PdfInputs<'a> {
    PdfInputs {
        report_bytes: report,
        attestation,
        bundle,
        expected_identity: None,
        expected_issuer: None,
    }
}

/// `(name, AFRelationship, bytes)` of every file the catalog associates.
fn associated_files(pdf: &[u8]) -> Vec<(String, String, Vec<u8>)> {
    let doc = lopdf::Document::load_mem(pdf).expect("lopdf reads the render");
    let catalog = doc.catalog().expect("catalog");
    let refs = catalog
        .get(b"AF")
        .and_then(lopdf::Object::as_array)
        .expect("AF array");
    refs.iter()
        .map(|r| {
            let spec = doc
                .dereference(r)
                .expect("filespec")
                .1
                .as_dict()
                .expect("dict");
            let name =
                String::from_utf8_lossy(spec.get(b"F").and_then(lopdf::Object::as_str).expect("F"))
                    .into_owned();
            let relation = String::from_utf8_lossy(
                spec.get(b"AFRelationship")
                    .and_then(lopdf::Object::as_name)
                    .expect("relation"),
            )
            .into_owned();
            let ef = spec
                .get(b"EF")
                .and_then(lopdf::Object::as_dict)
                .expect("EF");
            let stream = doc
                .dereference(ef.get(b"F").expect("EF F"))
                .expect("stream")
                .1
                .as_stream()
                .expect("stream")
                .clone();
            let data = stream.decompressed_content().unwrap_or(stream.content);
            (name, relation, data)
        })
        .collect()
}

#[test]
fn renders_a_pdf_a_3b_with_the_three_attachments() {
    let (report, bytes) = baked_report();
    let attestation = attestation_for(&report);
    let bundle = b"{\"bundle\":true}".to_vec();

    let pdf =
        render_disclosure_pdf(&inputs(&bytes, Some(&attestation), Some(&bundle))).expect("renders");

    assert!(pdf.starts_with(b"%PDF-"));
    let files = associated_files(&pdf);
    let find = |name: &str| {
        files
            .iter()
            .find(|f| f.0 == name)
            .unwrap_or_else(|| panic!("{name} attached"))
    };
    assert_eq!(
        ("Source", bytes.as_slice()),
        (
            find("report.json").1.as_str(),
            find("report.json").2.as_slice()
        )
    );
    assert_eq!(
        ("Supplement", attestation.as_slice()),
        (
            find("attestation.intoto.jsonl").1.as_str(),
            find("attestation.intoto.jsonl").2.as_slice()
        )
    );
    assert_eq!(
        ("Supplement", bundle.as_slice()),
        (
            find("bundle.sig").1.as_str(),
            find("bundle.sig").2.as_slice()
        )
    );
}

#[test]
fn same_inputs_render_the_same_bytes() {
    let (report, bytes) = baked_report();
    let attestation = attestation_for(&report);

    let first = render_disclosure_pdf(&inputs(&bytes, Some(&attestation), None)).expect("first");
    let second = render_disclosure_pdf(&inputs(&bytes, Some(&attestation), None)).expect("second");

    assert_eq!(first, second);
}

#[test]
fn a_hash_that_does_not_recompute_is_refused() {
    let (_, bytes) = baked_report();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    value["aggregate"]["total_requests"] = serde_json::json!(1);
    let edited = serde_json::to_vec(&value).expect("json");

    let err = render_disclosure_pdf(&inputs(&edited, None, None)).expect_err("refused");

    assert!(matches!(
        err,
        RenderError::Refused(RefusalReason::ContentHashMismatch)
    ));
}

#[test]
fn an_attestation_naming_another_hash_is_refused() {
    let (report, bytes) = baked_report();
    let foreign = attestation_naming(&report, &"ab".repeat(32));

    let err = render_disclosure_pdf(&inputs(&bytes, Some(&foreign), None)).expect_err("refused");

    assert!(matches!(
        err,
        RenderError::Refused(RefusalReason::BindingMismatch)
    ));
}

#[test]
fn a_newer_schema_is_unrenderable() {
    let (_, bytes) = baked_report();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    value["schema_version"] = "perf-sentinel-report/v1.8".into();

    let err = render_disclosure_pdf(&inputs(
        &serde_json::to_vec(&value).expect("json"),
        None,
        None,
    ))
    .expect_err("newer");

    assert!(matches!(err, RenderError::UnsupportedSchema(v) if v == "perf-sentinel-report/v1.8"));
}

#[test]
fn garbage_is_malformed() {
    let err = render_disclosure_pdf(&inputs(b"not a report", None, None)).expect_err("garbage");

    assert!(matches!(err, RenderError::Malformed(_)));
}

#[test]
fn exit_contract_reasons() {
    let cases = [
        (
            RenderError::Refused(RefusalReason::ContentHashMismatch),
            "refused",
            "content_hash_mismatch",
        ),
        (
            RenderError::Refused(RefusalReason::BindingMismatch),
            "refused",
            "binding_mismatch",
        ),
        (
            RenderError::Malformed(String::new()),
            "input_error",
            "malformed_report",
        ),
        (
            RenderError::UnsupportedSchema(String::new()),
            "unrenderable",
            "unsupported_schema_version",
        ),
        (
            RenderError::UnsupportedCharacters(vec!['ж']),
            "unrenderable",
            "unsupported_characters",
        ),
    ];
    for (err, result, reason) in cases {
        assert_eq!((result, reason), (err.result(), err.reason()));
    }
}

mod layout_and_pages {
    use super::super::layout::{Face, group4, text_width, wrap};
    use super::super::pages::{Op, PageContext, layout_pages};
    use super::*;

    fn rebaked(mut report: PeriodicReport) -> (PeriodicReport, Vec<u8>) {
        report.integrity.content_hash = compute_content_hash(&report).expect("hash");
        let bytes = serde_json::to_vec_pretty(&report).expect("serialise");
        (report, bytes)
    }

    fn texts(pages: &[Vec<Op>]) -> Vec<String> {
        pages.iter().map(|page| page_text(page)).collect()
    }

    fn page_text(page: &[Op]) -> String {
        page.iter()
            .filter_map(|op| match op {
                Op::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn has_qr(page: &[Op]) -> bool {
        page.iter().any(|op| matches!(op, Op::Qr { .. }))
    }

    #[test]
    fn wrap_breaks_on_spaces_within_the_width() {
        let width = text_width(Face::Regular, 10.0, "alpha beta").max(text_width(
            Face::Regular,
            10.0,
            "gamma delta",
        )) + 0.5;
        assert!(width < text_width(Face::Regular, 10.0, "alpha beta gamma"));
        let lines = wrap("alpha beta gamma delta", Face::Regular, 10.0, width);
        assert_eq!(vec!["alpha beta", "gamma delta"], lines);
    }

    #[test]
    fn wrap_splits_a_word_longer_than_the_line() {
        let width = text_width(Face::Mono, 10.0, "abcd") + 0.1;
        assert_eq!(
            vec!["abcd", "efgh", "ij"],
            wrap("abcdefghij", Face::Mono, 10.0, width)
        );
    }

    #[test]
    fn wrap_keeps_hash_groups_whole() {
        let code = group4(&format!("sha256:{}", "0123456789abcdef".repeat(4)));
        let width = text_width(Face::Mono, 9.0, "sha256: 0123 4567 89ab") + 0.1;
        let lines = wrap(&code, Face::Mono, 9.0, width);
        assert!(lines.len() > 1);
        for token in lines.iter().flat_map(|line| line.split(' ')).skip(1) {
            assert_eq!(4, token.len(), "{lines:?}");
        }
    }

    #[test]
    fn an_organisation_in_cyrillic_is_unrenderable() {
        let (mut report, _) = baked_report();
        report.organisation.name = "Сбербанк".to_string();
        let (_, bytes) = rebaked(report);
        let err = render_disclosure_pdf(&inputs(&bytes, None, None)).expect_err("cyrillic");
        assert!(
            matches!(err, RenderError::UnsupportedCharacters(ref chars) if chars.contains(&'С')),
            "{err:?}"
        );
    }

    #[test]
    fn an_emoji_in_a_disclaimer_is_unrenderable() {
        let (mut report, _) = baked_report();
        report
            .notes
            .disclaimers
            .push("Greener every quarter 🌍".to_string());
        let (_, bytes) = rebaked(report);
        let err = render_disclosure_pdf(&inputs(&bytes, None, None)).expect_err("emoji");
        assert!(
            matches!(err, RenderError::UnsupportedCharacters(ref chars) if chars == &vec!['🌍']),
            "{err:?}"
        );
    }

    #[test]
    fn latin_extended_renders() {
        let (mut report, _) = baked_report();
        report.organisation.name = "Société Générale Łódź Ærø".to_string();
        let (_, bytes) = rebaked(report);
        render_disclosure_pdf(&inputs(&bytes, None, None)).expect("latin extended renders");
    }

    #[test]
    fn a_v1_0_report_renders_its_missing_sections_as_not_reported() {
        let (mut report, _) = baked_report();
        report.schema_version = "perf-sentinel-report/v1.0".to_string();
        report.aggregate.canonical_waste = crate::report::periodic::schema::WasteTier::default();
        report.aggregate.operational_waste = crate::report::periodic::schema::WasteTier::default();
        report.aggregate.carbon_breakdown = None;
        report.methodology.standard_crosswalk = None;
        report.integrity.trace_integrity_chain = serde_json::Value::Null;
        let (report, bytes) = rebaked(report);
        render_disclosure_pdf(&inputs(&bytes, None, None)).expect("v1.0 renders");
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(all.contains("Not reported"), "{all}");
    }

    #[test]
    fn the_cover_carries_the_document_code_and_no_qr_without_a_transparency_url() {
        let (report, _) = baked_report();
        let pages = layout_pages(&PageContext::for_report(&report));
        let hex = report.integrity.content_hash.trim_start_matches("sha256:");
        assert!(page_text(&pages[0]).contains(&hex[..4]));
        assert!(page_text(&pages[0]).contains(&hex[60..]));
        assert!(!has_qr(&pages[0]));
    }

    #[test]
    fn a_transparency_url_draws_a_qr() {
        let (mut report, _) = baked_report();
        report.notes.reference_urls.insert(
            "transparency".to_string(),
            "https://example.org/transparency".to_string(),
        );
        let pages = layout_pages(&PageContext::for_report(&report));
        assert!(has_qr(&pages[0]));
        assert!(pages.iter().skip(1).all(|page| !has_qr(page)));
    }

    #[test]
    fn page_five_fills_the_verify_command_with_the_expected_identity() {
        let (report, _) = baked_report();
        let mut context = PageContext::for_report(&report);
        context.expected_identity =
            Some("https://github.com/acme/ci/.github/workflows/disclose.yml@refs/heads/main");
        context.expected_issuer = Some("https://token.actions.githubusercontent.com");
        let pages = layout_pages(&context);
        let last = page_text(pages.last().expect("pages"));
        assert!(last.contains("--expected-identity"), "{last}");
        assert!(
            last.contains("token.actions.githubusercontent.com"),
            "{last}"
        );
        assert!(last.contains("acme/ci"), "{last}");
    }

    #[test]
    fn every_disclaimer_is_copied_verbatim() {
        let (mut report, _) = baked_report();
        report
            .notes
            .disclaimers
            .push("Figures exclude the staging environment, measured separately.".to_string());
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        for disclaimer in &report.notes.disclaimers {
            assert!(all.contains(disclaimer.as_str()), "missing {disclaimer}");
        }
    }

    #[test]
    fn five_pages_at_least() {
        let (report, _) = baked_report();
        assert!(layout_pages(&PageContext::for_report(&report)).len() >= 5);
    }
}
