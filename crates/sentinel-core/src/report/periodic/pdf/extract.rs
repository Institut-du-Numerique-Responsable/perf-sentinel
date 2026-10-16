//! The report, attestation and bundle a disclosure PDF carries, read back
//! from any well-formed PDF, a copy a viewer re-saved included. The PDF is
//! untrusted: every stream is decoded under a cap and the walks are bounded.

use std::collections::BTreeSet;

use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId};

/// The three files a disclosure PDF attaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfAttachments {
    pub report: Vec<u8>,
    pub attestation: Option<Vec<u8>>,
    pub bundle: Option<Vec<u8>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("not a PDF")]
    NotAPdf,
    #[error("the PDF carries no report.json")]
    NoReport,
    #[error("the PDF carries {0} twice")]
    Duplicate(&'static str),
    #[error("{0} is larger than this command reads")]
    TooLarge(&'static str),
    #[error("unreadable PDF: {0}")]
    Malformed(String),
}

const REPORT: &str = "report.json";
const ATTESTATION: &str = "attestation.intoto.jsonl";
const BUNDLE: &str = "bundle.sig";

/// How deep a name tree may nest, and how many file specifications a PDF
/// may name, before it is refused as hostile.
const MAX_NAME_TREE_DEPTH: usize = 8;
const MAX_FILE_SPECS: usize = 256;

/// Read the attachments of `pdf`, `report_cap` and `sidecar_cap` bytes at
/// most once decoded.
///
/// # Errors
///
/// See [`ExtractError`].
pub fn extract_attachments(
    pdf: &[u8],
    report_cap: u64,
    sidecar_cap: u64,
) -> Result<PdfAttachments, ExtractError> {
    if !pdf.starts_with(b"%PDF-") {
        return Err(ExtractError::NotAPdf);
    }
    let report_cap = usize::try_from(report_cap).unwrap_or(usize::MAX);
    let sidecar_cap = usize::try_from(sidecar_cap).unwrap_or(usize::MAX);
    let options = LoadOptions {
        max_decompressed_size: Some(report_cap.max(sidecar_cap).saturating_add(1)),
        ..LoadOptions::default()
    };
    let doc = Document::load_mem_with_options(pdf, options)
        .map_err(|e| ExtractError::Malformed(e.to_string()))?;
    let mut found: [Option<Vec<u8>>; 3] = [None, None, None];
    for spec in file_specs(&doc)? {
        let Some(name) = spec_name(&doc, spec) else {
            continue;
        };
        let Some(slot) = [REPORT, ATTESTATION, BUNDLE]
            .iter()
            .position(|n| *n == name)
        else {
            continue;
        };
        let label = [REPORT, ATTESTATION, BUNDLE][slot];
        if found[slot].is_some() {
            return Err(ExtractError::Duplicate(label));
        }
        let cap = if slot == 0 { report_cap } else { sidecar_cap };
        found[slot] = Some(embedded_bytes(&doc, spec, label, cap)?);
    }
    let [report, attestation, bundle] = found;
    Ok(PdfAttachments {
        report: report.ok_or(ExtractError::NoReport)?,
        attestation,
        bundle,
    })
}

/// Every file specification the catalog names, through its `AF` array and
/// its `EmbeddedFiles` name tree, each once.
fn file_specs(doc: &Document) -> Result<Vec<ObjectId>, ExtractError> {
    let catalog = doc
        .catalog()
        .map_err(|e| ExtractError::Malformed(e.to_string()))?;
    let mut specs = BTreeSet::new();
    if let Ok(af) = catalog.get(b"AF").and_then(|o| resolve(doc, o)?.as_array()) {
        specs.extend(af.iter().filter_map(|o| o.as_reference().ok()));
    }
    let tree = catalog
        .get(b"Names")
        .and_then(|o| resolve(doc, o)?.as_dict())
        .and_then(|names| names.get(b"EmbeddedFiles"))
        .and_then(|o| resolve(doc, o)?.as_dict());
    if let Ok(tree) = tree {
        name_tree(doc, tree, 0, &mut specs)?;
    }
    if specs.len() > MAX_FILE_SPECS {
        return Err(ExtractError::Malformed(
            "too many attached files".to_string(),
        ));
    }
    Ok(specs.into_iter().collect())
}

/// The leaves of a name tree, its `Names` pairs and its `Kids`.
fn name_tree(
    doc: &Document,
    node: &Dictionary,
    depth: usize,
    specs: &mut BTreeSet<ObjectId>,
) -> Result<(), ExtractError> {
    if depth > MAX_NAME_TREE_DEPTH || specs.len() > MAX_FILE_SPECS {
        return Err(ExtractError::Malformed(
            "name tree too deep or too large".to_string(),
        ));
    }
    if let Ok(names) = node.get(b"Names").and_then(|o| resolve(doc, o)?.as_array()) {
        specs.extend(
            names
                .iter()
                .skip(1)
                .step_by(2)
                .filter_map(|o| o.as_reference().ok()),
        );
    }
    if let Ok(kids) = node.get(b"Kids").and_then(|o| resolve(doc, o)?.as_array()) {
        for kid in kids {
            if let Ok(kid) = resolve(doc, kid).and_then(Object::as_dict) {
                name_tree(doc, kid, depth + 1, specs)?;
            }
        }
    }
    Ok(())
}

fn resolve<'a>(doc: &'a Document, object: &'a Object) -> lopdf::Result<&'a Object> {
    match object {
        Object::Reference(id) => doc.get_object(*id),
        other => Ok(other),
    }
}

/// The file name of a specification, `UF` before `F`.
fn spec_name(doc: &Document, spec: ObjectId) -> Option<String> {
    let dict = doc.get_object(spec).ok()?.as_dict().ok()?;
    [b"UF".as_slice(), b"F".as_slice()]
        .iter()
        .find_map(|key| dict.get(key).ok()?.as_str().ok().map(text_string))
}

/// A PDF text string: UTF-16BE behind its byte order mark, otherwise bytes
/// read as Latin-1, which leaves every ASCII name intact.
fn text_string(bytes: &[u8]) -> String {
    if let Some(utf16) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = utf16
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes(*pair))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    bytes.iter().map(|b| char::from(*b)).collect()
}

/// The decoded bytes of the file a specification embeds.
fn embedded_bytes(
    doc: &Document,
    spec: ObjectId,
    label: &'static str,
    cap: usize,
) -> Result<Vec<u8>, ExtractError> {
    let malformed = |what: &str| ExtractError::Malformed(format!("{label}: {what}"));
    let dict = doc
        .get_object(spec)
        .and_then(Object::as_dict)
        .map_err(|_| malformed("not a file specification"))?;
    let ef = dict
        .get(b"EF")
        .and_then(|o| resolve(doc, o)?.as_dict())
        .map_err(|_| malformed("no embedded file"))?;
    let stream = [b"UF".as_slice(), b"F".as_slice()]
        .iter()
        .find_map(|key| ef.get(key).ok().and_then(|o| resolve(doc, o).ok()))
        .and_then(|o| o.as_stream().ok())
        .ok_or_else(|| malformed("no embedded stream"))?;
    if stream.dict.get(b"Filter").is_err() {
        if stream.content.len() > cap {
            return Err(ExtractError::TooLarge(label));
        }
        return Ok(stream.content.clone());
    }
    stream
        .decompressed_content_with_limit(cap)
        .map_err(|e| match e {
            lopdf::Error::Decompress(lopdf::DecompressError::MemoryLimitExceeded { .. }) => {
                ExtractError::TooLarge(label)
            }
            other => malformed(&other.to_string()),
        })
}
