//! `perf-sentinel disclose-pdf`: the exit codes and the JSON line are a
//! contract the Hub reads, never its stderr.

use std::io::Read as _;
use std::path::Path;

use sentinel_core::report::periodic::pdf::{PdfInputs, RenderError, render_disclosure_pdf};
use serde::Serialize;

use crate::limits::MAX_LOCAL_REPORT_BYTES;

/// A statement or a bundle is a few KB.
const MAX_SIDECAR_BYTES: u64 = 1024 * 1024;

pub struct Args<'a> {
    pub report: &'a Path,
    pub attestation: Option<&'a Path>,
    pub bundle: Option<&'a Path>,
    pub expected_identity: Option<&'a str>,
    pub expected_issuer: Option<&'a str>,
    pub output: &'a Path,
}

#[derive(Serialize)]
struct Outcome {
    result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

/// Print the JSON line and return its exit code.
fn finish(result: &'static str, reason: Option<&'static str>) -> i32 {
    println!(
        "{}",
        serde_json::to_string(&Outcome { result, reason }).expect("plain struct serialises")
    );
    match result {
        "rendered" => 0,
        "refused" => 1,
        "unrenderable" => 5,
        _ => 3,
    }
}

pub fn cmd_disclose_pdf(args: &Args<'_>) -> i32 {
    let report = match read(args.report, MAX_LOCAL_REPORT_BYTES) {
        Ok(bytes) => bytes,
        Err(reason) => return finish("input_error", Some(reason)),
    };
    let attestation = match args
        .attestation
        .map(|path| read(path, MAX_SIDECAR_BYTES))
        .transpose()
    {
        Ok(bytes) => bytes,
        Err(reason) => return finish("input_error", Some(reason)),
    };
    let bundle = match args
        .bundle
        .map(|path| read(path, MAX_SIDECAR_BYTES))
        .transpose()
    {
        Ok(bytes) => bytes,
        Err(reason) => return finish("input_error", Some(reason)),
    };
    let inputs = PdfInputs {
        report_bytes: &report,
        attestation: attestation.as_deref(),
        bundle: bundle.as_deref(),
        expected_identity: args.expected_identity,
        expected_issuer: args.expected_issuer,
    };
    match render_disclosure_pdf(&inputs) {
        Ok(pdf) => match crate::write_file_no_follow(args.output, &pdf) {
            Ok(()) => finish("rendered", None),
            Err(e) => {
                eprintln!("Error: write {}: {e}", args.output.display());
                finish("input_error", Some("output_unwritable"))
            }
        },
        Err(e) => refuse(&e),
    }
}

fn refuse(error: &RenderError) -> i32 {
    eprintln!("Error: {error}");
    finish(error.result(), Some(error.reason()))
}

/// Read through one handle, never past `cap`: a file swapped after a size
/// check cannot be loaded past it.
fn read(path: &Path, cap: u64) -> Result<Vec<u8>, &'static str> {
    let file = std::fs::File::open(path).map_err(|e| {
        eprintln!("Error: read {}: {e}", path.display());
        "missing_file"
    })?;
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes).map_err(|e| {
        eprintln!("Error: read {}: {e}", path.display());
        "missing_file"
    })?;
    if bytes.len() as u64 > cap {
        eprintln!("Error: {} exceeds the {cap}-byte cap", path.display());
        return Err("file_too_large");
    }
    Ok(bytes)
}
