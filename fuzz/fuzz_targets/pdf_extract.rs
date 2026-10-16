//! Fuzz the disclosure PDF reader behind `verify-hash --report report.pdf`:
//! the PDF is untrusted input, so any panic, or any allocation past the
//! caps, is a finding.

#![no_main]

use libfuzzer_sys::fuzz_target;
use sentinel_core::report::periodic::pdf::extract_attachments;

fuzz_target!(|data: &[u8]| {
    let _ = extract_attachments(data, 1 << 20, 1 << 16);
});
