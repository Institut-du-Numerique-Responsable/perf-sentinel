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
    value["schema_version"] = "perf-sentinel-report/v1.9".into();

    let err = render_disclosure_pdf(&inputs(
        &serde_json::to_vec(&value).expect("json"),
        None,
        None,
    ))
    .expect_err("newer");

    assert!(matches!(err, RenderError::UnsupportedSchema(v) if v == "perf-sentinel-report/v1.9"));
}

#[test]
fn an_unknown_schema_version_is_named() {
    let (report, _) = baked_report();
    let mut value = serde_json::to_value(&report).expect("value");
    value["schema_version"] = "garbage".into();
    let bytes = serde_json::to_vec(&value).expect("bytes");
    let err = render_disclosure_pdf(&inputs(&bytes, None, None)).expect_err("unknown schema");
    assert!(err.to_string().contains("garbage"), "{err}");
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
        (
            RenderError::ExportFailed(String::new()),
            "unrenderable",
            "pdf_export_failed",
        ),
    ];
    for (err, result, reason) in cases {
        assert_eq!((result, reason), (err.result(), err.reason()));
    }
}

mod layout_and_pages {
    use chrono::NaiveDate;

    use super::super::layout::{Face, group4, text_width, wrap};
    use super::super::pages::{Op, PageContext, layout_pages};
    use super::super::{PAGE_HEIGHT, PAGE_WIDTH};
    use super::*;
    use crate::report::periodic::Application;
    use crate::report::periodic::schema::{PeriodType, SCHEMA_VERSION};

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

    /// The ops of the verify section, which the annex now follows.
    fn verify_section(pages: &[Vec<Op>]) -> Vec<Op> {
        let start = pages
            .iter()
            .position(|page| page_text(page).contains("Five minutes to know it was not altered"))
            .expect("verify page");
        let end = pages
            .iter()
            .position(|page| page_text(page).contains("Every field of the report"))
            .expect("annex page");
        pages[start..end].iter().flatten().cloned().collect()
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
    fn a_huge_word_wraps_in_linear_time_and_bounded_lines() {
        let word = "x".repeat(200_000);
        let started = std::time::Instant::now();
        let lines = wrap(&word, Face::Mono, 9.0, 480.0);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(200_000, lines.iter().map(String::len).sum::<usize>());
        assert!(lines.iter().all(|line| line.chars().count() <= 200));
    }

    #[test]
    fn glyphs_without_advance_still_break_lines() {
        let lines = wrap(&"\u{200b}".repeat(1_000), Face::Regular, 10.0, 100.0);
        assert!(lines.iter().all(|line| line.chars().count() <= 200));
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
    fn the_cover_names_the_report_and_when_it_was_generated() {
        let (report, _) = baked_report();
        let cover = page_text(&layout_pages(&PageContext::for_report(&report))[0]);
        assert!(
            cover.contains(&report.report_metadata.report_uuid.to_string()),
            "{cover}"
        );
        assert!(cover.contains("2026-04-05T12:00:00Z"), "{cover}");
    }

    #[test]
    fn the_header_carries_the_vat_number_and_the_opencorporates_page() {
        let (mut report, _) = baked_report();
        report.organisation.identifiers.vat = Some("FR12123456789".to_string());
        report.organisation.identifiers.opencorporates_url =
            Some("https://opencorporates.com/companies/fr/123456789".to_string());
        let cover = page_text(&layout_pages(&PageContext::for_report(&report))[0]);
        assert!(cover.contains("VAT FR12123456789"), "{cover}");
        assert!(
            cover.contains("opencorporates.com/companies/fr/123456789"),
            "{cover}"
        );
    }

    #[test]
    fn each_waste_reading_prints_its_avoidable_energy() {
        let (mut report, _) = baked_report();
        report.aggregate.canonical_waste.energy_kwh = 3.27;
        report.aggregate.operational_waste.energy_kwh = 1.84;
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(all.contains("3.27 kWh"), "{all}");
        assert!(all.contains("1.84 kWh"), "{all}");
    }

    #[test]
    fn the_carbon_breakdown_is_printed_with_the_figures_outside_the_total() {
        let (mut report, _) = baked_report();
        report.aggregate.carbon_breakdown =
            Some(crate::report::periodic::schema::CarbonBreakdown {
                operational_kgco2eq: Some(1.71),
                embodied_kgco2eq: Some(0.2),
                database_kgco2eq: None,
                messaging_kgco2eq: None,
                database_kgco2eq_out_of_total: Some(0.12),
                messaging_kgco2eq_out_of_total: None,
                transport_kgco2eq: Some(0.04),
                transport_kgco2eq_low: Some(0.01),
                transport_kgco2eq_high: Some(0.04),
            });
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(all.contains("operational 1.71 kg"), "{all}");
        assert!(all.contains("embodied 0.20 kg"), "{all}");
        assert!(all.contains("network transport 0.04 kg"), "{all}");
        assert!(all.contains("database 0.12 kg"), "{all}");
        // No broker figure, only the at-a-glance perimeter sentence.
        assert!(!all.contains("broker 0."), "{all}");
        assert!(!all.contains("broker hosts"), "{all}");
    }

    fn at_a_glance_text(report: &PeriodicReport) -> String {
        texts(&layout_pages(&PageContext::for_report(report)))
            .into_iter()
            .find(|page| page.contains("Three numbers"))
            .expect("at a glance page")
    }

    fn in_total_block(kwh: f64) -> crate::report::periodic::schema::DatabaseWasteAggregate {
        crate::report::periodic::schema::DatabaseWasteAggregate {
            energy_kwh: kwh,
            in_totals_energy_kwh: kwh,
            windows_with_figure: 1,
            declared_windows: 1,
            in_totals_windows: 1,
            ..Default::default()
        }
    }

    #[test]
    fn at_a_glance_states_the_host_perimeter() {
        let (mut report, _) = baked_report();
        report.aggregate.database_waste = None;
        report.aggregate.messaging_waste = None;
        let text = at_a_glance_text(&report);
        assert!(
            text.contains("The database and broker servers are outside these totals."),
            "{text}"
        );
        report.aggregate.database_waste = Some(in_total_block(0.42));
        let text = at_a_glance_text(&report);
        assert!(
            text.contains("The totals include the database hosts (0.42 kWh)."),
            "{text}"
        );
        assert!(
            text.contains("The broker servers are outside these totals."),
            "{text}"
        );
        report.aggregate.messaging_waste = Some(in_total_block(0.07));
        let text = at_a_glance_text(&report);
        assert!(
            text.contains(
                "The totals include the database hosts (0.42 kWh) and broker hosts (0.07 kWh)."
            ),
            "{text}"
        );
        assert!(!text.contains("outside these totals"), "{text}");
        // A block whose figure stayed beside the totals leaves its hosts out.
        report.aggregate.messaging_waste =
            Some(crate::report::periodic::schema::DatabaseWasteAggregate {
                in_totals_energy_kwh: 0.0,
                in_totals_windows: 0,
                ..in_total_block(0.07)
            });
        let text = at_a_glance_text(&report);
        assert!(
            text.contains("The broker servers are outside these totals."),
            "{text}"
        );
    }

    #[test]
    fn in_total_host_carbon_is_printed_inside_the_total() {
        let (mut report, _) = baked_report();
        report.aggregate.carbon_breakdown =
            Some(crate::report::periodic::schema::CarbonBreakdown {
                operational_kgco2eq: Some(1.71),
                database_kgco2eq: Some(0.3),
                messaging_kgco2eq: Some(0.05),
                ..Default::default()
            });
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(
            all.contains(
                "Made of operational 1.71 kg, database hosts 0.30 kg, broker hosts 0.05 kg."
            ),
            "{all}"
        );
        assert!(!all.contains("Outside this total"), "{all}");
    }

    #[test]
    fn the_crosswalk_prints_its_caveats() {
        let (mut report, _) = baked_report();
        report.methodology.standard_crosswalk =
            Some(crate::report::periodic::schema::StandardCrosswalk {
                standard: "ESRS E1".to_string(),
                mappings: vec![],
                caveats: vec!["Location-based only, no market-based figure.".to_string()],
            });
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(
            all.contains("Location-based only, no market-based figure."),
            "{all}"
        );
    }

    #[test]
    fn no_coverage_bar_is_drawn_when_runtime_coverage_is_not_reported() {
        let (mut report, _) = baked_report();
        report.aggregate.runtime_windows_count = 0;
        report.aggregate.fallback_windows_count = 0;
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(all.contains("Runtime coverage: Not reported"), "{all}");
        assert!(!all.contains("an official report needs 75"), "{all}");
    }

    #[test]
    fn excluded_environments_are_printed_with_their_reason() {
        let (mut report, _) = baked_report();
        report.scope_manifest.environments_excluded.push(
            crate::report::periodic::schema::ExcludedEnv {
                name: "staging".to_string(),
                reason: "synthetic traffic only".to_string(),
            },
        );
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(all.contains("staging"), "{all}");
        assert!(all.contains("synthetic traffic only"), "{all}");
    }

    #[test]
    fn the_measuring_binary_row_carries_its_hash_and_provenance() {
        let (mut report, _) = baked_report();
        report.integrity.binary_hash = Some(format!("sha256:{}", "ab12".repeat(16)));
        report.integrity.binary_attestation =
            Some(crate::report::periodic::schema::BinaryAttestationMetadata {
                format: "slsa-provenance-v1".to_string(),
                attestation_url: "https://example.org/attestation".to_string(),
                builder_id: "https://github.com/actions/runner".to_string(),
                git_tag: "v0.31.0".to_string(),
                git_commit: "568d9d96f0e1".to_string(),
                slsa_level: "L3".to_string(),
            });
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(all.contains("ab12 ab12"), "{all}");
        assert!(all.contains("v0.31.0"), "{all}");
        assert!(all.contains("568d9d96f0e1"), "{all}");
        assert!(all.contains("SLSA L3"), "{all}");
    }

    #[test]
    fn energy_models_print_a_readable_name_beside_their_tag() {
        let (mut report, _) = baked_report();
        report.methodology.calibration_inputs.energy_source_models =
            ["io_proxy_v3", "kepler_ebpf", "vendor_meter"]
                .into_iter()
                .map(String::from)
                .collect();
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(
            all.contains(
                "energy models I/O proxy, monthly x hourly grid intensity (io_proxy_v3), Kepler, RAPL, active power only (kepler_ebpf), vendor_meter"
            ),
            "{all}"
        );
    }

    #[test]
    fn the_archive_row_names_unchained_dropped_and_outside_windows() {
        let (mut report, _) = baked_report();
        report.integrity.trace_integrity_chain = serde_json::json!({
            "windows_verified": 160,
            "windows_unchained": 2,
            "breaks": 0,
            "breaks_outside_period": 1,
            "windows_dropped": 3,
            "drop_counter_resets": 1
        });
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(all.contains("160 scoring windows verified"), "{all}");
        assert!(all.contains("2 written before chaining"), "{all}");
        assert!(all.contains("1 break outside the period"), "{all}");
        assert!(
            all.contains("3 windows not archived, a lower bound across 1 daemon restart"),
            "{all}"
        );
    }

    #[test]
    fn an_annex_after_the_verify_page_prints_every_field_and_marks_the_absent_ones() {
        let (report, _) = baked_report();
        let pages = texts(&layout_pages(&PageContext::for_report(&report)));
        let verify = pages
            .iter()
            .position(|page| page.contains("Five minutes to know it was not altered"))
            .expect("verify page");
        assert_eq!(verify, 4, "the annex must not move the verify page");
        let annex = pages[verify + 1..].join(" ");
        assert!(annex.contains("Every field of the report"), "{annex}");
        assert!(
            annex.contains(&format!(
                "report_metadata.report_uuid {}",
                report.report_metadata.report_uuid
            )),
            "{annex}"
        );
        assert!(
            annex.contains("organisation.identifiers.lei not reported"),
            "{annex}"
        );
        assert!(
            annex.contains("applications[0].service_name checkout"),
            "{annex}"
        );
    }

    #[test]
    fn the_longest_annex_paths_print_on_one_line() {
        let (report, _) = baked_report();
        let pages = layout_pages(&PageContext::for_report(&report));
        for path in [
            "methodology.calibration_inputs.binary_specpower_vintage",
            "aggregate.carbon_breakdown.messaging_kgco2eq_out_of_total",
            "methodology.standard_crosswalk.mappings[0].report_field",
        ] {
            assert!(
                pages
                    .iter()
                    .flatten()
                    .any(|op| matches!(op, Op::Text { text, .. } if text.trim() == path)),
                "{path} is cut"
            );
        }
    }

    #[test]
    fn the_annex_reads_the_attached_report_not_its_typed_defaults() {
        let (report, _) = baked_report();
        let mut context = PageContext::for_report(&report);
        let mut source = serde_json::to_value(&report).expect("value");
        let calibration = source["methodology"]["calibration_inputs"]
            .as_object_mut()
            .expect("calibration");
        calibration.remove("scaphandre_used");
        source["aggregate"]
            .as_object_mut()
            .expect("aggregate")
            .remove("period_coverage");
        context.source = source;
        let all = texts(&layout_pages(&context)).join(" ");
        assert!(
            all.contains("methodology.calibration_inputs.scaphandre_used not reported"),
            "{all}"
        );
        assert!(
            all.contains("aggregate.period_coverage not reported"),
            "{all}"
        );
    }

    #[test]
    fn the_annex_names_the_schema_it_lists_whatever_the_report_version() {
        let (mut report, _) = baked_report();
        report.schema_version = "perf-sentinel-report/v1.2".to_string();
        let all = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(
            all.contains(&format!("the {SCHEMA_VERSION} schema defines")),
            "{all}"
        );
        assert!(
            !all.contains("in the order of the attached report.json"),
            "{all}"
        );
    }

    #[test]
    fn a_large_waste_value_stays_clear_of_its_bar() {
        let (mut report, _) = baked_report();
        report.aggregate.canonical_waste.carbon_kgco2eq = 1234.56;
        report.aggregate.canonical_waste.energy_kwh = 12345.67;
        let pages = layout_pages(&PageContext::for_report(&report));
        let page = &pages[1];
        let (value_x, value_y) = page
            .iter()
            .find_map(|op| match op {
                Op::Text { x, y, text, .. } if text.contains("12345.67 kWh") => Some((*x, *y)),
                _ => None,
            })
            .expect("the canonical value");
        // The track of the same row sits 6 points above the value's baseline.
        let track_end = page
            .iter()
            .filter_map(|op| match op {
                Op::Box { x, y, w, h, .. }
                    if (*h - 7.0).abs() < f32::EPSILON && (value_y - 6.0 - y).abs() < 0.5 =>
                {
                    Some(x + w)
                }
                _ => None,
            })
            .fold(0.0_f32, f32::max);
        assert!(track_end > 0.0, "no track on the value's row");
        assert!(
            value_x > track_end,
            "value at {value_x}, track ends at {track_end}"
        );
    }

    #[test]
    fn a_transparency_url_without_a_qr_code_is_listed_with_the_references() {
        let (mut report, _) = baked_report();
        let url = format!("https://example.org/{}", "t".repeat(4000));
        report
            .notes
            .reference_urls
            .insert("transparency".to_string(), url);
        let pages = layout_pages(&PageContext::for_report(&report));
        assert!(!has_qr(&pages[0]), "too long for a QR code");
        let all = texts(&pages).join(" ");
        assert!(all.contains("· transparency example.org/"), "{all}");
    }

    #[test]
    fn reference_urls_other_than_transparency_are_listed() {
        let (mut report, _) = baked_report();
        report.notes.reference_urls.clear();
        report.notes.reference_urls.insert(
            "methodology".to_string(),
            "https://example.org/METHODOLOGY.md".to_string(),
        );
        report.notes.reference_urls.insert(
            "transparency".to_string(),
            "https://example.org/transparency".to_string(),
        );
        let method = texts(&layout_pages(&PageContext::for_report(&report))).join(" ");
        assert!(
            method.contains("methodology example.org/METHODOLOGY.md"),
            "{method}"
        );
    }

    #[test]
    fn page_five_fills_the_verify_command_with_the_expected_identity() {
        let (report, _) = baked_report();
        let mut context = PageContext::for_report(&report);
        context.expected_identity =
            Some("https://github.com/acme/ci/.github/workflows/disclose.yml@refs/heads/main");
        context.expected_issuer = Some("https://token.actions.githubusercontent.com");
        let pages = layout_pages(&context);
        let last = page_text(&verify_section(&pages));
        assert!(last.contains("--expected-identity"), "{last}");
        assert!(
            last.contains("token.actions.githubusercontent.com"),
            "{last}"
        );
        assert!(last.contains("acme/ci"), "{last}");
    }

    #[test]
    fn newlines_and_tabs_in_text_render() {
        let (mut report, _) = baked_report();
        report
            .notes
            .disclaimers
            .push("line one\nline two".to_string());
        report.notes.disclaimers.push("a\r\nb\tc".to_string());
        report.organisation.name = "Example\u{2028}SAS\u{85}".to_string();
        let (_, bytes) = rebaked(report);
        render_disclosure_pdf(&inputs(&bytes, None, None)).expect("whitespace renders as spaces");
    }

    #[test]
    fn a_control_character_is_unrenderable() {
        let (mut report, _) = baked_report();
        report.notes.disclaimers.push("bell\u{7}".to_string());
        let (_, bytes) = rebaked(report);
        let err =
            render_disclosure_pdf(&inputs(&bytes, None, None)).expect_err("control character");
        assert!(
            matches!(err, RenderError::UnsupportedCharacters(ref chars) if chars == &vec!['\u{7}']),
            "{err:?}"
        );
    }

    #[test]
    fn page_five_prints_the_full_document_code() {
        let (report, _) = baked_report();
        let pages = layout_pages(&PageContext::for_report(&report));
        let last = page_text(&verify_section(&pages));
        let hex = report.integrity.content_hash.trim_start_matches("sha256:");
        for group in hex.as_bytes().chunks(4) {
            assert!(last.contains(std::str::from_utf8(group).unwrap()), "{last}");
        }
    }

    #[test]
    fn an_unsigned_report_does_not_claim_a_signature_stops_forgers() {
        let (report, _) = baked_report();
        let pages = layout_pages(&PageContext::for_report(&report));
        let last = page_text(&verify_section(&pages));
        assert!(
            !last.contains("signed attestation names the original code"),
            "{last}"
        );
        assert!(last.contains("PDF editor"), "{last}");
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

    const MARGIN: f32 = 45.0;
    const BOTTOM: f32 = PAGE_HEIGHT - 70.0;
    const FOOTER_TEXT: f32 = PAGE_HEIGHT - 40.0;
    const FOOTER_RULE: f32 = PAGE_HEIGHT - 54.0;
    const G1_EXAMPLE: &str =
        include_str!("../../../../../../docs/schemas/examples/example-internal-G1.json");

    /// The G2 example with `n` applications whose names wrap.
    fn with_applications(n: usize) -> PeriodicReport {
        let (mut report, _) = baked_report();
        let template = report.applications[0].clone();
        report.applications = (0..n)
            .map(|index| {
                let mut app = template.clone();
                if let Application::G2(g2) = &mut app {
                    g2.service_name = format!("order history and invoice export {index}");
                    g2.display_name = None;
                }
                app
            })
            .collect();
        report
    }

    fn page_of(pages: &[Vec<Op>], needle: &str) -> usize {
        1 + pages
            .iter()
            .position(|page| page_text(page).contains(needle))
            .expect(needle)
    }

    #[test]
    fn the_cover_names_the_page_the_verify_section_lands_on() {
        let report = with_applications(30);
        let pages = layout_pages(&PageContext::for_report(&report));
        let verify = page_of(&pages, "Five minutes to know");
        assert!(verify > 5, "thirty applications push it past page 5");
        let cover = page_text(&pages[0]);
        assert!(cover.contains(&format!("age {verify}")), "{cover}");
        assert!(!cover.contains("age 5"), "{cover}");
    }

    #[test]
    fn a_long_identity_is_printed_whole_on_one_line() {
        let (report, _) = baked_report();
        let identity = "https://github.com/Institut-du-Numerique-Responsable/perf-sentinel/.github/workflows/disclosure-signing.yml@refs/heads/main";
        let mut context = PageContext::for_report(&report);
        context.expected_identity = Some(identity);
        context.expected_issuer = Some("https://token.actions.githubusercontent.com");
        let pages = layout_pages(&context);
        let quoted = format!("'{identity}'");
        let line = verify_section(&pages)
            .iter()
            .find_map(|op| match op {
                Op::Text {
                    x,
                    face,
                    size,
                    text,
                    ..
                } if text.trim() == quoted => Some(x + text_width(*face, *size, text)),
                _ => None,
            })
            .expect("the identity on one line");
        assert!(line <= PAGE_WIDTH - MARGIN, "ends at {line}");
    }

    #[test]
    fn a_g1_total_prints_no_carbon_it_does_not_carry() {
        let report: PeriodicReport = serde_json::from_str(G1_EXAMPLE).expect("G1 parses");
        let pages = layout_pages(&PageContext::for_report(&report));
        let page = &pages[page_of(&pages, "Which services consumed what") - 1];
        let total = page
            .iter()
            .find_map(|op| match op {
                Op::Text { y, text, .. } if text == "Total" => Some(*y),
                _ => None,
            })
            .expect("a Total row");
        let row: Vec<&str> = page
            .iter()
            .filter_map(|op| match op {
                Op::Text { y, text, .. } if (*y - total).abs() < 0.01 => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(row.contains(&"-"), "{row:?}");
        assert!(!row.contains(&"0.00"), "{row:?}");
    }

    #[test]
    fn the_cover_title_names_the_period_once() {
        let (mut report, _) = baked_report();
        report.period.period_type = PeriodType::CalendarMonth;
        report.period.from_date = NaiveDate::from_ymd_opt(2026, 3, 1).expect("date");
        report.period.to_date = NaiveDate::from_ymd_opt(2026, 3, 31).expect("date");
        let cover = page_text(&layout_pages(&PageContext::for_report(&report))[0]);
        assert!(!cover.contains("March to March"), "{cover}");
        assert!(cover.contains("March 2026"), "{cover}");

        report.period.period_type = PeriodType::Custom;
        report.period.from_date = NaiveDate::from_ymd_opt(2025, 11, 1).expect("date");
        report.period.to_date = NaiveDate::from_ymd_opt(2026, 1, 31).expect("date");
        let cover = page_text(&layout_pages(&PageContext::for_report(&report))[0]);
        assert!(cover.contains("November 2025 to January 2026"), "{cover}");
        assert!(
            cover.contains("1 November 2025 to 31 January 2026"),
            "{cover}"
        );
    }

    #[test]
    fn table_rows_stay_whole_and_off_the_footer() {
        for n in 18..40 {
            let report = with_applications(n);
            let pages = layout_pages(&PageContext::for_report(&report));
            for (index, page) in pages.iter().enumerate() {
                // Every line of an application name sits under its figures,
                // on the same page.
                let figures: Vec<f32> = page
                    .iter()
                    .filter_map(|op| match op {
                        Op::Text {
                            y,
                            face: Face::Mono,
                            size,
                            ..
                        } if (*size - 8.5).abs() < 0.01 => Some(*y),
                        _ => None,
                    })
                    .collect();
                for op in page {
                    if let Op::Text {
                        y,
                        face: Face::Bold,
                        size,
                        text,
                        ..
                    } = op
                        && (*size - 9.0).abs() < 0.01
                        && text.contains("export")
                    {
                        assert!(
                            figures.iter().any(|f| (0.0..60.0).contains(&(y - f))),
                            "{n} applications, page {}: {text} has no figures",
                            index + 1
                        );
                    }
                }
                for op in page {
                    let (top, bottom) = match op {
                        Op::Text { y, .. } => (*y, *y),
                        Op::Box { y, h, .. } => (*y, y + h),
                        _ => continue,
                    };
                    let footer = [FOOTER_TEXT, FOOTER_RULE]
                        .iter()
                        .any(|line| (top - line).abs() < 0.01);
                    assert!(
                        footer || bottom <= BOTTOM + 0.01,
                        "{n} applications, page {}: {op:?}",
                        index + 1
                    );
                }
            }
        }
    }

    #[test]
    fn long_names_and_urls_stay_on_the_page() {
        let (mut report, _) = baked_report();
        report.organisation.name =
            "Établissement public de coopération intercommunale du grand bassin\nde la Vilaine"
                .to_string();
        report.notes.reference_urls.insert(
            "transparency".to_string(),
            "https://www.example-collectivite.fr/developpement-durable/transparence/numerique"
                .to_string(),
        );
        let pages = layout_pages(&PageContext::for_report(&report));
        for op in pages.iter().flatten() {
            if let Op::Text {
                x,
                face,
                size,
                text,
                ..
            } = op
            {
                let right = x + text_width(*face, *size, text);
                assert!(*x >= 20.0 && right <= PAGE_WIDTH - 20.0, "{op:?}");
            }
        }
        let logo = logo::logo();
        let logo_right = MARGIN + 26.0 * logo.view_box[2] / logo.view_box[3];
        let (x, right) = pages[0]
            .iter()
            .find_map(|op| match op {
                Op::Text {
                    x,
                    y,
                    face,
                    size,
                    text,
                    ..
                } if (*y - 56.0).abs() < 0.01 => Some((*x, x + text_width(*face, *size, text))),
                _ => None,
            })
            .expect("the header name");
        assert!(x >= logo_right, "the name starts at {x}, over the logo");
        assert!(
            (right - (PAGE_WIDTH - MARGIN)).abs() < 0.5,
            "ends at {right}"
        );
        for page in &pages {
            let footer: Vec<(f32, f32)> = page
                .iter()
                .filter_map(|op| match op {
                    Op::Text {
                        x,
                        y,
                        face,
                        size,
                        text,
                        ..
                    } if (*y - FOOTER_TEXT).abs() < 0.01 => {
                        Some((*x, x + text_width(*face, *size, text)))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(footer.len(), 2, "{footer:?}");
            assert!(
                footer[0].1 < footer[1].0,
                "the footer halves overlap: {footer:?}"
            );
        }
        let (_, bytes) = rebaked(report);
        render_disclosure_pdf(&inputs(&bytes, None, None)).expect("the ellipsis has a glyph");
    }
}

mod extraction {
    use super::*;

    fn rendered() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
        let (report, bytes) = baked_report();
        let attestation = attestation_for(&report);
        let bundle = b"{\"bundle\":true}".to_vec();
        let pdf = render_disclosure_pdf(&inputs(&bytes, Some(&attestation), Some(&bundle)))
            .expect("renders");
        (pdf, bytes, attestation, bundle)
    }

    #[test]
    fn extracts_the_three_files_byte_for_byte() {
        let (pdf, report, attestation, bundle) = rendered();
        let files = extract_attachments(&pdf, 1 << 20, 1 << 20).expect("extracts");
        assert_eq!(report, files.report);
        assert_eq!(Some(attestation), files.attestation);
        assert_eq!(Some(bundle), files.bundle);
    }

    #[test]
    fn a_copy_resaved_with_object_streams_still_extracts() {
        let (pdf, report, ..) = rendered();
        let mut doc = lopdf::Document::load_mem(&pdf).expect("load");
        doc.compress();
        let mut resaved = Vec::new();
        doc.save_modern(&mut resaved)
            .expect("save with object streams");
        assert_ne!(pdf, resaved);
        let files =
            extract_attachments(&resaved, 1 << 20, 1 << 20).expect("extracts the re-saved copy");
        assert_eq!(report, files.report);
    }

    #[test]
    fn a_pdf_without_report_json_is_no_report() {
        let mut doc = lopdf::Document::with_version("1.7");
        let mut pages = lopdf::Dictionary::new();
        pages.set("Type", lopdf::Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Vec::<lopdf::Object>::new());
        pages.set("Count", 0);
        let pages = doc.add_object(pages);
        let mut catalog = lopdf::Dictionary::new();
        catalog.set("Type", lopdf::Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", pages);
        let catalog = doc.add_object(catalog);
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("save");
        assert!(matches!(
            extract_attachments(&bytes, 1 << 20, 1 << 20),
            Err(ExtractError::NoReport)
        ));
    }

    #[test]
    fn two_report_json_are_duplicate() {
        let (pdf, ..) = rendered();
        let mut doc = lopdf::Document::load_mem(&pdf).expect("load");
        let stream = doc.add_object(lopdf::Stream::new(lopdf::Dictionary::new(), b"{}".to_vec()));
        let mut ef = lopdf::Dictionary::new();
        ef.set("F", stream);
        let mut spec = lopdf::Dictionary::new();
        spec.set("Type", lopdf::Object::Name(b"Filespec".to_vec()));
        spec.set("F", lopdf::Object::string_literal("report.json"));
        spec.set("EF", ef);
        let spec = doc.add_object(spec);
        let catalog = doc.catalog_mut().expect("catalog");
        let mut af = catalog
            .get(b"AF")
            .and_then(lopdf::Object::as_array)
            .expect("AF")
            .clone();
        af.push(spec.into());
        catalog.set("AF", af);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("save");
        assert!(matches!(
            extract_attachments(&bytes, 1 << 20, 1 << 20),
            Err(ExtractError::Duplicate("report.json"))
        ));
    }

    #[test]
    fn a_name_tree_that_loops_on_itself_is_walked_once() {
        let (pdf, ..) = rendered();
        let mut doc = lopdf::Document::load_mem(&pdf).expect("load");
        let node = doc.new_object_id();
        let mut tree = lopdf::Dictionary::new();
        // Ten references to itself: each node is walked once, the loop ignored.
        tree.set("Kids", vec![lopdf::Object::Reference(node); 10]);
        doc.objects.insert(node, lopdf::Object::Dictionary(tree));
        let mut names = lopdf::Dictionary::new();
        names.set("EmbeddedFiles", node);
        doc.catalog_mut().expect("catalog").set("Names", names);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("save");
        let started = std::time::Instant::now();
        let result = extract_attachments(&bytes, 1 << 20, 1 << 20);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn a_wide_name_tree_shared_between_levels_is_walked_once() {
        let (pdf, ..) = rendered();
        let mut doc = lopdf::Document::load_mem(&pdf).expect("load");
        // Five levels, each node listing the next one 200 times: 200^4 walks
        // without a visited set, five with one.
        let mut below = doc.add_object(lopdf::Dictionary::new());
        for _ in 0..4 {
            let mut node = lopdf::Dictionary::new();
            node.set("Kids", vec![lopdf::Object::Reference(below); 200]);
            below = doc.add_object(node);
        }
        let mut names = lopdf::Dictionary::new();
        names.set("EmbeddedFiles", below);
        doc.catalog_mut().expect("catalog").set("Names", names);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("save");
        let started = std::time::Instant::now();
        let result = extract_attachments(&bytes, 1 << 20, 1 << 20);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn object_streams_ended_by_a_nul_are_counted() {
        let mut pdf = b"%PDF-1.7\n".to_vec();
        for index in 0..100 {
            // NUL is PDF whitespace: the parser ends the name there.
            pdf.extend_from_slice(
                format!("{index} 0 obj << /Type /ObjStm\0>> endobj\n").as_bytes(),
            );
        }
        assert!(
            matches!(extract_attachments(&pdf, 1 << 20, 1 << 20), Err(ExtractError::Malformed(m)) if m.contains("object streams"))
        );
    }

    #[test]
    fn too_many_object_streams_are_refused_before_decoding() {
        let mut pdf = b"%PDF-1.7\n".to_vec();
        for index in 0..100 {
            // `#4F` is an escaped `O`: the count reads names as the parser does.
            pdf.extend_from_slice(
                format!("{index} 0 obj << /Type /#4FbjStm >> endobj\n").as_bytes(),
            );
        }
        assert!(
            matches!(extract_attachments(&pdf, 1 << 20, 1 << 20), Err(ExtractError::Malformed(m)) if m.contains("object streams"))
        );
    }

    #[test]
    fn a_pdf_over_the_input_cap_is_refused_before_parsing() {
        let mut pdf = b"%PDF-1.7\n".to_vec();
        pdf.resize(MAX_PDF_BYTES + 1, b' ');
        assert!(matches!(
            extract_attachments(&pdf, 1 << 20, 1 << 20),
            Err(ExtractError::TooLarge("the PDF"))
        ));
    }

    #[test]
    fn no_report_names_the_viewer_that_dropped_it() {
        let mut doc = lopdf::Document::with_version("1.7");
        let mut pages = lopdf::Dictionary::new();
        pages.set("Type", lopdf::Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Vec::<lopdf::Object>::new());
        pages.set("Count", 0);
        let pages = doc.add_object(pages);
        let mut catalog = lopdf::Dictionary::new();
        catalog.set("Type", lopdf::Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", pages);
        let catalog = doc.add_object(catalog);
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("save");
        let err = extract_attachments(&bytes, 1 << 20, 1 << 20).expect_err("no report");
        assert!(err.to_string().contains("original file"), "{err}");
    }

    #[test]
    fn an_attachment_over_the_cap_is_too_large() {
        let (pdf, ..) = rendered();
        assert!(matches!(
            extract_attachments(&pdf, 16, 1 << 20),
            Err(ExtractError::TooLarge("report.json"))
        ));
    }

    #[test]
    fn a_text_file_is_not_a_pdf() {
        assert!(matches!(
            extract_attachments(b"hello", 1 << 20, 1 << 20),
            Err(ExtractError::NotAPdf)
        ));
    }
}
