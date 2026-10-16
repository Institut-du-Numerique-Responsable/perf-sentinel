//! `perf-sentinel disclose-pdf`: the exit codes and the JSON line are a
//! contract the Hub reads, never its stderr.

use std::path::Path;

use sentinel_core::report::periodic::pdf::{PdfInputs, RenderError, render_disclosure_pdf};
use serde::Serialize;

use crate::limits::{CappedRead, MAX_LOCAL_REPORT_BYTES, read_capped};

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

/// A capped read through one handle, its failure named in the contract's terms.
fn read(path: &Path, cap: u64) -> Result<Vec<u8>, &'static str> {
    match read_capped(path, cap) {
        Ok(CappedRead::Bytes(bytes)) => Ok(bytes),
        Ok(CappedRead::TooLarge(len)) => {
            eprintln!(
                "Error: {} is {len} bytes, exceeds the {cap}-byte cap",
                path.display()
            );
            Err("file_too_large")
        }
        Err(e) => {
            eprintln!("Error: read {}: {e}", path.display());
            Err("missing_file")
        }
    }
}
