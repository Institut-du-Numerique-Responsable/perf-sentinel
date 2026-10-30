//! A periodic disclosure as a PDF/A-3b a non-technical reader can follow,
//! carrying the report, its attestation and its bundle as attachments.
//! A rendered PDF stands for two checks: the content hash recomputes, and
//! the attestation, when given, names it.

mod annex;
mod extract;
pub(crate) mod layout;
pub(crate) mod logo;
pub(crate) mod pages;

use chrono::{Datelike, Timelike};
use krilla::color::rgb;
use krilla::configure::{Archival, ConfigurationBuilder};
use krilla::embed::{AssociationKind, EmbeddedFile, MimeType};
use krilla::geom::{Path, PathBuilder, Point, Rect, Transform};
use krilla::metadata::{DateTime, Metadata};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, LinearGradient, SpreadMethod, Stop, Stroke, StrokeDash};
use krilla::surface::Surface;
use krilla::text::{Font, TextDirection};
use krilla::{Document, SerializeSettings};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

pub use extract::{
    ExtractError, MAX_PDF_BYTES, PdfAttachments, extract_attachments, extract_named,
};
use layout::{Face, missing_glyphs};
pub use pages::verify_command_words;
use pages::{Op, Outline, PageContext, Rgb, layout_pages};

use super::version::{SchemaVersionError, check_schema_version};
use super::{
    AttestationBinding, InTotoStatement, PeriodicReport, check_attestation_binding,
    compute_content_hash,
};

/// A4 in points.
const PAGE_WIDTH: f32 = 595.28;
const PAGE_HEIGHT: f32 = 841.89;

/// What `disclose-pdf` renders from. Attachments are embedded byte for byte.
pub struct PdfInputs<'a> {
    pub report_bytes: &'a [u8],
    pub attestation: Option<&'a [u8]>,
    pub bundle: Option<&'a [u8]>,
    pub expected_identity: Option<&'a str>,
    pub expected_issuer: Option<&'a str>,
}

/// Why the document claims something it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    ContentHashMismatch,
    BindingMismatch,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("the report is not what it claims: {}", .0.as_str())]
    Refused(RefusalReason),
    #[error("not a readable periodic report: {0}")]
    Malformed(String),
    #[error("schema {0} is newer than this engine reads, upgrade perf-sentinel")]
    UnsupportedSchema(String),
    #[error("the bundled fonts cannot draw {}", describe_chars(.0))]
    UnsupportedCharacters(Vec<char>),
    /// The PDF writer refused the document, a renderer limit rather than a
    /// fault of the report.
    #[error("the PDF writer refused the document: {0}")]
    ExportFailed(String),
}

impl RefusalReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ContentHashMismatch => "content_hash_mismatch",
            Self::BindingMismatch => "binding_mismatch",
        }
    }
}

impl RenderError {
    /// The reason of the exit contract, from a closed set.
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Refused(reason) => reason.as_str(),
            Self::Malformed(_) => "malformed_report",
            Self::UnsupportedSchema(_) => "unsupported_schema_version",
            Self::UnsupportedCharacters(_) => "unsupported_characters",
            Self::ExportFailed(_) => "pdf_export_failed",
        }
    }

    /// The result of the exit contract: `refused`, `input_error` or `unrenderable`.
    #[must_use]
    pub fn result(&self) -> &'static str {
        match self {
            Self::Refused(_) => "refused",
            Self::Malformed(_) => "input_error",
            Self::UnsupportedSchema(_) | Self::UnsupportedCharacters(_) | Self::ExportFailed(_) => {
                "unrenderable"
            }
        }
    }
}

fn describe_chars(chars: &[char]) -> String {
    chars
        .iter()
        .map(|c| format!("U+{:04X}", u32::from(*c)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Check the report and render it.
///
/// # Errors
///
/// See [`RenderError`]: nothing is rendered unless both checks pass.
pub fn render_disclosure_pdf(inputs: &PdfInputs<'_>) -> Result<Vec<u8>, RenderError> {
    let (report, binding) = checked_report(inputs)?;
    let context = PageContext {
        report: &report,
        // checked_report already parsed these bytes, so this cannot fail.
        source: serde_json::from_slice(inputs.report_bytes).unwrap_or_default(),
        attachments: attachment_digests(inputs),
        binding,
        expected_identity: inputs.expected_identity,
        expected_issuer: inputs.expected_issuer,
    };
    let sheets = layout_pages(&context);
    let missing = missing_glyphs(sheets.iter().flatten().filter_map(|op| match op {
        Op::Text { face, text, .. } => Some((*face, text.as_str())),
        _ => None,
    }));
    if !missing.is_empty() {
        return Err(RenderError::UnsupportedCharacters(missing));
    }
    build_document(&report, inputs, &sheets)
}

/// The lowercase hex SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn attachment_digests(inputs: &PdfInputs<'_>) -> Vec<(&'static str, String, usize)> {
    let mut out = vec![(
        "report.json",
        sha256_hex(inputs.report_bytes),
        inputs.report_bytes.len(),
    )];
    if let Some(attestation) = inputs.attestation {
        out.push((
            "attestation.intoto.jsonl",
            sha256_hex(attestation),
            attestation.len(),
        ));
    }
    if let Some(bundle) = inputs.bundle {
        out.push(("bundle.sig", sha256_hex(bundle), bundle.len()));
    }
    out
}

/// The report, once its schema, hash and binding are known good, with the
/// binding found (`None` without an attestation).
fn checked_report(
    inputs: &PdfInputs<'_>,
) -> Result<(PeriodicReport, Option<AttestationBinding>), RenderError> {
    match check_schema_version(inputs.report_bytes) {
        Ok(()) => {}
        Err(SchemaVersionError::Unsupported(version)) => {
            return Err(RenderError::UnsupportedSchema(version));
        }
        Err(e @ (SchemaVersionError::Missing | SchemaVersionError::Unknown(_))) => {
            return Err(RenderError::Malformed(e.to_string()));
        }
    }
    let report: PeriodicReport = serde_json::from_slice(inputs.report_bytes)
        .map_err(|e| RenderError::Malformed(e.to_string()))?;
    let recomputed =
        compute_content_hash(&report).map_err(|e| RenderError::Malformed(e.to_string()))?;
    if recomputed != report.integrity.content_hash {
        return Err(RenderError::Refused(RefusalReason::ContentHashMismatch));
    }
    let Some(attestation) = inputs.attestation else {
        return Ok((report, None));
    };
    let statement = parse_statement(attestation)?;
    let binding = check_attestation_binding(&report, inputs.report_bytes, &statement)
        .map_err(|e| RenderError::Malformed(e.to_string()))?;
    if matches!(binding, AttestationBinding::Mismatch(_)) {
        return Err(RenderError::Refused(RefusalReason::BindingMismatch));
    }
    Ok((report, Some(binding)))
}

/// The first statement of an in-toto `.jsonl`.
fn parse_statement(attestation: &[u8]) -> Result<InTotoStatement, RenderError> {
    let line = attestation
        .split(|b| *b == b'\n')
        .find(|line| !line.iter().all(u8::is_ascii_whitespace))
        .ok_or_else(|| RenderError::Malformed("empty attestation".to_string()))?;
    serde_json::from_slice(line).map_err(|e| RenderError::Malformed(format!("attestation: {e}")))
}

pub(crate) struct Fonts {
    regular: Font,
    bold: Font,
    mono: Font,
}

impl Fonts {
    pub(crate) fn load() -> Self {
        let load = |face: Face| Font::new(face.bytes().into(), 0).expect("bundled font parses");
        Self {
            regular: load(Face::Regular),
            bold: load(Face::Bold),
            mono: load(Face::Mono),
        }
    }

    fn get(&self, face: Face) -> Font {
        match face {
            Face::Regular => self.regular.clone(),
            Face::Bold => self.bold.clone(),
            Face::Mono => self.mono.clone(),
        }
    }
}

fn build_document(
    report: &PeriodicReport,
    inputs: &PdfInputs<'_>,
    sheets: &[Vec<Op>],
) -> Result<Vec<u8>, RenderError> {
    let mut document = archival_document();
    let date = pdf_date(report.report_metadata.generated_at);
    document.set_metadata(
        Metadata::new()
            .title(format!(
                "{} energy and carbon disclosure",
                report.organisation.name
            ))
            .language("en".to_string())
            .creator(format!("perf-sentinel {}", env!("CARGO_PKG_VERSION")))
            .creation_date(date)
            .document_id(report.integrity.content_hash.clone()),
    );
    embed(
        &mut document,
        "report.json",
        "application/json",
        AssociationKind::Source,
        inputs.report_bytes,
        date,
        "The periodic report this document presents",
        true,
    );
    if let Some(attestation) = inputs.attestation {
        embed(
            &mut document,
            "attestation.intoto.jsonl",
            "application/jsonl",
            AssociationKind::Supplement,
            attestation,
            date,
            "The in-toto attestation naming the report's content hash",
            true,
        );
    }
    if let Some(bundle) = inputs.bundle {
        embed(
            &mut document,
            "bundle.sig",
            "application/json",
            AssociationKind::Supplement,
            bundle,
            date,
            "The Sigstore bundle signing the attestation",
            true,
        );
    }
    add_pages(&mut document, sheets, PAGE_WIDTH, PAGE_HEIGHT);
    document
        .finish()
        .map_err(|e| RenderError::ExportFailed(format!("{e:?}")))
}

/// An empty document that krilla validates as PDF/A-3b.
pub(crate) fn archival_document() -> Document {
    let configuration = ConfigurationBuilder::new()
        .with_archival_validator(Archival::A3_B)
        .finish()
        .expect("PDF/A-3b is a valid configuration");
    Document::new_with(SerializeSettings {
        configuration,
        ..SerializeSettings::default()
    })
}

/// One `width` x `height` page per sheet, its operations drawn in order.
pub(crate) fn add_pages(document: &mut Document, sheets: &[Vec<Op>], width: f32, height: f32) {
    let fonts = Fonts::load();
    for sheet in sheets {
        let mut page = document
            .start_page_with(PageSettings::from_wh(width, height).expect("a valid page size"));
        let mut surface = page.surface();
        for op in sheet {
            draw(&mut surface, &fonts, op);
        }
        surface.finish();
        page.finish();
    }
}

pub(crate) fn draw(surface: &mut Surface<'_>, fonts: &Fonts, op: &Op) {
    match op {
        Op::Text {
            x,
            y,
            face,
            size,
            color,
            text,
        } => {
            surface.set_stroke(None);
            fill(surface, *color);
            surface.draw_text(
                Point::from_xy(*x, *y),
                fonts.get(*face),
                *size,
                text,
                false,
                TextDirection::Auto,
            );
        }
        Op::Box {
            x,
            y,
            w,
            h,
            radius,
            fill: color,
            outline,
        } => draw_box(surface, (*x, *y, *w, *h), *radius, *color, *outline),
        Op::Circle {
            cx,
            cy,
            r,
            fill: color,
        } => {
            surface.set_stroke(None);
            fill(surface, *color);
            if let Some(path) = rounded_rect(cx - r, cy - r, 2.0 * r, 2.0 * r, *r) {
                surface.draw_path(&path);
            }
        }
        Op::Qr {
            x,
            y,
            side,
            modules,
        } => draw_qr(surface, *x, *y, *side, modules),
        Op::Logo { x, y, height } => draw_logo(surface, *x, *y, *height),
    }
}

fn color(rgb: Rgb) -> rgb::Color {
    rgb::Color::new(rgb.0, rgb.1, rgb.2)
}

fn fill(surface: &mut Surface<'_>, rgb: Rgb) {
    surface.set_fill(Some(Fill {
        paint: color(rgb).into(),
        ..Fill::default()
    }));
}

fn draw_box(
    surface: &mut Surface<'_>,
    (x, y, w, h): (f32, f32, f32, f32),
    radius: f32,
    fill_color: Option<Rgb>,
    outline: Option<Outline>,
) {
    let Some(path) = rounded_rect(x, y, w, h, radius) else {
        return;
    };
    surface.set_fill(None);
    if let Some(rgb) = fill_color {
        fill(surface, rgb);
    }
    surface.set_stroke(outline.map(|outline| Stroke {
        paint: color(outline.color).into(),
        width: outline.width,
        dash: outline.dashed.then(|| StrokeDash {
            array: vec![2.5, 2.0],
            offset: 0.0,
        }),
        ..Stroke::default()
    }));
    surface.draw_path(&path);
    surface.set_stroke(None);
}

/// A rectangle with its corners rounded by `radius`, cubic arcs at the
/// usual 0.5523 handle length.
fn rounded_rect(left: f32, top: f32, width: f32, height: f32, radius: f32) -> Option<Path> {
    let corner = radius.min(width / 2.0).min(height / 2.0).max(0.0);
    let mut builder = PathBuilder::new();
    if corner == 0.0 {
        builder.push_rect(Rect::from_xywh(left, top, width, height)?);
        return builder.finish();
    }
    let handle = corner * 0.552_285;
    let (right, bottom) = (left + width, top + height);
    builder.move_to(left + corner, top);
    builder.line_to(right - corner, top);
    builder.cubic_to(
        right - corner + handle,
        top,
        right,
        top + corner - handle,
        right,
        top + corner,
    );
    builder.line_to(right, bottom - corner);
    builder.cubic_to(
        right,
        bottom - corner + handle,
        right - corner + handle,
        bottom,
        right - corner,
        bottom,
    );
    builder.line_to(left + corner, bottom);
    builder.cubic_to(
        left + corner - handle,
        bottom,
        left,
        bottom - corner + handle,
        left,
        bottom - corner,
    );
    builder.line_to(left, top + corner);
    builder.cubic_to(
        left,
        top + corner - handle,
        left + corner - handle,
        top,
        left + corner,
        top,
    );
    builder.close();
    builder.finish()
}

fn draw_qr(surface: &mut Surface<'_>, x: f32, y: f32, side: f32, modules: &[Vec<bool>]) {
    surface.set_stroke(None);
    fill(surface, (0, 0, 0));
    #[allow(clippy::cast_precision_loss)] // at most 177 modules a side
    let unit = side / modules.len().max(1) as f32;
    let mut builder = PathBuilder::new();
    for (row, line) in modules.iter().enumerate() {
        for (col, dark) in line.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // at most 177 modules a side
            let cell = Rect::from_xywh(x + col as f32 * unit, y + row as f32 * unit, unit, unit);
            if let (true, Some(cell)) = (*dark, cell) {
                builder.push_rect(cell);
            }
        }
    }
    if let Some(path) = builder.finish() {
        surface.draw_path(&path);
    }
}

fn draw_logo(surface: &mut Surface<'_>, x: f32, y: f32, height: f32) {
    let logo = logo::logo();
    let [min_x, min_y, _, view_height] = logo.view_box;
    let scale = height / view_height;
    surface.set_stroke(None);
    surface.push_transform(&Transform::from_row(
        scale,
        0.0,
        0.0,
        scale,
        x - min_x * scale,
        y - min_y * scale,
    ));
    for path in &logo.paths {
        let Some(shape) = logo_path(&path.segments) else {
            continue;
        };
        surface.set_fill(Some(logo_fill(path.paint)));
        surface.draw_path(&shape);
    }
    surface.pop();
}

fn logo_path(segments: &[logo::Segment]) -> Option<Path> {
    let mut builder = PathBuilder::new();
    for segment in segments {
        match *segment {
            logo::Segment::Move(x, y) => builder.move_to(x, y),
            logo::Segment::Line(x, y) => builder.line_to(x, y),
            logo::Segment::Cubic(x1, y1, x2, y2, x, y) => builder.cubic_to(x1, y1, x2, y2, x, y),
            logo::Segment::Close => builder.close(),
        }
    }
    builder.finish()
}

fn logo_fill(paint: logo::Paint) -> Fill {
    match paint {
        logo::Paint::Solid(rgb) => Fill {
            paint: color(rgb).into(),
            ..Fill::default()
        },
        logo::Paint::Gradient { from, to } => Fill {
            paint: LinearGradient {
                x1: from.0,
                y1: from.1,
                x2: to.0,
                y2: to.1,
                transform: Transform::default(),
                spread_method: SpreadMethod::Pad,
                stops: vec![
                    Stop {
                        offset: NormalizedF32::ZERO,
                        color: color(from.2).into(),
                        opacity: NormalizedF32::ONE,
                    },
                    Stop {
                        offset: NormalizedF32::ONE,
                        color: color(to.2).into(),
                        opacity: NormalizedF32::ONE,
                    },
                ],
                anti_alias: true,
            }
            .into(),
            ..Fill::default()
        },
    }
}

#[allow(clippy::too_many_arguments)] // one per EmbeddedFile field the callers vary
pub(crate) fn embed(
    document: &mut Document,
    path: &str,
    mime: &str,
    association_kind: AssociationKind,
    data: &[u8],
    date: DateTime,
    description: &str,
    compress: bool,
) {
    document.embed_file(EmbeddedFile {
        path: path.to_string(),
        mime_type: MimeType::new(mime),
        description: Some(description.to_string()),
        association_kind,
        data: data.to_vec().into(),
        modification_date: Some(date),
        compress: Some(compress),
        location: None,
    });
}

/// A document's own date, never the clock, so a render is reproducible.
pub(crate) fn pdf_date(at: chrono::DateTime<chrono::Utc>) -> DateTime {
    let narrow = |v: u32| u8::try_from(v).expect("calendar fields fit in u8");
    DateTime::new(u16::try_from(at.year()).unwrap_or(9999))
        .month(narrow(at.month()))
        .day(narrow(at.day()))
        .hour(narrow(at.hour()))
        .minute(narrow(at.minute()))
        .second(narrow(at.second()))
        .utc_offset_hour(0)
        .utc_offset_minute(0)
}

#[cfg(test)]
pub(crate) mod tests;
