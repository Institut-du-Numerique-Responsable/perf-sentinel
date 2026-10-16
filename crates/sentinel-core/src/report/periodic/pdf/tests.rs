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
