//! The five pages as drawing operations, so their content is testable
//! without a PDF and every string drawn is checked against the fonts
//! before krilla sees it. Content overflows onto continuation pages.

use serde::Serialize;

use super::layout::{Face, group4, wrap};
use super::{PAGE_HEIGHT, PAGE_WIDTH};
use crate::report::periodic::schema::WasteTier;
use crate::report::periodic::{Application, AttestationBinding, PeriodicReport};

pub(crate) type Rgb = (u8, u8, u8);

const TEXT: Rgb = (0x20, 0x26, 0x1f);
const TEXT_2: Rgb = (0x55, 0x6b, 0x5c);
const TEXT_3: Rgb = (0x6a, 0x6a, 0x6a);
const BRAND: Rgb = (0x10, 0x7a, 0x56);
const SURFACE: Rgb = (0xf7, 0xf7, 0xf7);
const BORDER: Rgb = (0xdc, 0xdc, 0xdc);
const WARN: Rgb = (0x75, 0x46, 0x18);

const MARGIN: f32 = 56.0;
const BOTTOM: f32 = PAGE_HEIGHT - 64.0;
const CONTENT_WIDTH: f32 = PAGE_WIDTH - 2.0 * MARGIN;
const LABEL_WIDTH: f32 = 150.0;
const NOT_REPORTED: &str = "Not reported";
/// The coverage an official disclosure needs.
const OFFICIAL_COVERAGE: f64 = 0.75;

/// One drawing operation. `y` grows downwards from the top of the page,
/// and a text's `y` is its baseline.
#[derive(Debug, Clone)]
pub(crate) enum Op {
    Text {
        x: f32,
        y: f32,
        face: Face,
        size: f32,
        color: Rgb,
        text: String,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgb,
    },
    Qr {
        x: f32,
        y: f32,
        side: f32,
        modules: Vec<Vec<bool>>,
    },
}

/// What the pages say besides the report itself.
pub(crate) struct PageContext<'a> {
    pub(crate) report: &'a PeriodicReport,
    /// Each attached file's name and SHA-256 in hex.
    pub(crate) attachments: Vec<(&'static str, String)>,
    /// `None` when no attestation is attached.
    pub(crate) binding: Option<AttestationBinding>,
    pub(crate) expected_identity: Option<&'a str>,
    pub(crate) expected_issuer: Option<&'a str>,
}

#[cfg(test)]
impl<'a> PageContext<'a> {
    pub(crate) fn for_report(report: &'a PeriodicReport) -> Self {
        Self {
            report,
            attachments: Vec::new(),
            binding: None,
            expected_identity: None,
            expected_issuer: None,
        }
    }
}

/// Every page of the document, in order.
pub(crate) fn layout_pages(context: &PageContext<'_>) -> Vec<Vec<Op>> {
    let mut writer = Writer::default();
    cover(&mut writer, context);
    writer.new_page();
    at_a_glance(&mut writer, context.report);
    writer.new_page();
    applications(&mut writer, context.report);
    writer.new_page();
    method(&mut writer, context.report);
    writer.new_page();
    verify(&mut writer, context);
    writer.finish(&context.report.integrity.content_hash)
}

struct Writer {
    pages: Vec<Vec<Op>>,
    y: f32,
}

impl Default for Writer {
    fn default() -> Self {
        Self {
            pages: vec![Vec::new()],
            y: MARGIN,
        }
    }
}

impl Writer {
    fn new_page(&mut self) {
        self.pages.push(Vec::new());
        self.y = MARGIN;
    }

    fn ensure(&mut self, height: f32) {
        if self.y + height > BOTTOM {
            self.new_page();
        }
    }

    fn push(&mut self, op: Op) {
        self.pages.last_mut().expect("a page").push(op);
    }

    fn text_at(&mut self, x: f32, face: Face, size: f32, color: Rgb, text: &str) {
        let y = self.y + size;
        self.push(Op::Text {
            x,
            y,
            face,
            size,
            color,
            text: text.to_string(),
        });
    }

    /// Wrapped text at `x`, `width` wide, advancing the cursor.
    fn block(&mut self, x: f32, width: f32, face: Face, size: f32, color: Rgb, text: &str) {
        let line_height = size * 1.45;
        for line in wrap(text, face, size, width) {
            self.ensure(line_height);
            self.text_at(x, face, size, color, &line);
            self.y += line_height;
        }
    }

    fn para(&mut self, text: &str) {
        self.block(MARGIN, CONTENT_WIDTH, Face::Regular, 10.0, TEXT, text);
        self.y += 4.0;
    }

    fn overline(&mut self, text: &str) {
        self.ensure(24.0);
        self.text_at(MARGIN, Face::Mono, 7.5, TEXT_3, &text.to_uppercase());
        self.y += 14.0;
    }

    fn heading(&mut self, text: &str) {
        self.ensure(40.0);
        self.y += 8.0;
        self.block(MARGIN, CONTENT_WIDTH, Face::Bold, 15.0, TEXT, text);
        self.y += 4.0;
    }

    fn subheading(&mut self, text: &str) {
        self.ensure(30.0);
        self.y += 6.0;
        self.block(MARGIN, CONTENT_WIDTH, Face::Bold, 11.0, TEXT, text);
    }

    /// A label in the left column and its value wrapped in the right one.
    fn row(&mut self, label: &str, value: &str) {
        self.ensure(16.0);
        let top = self.y;
        self.block(
            MARGIN,
            LABEL_WIDTH - 10.0,
            Face::Regular,
            9.5,
            TEXT_2,
            label,
        );
        let label_bottom = self.y;
        self.y = top;
        self.block(
            MARGIN + LABEL_WIDTH,
            CONTENT_WIDTH - LABEL_WIDTH,
            Face::Regular,
            9.5,
            TEXT,
            value,
        );
        self.y = self.y.max(label_bottom) + 3.0;
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        self.push(Op::Rect { x, y, w, h, color });
    }

    /// Page numbers and the short document code on every page.
    fn finish(mut self, content_hash: &str) -> Vec<Vec<Op>> {
        let total = self.pages.len();
        let short = group4(&content_hash.chars().take(23).collect::<String>());
        for (index, page) in self.pages.iter_mut().enumerate() {
            page.push(Op::Text {
                x: MARGIN,
                y: PAGE_HEIGHT - 32.0,
                face: Face::Mono,
                size: 7.5,
                color: TEXT_3,
                text: format!("{short} | page {} of {total}", index + 1),
            });
        }
        self.pages
    }
}

/// A serde unit variant as its wire name, `calendar-quarter` and the like.
fn wire<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn kwh(value: f64) -> String {
    format!("{value:.2} kWh")
}

fn kg(value: f64) -> String {
    format!("{value:.2} kg CO2e")
}

fn percent(ratio: f64) -> String {
    format!("{:.1} %", ratio * 100.0)
}

fn or_not_reported(value: Option<String>) -> String {
    value.unwrap_or_else(|| NOT_REPORTED.to_string())
}

fn cover(w: &mut Writer, context: &PageContext<'_>) {
    let report = context.report;
    let metadata = &report.report_metadata;
    w.overline("Energy and carbon disclosure");
    w.block(
        MARGIN,
        CONTENT_WIDTH,
        Face::Bold,
        24.0,
        TEXT,
        &report.organisation.name,
    );
    w.y += 4.0;
    let period = &report.period;
    w.block(
        MARGIN,
        CONTENT_WIDTH,
        Face::Regular,
        12.0,
        TEXT_2,
        &format!(
            "{} to {}, {}, {} days covered",
            period.from_date,
            period.to_date,
            wire(&period.period_type),
            period.days_covered
        ),
    );
    w.block(
        MARGIN,
        CONTENT_WIDTH,
        Face::Regular,
        10.0,
        TEXT_2,
        &format!(
            "Intent {}, {} report",
            wire(&metadata.intent),
            wire(&metadata.confidentiality_level)
        ),
    );
    w.y += 18.0;
    figures(w, report);
    w.y += 22.0;
    w.overline("Declared integrity level, not checked by this document");
    w.para(&wire(&metadata.integrity_level));
    w.y += 6.0;
    w.overline("Document code");
    w.block(
        MARGIN,
        CONTENT_WIDTH * 0.62,
        Face::Mono,
        10.5,
        TEXT,
        &group4(&report.integrity.content_hash),
    );
    w.y += 10.0;
    transparency(w, report);
    w.y += 8.0;
    w.block(
        MARGIN,
        CONTENT_WIDTH,
        Face::Regular,
        8.5,
        TEXT_3,
        &format!(
            "Generated {} by {}, perf-sentinel {}. Page 5 says how to verify this document.",
            metadata.generated_at.format("%Y-%m-%d %H:%M UTC"),
            metadata.generated_by,
            metadata.perf_sentinel_version
        ),
    );
}

/// Energy, carbon and avoidable carbon, side by side.
fn figures(w: &mut Writer, report: &PeriodicReport) {
    let aggregate = &report.aggregate;
    let tiles = [
        ("Energy", kwh(aggregate.total_energy_kwh)),
        ("Carbon", kg(aggregate.total_carbon_kgco2eq)),
        (
            "Avoidable carbon",
            kg(aggregate.estimated_optimization_potential_kgco2eq),
        ),
    ];
    let gap = 10.0;
    let tile_width = (CONTENT_WIDTH - 2.0 * gap) / 3.0;
    let top = w.y;
    for (index, (label, value)) in tiles.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // three tiles
        let x = MARGIN + index as f32 * (tile_width + gap);
        w.rect(x, top, tile_width, 64.0, SURFACE);
        w.y = top + 12.0;
        w.text_at(x + 12.0, Face::Regular, 9.0, TEXT_2, label);
        w.y = top + 30.0;
        w.text_at(x + 12.0, Face::Bold, 15.0, TEXT, value);
    }
    w.y = top + 64.0;
}

/// The QR code to the organisation's own transparency page, when the
/// report names one.
fn transparency(w: &mut Writer, report: &PeriodicReport) {
    let Some(url) = report.notes.reference_urls.get("transparency") else {
        return;
    };
    let Some(modules) = qr_modules(url) else {
        return;
    };
    let side = 92.0;
    w.ensure(side + 30.0);
    let top = w.y;
    w.push(Op::Qr {
        x: MARGIN,
        y: top,
        side,
        modules,
    });
    w.y = top + 8.0;
    w.block(
        MARGIN + side + 14.0,
        CONTENT_WIDTH - side - 14.0,
        Face::Regular,
        9.0,
        TEXT_2,
        "Compare the document code with the copy the organisation publishes:",
    );
    w.block(
        MARGIN + side + 14.0,
        CONTENT_WIDTH - side - 14.0,
        Face::Mono,
        8.5,
        BRAND,
        url,
    );
    w.y = w.y.max(top + side);
}

fn qr_modules(url: &str) -> Option<Vec<Vec<bool>>> {
    let code = fast_qr::QRBuilder::new(url).build().ok()?;
    Some(
        (0..code.size)
            .map(|row| code[row].iter().map(|module| module.value()).collect())
            .collect(),
    )
}

fn at_a_glance(w: &mut Writer, report: &PeriodicReport) {
    let aggregate = &report.aggregate;
    w.overline("At a glance");
    w.heading("Carbon");
    w.row("Total", &kg(aggregate.total_carbon_kgco2eq));
    w.row(
        "Directional range",
        &format!(
            "{} to {}: the figures carry a 2x directional uncertainty bracket.",
            kg(aggregate.total_carbon_kgco2eq / 2.0),
            kg(aggregate.total_carbon_kgco2eq * 2.0)
        ),
    );
    range_bar(w);
    carbon_breakdown(w, report);
    w.heading("Avoidable waste");
    waste_tier(w, "Canonical reading", &aggregate.canonical_waste);
    waste_tier(w, "Operational reading", &aggregate.operational_waste);
    w.heading("Coverage");
    coverage(w, report);
    w.heading("ESRS E1 crosswalk");
    crosswalk(w, report);
}

/// The 2x band on a log scale from a quarter to four times the total, so
/// the band and the total always sit at the same places.
fn range_bar(w: &mut Writer) {
    w.ensure(22.0);
    let top = w.y + 4.0;
    w.rect(MARGIN, top + 4.0, CONTENT_WIDTH, 2.0, BORDER);
    w.rect(
        MARGIN + CONTENT_WIDTH * 0.25,
        top,
        CONTENT_WIDTH * 0.5,
        10.0,
        (0xe0, 0xf4, 0xed),
    );
    w.rect(
        MARGIN + CONTENT_WIDTH * 0.5 - 1.0,
        top - 2.0,
        2.0,
        14.0,
        BRAND,
    );
    w.y = top + 18.0;
}

fn carbon_breakdown(w: &mut Writer, report: &PeriodicReport) {
    let Some(breakdown) = report.aggregate.carbon_breakdown.as_ref() else {
        w.row("Breakdown", NOT_REPORTED);
        return;
    };
    w.row(
        "Operational",
        &or_not_reported(breakdown.operational_kgco2eq.map(kg)),
    );
    w.row(
        "Embodied",
        &or_not_reported(breakdown.embodied_kgco2eq.map(kg)),
    );
    let transport = breakdown.transport_kgco2eq.map(|value| {
        match (
            breakdown.transport_kgco2eq_low,
            breakdown.transport_kgco2eq_high,
        ) {
            (Some(low), Some(high)) => {
                format!("{}, sourced range {} to {}", kg(value), kg(low), kg(high))
            }
            _ => kg(value),
        }
    });
    w.row("Transport", &or_not_reported(transport));
}

fn waste_tier(w: &mut Writer, label: &str, tier: &WasteTier) {
    if tier.is_default() {
        w.row(label, NOT_REPORTED);
        return;
    }
    w.row(
        label,
        &format!(
            "{} and {} avoidable, {} of the energy, N+1 from {} repetitions",
            kwh(tier.energy_kwh),
            kg(tier.carbon_kgco2eq),
            percent(tier.waste_ratio),
            tier.n_plus_one_threshold
        ),
    );
}

fn coverage(w: &mut Writer, report: &PeriodicReport) {
    let covered = report.aggregate.period_coverage;
    let below = covered < OFFICIAL_COVERAGE;
    w.row(
        "Period covered",
        &format!(
            "{}, against the {} an official disclosure needs{}",
            percent(covered),
            percent(OFFICIAL_COVERAGE),
            if below { ", below it" } else { "" }
        ),
    );
    w.ensure(18.0);
    let top = w.y + 2.0;
    #[allow(clippy::cast_possible_truncation)] // a ratio in [0, 1]
    let filled = CONTENT_WIDTH * covered.clamp(0.0, 1.0) as f32;
    w.rect(MARGIN, top, CONTENT_WIDTH, 8.0, SURFACE);
    w.rect(MARGIN, top, filled, 8.0, if below { WARN } else { BRAND });
    #[allow(clippy::cast_possible_truncation)] // a constant ratio
    let notch = MARGIN + CONTENT_WIDTH * OFFICIAL_COVERAGE as f32;
    w.rect(notch - 0.75, top - 3.0, 1.5, 14.0, TEXT);
    w.y = top + 16.0;
    let temporal = &report.aggregate.temporal_coverage;
    let days = if temporal.days_in_period == 0 {
        NOT_REPORTED.to_string()
    } else {
        format!(
            "{} of {} days observed, largest gap {} days",
            temporal.observed_days, temporal.days_in_period, temporal.largest_gap_days
        )
    };
    w.row("Days observed", &days);
}

fn crosswalk(w: &mut Writer, report: &PeriodicReport) {
    let Some(crosswalk) = report.methodology.standard_crosswalk.as_ref() else {
        w.para(NOT_REPORTED);
        return;
    };
    w.para(&crosswalk.standard);
    for entry in &crosswalk.mappings {
        w.row(
            &entry.datapoint,
            &format!("{}. {}", entry.report_field, entry.note),
        );
    }
}

/// Name, requests and energy, whatever the granularity.
fn application_figures(application: &Application) -> (&str, u64, f64) {
    match application {
        Application::G1(app) => (
            app.display_name.as_deref().unwrap_or(&app.service_name),
            app.total_requests,
            app.energy_kwh,
        ),
        Application::G2(app) => (
            app.display_name.as_deref().unwrap_or(&app.service_name),
            app.total_requests,
            app.energy_kwh,
        ),
    }
}

fn service_name(application: &Application) -> &str {
    match application {
        Application::G1(app) => &app.service_name,
        Application::G2(app) => &app.service_name,
    }
}

fn applications(w: &mut Writer, report: &PeriodicReport) {
    let scope = &report.scope_manifest;
    w.overline("Applications");
    w.heading("Where the energy goes");
    w.para(&format!(
        "{} of {} declared applications measured, in {}.",
        scope.applications_measured,
        scope.total_applications_declared,
        if scope.environments_measured.is_empty() {
            NOT_REPORTED.to_string()
        } else {
            scope.environments_measured.join(", ")
        }
    ));
    let total = report.aggregate.total_energy_kwh;
    for application in &report.applications {
        let (name, requests, energy) = application_figures(application);
        let share = if total > 0.0 { energy / total } else { 0.0 };
        let measured = report
            .aggregate
            .per_service_measured_ratio
            .get(service_name(application))
            .map_or_else(
                || NOT_REPORTED.to_string(),
                |ratio| format!("{} measured", percent(*ratio)),
            );
        w.row(
            name,
            &format!(
                "{}, {} of the energy, {requests} requests, {measured}",
                kwh(energy),
                percent(share)
            ),
        );
    }
    if !scope.applications_excluded.is_empty() {
        w.subheading("Excluded");
        for excluded in &scope.applications_excluded {
            w.row(&excluded.service_name, &excluded.reason);
        }
    }
}

fn method(w: &mut Writer, report: &PeriodicReport) {
    let methodology = &report.methodology;
    w.overline("Method and limits");
    w.heading("How the figures are made");
    w.row("Specification", &methodology.sci_specification);
    w.row("perf-sentinel", &methodology.perf_sentinel_version);
    w.row("Conformance", &wire(&methodology.conformance));
    w.row("Detectors", &methodology.enabled_patterns.join(", "));
    for disabled in &methodology.disabled_patterns {
        w.row(&format!("{} off", disabled.name), &disabled.reason);
    }
    w.heading("Disclaimers");
    if report.notes.disclaimers.is_empty() {
        w.para(NOT_REPORTED);
    }
    for disclaimer in &report.notes.disclaimers {
        w.para(disclaimer);
    }
    w.heading("What this document is not");
    w.para("A directional estimate published for transparency, not an audited inventory and not regulatory-grade.");
    if let Some(crosswalk) = report.methodology.standard_crosswalk.as_ref() {
        for caveat in &crosswalk.caveats {
            w.para(caveat);
        }
    }
}

fn verify(w: &mut Writer, context: &PageContext<'_>) {
    let report = context.report;
    w.overline("Verify this document");
    w.heading("Who checked what");
    w.row(
        "Content hash",
        &format!(
            "Recomputed by perf-sentinel {} when this PDF was rendered: it matches.",
            env!("CARGO_PKG_VERSION")
        ),
    );
    w.row("Attestation", binding_text(context.binding.as_ref()));
    w.row("Signature", &signature_text(report));
    w.row(
        "Producing binary",
        &or_not_reported(report.integrity.binary_hash.clone()),
    );
    attachments(w, context);
    w.heading("Without a terminal");
    w.para("1. Compare the document code on page 1 with the one the organisation publishes on its own site.");
    w.para("2. Open this PDF in a viewer that lists attachments, such as Firefox, and keep report.json.");
    w.para("3. Have the command below run on this PDF. TRUSTED means these are the figures that were signed.");
    w.heading("The command");
    let command = format!(
        "perf-sentinel verify-hash --report report.pdf --expected-identity {} --expected-issuer {}",
        context.expected_identity.unwrap_or("<identity>"),
        context.expected_issuer.unwrap_or("<issuer>")
    );
    w.block(MARGIN, CONTENT_WIDTH, Face::Mono, 8.5, TEXT, &command);
    w.y += 6.0;
    w.para("A copy whose figures were edited after signing answers UNTRUSTED.");
}

fn binding_text(binding: Option<&AttestationBinding>) -> &'static str {
    match binding {
        None => "No attestation attached.",
        Some(AttestationBinding::ContentHash) => {
            "Names this content hash, checked when this PDF was rendered."
        }
        Some(AttestationBinding::FileBytes) => {
            "Names the exact bytes of the attached report, an older format, checked when this PDF was rendered."
        }
        Some(_) => {
            "An older format naming report bytes that changed since: it does not tie these figures."
        }
    }
}

fn signature_text(report: &PeriodicReport) -> String {
    report.integrity.signature.as_ref().map_or_else(
        || "Not signed.".to_string(),
        |signature| {
            format!(
                "Declared signer {} ({}), not checked by this document: run the command below.",
                signature.signer_identity, signature.signer_issuer
            )
        },
    )
}

fn attachments(w: &mut Writer, context: &PageContext<'_>) {
    if context.attachments.is_empty() {
        return;
    }
    w.subheading("Attachments");
    for (name, sha256) in &context.attachments {
        let top = w.y;
        w.block(MARGIN, LABEL_WIDTH - 10.0, Face::Mono, 8.5, TEXT, name);
        let bottom = w.y;
        w.y = top;
        w.block(
            MARGIN + LABEL_WIDTH,
            CONTENT_WIDTH - LABEL_WIDTH,
            Face::Mono,
            8.0,
            TEXT_2,
            sha256,
        );
        w.y = w.y.max(bottom) + 2.0;
    }
}
