//! Operational thresholds shared by multiple CLI subcommands.

/// Hard cap on a local `--report` file read. Legitimate periodic
/// disclosures are well under 10 MB, and 64 MiB leaves room for outliers
/// (deep G1 archives, large per-service breakdowns) while bounding a
/// single `Vec<u8>` allocation against a poisoned mirror or a runaway
/// artefact feeding the binary multi-GB JSON.
pub const MAX_LOCAL_REPORT_BYTES: u64 = 64 * 1024 * 1024;

/// Hard cap on a local trace-file read for the batch subcommands
/// (`analyze`, `diff`, `report`, `explain`, `calibrate`, `bench`,
/// `pg-stat`). Local files the operator passes explicitly are a
/// different trust model from the daemon's network listeners, so this
/// is decoupled from `[daemon] max_payload_size` (whose 100 MiB
/// ceiling exists to bound unauthenticated OTLP requests). 1 GiB
/// bounds the single `Vec<u8>` allocation while letting real
/// large-fleet exports (hundreds of MB of traces) load. The whole
/// parsed dataset must fit in RAM anyway, because batch correlation
/// works on the whole set.
pub const MAX_BATCH_INPUT_BYTES: usize = 1024 * 1024 * 1024;

/// What a capped read found: the bytes, or a file over the cap and its size.
pub enum CappedRead {
    Bytes(Vec<u8>),
    TooLarge(u64),
}

/// Read `path` through one handle, so the size checked is the size of the
/// file read and never of one swapped in between. The bounded read still
/// stops a device or pipe whose metadata says 0.
pub fn read_capped(path: &std::path::Path, max: u64) -> std::io::Result<CappedRead> {
    use std::io::Read as _;
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len > max {
        return Ok(CappedRead::TooLarge(len));
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Ok(CappedRead::TooLarge(bytes.len() as u64));
    }
    Ok(CappedRead::Bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_under_the_cap_is_read_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("report.json");
        std::fs::write(&path, b"{}").expect("write");

        assert!(matches!(read_capped(&path, 2), Ok(CappedRead::Bytes(bytes)) if bytes == b"{}"));
    }

    #[test]
    fn a_file_over_the_cap_reports_its_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("report.json");
        std::fs::write(&path, b"{\"a\":1}").expect("write");

        assert!(matches!(read_capped(&path, 3), Ok(CappedRead::TooLarge(7))));
    }

    /// A device reports a length of 0: only the bounded read stops it.
    #[cfg(unix)]
    #[test]
    fn a_file_whose_metadata_lies_is_still_bounded() {
        let read = read_capped(std::path::Path::new("/dev/zero"), 16);

        assert!(matches!(read, Ok(CappedRead::TooLarge(17))));
    }
}
