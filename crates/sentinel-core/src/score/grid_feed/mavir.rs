//! MAVIR (Hungary): ratio of CO2 emitted by domestic power plants to gross
//! domestic generation, per quarter hour, from the export of chart 1000687
//! on the public real-time data warehouse. No key. The export only exists
//! as XLSX, read here with a minimal ZIP reader.

use std::io::Read;

use chrono::{DateTime, TimeDelta, Utc};

use super::scraper::{FeedError, check_value};

const QUARTER_SECS: i64 = 900;
/// Words that pick, among the four ratio columns, the measured one ("tény",
/// not "becsült") without household and self-consumption solar ("nélkül",
/// not "-vel"), which MAVIR fills 25 to 40 minutes late. Its header reads
/// "Termelők által kiadott üvegház hatású gázok aránya a hazai bruttó
/// termelésben - tény - HMKE és SCTE nélkül". Matching words rather than the
/// whole label survives a rewording, and two matches fail rather than guess.
const ACTUAL: [&str; 3] = ["aránya", "tény", "nélkül"];
const TIME: &str = "Időpont";
/// Pins the Hungarian headers and timestamps the parser expects.
pub(super) const COOKIE: &str = "PLAY_LANG=hu";
/// The unit, tCO2 per `MWh`, is printed nowhere: a ratio above any fossil
/// plant's, or below what a grid with gas plants online reaches, means it
/// changed (g/kWh reads 1000 times higher, t/kWh 1000 times lower).
const RATIO_RANGE: std::ops::RangeInclusive<f64> = 0.01..=1.5;
const WORKSHEET: &[u8] = b"xl/worksheets/sheet1.xml";
/// The sheet holds a few rows: this only bounds a malformed archive.
const MAX_WORKSHEET_BYTES: u64 = 1024 * 1024;

/// The last hour on the quarter grid, or every label shifts. Cells are
/// filled up to the label `toTime`, so a quarter ahead takes in the one in
/// progress.
pub(super) fn url(endpoint: &str, now: DateTime<Utc>) -> String {
    let quarter = now.timestamp().div_euclid(QUARTER_SECS) * QUARTER_SECS;
    let from = (quarter - 3600) * 1000;
    let to = (quarter + QUARTER_SECS) * 1000;
    format!(
        "{endpoint}/chart/1000687/export?exportType=xlsx&fromTime={from}&toTime={to}\
         &periodType=min&period=15"
    )
}

/// The newest quarter with a measured ratio, dated by its start: labels
/// mark the end. The quarter in progress is included and revised by a few
/// g/kWh once it closes. tCO2/MWh times 1000 gives gCO2/kWh.
pub(super) fn parse(body: &[u8]) -> Result<Vec<(String, f64, DateTime<Utc>)>, FeedError> {
    let xml = worksheet(body).ok_or_else(|| {
        FeedError::Parse("not an XLSX export with a readable worksheet".to_string())
    })?;
    let rows = rows(&xml);
    let header = rows.first().ok_or(FeedError::Empty)?;
    let column = |words: &[&str]| {
        let mut found = header
            .iter()
            .filter(|(_, text)| words.iter().all(|w| text.contains(w)))
            .map(|(column, _)| *column);
        match (found.next(), found.next()) {
            (Some(column), None) => Ok(column),
            _ => Err(FeedError::Parse(format!("no single column with {words:?}"))),
        }
    };
    let (time, actual) = (column(&[TIME])?, column(&ACTUAL)?);
    let mut latest: Option<(f64, DateTime<Utc>)> = None;
    for row in rows.iter().skip(1) {
        let cell = |wanted: &str| {
            row.iter()
                .find(|(c, _)| *c == wanted)
                .map(|(_, t)| t.trim())
        };
        let (Some(label), Some(raw)) = (cell(time), cell(actual)) else {
            continue;
        };
        let end = DateTime::parse_from_str(label, "%Y.%m.%d %H:%M:%S %z")
            .map_err(|e| FeedError::Parse(format!("{TIME}: {e}")))?;
        let at = end.with_timezone(&Utc) - TimeDelta::seconds(QUARTER_SECS);
        let ratio: f64 = raw
            .parse()
            .map_err(|e| FeedError::Parse(format!("ratio {raw:?}: {e}")))?;
        if latest.is_none_or(|(_, seen)| at > seen) {
            latest = Some((ratio, at));
        }
    }
    let (ratio, at) = latest.ok_or(FeedError::Empty)?;
    if !RATIO_RANGE.contains(&ratio) {
        return Err(FeedError::InvalidValue);
    }
    Ok(vec![(
        "HU".to_string(),
        check_value(Some(ratio * 1000.0))?,
        at,
    )])
}

/// Cells of each row as (column letters, text). The export writes inline
/// strings and numbers, never shared strings.
fn rows(xml: &str) -> Vec<Vec<(&str, &str)>> {
    xml.split("<row")
        .skip(1)
        .map(|row| {
            let row = row.split_once("</row>").map_or(row, |(row, _)| row);
            row.split("<c ").skip(1).filter_map(cell).collect()
        })
        .collect()
}

fn cell(raw: &str) -> Option<(&str, &str)> {
    let (attributes, content) = raw.split_once('>')?;
    if attributes.ends_with('/') {
        return None;
    }
    let reference = attributes
        .split_whitespace()
        .find_map(|a| a.strip_prefix("r=\""))?
        .split('"')
        .next()?;
    let column = reference.trim_end_matches(|c: char| c.is_ascii_digit());
    let text =
        element_text(content, "<v", "</v>").or_else(|| element_text(content, "<t", "</t>"))?;
    Some((column, text))
}

fn element_text<'a>(content: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let (_, opened) = content.split_once(open)?;
    let (_, text) = opened.split_once('>')?;
    text.split_once(close).map(|(text, _)| text)
}

/// Little-endian integer of `len` bytes at `at`.
fn le(bytes: &[u8], at: usize, len: usize) -> Option<usize> {
    let raw = bytes.get(at..at.checked_add(len)?)?;
    usize::try_from(
        raw.iter()
            .rev()
            .fold(0u64, |acc, b| acc << 8 | u64::from(*b)),
    )
    .ok()
}

/// The worksheet of the archive, inflated. Java writes each entry's sizes
/// after its data, so they are read from the central directory.
fn worksheet(xlsx: &[u8]) -> Option<String> {
    let end = xlsx.windows(4).rposition(|w| w == b"PK\x05\x06")?;
    let entries = le(xlsx, end + 10, 2)?;
    let mut at = le(xlsx, end + 16, 4)?;
    for _ in 0..entries {
        if xlsx.get(at..at + 4)? != b"PK\x01\x02" {
            return None;
        }
        let name_len = le(xlsx, at + 28, 2)?;
        if xlsx.get(at + 46..at + 46 + name_len)? == WORKSHEET {
            let method = le(xlsx, at + 10, 2)?;
            let size = le(xlsx, at + 20, 4)?;
            let local = le(xlsx, at + 42, 4)?;
            if xlsx.get(local..local + 4)? != b"PK\x03\x04" {
                return None;
            }
            let start = local + 30 + le(xlsx, local + 26, 2)? + le(xlsx, local + 28, 2)?;
            let data = xlsx.get(start..start.checked_add(size)?)?;
            let mut xml = String::new();
            match method {
                0 => xml.push_str(std::str::from_utf8(data).ok()?),
                8 => {
                    flate2::read::DeflateDecoder::new(data)
                        .take(MAX_WORKSHEET_BYTES)
                        .read_to_string(&mut xml)
                        .ok()?;
                }
                _ => return None,
            }
            return Some(xml);
        }
        at += 46 + name_len + le(xlsx, at + 30, 2)? + le(xlsx, at + 32, 2)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A one-sheet archive laid out like MAVIR's: sizes zeroed in the local
    /// header and given in a data descriptor and the central directory.
    fn xlsx(sheet: &str) -> Vec<u8> {
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(sheet.as_bytes()).unwrap();
        let data = encoder.finish().unwrap();
        let n = |v: usize, len: usize| v.to_le_bytes()[..len].to_vec();
        let mut out = b"PK\x03\x04".to_vec();
        for (v, len) in [(45, 2), (8, 2), (8, 2), (0, 4), (0, 4), (0, 4), (0, 4)] {
            out.extend(n(v, len));
        }
        out.extend(n(WORKSHEET.len(), 2));
        out.extend(n(0, 2));
        out.extend(WORKSHEET);
        out.extend(&data);
        out.extend(b"PK\x07\x08\0\0\0\0");
        out.extend(n(data.len(), 4));
        out.extend(n(sheet.len(), 4));
        let directory = out.len();
        out.extend(b"PK\x01\x02");
        for (v, len) in [(45, 2), (45, 2), (8, 2), (8, 2), (0, 4), (0, 4)] {
            out.extend(n(v, len));
        }
        out.extend(n(data.len(), 4));
        out.extend(n(sheet.len(), 4));
        out.extend(n(WORKSHEET.len(), 2));
        for (v, len) in [(0, 2), (0, 2), (0, 2), (0, 2), (0, 4), (0, 4)] {
            out.extend(n(v, len));
        }
        out.extend(WORKSHEET);
        let directory_len = out.len() - directory;
        out.extend(b"PK\x05\x06\0\0\0\0");
        out.extend(n(1, 2));
        out.extend(n(1, 2));
        out.extend(n(directory_len, 4));
        out.extend(n(directory, 4));
        out.extend(n(0, 2));
        out
    }

    /// The measured column's header in the live export.
    const LABEL: &str = "Termelők által kiadott üvegház hatású gázok aránya a hazai bruttó termelésben - tény - HMKE és SCTE nélkül";

    fn header() -> String {
        let estimated = LABEL.replace("tény", "becsült");
        let with_solar = LABEL.replace("nélkül", "-vel");
        format!(
            r#"<row r="1"><c r="A1" s="1" t="inlineStr"><is><t>Időpont</t></is></c><c r="B1" s="1" t="inlineStr"><is><t>{estimated}</t></is></c><c r="D1" s="1" t="inlineStr"><is><t>{LABEL}</t></is></c><c r="E1" s="1" t="inlineStr"><is><t>{with_solar}</t></is></c></row>"#
        )
    }

    /// Rows of the live export of 2026-10-20 13:29 UTC: the last label is
    /// past `toTime` and empty.
    fn sheet(header: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><worksheet><sheetData>
{header}
<row r="5"><c r="A5" s="2" t="inlineStr"><is><t>2026.10.20 15:15:00 +0200</t></is></c><c r="B5" s="3" t="n"><v>0.109</v></c><c r="D5" s="3" t="n"><v>0.076</v></c></row>
<row r="6"><c r="A6" s="2" t="inlineStr"><is><t>2026.10.20 15:30:00 +0200</t></is></c><c r="B6" s="3" t="n"><v>0.112</v></c><c r="D6" s="3" t="n"><v>0.078</v></c></row>
<row r="7"><c r="A7" s="2" t="inlineStr"><is><t>2026.10.20 15:45:00 +0200</t></is></c><c r="B7" s="3" t="n"><v>0.141</v></c></row>
</sheetData></worksheet>"#
        )
    }

    #[test]
    fn url_stays_on_the_quarter_grid() {
        // 2026-10-20 13:29:37 UTC.
        let now = DateTime::from_timestamp(1_792_502_977, 0).unwrap();
        let url = url("https://rtdwweb.mavir.hu/rtdwweb/webuser", now);
        assert_eq!(
            url,
            "https://rtdwweb.mavir.hu/rtdwweb/webuser/chart/1000687/export?exportType=xlsx\
             &fromTime=1792498500000&toTime=1792503000000&periodType=min&period=15"
        );
        assert!(url.parse::<hyper::Uri>().is_ok());
    }

    #[test]
    fn takes_the_newest_measured_quarter_dated_by_its_start() {
        let zones = parse(&xlsx(&sheet(&header()))).unwrap();
        assert_eq!(zones[0].0, "HU");
        assert!((zones[0].1 - 78.0).abs() < 1e-9);
        assert_eq!(zones[0].2.to_rfc3339(), "2026-10-20T13:15:00+00:00");
    }

    #[test]
    fn a_reworded_header_is_still_found_but_two_matches_fail() {
        let reworded = header().replace("Termelők által kiadott", "Erőművek");
        let zones = parse(&xlsx(&sheet(&reworded))).unwrap();
        assert!((zones[0].1 - 78.0).abs() < 1e-9);
        let twice = header().replace("becsült", "tény");
        let err = parse(&xlsx(&sheet(&twice))).unwrap_err();
        assert!(
            matches!(err, FeedError::Parse(ref m) if m.contains("no single column")),
            "{err:?}"
        );
    }

    #[test]
    fn english_headers_are_a_parse_error() {
        let english = header().replace(LABEL, "Measured ratio of greenhouse gas emission");
        let err = parse(&xlsx(&sheet(&english))).unwrap_err();
        assert!(
            matches!(err, FeedError::Parse(ref m) if m.contains("tény")),
            "{err:?}"
        );
    }

    #[test]
    fn a_ratio_out_of_any_grid_range_means_the_unit_changed() {
        let grams = sheet(&header()).replace("<v>0.078</v>", "<v>78</v>");
        assert!(matches!(parse(&xlsx(&grams)), Err(FeedError::InvalidValue)));
        let per_kwh = sheet(&header()).replace("<v>0.078</v>", "<v>0.000078</v>");
        assert!(matches!(
            parse(&xlsx(&per_kwh)),
            Err(FeedError::InvalidValue)
        ));
    }

    #[test]
    fn no_measured_quarter_is_empty() {
        let sheet = format!("<worksheet><sheetData>{}</sheetData></worksheet>", header());
        assert!(matches!(parse(&xlsx(&sheet)), Err(FeedError::Empty)));
    }

    #[test]
    fn a_broken_archive_is_a_parse_error_not_a_panic() {
        let archive = xlsx(&sheet(&header()));
        for cut in [0, 4, 30, archive.len() / 2, archive.len() - 10] {
            assert!(
                matches!(parse(&archive[..cut]), Err(FeedError::Parse(_))),
                "cut {cut}"
            );
        }
        assert!(matches!(
            parse(b"<html>Oops, an error occurred</html>"),
            Err(FeedError::Parse(_))
        ));
    }
}
