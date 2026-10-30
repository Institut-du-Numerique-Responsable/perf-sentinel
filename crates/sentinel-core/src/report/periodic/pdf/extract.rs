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
    #[error(
        "the PDF carries no report.json: a viewer that saved it again may have dropped its attachments, use the original file"
    )]
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
/// Name tree nodes walked at most, each once.
const MAX_NAME_TREE_NODES: usize = 1024;
/// Object and cross-reference streams decoded while loading, each under
/// `LOAD_STREAM_CAP`: lopdf bounds one stream, these bound them all.
const MAX_LOAD_STREAMS: usize = 32;
const LOAD_STREAM_CAP: usize = 4 * 1024 * 1024;
/// The largest PDF read: lopdf holds every object in memory, about sixty
/// times the input. A render is a few tens of KB, attachments compressed.
pub const MAX_PDF_BYTES: usize = 4 * 1024 * 1024;
/// What ends a name, as ISO 32000 defines it: NUL is whitespace too.
const PDF_WHITESPACE: &[u8] = b" \t\n\r\x0C\0";
const PDF_DELIMITERS: &[u8] = b"/<>[]()%{}";

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
    let report_cap = usize::try_from(report_cap).unwrap_or(usize::MAX);
    let sidecar_cap = usize::try_from(sidecar_cap).unwrap_or(usize::MAX);
    let doc = load(pdf)?;
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

/// Read the one attachment of `pdf` named `name`, `cap` bytes at most once
/// decoded, `None` when the PDF attaches no file of that name.
///
/// # Errors
///
/// See [`ExtractError`]: `Duplicate(name)` when two files carry the name.
pub fn extract_named(
    pdf: &[u8],
    name: &'static str,
    cap: u64,
) -> Result<Option<Vec<u8>>, ExtractError> {
    let cap = usize::try_from(cap).unwrap_or(usize::MAX);
    let doc = load(pdf)?;
    let mut found = None;
    for spec in file_specs(&doc)? {
        if spec_name(&doc, spec).as_deref() != Some(name) {
            continue;
        }
        if found.is_some() {
            return Err(ExtractError::Duplicate(name));
        }
        found = Some(embedded_bytes(&doc, spec, name, cap)?);
    }
    Ok(found)
}

/// `pdf` parsed under the size and stream bounds, once it looks like a PDF.
fn load(pdf: &[u8]) -> Result<Document, ExtractError> {
    if !pdf.starts_with(b"%PDF-") {
        return Err(ExtractError::NotAPdf);
    }
    if pdf.len() > MAX_PDF_BYTES {
        return Err(ExtractError::TooLarge("the PDF"));
    }
    if load_streams(pdf) > MAX_LOAD_STREAMS {
        return Err(ExtractError::Malformed(
            "too many object streams".to_string(),
        ));
    }
    let options = LoadOptions {
        max_decompressed_size: Some(LOAD_STREAM_CAP),
        ..LoadOptions::default()
    };
    Document::load_mem_with_options(pdf, options)
        .map_err(|e| ExtractError::Malformed(e.to_string()))
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
        let mut walk = Walk {
            specs: &mut specs,
            visited: BTreeSet::new(),
        };
        walk.node(doc, tree, 0)?;
    }
    if specs.len() > MAX_FILE_SPECS {
        return Err(ExtractError::Malformed(
            "too many attached files".to_string(),
        ));
    }
    Ok(specs.into_iter().collect())
}

/// A name tree walk that visits each node once, under a global budget.
struct Walk<'a> {
    specs: &'a mut BTreeSet<ObjectId>,
    visited: BTreeSet<ObjectId>,
}

impl Walk<'_> {
    /// The leaves of a name tree, its `Names` pairs and its `Kids`.
    fn node(
        &mut self,
        doc: &Document,
        node: &Dictionary,
        depth: usize,
    ) -> Result<(), ExtractError> {
        if depth > MAX_NAME_TREE_DEPTH || self.specs.len() > MAX_FILE_SPECS {
            return Err(ExtractError::Malformed(
                "name tree too deep or too large".to_string(),
            ));
        }
        if let Ok(names) = node.get(b"Names").and_then(|o| resolve(doc, o)?.as_array()) {
            self.specs.extend(
                names
                    .iter()
                    .skip(1)
                    .step_by(2)
                    .filter_map(|o| o.as_reference().ok()),
            );
        }
        let Ok(kids) = node.get(b"Kids").and_then(|o| resolve(doc, o)?.as_array()) else {
            return Ok(());
        };
        for kid in kids {
            let Ok(id) = kid.as_reference() else {
                continue;
            };
            if !self.visited.insert(id) {
                continue;
            }
            if self.visited.len() > MAX_NAME_TREE_NODES {
                return Err(ExtractError::Malformed("name tree too large".to_string()));
            }
            if let Ok(kid) = doc.get_object(id).and_then(Object::as_dict) {
                self.node(doc, kid, depth + 1)?;
            }
        }
        Ok(())
    }
}

/// How many object and cross-reference streams the file declares, its
/// names read as the parser reads them, `#xx` escapes decoded. Neither can
/// sit inside an object stream, so the raw bytes hold them all.
fn load_streams(pdf: &[u8]) -> usize {
    let mut count = 0;
    let mut at = 0;
    while let Some(offset) = pdf[at..].iter().position(|b| *b == b'/') {
        let start = at + offset + 1;
        let end = pdf[start..]
            .iter()
            .position(|b| PDF_WHITESPACE.contains(b) || PDF_DELIMITERS.contains(b))
            .map_or(pdf.len(), |len| start + len);
        let name = decode_name(&pdf[start..end]);
        if name == b"ObjStm" || name == b"XRef" {
            count += 1;
        }
        at = end;
    }
    count
}

fn decode_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        let hex = raw
            .get(index + 1..index + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (raw[index], hex) {
            (b'#', Some(byte)) => {
                out.push(byte);
                index += 3;
            }
            (byte, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    out
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
