//! The five pages as drawing operations, laid out after the 14 October
//! mock-ups. Their content is testable without a PDF, and every string
//! drawn is checked against the fonts before krilla sees it. Content that
//! does not fit continues on the next page.

use serde::Serialize;

use chrono::Datelike as _;

use super::annex;
use super::layout::{Face, fit, group4, text_width, wrap};
use super::{PAGE_HEIGHT, PAGE_WIDTH};
use crate::report::periodic::schema::{
    Aggregate, CarbonBreakdown, IntegrityLevel, Period, PeriodType, ReportIntent, SCHEMA_VERSION,
    WasteTier,
};
use crate::report::periodic::{Application, AttestationBinding, Confidentiality, PeriodicReport};

pub(crate) type Rgb = (u8, u8, u8);

const TEXT: Rgb = (0x20, 0x26, 0x1f);
const TEXT_2: Rgb = (0x55, 0x6b, 0x5c);
const TEXT_3: Rgb = (0x6a, 0x6a, 0x6a);
const BORDER: Rgb = (0xeb, 0xeb, 0xeb);
const BORDER_2: Rgb = (0xdc, 0xdc, 0xdc);
const DASH: Rgb = (0x80, 0x80, 0x80);
const SURFACE: Rgb = (0xf7, 0xf7, 0xf7);
const TRACK: Rgb = (0xee, 0xf1, 0xef);
const BRAND: Rgb = (0x0b, 0xa6, 0x71);
const BRAND_TEXT: Rgb = (0x10, 0x7a, 0x56);
const ACCENT_BG: Rgb = (0xe0, 0xf4, 0xed);
const ACCENT_BD: Rgb = (0xc6, 0xe6, 0xd8);
const INFO: Rgb = (0x1f, 0x5e, 0x9e);
const WARN_FG: Rgb = (0x75, 0x46, 0x18);
const WARN_BG: Rgb = (0xff, 0xe0, 0xc2);
const WARN_BD: Rgb = (0xf0, 0xd2, 0xb2);
const OK_FG: Rgb = (0x1b, 0x53, 0x0d);
const OK_BG: Rgb = (0xdd, 0xf3, 0xdc);
const OK_BD: Rgb = (0xc6, 0xe6, 0xc4);

const MARGIN: f32 = 45.0;
const RIGHT: f32 = PAGE_WIDTH - MARGIN;
const CONTENT_WIDTH: f32 = RIGHT - MARGIN;
const BOTTOM: f32 = PAGE_HEIGHT - 70.0;
const LABEL_WIDTH: f32 = 123.0;
const NOT_REPORTED: &str = "Not reported";
/// The annex's path column, wide enough for most schema paths on one line.
const ANNEX_LABEL: f32 = 254.0;
/// The coverage an official disclosure needs.
const OFFICIAL_COVERAGE: f64 = 0.75;

// The application table, as offsets from the left margin.
const NAME_WIDTH: f32 = 96.0;
const REQUESTS_RIGHT: f32 = 160.0;
const SHARE_LEFT: f32 = 170.0;
const SHARE_RIGHT: f32 = 284.0;
/// Right edges of kWh, kgCO2eq, efficiency and anti-patterns.
const FIGURE_RIGHTS: [f32; 4] = [326.0, 374.0, 428.0, CONTENT_WIDTH];

/// How a box outline is drawn.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Outline {
    pub(crate) color: Rgb,
    pub(crate) width: f32,
    pub(crate) dashed: bool,
}

const fn solid(color: Rgb, width: f32) -> Outline {
    Outline {
        color,
        width,
        dashed: false,
    }
}

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
    Box {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        fill: Option<Rgb>,
        outline: Option<Outline>,
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
        fill: Rgb,
    },
    Qr {
        x: f32,
        y: f32,
        side: f32,
        modules: Vec<Vec<bool>>,
    },
    Logo {
        x: f32,
        y: f32,
        height: f32,
    },
}

/// What the pages say besides the report itself.
pub(crate) struct PageContext<'a> {
    pub(crate) report: &'a PeriodicReport,
    /// The attached report.json as written, which the annex prints: the
    /// typed report fills in defaults for the fields it does not carry.
    pub(crate) source: serde_json::Value,
    /// Each attached file's name, SHA-256 in hex and size in bytes.
    pub(crate) attachments: Vec<(&'static str, String, usize)>,
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
            source: serde_json::to_value(report).expect("a report serialises"),
            attachments: Vec::new(),
            binding: None,
            expected_identity: None,
            expected_issuer: None,
        }
    }
}

/// Every page of the document, in order.
pub(crate) fn layout_pages(context: &PageContext<'_>) -> Vec<Vec<Op>> {
    let (pages, verify_page) = lay_out(context, 5);
    if verify_page == 5 {
        return pages;
    }
    // The cover names the verify page, and that number never moves it.
    lay_out(context, verify_page).0
}

/// The pages, and the number of the one the verify section starts on.
fn lay_out(context: &PageContext<'_>, verify_page: usize) -> (Vec<Vec<Op>>, usize) {
    let mut writer = Writer::default();
    cover(&mut writer, context, verify_page);
    writer.new_page();
    at_a_glance(&mut writer, context.report);
    writer.new_page();
    applications(&mut writer, context.report);
    writer.new_page();
    method(&mut writer, context.report);
    writer.new_page();
    let actual = writer.pages.len();
    verify(&mut writer, context);
    writer.new_page();
    annex_pages(&mut writer, context);
    (writer.finish(context.report), actual)
}

/// How `Writer::columns` splits a line: where the value column starts, the
/// text size, the space under the row and the least room it takes.
struct Columns {
    split: f32,
    size: f32,
    gap: f32,
    min_height: f32,
}

/// One cell of `Writer::columns`.
#[derive(Clone, Copy)]
struct Cell<'a> {
    text: &'a str,
    face: Face,
    color: Rgb,
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

    /// A single line with its baseline at `baseline`.
    fn line(&mut self, x: f32, baseline: f32, face: Face, size: f32, color: Rgb, text: &str) {
        self.push(Op::Text {
            x,
            y: baseline,
            face,
            size,
            color,
            text: plain(text),
        });
    }

    /// A single line ending at `right`.
    fn line_right(
        &mut self,
        right: f32,
        baseline: f32,
        face: Face,
        size: f32,
        color: Rgb,
        text: &str,
    ) {
        let text = plain(text);
        let x = right - text_width(face, size, &text);
        self.line(x, baseline, face, size, color, &text);
    }

    /// A single line centred on `centre`.
    fn line_centred(
        &mut self,
        centre: f32,
        baseline: f32,
        face: Face,
        size: f32,
        color: Rgb,
        text: &str,
    ) {
        let text = plain(text);
        let x = centre - text_width(face, size, &text) / 2.0;
        self.line(x, baseline, face, size, color, &text);
    }

    /// Upper-case text with wide tracking, one glyph at a time. Returns
    /// where the text ends.
    fn tracked(&mut self, x: f32, baseline: f32, size: f32, color: Rgb, text: &str) -> f32 {
        let mut pen = x;
        for c in text.to_uppercase().chars() {
            let glyph = c.to_string();
            if !c.is_whitespace() {
                self.line(pen, baseline, Face::Mono, size, color, &glyph);
            }
            pen += text_width(Face::Mono, size, &glyph) + size * 0.32;
        }
        pen
    }

    /// Wrapped text at `x`, `width` wide, advancing the cursor.
    fn block(&mut self, x: f32, width: f32, face: Face, size: f32, color: Rgb, text: &str) {
        let line_height = size * 1.5;
        for line in wrap(&plain(text), face, size, width) {
            self.ensure(line_height);
            self.line(x, self.y + size, face, size, color, &line);
            self.y += line_height;
        }
    }

    fn para(&mut self, size: f32, color: Rgb, text: &str) {
        self.block(MARGIN, CONTENT_WIDTH, Face::Regular, size, color, text);
        self.y += size * 0.5;
    }

    fn rule(&mut self, x: f32, y: f32, w: f32) {
        self.boxed(x, y, w, 0.6, 0.0, Some(BORDER), None);
    }

    #[allow(clippy::too_many_arguments)] // one call per drawn box, every value named at the call
    fn boxed(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        fill: Option<Rgb>,
        outline: Option<Outline>,
    ) {
        self.push(Op::Box {
            x,
            y,
            w,
            h,
            radius,
            fill,
            outline,
        });
    }

    /// The overline of a page, with a hairline running to the margin.
    fn section(&mut self, label: &str) {
        let end = self.tracked(MARGIN, self.y + 7.0, 7.0, TEXT_3, label);
        self.rule(end + 6.0, self.y + 4.5, RIGHT - end - 6.0);
        self.y += 18.0;
    }

    fn title(&mut self, text: &str) {
        self.block(MARGIN, CONTENT_WIDTH, Face::Bold, 16.5, TEXT, text);
        self.y += 2.0;
    }

    fn heading(&mut self, text: &str) {
        self.ensure(40.0);
        self.y += 8.0;
        self.block(MARGIN, CONTENT_WIDTH, Face::Bold, 11.0, TEXT, text);
        self.y += 1.0;
    }

    /// A label in the left column and its value wrapped in the right one.
    fn row(&mut self, label: &str, value: &str, value_face: Face) {
        self.columns(
            &Columns {
                split: LABEL_WIDTH,
                size: 8.5,
                gap: 1.5,
                min_height: 16.0,
            },
            Cell {
                text: label,
                face: Face::Regular,
                color: TEXT_2,
            },
            Cell {
                text: value,
                face: value_face,
                color: TEXT,
            },
        );
    }

    /// Two cells side by side, kept on one page when they fit on one.
    fn columns(&mut self, layout: &Columns, label: Cell<'_>, value: Cell<'_>) {
        let label_width = layout.split - 8.0;
        let value_width = CONTENT_WIDTH - layout.split;
        let label_height = block_height(label.text, label.face, layout.size, label_width);
        let value_height = block_height(value.text, value.face, layout.size, value_width);
        self.ensure((label_height.max(value_height) + layout.gap).max(layout.min_height));
        let top = self.y;
        self.block(
            MARGIN,
            label_width,
            label.face,
            layout.size,
            label.color,
            label.text,
        );
        let label_bottom = self.y;
        let page = self.pages.len();
        self.y = top;
        self.block(
            MARGIN + layout.split,
            value_width,
            value.face,
            layout.size,
            value.color,
            value.text,
        );
        // A value taller than the page broke onto the next one, where the
        // label's bottom means nothing.
        if self.pages.len() == page {
            self.y = self.y.max(label_bottom);
        }
        self.y += layout.gap;
    }

    /// A coloured box holding a bold title and a paragraph.
    fn callout(&mut self, (fg, bg, bd): (Rgb, Rgb, Rgb), title: &str, body: &str) {
        let inner = CONTENT_WIDTH - 24.0;
        let lines = wrap(&plain(body), Face::Regular, 9.0, inner).len();
        #[allow(clippy::cast_precision_loss)] // a few lines
        let height = 40.0 + lines as f32 * 13.5;
        self.ensure(height + 8.0);
        let top = self.y + 6.0;
        self.boxed(
            MARGIN,
            top,
            CONTENT_WIDTH,
            height,
            8.0,
            Some(bg),
            Some(solid(bd, 0.8)),
        );
        self.y = top + 12.0;
        self.block(MARGIN + 12.0, inner, Face::Bold, 10.5, fg, title);
        self.y += 2.0;
        self.block(MARGIN + 12.0, inner, Face::Regular, 9.0, fg, body);
        self.y = top + height + 8.0;
    }

    /// The running footer of every page, once their count is known.
    fn finish(self, report: &PeriodicReport) -> Vec<Vec<Op>> {
        let total = self.pages.len();
        let short = short_code(&report.integrity.content_hash);
        let left = format!(
            "{} · {} · {}",
            report.organisation.name,
            period_label(report),
            report.schema_version
        );
        let base = PAGE_HEIGHT - 40.0;
        let right_width = |index: usize| {
            text_width(
                Face::Mono,
                7.0,
                &format!("{short} · page {} of {total}", index + 1),
            )
        };
        self.pages
            .into_iter()
            .enumerate()
            .map(|(index, page)| {
                let mut footer = Writer {
                    pages: vec![page],
                    y: 0.0,
                };
                footer.rule(MARGIN, base - 14.0, CONTENT_WIDTH);
                let room = CONTENT_WIDTH - right_width(index) - 12.0;
                let left = fit(&plain(&left), Face::Mono, 7.0, room);
                footer.line(MARGIN, base, Face::Mono, 7.0, TEXT_2, &left);
                footer.line_right(
                    RIGHT,
                    base,
                    Face::Mono,
                    7.0,
                    TEXT_2,
                    &format!("{short} · page {} of {total}", index + 1),
                );
                footer.pages.pop().expect("the page")
            })
            .collect()
    }
}

/// Every whitespace as U+0020: a newline, a tab or a line separator in a
/// report string has no glyph to draw.
fn plain(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect()
}

/// The height `Writer::block` takes to draw `text`.
fn block_height(text: &str, face: Face, size: f32, width: f32) -> f32 {
    #[allow(clippy::cast_precision_loss)] // a few lines
    let lines = wrap(&plain(text), face, size, width).len() as f32;
    lines * size * 1.5
}

/// `sha256:3f9c…a71e`, as the footer prints it.
fn short_code(hash: &str) -> String {
    let hex: Vec<char> = hash.trim_start_matches("sha256:").chars().collect();
    let head: String = hex.iter().take(4).collect();
    let tail: String = hex.iter().skip(hex.len().saturating_sub(4)).collect();
    format!("sha256:{head}…{tail}")
}

/// A serde unit variant as its wire name, `calendar-quarter` and the like.
fn wire<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `4150000` as `4 150 000`.
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, c) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn percent(ratio: f64) -> String {
    let value = ratio * 100.0;
    if (value - value.round()).abs() < 0.05 {
        format!("{value:.0} %")
    } else {
        format!("{value:.1} %")
    }
}

fn number_word(n: u32) -> String {
    const WORDS: [&str; 11] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    ];
    WORDS
        .get(n as usize)
        .map_or_else(|| n.to_string(), |word| (*word).to_string())
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn period_label(report: &PeriodicReport) -> String {
    let period = &report.period;
    match period.period_type {
        PeriodType::CalendarQuarter => {
            let month: u32 = period
                .from_date
                .format("%m")
                .to_string()
                .parse()
                .unwrap_or(1);
            format!("{} Q{}", period.from_date.format("%Y"), month.div_ceil(3))
        }
        PeriodType::CalendarMonth => period.from_date.format("%B %Y").to_string(),
        PeriodType::CalendarYear => period.from_date.format("%Y").to_string(),
        PeriodType::Custom => format!("{} to {}", period.from_date, period.to_date),
    }
}

fn period_noun(report: &PeriodicReport) -> &'static str {
    match report.period.period_type {
        PeriodType::CalendarQuarter => "quarter",
        PeriodType::CalendarMonth => "month",
        PeriodType::CalendarYear => "year",
        PeriodType::Custom => "period",
    }
}

fn integrity_label(level: IntegrityLevel) -> &'static str {
    match level {
        IntegrityLevel::None => "no integrity data",
        IntegrityLevel::HashOnly => "hash only, not signed",
        IntegrityLevel::Signed => "signed",
        IntegrityLevel::SignedWithAttestation => "signed, with binary provenance",
        IntegrityLevel::Audited => "audited",
    }
}

fn is_signed(report: &PeriodicReport) -> bool {
    report.integrity.signature.is_some()
}

// ---------------------------------------------------------------- page 1

fn cover(w: &mut Writer, context: &PageContext<'_>, verify_page: usize) {
    let report = context.report;
    header(w, report);
    w.y = 160.0;
    let metadata = &report.report_metadata;
    let overline = format!(
        "Energy and carbon disclosure · {} · {}",
        wire(&metadata.intent),
        wire(&metadata.confidentiality_level)
    );
    w.tracked(MARGIN, w.y + 8.0, 7.5, BRAND_TEXT, &overline);
    w.y += 24.0;
    let period = &report.period;
    w.block(
        MARGIN,
        CONTENT_WIDTH,
        Face::Bold,
        26.0,
        TEXT,
        "What our software consumed,",
    );
    w.block(
        MARGIN,
        CONTENT_WIDTH,
        Face::Bold,
        26.0,
        TEXT,
        &period_span(period),
    );
    w.y += 4.0;
    let from = if period.from_date.year() == period.to_date.year() {
        "%-d %B"
    } else {
        "%-d %B %Y"
    };
    let subtitle = format!(
        "{}, {} to {}, {} days. Measured by Perf Sentinel {}.",
        capitalised(&wire(&period.period_type).replace('-', " ")),
        period.from_date.format(from),
        period.to_date.format("%-d %B %Y"),
        period.days_covered,
        metadata.perf_sentinel_version
    );
    w.block(
        MARGIN,
        CONTENT_WIDTH,
        Face::Regular,
        13.0,
        TEXT_2,
        &subtitle,
    );
    w.y += 18.0;
    cover_cards(w, report);
    w.y += 22.0;
    w.rule(MARGIN, w.y, CONTENT_WIDTH);
    w.y += 18.0;
    proof_block(w, report, verify_page);
}

/// The period as the cover title says it: `March 2026`, `January to
/// March 2026`, `November 2025 to January 2026`.
fn period_span(period: &Period) -> String {
    let (from, to) = (period.from_date, period.to_date);
    if from.year() != to.year() {
        format!("{} to {}", from.format("%B %Y"), to.format("%B %Y"))
    } else if from.month() == to.month() {
        to.format("%B %Y").to_string()
    } else {
        format!("{} to {}", from.format("%B"), to.format("%B %Y"))
    }
}

fn header(w: &mut Writer, report: &PeriodicReport) {
    let height = 26.0;
    w.push(Op::Logo {
        x: MARGIN,
        y: 40.0,
        height,
    });
    let [_, _, logo_w, logo_h] = super::logo::logo().view_box;
    let room = CONTENT_WIDTH - height * logo_w / logo_h - 16.0;
    let name = fit(&plain(&report.organisation.name), Face::Bold, 12.0, room);
    w.line_right(RIGHT, 56.0, Face::Bold, 12.0, TEXT, &name);
    let ids = &report.organisation.identifiers;
    let parts: Vec<String> = [
        ids.siren.as_ref().map(|v| format!("SIREN {v}")),
        ids.vat.as_ref().map(|v| format!("VAT {v}")),
        ids.lei.as_ref().map(|v| format!("LEI {v}")),
        Some(report.organisation.country.clone()),
        report
            .organisation
            .sector
            .as_ref()
            .map(|v| format!("NACE {v}")),
        ids.domain.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect();
    let line = fit(&plain(&parts.join(" · ")), Face::Mono, 7.5, room);
    w.line_right(RIGHT, 70.0, Face::Mono, 7.5, TEXT_2, &line);
    // A URL cut short would point nowhere, so it gets a line of its own.
    if let Some(url) = ids
        .opencorporates_url
        .as_deref()
        .filter(|url| !url.is_empty())
    {
        let url = fit(
            &plain(url.trim_start_matches("https://")),
            Face::Mono,
            7.5,
            room,
        );
        w.line_right(RIGHT, 82.0, Face::Mono, 7.5, TEXT_2, &url);
    }
}

/// One figure card: label, value, unit and the line under it.
struct Card {
    label: String,
    value: String,
    unit: &'static str,
    note: String,
}

/// A row of three figures in one bordered box, split by hairlines.
fn cards(w: &mut Writer, cards: &[Card; 3]) {
    let width = CONTENT_WIDTH / 3.0;
    let inner = width - 26.0;
    let note_lines = cards
        .iter()
        .map(|card| wrap(&card.note, Face::Regular, 8.0, inner).len())
        .max()
        .unwrap_or(1);
    #[allow(clippy::cast_precision_loss)] // a few lines
    let height = 58.0 + note_lines as f32 * 12.0;
    w.ensure(height + 4.0);
    let top = w.y;
    w.boxed(
        MARGIN,
        top,
        CONTENT_WIDTH,
        height,
        8.0,
        None,
        Some(solid(BORDER_2, 0.8)),
    );
    for (index, card) in cards.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // three cards
        let x = MARGIN + index as f32 * width;
        if index > 0 {
            w.boxed(x, top, 0.6, height, 0.0, Some(BORDER_2), None);
        }
        let left = x + 13.0;
        w.line(left, top + 21.0, Face::Regular, 8.5, TEXT_2, &card.label);
        w.line(left, top + 45.0, Face::Mono, 21.0, TEXT, &card.value);
        let unit_x = left + text_width(Face::Mono, 21.0, &card.value) + 3.0;
        w.line(unit_x, top + 45.0, Face::Mono, 8.0, TEXT_2, card.unit);
        w.y = top + 52.0;
        w.block(left, inner, Face::Regular, 8.0, TEXT_2, &card.note);
    }
    w.y = top + height;
}

fn cover_cards(w: &mut Writer, report: &PeriodicReport) {
    let aggregate = &report.aggregate;
    let scope = &report.scope_manifest;
    let requests = match scope.coverage_percentage {
        Some(coverage) => format!(
            "{} requests measured, {coverage:.1} % of those declared",
            grouped(scope.requests_measured)
        ),
        None => format!("{} requests measured", grouped(scope.requests_measured)),
    };
    let carbon = aggregate.total_carbon_kgco2eq;
    let canonical = &aggregate.canonical_waste;
    let avoidable = if canonical.is_default() {
        format!("the canonical reading is {}", NOT_REPORTED.to_lowercase())
    } else {
        format!(
            "{} of measured I/O, at the fixed canonical threshold",
            percent(canonical.waste_ratio)
        )
    };
    cards(
        w,
        &[
            Card {
                label: "Energy, operational".into(),
                value: format!("{:.2}", aggregate.total_energy_kwh),
                unit: "kWh",
                note: requests,
            },
            Card {
                label: "Carbon, location-based".into(),
                value: format!("{carbon:.2}"),
                unit: "kgCO₂eq",
                note: format!(
                    "a directional estimate, true value likely between {:.2} and {:.2}",
                    carbon / 2.0,
                    carbon * 2.0
                ),
            },
            Card {
                label: "Avoidable with known fixes".into(),
                value: format!("{:.2}", aggregate.estimated_optimization_potential_kgco2eq),
                unit: "kgCO₂eq",
                note: avoidable,
            },
        ],
    );
}

/// The declared level, what proves the document, the code and the QR.
fn proof_block(w: &mut Writer, report: &PeriodicReport, verify_page: usize) {
    let org = &report.organisation.name;
    let top = w.y;
    let qr_side = 110.0;
    let column = CONTENT_WIDTH - qr_side - 28.0;
    let pill = format!(
        "Declared: {}",
        integrity_label(report.report_metadata.integrity_level)
    )
    .to_uppercase();
    let pill_width = text_width(Face::Bold, 7.5, &pill) + 30.0;
    w.boxed(
        MARGIN,
        top,
        pill_width,
        18.0,
        9.0,
        None,
        Some(Outline {
            color: DASH,
            width: 0.8,
            dashed: true,
        }),
    );
    w.push(Op::Circle {
        cx: MARGIN + 11.0,
        cy: top + 9.0,
        r: 2.6,
        fill: OK_FG,
    });
    w.line(MARGIN + 19.0, top + 11.8, Face::Bold, 7.5, OK_FG, &pill);
    w.y = top + 26.0;
    let proof = if is_signed(report) {
        format!(
            "This document carries the machine-readable report it was rendered from, as an attachment, with the signature {org}'s pipeline placed on it in a public log. The printed code alone proves nothing: the proof is that signature, checked against the signer {org} publishes. Page {verify_page} shows how, in five minutes."
        )
    } else {
        format!(
            "This document carries the machine-readable report it was rendered from, as an attachment. The printed code alone proves nothing: the proof is the attached report, rechecked with the command on page {verify_page}. This report is not signed."
        )
    };
    w.block(MARGIN, column, Face::Regular, 9.0, TEXT_2, &proof);
    w.y += 6.0;
    w.block(MARGIN, column, Face::Mono, 8.0, TEXT_2, "document code");
    w.block(
        MARGIN,
        column,
        Face::Mono,
        8.5,
        TEXT,
        &group4(&report.integrity.content_hash),
    );
    w.y += 4.0;
    w.block(
        MARGIN,
        column,
        Face::Mono,
        8.0,
        TEXT,
        &format!(
            "report {} · generated {}",
            report.report_metadata.report_uuid,
            report
                .report_metadata
                .generated_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ),
    );
    if let Some(signature) = report.integrity.signature.as_ref() {
        w.y += 4.0;
        w.block(
            MARGIN,
            column,
            Face::Mono,
            8.0,
            TEXT,
            &format!(
                "signed {} · Rekor log index {}",
                signature.signed_at,
                grouped(signature.rekor_log_index)
            ),
        );
    }
    let text_bottom = w.y;
    w.y = top;
    transparency(w, report, RIGHT - qr_side, qr_side);
    w.y = w.y.max(text_bottom);
}

/// The QR code to the organisation's own transparency page, when the
/// report names one.
fn transparency(w: &mut Writer, report: &PeriodicReport, x: f32, side: f32) {
    let Some(url) = report.notes.reference_urls.get("transparency") else {
        return;
    };
    let Some(modules) = qr_modules(url) else {
        return;
    };
    let top = w.y;
    w.push(Op::Qr {
        x,
        y: top,
        side,
        modules,
    });
    let caption = format!(
        "Scan to open {}'s transparency page, which lists this document code.",
        report.organisation.name
    );
    let mut baseline = top + side + 12.0;
    let lines = wrap(&caption, Face::Regular, 7.5, side + 20.0)
        .into_iter()
        .chain(wrap(
            url.trim_start_matches("https://"),
            Face::Regular,
            7.5,
            side + 20.0,
        ));
    for line in lines {
        w.line_centred(x + side / 2.0, baseline, Face::Regular, 7.5, TEXT_2, &line);
        baseline += 10.5;
    }
    w.y = baseline;
}

fn qr_modules(url: &str) -> Option<Vec<Vec<bool>>> {
    let code = fast_qr::QRBuilder::new(url).build().ok()?;
    Some(
        (0..code.size)
            .map(|row| code[row].iter().map(|module| module.value()).collect())
            .collect(),
    )
}

// ---------------------------------------------------------------- page 2

fn at_a_glance(w: &mut Writer, report: &PeriodicReport) {
    let aggregate = &report.aggregate;
    w.section("At a glance");
    w.title("Three numbers, and how sure we are of them");
    let intro = format!(
        "Energy is measured from the work the services did. {} Carbon applies the carbon intensity of the electricity grid to that energy. Avoidable is what a known class of defects, repeated database and HTTP calls, wasted.",
        host_perimeter(aggregate)
    );
    w.para(10.5, TEXT_2, &intro);
    w.y += 2.0;
    let carbon = aggregate.total_carbon_kgco2eq;
    cards(
        w,
        &[
            Card {
                label: "Energy, operational".into(),
                value: format!("{:.2}", aggregate.total_energy_kwh),
                unit: "kWh",
                note: format!(
                    "over {} days, {} requests",
                    report.period.days_covered,
                    grouped(aggregate.total_requests)
                ),
            },
            Card {
                label: "Carbon, location-based".into(),
                value: format!("{carbon:.2}"),
                unit: "kgCO₂eq",
                note: "a directional estimate, see the band below".into(),
            },
            Card {
                label: "Efficiency score".into(),
                value: format!("{:.1}", aggregate.aggregate_efficiency_score),
                unit: "/ 100",
                note: "higher is better, computed by the engine".into(),
            },
        ],
    );
    w.heading("The carbon figure and its uncertainty");
    w.para(
        8.5,
        TEXT_2,
        "The method is a directional estimate with a ~2x bracket. The true value is likely to sit anywhere in the shaded band.",
    );
    uncertainty_band(w, carbon);
    if let Some(text) = aggregate
        .carbon_breakdown
        .as_ref()
        .and_then(carbon_breakdown)
    {
        w.para(8.5, TEXT_2, &text);
    }
    w.heading("Avoidable waste, two readings of the same data");
    // Both bars share one track length, sized from the wider value, so their
    // fills stay comparable.
    let value_room = [&aggregate.canonical_waste, &aggregate.operational_waste]
        .into_iter()
        .map(|tier| text_width(Face::Mono, 8.5, &waste_value(tier)) + 16.0)
        .fold(0.0_f32, f32::max);
    waste_bar(
        w,
        "Canonical",
        &aggregate.canonical_waste,
        BRAND,
        value_room,
    );
    waste_bar(
        w,
        "Operational",
        &aggregate.operational_waste,
        INFO,
        value_room,
    );
    w.para(
        8.5,
        TEXT_2,
        "The canonical figure uses a threshold fixed in the measuring binary, so the operator cannot shrink it by tuning. The operational figure uses the operator's own threshold and is what their dashboard shows. Both are published so a reader can compare.",
    );
    w.heading(&format!(
        "How much of the {} was actually measured",
        period_noun(report)
    ));
    // period_coverage reads 1.0 when absent, so a bar would show a full
    // measure the card beside it calls not reported.
    if aggregate.runtime_windows_count > 0 || aggregate.fallback_windows_count > 0 {
        coverage_bar(w, aggregate.period_coverage);
    }
    coverage_cards(w, report);
    w.heading("Where these numbers go in European climate reporting");
    crosswalk(w, report);
}

/// The 2x band, drawn at fixed places around the total, not to scale.
fn uncertainty_band(w: &mut Writer, carbon: f64) {
    w.ensure(36.0);
    let top = w.y + 4.0;
    w.boxed(MARGIN, top, CONTENT_WIDTH, 11.0, 5.5, Some(TRACK), None);
    let band_x = MARGIN + CONTENT_WIDTH * 0.18;
    let band_w = CONTENT_WIDTH * 0.64;
    w.boxed(
        band_x,
        top,
        band_w,
        11.0,
        5.5,
        Some(ACCENT_BG),
        Some(solid(ACCENT_BD, 0.6)),
    );
    let centre = MARGIN + CONTENT_WIDTH * 0.5;
    w.push(Op::Circle {
        cx: centre,
        cy: top + 5.5,
        r: 7.0,
        fill: BRAND,
    });
    let base = top + 25.0;
    w.line_centred(
        band_x,
        base,
        Face::Mono,
        7.5,
        TEXT_2,
        &format!("{:.2}", carbon / 2.0),
    );
    w.line_centred(
        centre,
        base,
        Face::Mono,
        7.5,
        TEXT,
        &format!("{carbon:.2} kgCO₂eq"),
    );
    w.line_centred(
        band_x + band_w,
        base,
        Face::Mono,
        7.5,
        TEXT_2,
        &format!("{:.2}", carbon * 2.0),
    );
    w.y = base + 6.0;
}

fn waste_value(tier: &WasteTier) -> String {
    format!(
        "{:.2} kg · {:.2} kWh · {}",
        tier.carbon_kgco2eq,
        tier.energy_kwh,
        percent(tier.waste_ratio)
    )
}

fn waste_bar(w: &mut Writer, label: &str, tier: &WasteTier, color: Rgb, value_room: f32) {
    w.ensure(18.0);
    let top = w.y + 2.0;
    if tier.is_default() {
        w.line(
            MARGIN,
            top + 9.0,
            Face::Regular,
            9.0,
            TEXT,
            &format!("{label}: {NOT_REPORTED}"),
        );
        w.y = top + 16.0;
        return;
    }
    w.line(
        MARGIN,
        top + 9.0,
        Face::Regular,
        9.0,
        TEXT,
        &format!("{label}, N+1 from {}", tier.n_plus_one_threshold),
    );
    let track_x = MARGIN + 120.0;
    let track_w = CONTENT_WIDTH - 120.0 - value_room;
    w.boxed(track_x, top + 3.0, track_w, 7.0, 3.5, Some(TRACK), None);
    #[allow(clippy::cast_possible_truncation)] // a ratio in [0, 1]
    let filled = track_w * tier.waste_ratio.clamp(0.0, 1.0) as f32;
    w.boxed(
        track_x,
        top + 3.0,
        filled.max(7.0),
        7.0,
        3.5,
        Some(color),
        None,
    );
    w.line_right(RIGHT, top + 9.0, Face::Mono, 8.5, TEXT, &waste_value(tier));
    w.y = top + 17.0;
}

fn coverage_bar(w: &mut Writer, covered: f64) {
    w.ensure(32.0);
    let top = w.y + 4.0;
    w.boxed(MARGIN, top, CONTENT_WIDTH, 7.0, 3.5, Some(TRACK), None);
    #[allow(clippy::cast_possible_truncation)] // a ratio in [0, 1]
    let filled = CONTENT_WIDTH * covered.clamp(0.0, 1.0) as f32;
    let color = if covered < OFFICIAL_COVERAGE {
        WARN_FG
    } else {
        BRAND
    };
    w.boxed(MARGIN, top, filled.max(7.0), 7.0, 3.5, Some(color), None);
    #[allow(clippy::cast_possible_truncation)] // a constant ratio
    let notch = MARGIN + CONTENT_WIDTH * OFFICIAL_COVERAGE as f32;
    w.boxed(notch - 0.6, top - 4.0, 1.2, 15.0, 0.0, Some(TEXT), None);
    let label = format!("an official report needs {}", percent(OFFICIAL_COVERAGE));
    w.line_centred(notch, top + 22.0, Face::Mono, 7.0, TEXT_2, &label);
    w.y = top + 30.0;
}

fn coverage_cards(w: &mut Writer, report: &PeriodicReport) {
    let aggregate = &report.aggregate;
    let windows = aggregate.runtime_windows_count + aggregate.fallback_windows_count;
    let runtime = if windows == 0 {
        (
            format!("Runtime coverage: {NOT_REPORTED}"),
            "No scoring window carried an energy model.".to_string(),
        )
    } else {
        (
            format!("Runtime coverage {}", percent(aggregate.period_coverage)),
            format!(
                "{} of {windows} scoring windows carried per-service energy attribution. The other {} fell back to a proportional share of I/O operations. An official report needs at least {}.",
                aggregate.runtime_windows_count,
                aggregate.fallback_windows_count,
                percent(OFFICIAL_COVERAGE)
            ),
        )
    };
    let temporal = &aggregate.temporal_coverage;
    let days = if temporal.days_in_period == 0 {
        (
            format!("Temporal coverage: {NOT_REPORTED}"),
            "The report predates this measure.".to_string(),
        )
    } else {
        (
            format!("Temporal coverage {}", percent(temporal.temporal_coverage)),
            format!(
                "{} of {} days carried measured traffic, the largest gap was {} days. Quiet days lower this figure too, so it is a floor on activity, not an uptime claim.",
                temporal.observed_days, temporal.days_in_period, temporal.largest_gap_days
            ),
        )
    };
    let gap = 14.0;
    let width = (CONTENT_WIDTH - gap) / 2.0;
    let inner = width - 26.0;
    let lines = [&runtime.1, &days.1]
        .iter()
        .map(|body| wrap(body, Face::Regular, 8.5, inner).len())
        .max()
        .unwrap_or(1);
    #[allow(clippy::cast_precision_loss)] // a few lines
    let height = 40.0 + lines as f32 * 12.75;
    w.ensure(height + 10.0);
    let top = w.y + 6.0;
    for (index, (title, body)) in [runtime, days].iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // two cards
        let x = MARGIN + index as f32 * (width + gap);
        w.boxed(
            x,
            top,
            width,
            height,
            8.0,
            Some(SURFACE),
            Some(solid(BORDER_2, 0.6)),
        );
        w.y = top + 12.0;
        w.block(x + 13.0, inner, Face::Bold, 10.5, TEXT, title);
        w.y += 2.0;
        w.block(x + 13.0, inner, Face::Regular, 8.5, TEXT, body);
    }
    w.y = top + height + 4.0;
}

fn crosswalk(w: &mut Writer, report: &PeriodicReport) {
    let Some(crosswalk) = report.methodology.standard_crosswalk.as_ref() else {
        w.para(8.5, TEXT_2, NOT_REPORTED);
        return;
    };
    let mappings: Vec<String> = crosswalk
        .mappings
        .iter()
        .map(|entry| {
            format!(
                "{} feeds {}: {}",
                entry.report_field, entry.datapoint, entry.note
            )
        })
        .collect();
    w.para(
        8.5,
        TEXT_2,
        &format!(
            "{}, a mapping aid and not a certification. {}",
            crosswalk.standard,
            mappings.join(" ")
        ),
    );
    if !crosswalk.caveats.is_empty() {
        w.para(
            8.5,
            TEXT_2,
            &format!(
                "Before using these figures for {}: {}",
                crosswalk.standard,
                crosswalk.caveats.join(" ")
            ),
        );
    }
}

/// Which database and broker hosts the totals include, and which stay out.
fn host_perimeter(aggregate: &Aggregate) -> String {
    let hosts = [
        ("database", &aggregate.database_waste),
        ("broker", &aggregate.messaging_waste),
    ];
    let mut inside = Vec::new();
    let mut outside = Vec::new();
    for (noun, block) in hosts {
        // Same predicate as the disclaimers and the preview.
        match block.as_ref().filter(|b| b.in_totals_windows > 0) {
            Some(b) if b.in_totals_windows < b.windows_with_figure => inside.push(format!(
                "{noun} hosts ({} kWh, in {} of {} windows)",
                host_kwh(b.in_totals_energy_kwh),
                b.in_totals_windows,
                b.windows_with_figure
            )),
            Some(b) => inside.push(format!(
                "{noun} hosts ({} kWh)",
                host_kwh(b.in_totals_energy_kwh)
            )),
            None => outside.push(noun),
        }
    }
    let included =
        (!inside.is_empty()).then(|| format!("The totals include the {}.", inside.join(" and ")));
    let excluded = match outside.as_slice() {
        [] => None,
        [one] => Some(format!("The {one} servers are outside these totals.")),
        _ => Some("The database and broker servers are outside these totals.".to_string()),
    };
    [included, excluded]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Host kWh with two decimals, scientific below 0.005 so a small in-total
/// host never reads as zero.
fn host_kwh(v: f64) -> String {
    if v == 0.0 || v >= 0.005 {
        format!("{v:.2}")
    } else {
        format!("{v:.1e}")
    }
}

/// The in-total database and broker host rows of the application table:
/// label, kWh and kgCO2eq. Their energy is inside `total_energy_kwh` and
/// in no application row.
fn host_rows(aggregate: &Aggregate) -> Vec<(&'static str, f64, f64)> {
    let breakdown = aggregate.carbon_breakdown.as_ref();
    [
        (
            "Database hosts",
            &aggregate.database_waste,
            breakdown.and_then(|b| b.database_kgco2eq),
        ),
        (
            "Broker hosts",
            &aggregate.messaging_waste,
            breakdown.and_then(|b| b.messaging_kgco2eq),
        ),
    ]
    .into_iter()
    .filter_map(|(label, block, kg)| {
        block
            .as_ref()
            .filter(|b| b.in_totals_windows > 0)
            .map(|b| (label, b.in_totals_energy_kwh, kg.unwrap_or(0.0)))
    })
    .collect()
}

/// The SCI terms the carbon total is made of, then what stays outside it.
fn carbon_breakdown(breakdown: &CarbonBreakdown) -> Option<String> {
    let transport = breakdown.transport_kgco2eq.map(|mid| {
        match (
            breakdown.transport_kgco2eq_low,
            breakdown.transport_kgco2eq_high,
        ) {
            (Some(low), Some(high)) => {
                format!("network transport {mid:.2} kg, a ceiling between {low:.2} and {high:.2}")
            }
            _ => format!("network transport {mid:.2} kg, a ceiling"),
        }
    });
    let inside: Vec<String> = [
        breakdown
            .operational_kgco2eq
            .map(|v| format!("operational {v:.2} kg")),
        breakdown
            .embodied_kgco2eq
            .map(|v| format!("embodied {v:.2} kg")),
        transport,
        breakdown
            .database_kgco2eq
            .map(|v| format!("database hosts {v:.2} kg")),
        breakdown
            .messaging_kgco2eq
            .map(|v| format!("broker hosts {v:.2} kg")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let outside: Vec<String> = [
        breakdown
            .database_kgco2eq_out_of_total
            .map(|v| format!("database {v:.2} kg")),
        breakdown
            .messaging_kgco2eq_out_of_total
            .map(|v| format!("broker {v:.2} kg")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let made_of = (!inside.is_empty()).then(|| format!("Made of {}.", inside.join(", ")));
    let beside = (!outside.is_empty()).then(|| {
        format!(
            "Outside this total, from windows that kept the host figure beside it: {}.",
            outside.join(", ")
        )
    });
    let text: Vec<String> = [made_of, beside].into_iter().flatten().collect();
    (!text.is_empty()).then(|| text.join(" "))
}

// ---------------------------------------------------------------- page 3

/// One row of the application table, whatever the granularity. Carbon is
/// `None` where the report carries none per application.
struct AppFigures<'a> {
    name: &'a str,
    requests: u64,
    energy: f64,
    carbon: Option<f64>,
    efficiency: f64,
    patterns: u64,
}

fn application_figures(application: &Application) -> AppFigures<'_> {
    match application {
        Application::G1(app) => AppFigures {
            name: app.display_name.as_deref().unwrap_or(&app.service_name),
            requests: app.total_requests,
            energy: app.energy_kwh,
            carbon: None,
            efficiency: app.efficiency_score,
            patterns: app.anti_patterns.len() as u64,
        },
        Application::G2(app) => AppFigures {
            name: app.display_name.as_deref().unwrap_or(&app.service_name),
            requests: app.total_requests,
            energy: app.energy_kwh,
            carbon: Some(app.carbon_kgco2eq),
            efficiency: app.efficiency_score,
            patterns: app.anti_patterns_detected_count,
        },
    }
}

fn applications(w: &mut Writer, report: &PeriodicReport) {
    let scope = &report.scope_manifest;
    let org = &report.organisation.name;
    w.section("Applications");
    w.title("Which services consumed what");
    let all_g2 = report
        .applications
        .iter()
        .all(|app| matches!(app, Application::G2(_)));
    let detail = if all_g2 {
        "The count of anti-patterns is published, their detail is not, by choice of granularity."
    } else {
        "Each anti-pattern is detailed in the attached report."
    };
    w.para(
        10.5,
        TEXT_2,
        &format!(
            "{} of the {} applications {org} declared were measured. Per-service carbon is operational only. {detail}",
            capitalised(&number_word(scope.applications_measured)),
            number_word(scope.total_applications_declared),
        ),
    );
    application_table(w, report);
    w.heading("Scope, as declared and as measured");
    scope_rows(w, report);
    w.callout(
        (WARN_FG, WARN_BG, WARN_BD),
        "Reading the efficiency score",
        "The score rewards services whose I/O is useful work rather than repetition. A lower score does not mean a larger footprint: a service can score higher than another while consuming less, because it carries less traffic, not because it is better written.",
    );
}

/// The height of a table row, its name wrapped in the first column.
fn row_height(name: &str) -> f32 {
    (6.0 + block_height(name, Face::Bold, 9.0, NAME_WIDTH)).max(23.0) + 3.0
}

/// A table row: the name in bold, then the right-aligned figures.
fn table_row(w: &mut Writer, name: &str, requests: &str, figures: [&str; 4]) {
    let top = w.y;
    let base = top + 15.0;
    w.y = top + 6.0;
    w.block(MARGIN + 4.0, NAME_WIDTH, Face::Bold, 9.0, TEXT, name);
    let name_bottom = w.y;
    w.line_right(
        MARGIN + REQUESTS_RIGHT,
        base,
        Face::Mono,
        8.5,
        TEXT,
        requests,
    );
    for (right, figure) in FIGURE_RIGHTS.iter().zip(figures) {
        w.line_right(MARGIN + right - 4.0, base, Face::Mono, 8.5, TEXT, figure);
    }
    w.y = name_bottom.max(top + 23.0);
    w.rule(MARGIN, w.y, CONTENT_WIDTH);
}

fn table_header(w: &mut Writer) {
    w.ensure(60.0);
    w.y += 6.0;
    let base = w.y + 10.0;
    w.line(
        MARGIN + 4.0,
        base,
        Face::Regular,
        7.5,
        TEXT_2,
        "Application",
    );
    w.line_right(
        MARGIN + REQUESTS_RIGHT,
        base,
        Face::Mono,
        7.5,
        TEXT_2,
        "Requests",
    );
    w.line(
        MARGIN + SHARE_LEFT,
        base,
        Face::Regular,
        7.5,
        TEXT_2,
        "Share of energy",
    );
    for (right, label) in
        FIGURE_RIGHTS
            .iter()
            .zip(["kWh", "kgCO₂eq", "Efficiency", "Anti-patterns"])
    {
        w.line_right(MARGIN + right - 4.0, base, Face::Mono, 7.5, TEXT_2, label);
    }
    w.y = base + 8.0;
    w.rule(MARGIN, w.y, CONTENT_WIDTH);
}

fn application_table(w: &mut Writer, report: &PeriodicReport) {
    table_header(w);
    let total_energy = report.aggregate.total_energy_kwh;
    // Carbon totals only when every row carries it, as G2 rows do.
    let mut totals = (0_u64, Some(0.0_f64), 0_u64);
    for application in &report.applications {
        let app = application_figures(application);
        totals.0 += app.requests;
        totals.1 = totals.1.zip(app.carbon).map(|(sum, carbon)| sum + carbon);
        totals.2 += app.patterns;
        w.ensure(row_height(app.name));
        let share = if total_energy > 0.0 {
            app.energy / total_energy
        } else {
            0.0
        };
        share_bar(w, share);
        table_row(
            w,
            app.name,
            &grouped(app.requests),
            [
                &format!("{:.2}", app.energy),
                &app.carbon
                    .map_or_else(|| "-".to_string(), |c| format!("{c:.2}")),
                &format!("{:.1}", app.efficiency),
                &app.patterns.to_string(),
            ],
        );
    }
    let reason_width = CONTENT_WIDTH - NAME_WIDTH - 14.0;
    for excluded in &report.scope_manifest.applications_excluded {
        let reason = format!(
            "Excluded by the operator: {}. Declared, not measured.",
            excluded.reason
        );
        let name_height = block_height(&excluded.service_name, Face::Bold, 9.0, NAME_WIDTH);
        let reason_height = block_height(&reason, Face::Regular, 8.5, reason_width);
        w.ensure(6.0 + name_height.max(reason_height) + 10.0);
        let top = w.y;
        w.y = top + 6.0;
        w.block(
            MARGIN + 4.0,
            NAME_WIDTH,
            Face::Bold,
            9.0,
            TEXT_2,
            &excluded.service_name,
        );
        let name_bottom = w.y;
        w.y = top + 6.0;
        w.block(
            MARGIN + NAME_WIDTH + 14.0,
            reason_width,
            Face::Regular,
            8.5,
            TEXT_2,
            &reason,
        );
        w.y = w.y.max(name_bottom) + 4.0;
        w.rule(MARGIN, w.y, CONTENT_WIDTH);
    }
    for (label, energy, carbon) in host_rows(&report.aggregate) {
        totals.1 = totals.1.map(|sum| sum + carbon);
        w.ensure(row_height(label));
        share_bar(
            w,
            if total_energy > 0.0 {
                energy / total_energy
            } else {
                0.0
            },
        );
        table_row(
            w,
            label,
            "-",
            [&host_kwh(energy), &format!("{carbon:.2}"), "-", "-"],
        );
    }
    w.ensure(row_height("Total"));
    table_row(
        w,
        "Total",
        &grouped(totals.0),
        [
            &format!("{total_energy:.2}"),
            &totals
                .1
                .map_or_else(|| "-".to_string(), |carbon| format!("{carbon:.2}")),
            &format!("{:.1}", report.aggregate.aggregate_efficiency_score),
            &totals.2.to_string(),
        ],
    );
}

/// The share-of-energy bar and its percentage, in the row about to be drawn.
fn share_bar(w: &mut Writer, share: f64) {
    let x = MARGIN + SHARE_LEFT;
    let track_w = SHARE_RIGHT - SHARE_LEFT - 38.0;
    let top = w.y + 9.0;
    w.boxed(x, top, track_w, 5.0, 2.5, Some(TRACK), None);
    #[allow(clippy::cast_possible_truncation)] // a ratio in [0, 1]
    let filled = track_w * share.clamp(0.0, 1.0) as f32;
    w.boxed(x, top, filled.max(5.0), 5.0, 2.5, Some(BRAND), None);
    w.line_right(
        MARGIN + SHARE_RIGHT,
        top + 5.5,
        Face::Regular,
        8.0,
        TEXT,
        &percent(share),
    );
}

fn scope_rows(w: &mut Writer, report: &PeriodicReport) {
    let scope = &report.scope_manifest;
    let environments = if scope.environments_measured.is_empty() {
        NOT_REPORTED.to_string()
    } else {
        format!(
            "{} environment{} ({})",
            scope.environments_measured.len(),
            if scope.environments_measured.len() == 1 {
                ""
            } else {
                "s"
            },
            scope.environments_measured.join(", ")
        )
    };
    let excluded = if scope.environments_excluded.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = scope
            .environments_excluded
            .iter()
            .map(|env| format!("{} ({})", env.name, env.reason))
            .collect();
        format!(" · excluded {}", names.join(", "))
    };
    let declared_requests = scope.total_requests_in_period.map_or_else(
        || "requests in the period not declared".to_string(),
        |n| format!("{} requests in the period", grouped(n)),
    );
    w.row(
        "Declared by the operator",
        &format!(
            "{} applications · {environments}{excluded} · {declared_requests}",
            scope.total_applications_declared
        ),
        Face::Mono,
    );
    let coverage = scope.coverage_percentage.map_or_else(
        || "coverage not computed".to_string(),
        |c| format!("{c:.1} % coverage"),
    );
    w.row(
        "Derived by the engine",
        &format!(
            "{} applications measured · {} requests measured · {coverage}",
            scope.applications_measured,
            grouped(scope.requests_measured)
        ),
        Face::Mono,
    );
    let calibration = &report.methodology.calibration_inputs;
    let region = if calibration.cloud_regions.is_empty() {
        NOT_REPORTED.to_string()
    } else {
        format!(
            "{} · carbon intensity from {}",
            calibration.cloud_regions.join(", "),
            calibration.carbon_intensity_source
        )
    };
    w.row("Region", &region, Face::Mono);
}

// ---------------------------------------------------------------- page 4

fn method(w: &mut Writer, report: &PeriodicReport) {
    let methodology = &report.methodology;
    w.section("Methodology and limits");
    w.title("How the figures were produced, and what they are not");
    w.y += 2.0;
    w.row(
        "Specification",
        &format!(
            "{} · conformance {}",
            methodology.sci_specification,
            wire(&methodology.conformance)
        ),
        Face::Mono,
    );
    w.row("Measuring binary", &measuring_binary(report), Face::Mono);
    w.row("Detectors enabled", &detectors(report), Face::Mono);
    let disabled = if methodology.disabled_patterns.is_empty() {
        "none".to_string()
    } else {
        methodology
            .disabled_patterns
            .iter()
            .map(|pattern| format!("{} ({})", pattern.name, pattern.reason))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    w.row("Detectors disabled", &disabled, Face::Mono);
    w.row(
        "Runtime calibration",
        &format!(
            "{} this period · {} runtime windows, {} fallback windows",
            if methodology.calibration_inputs.calibration_applied {
                "applied"
            } else {
                "not applied"
            },
            report.aggregate.runtime_windows_count,
            report.aggregate.fallback_windows_count
        ),
        Face::Mono,
    );
    w.row("Archive", &archive_summary(report), Face::Mono);
    if let Some(references) = references(report) {
        w.row("References", &references, Face::Mono);
    }
    w.heading("Disclaimers, exactly as the report carries them");
    disclaimers(w, report);
    w.heading("What this document is");
    let audience = match report.report_metadata.confidentiality_level {
        Confidentiality::Public => "A public transparency report",
        Confidentiality::Internal => "An internal report",
    };
    w.para(
        8.5,
        TEXT_2,
        &format!(
            "{audience} produced by an open-source tool, published by {} on its own responsibility. It is not an audited inventory and does not make a CSRD submission. The scope is IT compute observed through application traces: no end-user devices, no buildings.",
            report.organisation.name
        ),
    );
}

fn measuring_binary(report: &PeriodicReport) -> String {
    let methodology = &report.methodology;
    let calibration = &methodology.calibration_inputs;
    let mut parts = vec![
        format!("perf-sentinel {}", methodology.perf_sentinel_version),
        format!(
            "SPECpower table {}",
            calibration
                .binary_specpower_vintage
                .as_deref()
                .unwrap_or(&calibration.specpower_table_version)
        ),
    ];
    if !calibration.energy_source_models.is_empty() {
        let models: Vec<String> = calibration
            .energy_source_models
            .iter()
            .map(|tag| match crate::score::carbon::co2_model_label(tag) {
                Some(label) => format!("{label} ({tag})"),
                None => tag.clone(),
            })
            .collect();
        parts.push(format!("energy models {}", models.join(", ")));
    }
    if let Some(attestation) = report.integrity.binary_attestation.as_ref() {
        parts.push(format!(
            "built from {} at {}, SLSA {}",
            attestation.git_tag, attestation.git_commit, attestation.slsa_level
        ));
    }
    if let Some(hash) = report.integrity.binary_hash.as_deref() {
        parts.push(format!("binary {}", group4(hash)));
    }
    parts.join(" · ")
}

/// The report's reference links, the transparency page aside when the cover
/// already prints it beside its QR code.
fn references(report: &PeriodicReport) -> Option<String> {
    let links: Vec<String> = report
        .notes
        .reference_urls
        .iter()
        .filter(|(key, url)| key.as_str() != "transparency" || qr_modules(url).is_none())
        .map(|(key, url)| format!("{key} {}", url.trim_start_matches("https://")))
        .collect();
    (!links.is_empty()).then(|| links.join(" · "))
}

fn disclaimers(w: &mut Writer, report: &PeriodicReport) {
    if report.notes.disclaimers.is_empty() {
        w.para(9.0, TEXT, NOT_REPORTED);
    }
    for (index, disclaimer) in report.notes.disclaimers.iter().enumerate() {
        w.ensure(14.0);
        w.line(
            MARGIN + 4.0,
            w.y + 9.0,
            Face::Regular,
            9.0,
            TEXT,
            &format!("{}.", index + 1),
        );
        w.block(
            MARGIN + 18.0,
            CONTENT_WIDTH - 18.0,
            Face::Regular,
            9.0,
            TEXT,
            disclaimer,
        );
        w.y += 3.0;
    }
}

fn detectors(report: &PeriodicReport) -> String {
    let methodology = &report.methodology;
    let (core, other): (Vec<&str>, Vec<&str>) = methodology
        .enabled_patterns
        .iter()
        .map(String::as_str)
        .partition(|pattern| {
            methodology
                .core_patterns_required
                .iter()
                .any(|core| core == pattern)
        });
    match (core.is_empty(), other.is_empty()) {
        (true, true) => "none".to_string(),
        (false, true) => format!("{} (core, required)", core.join(", ")),
        (true, false) => other.join(", "),
        (false, false) => format!(
            "{} (core, required) · {}",
            core.join(", "),
            other.join(", ")
        ),
    }
}

fn archive_summary(report: &PeriodicReport) -> String {
    let chain = &report.integrity.trace_integrity_chain;
    let field = |name: &str| chain.get(name).and_then(serde_json::Value::as_u64);
    let (Some(verified), Some(breaks)) = (field("windows_verified"), field("breaks")) else {
        return NOT_REPORTED.to_string();
    };
    let mut parts = vec![
        format!("{verified} scoring windows verified, hash-chained by the daemon"),
        counted(breaks, "break", "breaks"),
    ];
    if let Some(unchained) = field("windows_unchained").filter(|n| *n > 0) {
        parts.push(format!(
            "{unchained} written before chaining, not attestable"
        ));
    }
    if let Some(outside) = field("breaks_outside_period").filter(|n| *n > 0) {
        parts.push(format!(
            "{} outside the period",
            counted(outside, "break", "breaks")
        ));
    }
    if let Some(dropped) = field("windows_dropped") {
        let mut part = format!("{} not archived", counted(dropped, "window", "windows"));
        if let Some(resets) = field("drop_counter_resets").filter(|n| *n > 0) {
            part.push_str(", a lower bound across ");
            part.push_str(&counted(resets, "daemon restart", "daemon restarts"));
        }
        parts.push(part);
    }
    parts.join(" · ")
}

fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

// ---------------------------------------------------------------- page 5

fn verify(w: &mut Writer, context: &PageContext<'_>) {
    let report = context.report;
    w.section("Verify this document");
    w.title("Five minutes to know it was not altered");
    w.para(
        10.5,
        TEXT_2,
        "The check is on the report attached inside this PDF, not on its layout.",
    );
    claims(w, context);
    attachments(w, context);
    w.heading("If you do not run commands");
    steps(w, report);
    w.heading("If you do");
    command_box(w, context);
    w.para(
        8.5,
        TEXT_2,
        "It reads the attachments out of the PDF itself. TRUSTED needs cosign and gh on the PATH, perf-sentinel alone stops at PARTIAL.",
    );
    tamper_callout(w, context);
}

/// What a forger meets, said only as far as this document can back it.
fn tamper_callout(w: &mut Writer, context: &PageContext<'_>) {
    let org = &context.report.organisation.name;
    let bound = matches!(
        context.binding,
        Some(AttestationBinding::ContentHash | AttestationBinding::FileBytes)
    );
    let editor = "The code covers the attached report.json only: a PDF editor can change the printed figures without changing it, so compare the figures with the attached report.";
    if is_signed(context.report) && bound {
        w.callout(
            (OK_FG, OK_BG, OK_BD),
            "What a tampered copy looks like",
            &format!(
                "A changed figure in the attached report changes the document code, so it fails step 2. A forger who prints the new code instead fails the command: the signed attestation names the original code, and the signature sits in a public, append-only log that neither {org} nor the forger can edit. {editor}"
            ),
        );
        return;
    }
    let why = if is_signed(context.report) {
        "No attestation attached here ties the signature to these figures"
    } else {
        "This report is not signed"
    };
    w.callout(
        (WARN_FG, WARN_BG, WARN_BD),
        "What this document cannot prove",
        &format!(
            "{why}, so nothing but the page where {org} publishes its codes ties it to {org}. {editor}"
        ),
    );
}

fn claims(w: &mut Writer, context: &PageContext<'_>) {
    let report = context.report;
    let org = &report.organisation.name;
    let rendered = "perf-sentinel when this PDF was rendered, and you";
    let binding = match context.binding {
        None => "nobody, no attestation is attached",
        Some(AttestationBinding::ContentHash | AttestationBinding::FileBytes) => rendered,
        Some(_) => "nobody, this older attestation does not tie these figures",
    };
    let by_you = "you, with the command below";
    let rows = [
        (
            format!(
                "The figures match document code {}",
                group4(&report.integrity.content_hash)
            ),
            rendered,
        ),
        (
            "The signed attestation names that same code".to_string(),
            binding,
        ),
        (
            format!("{org}'s pipeline signed it, in the public Sigstore log"),
            if is_signed(report) {
                by_you
            } else {
                "nobody, this report is not signed"
            },
        ),
        (
            "The measuring binary is an official Perf Sentinel release".to_string(),
            if report.integrity.binary_hash.is_some() {
                by_you
            } else {
                "nobody, the binary is not declared"
            },
        ),
        (
            "An auditor reviewed the figures".to_string(),
            if report.report_metadata.intent == ReportIntent::Audited {
                "declared by the report, not checked here"
            } else {
                "nobody, this report is not audited"
            },
        ),
    ];
    let split = MARGIN + CONTENT_WIDTH * 0.58;
    w.ensure(40.0);
    w.y += 6.0;
    w.line(MARGIN + 4.0, w.y + 9.0, Face::Regular, 7.5, TEXT_2, "Claim");
    w.line(split, w.y + 9.0, Face::Regular, 7.5, TEXT_2, "Checked by");
    w.y += 15.0;
    w.rule(MARGIN, w.y, CONTENT_WIDTH);
    let claim_width = split - MARGIN - 12.0;
    for (claim, by) in &rows {
        let height = block_height(claim, Face::Bold, 8.5, claim_width).max(block_height(
            by,
            Face::Regular,
            8.5,
            RIGHT - split,
        ));
        w.ensure((height + 7.0).max(22.0));
        let top = w.y + 5.0;
        w.y = top;
        w.block(MARGIN + 4.0, claim_width, Face::Bold, 8.5, TEXT, claim);
        let claim_bottom = w.y;
        w.y = top;
        w.block(split, RIGHT - split, Face::Regular, 8.5, TEXT, by);
        w.y = w.y.max(claim_bottom) + 2.0;
        w.rule(MARGIN, w.y, CONTENT_WIDTH);
    }
}

fn attachments(w: &mut Writer, context: &PageContext<'_>) {
    if context.attachments.is_empty() {
        return;
    }
    w.heading("Attached to this PDF");
    for (name, sha256, size) in &context.attachments {
        w.ensure(32.0);
        let top = w.y + 4.0;
        w.boxed(
            MARGIN + 2.0,
            top + 1.0,
            9.0,
            12.0,
            1.5,
            None,
            Some(solid(TEXT_2, 0.7)),
        );
        let role = match *name {
            "report.json" => "source",
            "attestation.intoto.jsonl" => "supplement, names the document code",
            _ => "supplement, Sigstore bundle",
        };
        w.line(MARGIN + 20.0, top + 9.0, Face::Bold, 8.5, TEXT, name);
        let role_x = MARGIN + 20.0 + text_width(Face::Bold, 8.5, name);
        w.line(
            role_x,
            top + 9.0,
            Face::Regular,
            8.5,
            BRAND_TEXT,
            &format!(" · {role}"),
        );
        w.line_right(
            RIGHT,
            top + 9.0,
            Face::Regular,
            8.5,
            TEXT_2,
            &format!("{} KB", size.div_ceil(1024)),
        );
        w.line(
            MARGIN + 20.0,
            top + 21.0,
            Face::Mono,
            7.0,
            TEXT_2,
            &format!("sha256 {sha256}"),
        );
        w.y = top + 28.0;
        w.rule(MARGIN, w.y, CONTENT_WIDTH);
    }
}

fn steps(w: &mut Writer, report: &PeriodicReport) {
    let org = &report.organisation.name;
    let first = report.notes.reference_urls.get("transparency").map_or_else(
        || {
            format!(
                "Ask {org} for the page where it publishes its reports. It should list the document code of each report."
            )
        },
        |url| {
            format!(
                "Scan the code on the cover, or open {}, the page {org} publishes under its own domain. It lists the document code of each report.",
                url.trim_start_matches("https://")
            )
        },
    );
    let steps = [
        first,
        "Compare that code with the document code printed in full on page 1 and in the table above, every group. A different code means a different report. A matching code is only as good as the page you compared it with.".to_string(),
        "For proof, hand this PDF to anyone with a terminal. The steps below need free tools and no account.".to_string(),
    ];
    for (index, step) in steps.iter().enumerate() {
        w.ensure(30.0);
        let top = w.y + 3.0;
        w.push(Op::Circle {
            cx: MARGIN + 9.0,
            cy: top + 8.0,
            r: 8.0,
            fill: ACCENT_BG,
        });
        w.line_centred(
            MARGIN + 9.0,
            top + 11.0,
            Face::Bold,
            8.0,
            BRAND_TEXT,
            &(index + 1).to_string(),
        );
        w.y = top;
        w.block(
            MARGIN + 28.0,
            CONTENT_WIDTH - 28.0,
            Face::Regular,
            9.5,
            TEXT,
            step,
        );
        w.y += 4.0;
    }
}

/// The verify command page 5 prints, as words, placeholders in angle
/// brackets where the reader fills in a value.
pub fn verify_command_words(
    report: &PeriodicReport,
    identity: Option<&str>,
    issuer: Option<&str>,
) -> Vec<String> {
    let mut words: Vec<String> = ["perf-sentinel", "verify-hash", "--report", "report.pdf"]
        .map(String::from)
        .to_vec();
    if report.integrity.binary_hash.is_some() {
        words.push("--verify-binary".to_string());
        words.push("<perf-sentinel-binary>".to_string());
    }
    words.extend([
        "--expected-issuer".to_string(),
        issuer.unwrap_or("<issuer>").to_string(),
        "--expected-identity".to_string(),
        identity.unwrap_or("<identity>").to_string(),
    ]);
    words
}

fn command_box(w: &mut Writer, context: &PageContext<'_>) {
    let report = context.report;
    let words = verify_command_words(report, context.expected_identity, context.expected_issuer);
    // One option per line, the identity quoted on its own line.
    let mut command = vec![format!("{} \\", words[..4].join(" "))];
    let mut rest = words[4..].chunks(2).peekable();
    while let Some(pair) = rest.next() {
        let last = rest.peek().is_none();
        if pair[0] == "--expected-identity" {
            command.push("  --expected-identity \\".to_string());
            command.push(format!("  '{}'", pair[1]));
        } else {
            command.push(format!(
                "  {}{}",
                pair.join(" "),
                if last { "" } else { " \\" }
            ));
        }
    }
    command.push("# TRUSTED    code, signature and binary all check out".to_string());
    command.push(format!(
        "# UNTRUSTED  something was altered, or the signer is not {}'s",
        plain(&report.organisation.name)
    ));
    let inner = CONTENT_WIDTH - 24.0;
    let lines: Vec<(String, f32)> = command
        .iter()
        .flat_map(|line| command_lines(line, inner))
        .collect();
    #[allow(clippy::cast_precision_loss)] // a few lines
    let height = 20.0 + lines.len() as f32 * 11.0;
    w.ensure(height + 8.0);
    let top = w.y + 4.0;
    w.boxed(MARGIN, top, CONTENT_WIDTH, height, 7.0, Some(TRACK), None);
    let mut baseline = top + 17.0;
    for (line, size) in &lines {
        w.line(MARGIN + 12.0, baseline, Face::Mono, *size, TEXT, line);
        baseline += 11.0;
    }
    w.y = top + height + 8.0;
}

/// One line of the command at 7.5 pt, smaller when it does not fit: a
/// token broken across lines no longer matches once copied. Wrapped only
/// below 5.5 pt, a comment's next lines starting with `#` again.
fn command_lines(line: &str, width: f32) -> Vec<(String, f32)> {
    let mut size = 7.5;
    while size > 5.5 && text_width(Face::Mono, size, line) > width {
        size -= 0.25;
    }
    if text_width(Face::Mono, size, line) <= width {
        return vec![(line.to_string(), size)];
    }
    let pad = if line.starts_with('#') {
        "# ".to_string()
    } else {
        " ".repeat(line.len() - line.trim_start().len())
    };
    let room = width - text_width(Face::Mono, size, &pad);
    wrap(line, Face::Mono, size, room)
        .into_iter()
        .enumerate()
        .map(|(index, piece)| {
            let lead = if index == 0 && line.starts_with('#') {
                ""
            } else {
                pad.as_str()
            };
            (format!("{lead}{piece}"), size)
        })
        .collect()
}

// ---------------------------------------------------------------- annex

/// Every field the schema defines, as the attached report carries it, so
/// what it leaves out shows instead of disappearing.
fn annex_pages(w: &mut Writer, context: &PageContext<'_>) {
    w.section("Annex");
    w.title("Every field of the report, as it carries it");
    w.para(
        9.0,
        TEXT_2,
        &format!(
            "Each field the {SCHEMA_VERSION} schema defines, in the order it defines them, as the attached report.json carries it. A report written against an earlier version lacks the fields added since. \"{}\" means the report does not carry the field, or carries it empty. This annex prints the attached report and checks nothing: the proof is still that report, checked as the verify page shows.",
            annex::ABSENT
        ),
    );
    for (section, rows) in annex::rows(&context.source) {
        w.heading(&section);
        for row in &rows {
            let (value, color) = row
                .value
                .as_deref()
                .map_or((annex::ABSENT, WARN_FG), |value| (value, TEXT));
            w.columns(
                &Columns {
                    split: ANNEX_LABEL,
                    size: 7.0,
                    gap: 1.0,
                    min_height: 0.0,
                },
                Cell {
                    text: &row.path,
                    face: Face::Mono,
                    color: TEXT_2,
                },
                Cell {
                    text: value,
                    face: Face::Mono,
                    color,
                },
            );
        }
    }
}
