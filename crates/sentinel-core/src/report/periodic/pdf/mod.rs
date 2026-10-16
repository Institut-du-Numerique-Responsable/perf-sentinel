//! A periodic disclosure as a PDF/A-3b a non-technical reader can follow,
//! carrying the report, its attestation and its bundle as attachments.
//! A rendered PDF stands for two checks: the content hash recomputes, and
//! the attestation, when given, names it.

use chrono::{Datelike, Timelike};
use krilla::configure::{Archival, ConfigurationBuilder};
use krilla::embed::{AssociationKind, EmbeddedFile, MimeType};
use krilla::geom::Point;
use krilla::metadata::{DateTime, Metadata};
use krilla::page::PageSettings;
use krilla::text::{Font, TextDirection};
use krilla::{Document, SerializeSettings};

use super::version::{SchemaVersionError, check_schema_version};
use super::{
    AttestationBinding, InTotoStatement, PeriodicReport, check_attestation_binding,
    compute_content_hash,
};

const DM_SANS_REGULAR: &[u8] = include_bytes!("fonts/DMSans-Regular.ttf");
const DM_SANS_BOLD: &[u8] = include_bytes!("fonts/DMSans-Bold.ttf");
const JETBRAINS_MONO: &[u8] = include_bytes!("fonts/JetBrainsMono-Regular.ttf");

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
        }
    }

    /// The result of the exit contract: `refused`, `input_error` or `unrenderable`.
    #[must_use]
    pub fn result(&self) -> &'static str {
        match self {
            Self::Refused(_) => "refused",
            Self::Malformed(_) => "input_error",
            Self::UnsupportedSchema(_) | Self::UnsupportedCharacters(_) => "unrenderable",
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
    let report = checked_report(inputs)?;
    build_document(&report, inputs)
}

/// The report, once its schema, hash and binding are known good.
fn checked_report(inputs: &PdfInputs<'_>) -> Result<PeriodicReport, RenderError> {
    match check_schema_version(inputs.report_bytes) {
        Ok(()) => {}
        Err(SchemaVersionError::Unsupported(version)) => {
            return Err(RenderError::UnsupportedSchema(version));
        }
        Err(SchemaVersionError::Missing) => {
            return Err(RenderError::Malformed("no schema_version".to_string()));
        }
    }
    let report: PeriodicReport = serde_json::from_slice(inputs.report_bytes)
        .map_err(|e| RenderError::Malformed(e.to_string()))?;
    let recomputed =
        compute_content_hash(&report).map_err(|e| RenderError::Malformed(e.to_string()))?;
    if recomputed != report.integrity.content_hash {
        return Err(RenderError::Refused(RefusalReason::ContentHashMismatch));
    }
    if let Some(attestation) = inputs.attestation {
        let statement = parse_statement(attestation)?;
        let binding = check_attestation_binding(&report, inputs.report_bytes, &statement)
            .map_err(|e| RenderError::Malformed(e.to_string()))?;
        if matches!(binding, AttestationBinding::Mismatch(_)) {
            return Err(RenderError::Refused(RefusalReason::BindingMismatch));
        }
    }
    Ok(report)
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
    pub(crate) regular: Font,
    pub(crate) bold: Font,
    pub(crate) mono: Font,
}

impl Fonts {
    fn load() -> Self {
        let load = |bytes: &'static [u8]| Font::new(bytes.into(), 0).expect("bundled font parses");
        Self {
            regular: load(DM_SANS_REGULAR),
            bold: load(DM_SANS_BOLD),
            mono: load(JETBRAINS_MONO),
        }
    }
}

fn build_document(report: &PeriodicReport, inputs: &PdfInputs<'_>) -> Result<Vec<u8>, RenderError> {
    let configuration = ConfigurationBuilder::new()
        .with_archival_validator(Archival::A3_B)
        .finish()
        .expect("PDF/A-3b is a valid configuration");
    let settings = SerializeSettings {
        configuration,
        ..SerializeSettings::default()
    };
    let mut document = Document::new_with(settings);
    let date = pdf_date(report);
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
        );
    }
    let fonts = Fonts::load();
    let mut page = document.start_page_with(
        PageSettings::from_wh(PAGE_WIDTH, PAGE_HEIGHT).expect("A4 is a valid page"),
    );
    let mut surface = page.surface();
    surface.draw_text(
        Point::from_xy(56.0, 56.0),
        fonts.bold.clone(),
        18.0,
        "Energy and carbon disclosure",
        false,
        TextDirection::Auto,
    );
    surface.draw_text(
        Point::from_xy(56.0, 68.0),
        fonts.regular.clone(),
        11.0,
        &report.organisation.name,
        false,
        TextDirection::Auto,
    );
    surface.draw_text(
        Point::from_xy(56.0, 80.0),
        fonts.mono.clone(),
        9.0,
        &report.integrity.content_hash,
        false,
        TextDirection::Auto,
    );
    surface.finish();
    page.finish();
    document
        .finish()
        .map_err(|e| RenderError::Malformed(format!("PDF export: {e:?}")))
}

fn embed(
    document: &mut Document,
    path: &str,
    mime: &str,
    association_kind: AssociationKind,
    data: &[u8],
    date: DateTime,
    description: &str,
) {
    document.embed_file(EmbeddedFile {
        path: path.to_string(),
        mime_type: MimeType::new(mime),
        description: Some(description.to_string()),
        association_kind,
        data: data.to_vec().into(),
        modification_date: Some(date),
        compress: Some(true),
        location: None,
    });
}

/// The report's own date, never the clock, so a render is reproducible.
fn pdf_date(report: &PeriodicReport) -> DateTime {
    let at = report.report_metadata.generated_at;
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
