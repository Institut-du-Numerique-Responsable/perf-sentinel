//! Fingrid open data (Finland): emission factor of production (dataset 266)
//! or of consumption (265), gCO2/kWh, every 3 minutes, behind a free key.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::config::FingridDataset;
use super::scraper::{FeedError, check_value, parse_rfc3339};

/// `/data/latest` answers one flat object, or 404 when the dataset is empty.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Latest {
    start_time: String,
    value: Option<f64>,
}

pub(super) fn url(endpoint: &str, dataset: FingridDataset) -> String {
    format!("{endpoint}/datasets/{}/data/latest", dataset.id())
}

pub(super) fn parse(body: &[u8]) -> Result<Vec<(String, f64, DateTime<Utc>)>, FeedError> {
    let latest: Latest =
        serde_json::from_slice(body).map_err(|e| FeedError::Parse(e.to_string()))?;
    let value = check_value(latest.value)?;
    // RFC 3339 in UTC, with milliseconds or seven fractional digits.
    let at = parse_rfc3339(&latest.start_time, "startTime")?;
    Ok(vec![("FI".to_string(), value, at)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape of `/data/latest` from the official `OpenAPI` example.
    const BODY: &str = r#"{"datasetId":266,"startTime":"2026-10-20T12:24:00.000Z","endTime":"2026-10-20T12:24:00.000Z","value":72.5}"#;

    #[test]
    fn url_reads_the_latest_value_of_the_chosen_dataset() {
        assert_eq!(
            url("https://data.fingrid.fi/api", FingridDataset::Production),
            "https://data.fingrid.fi/api/datasets/266/data/latest"
        );
        assert_eq!(
            url("https://data.fingrid.fi/api", FingridDataset::Consumption),
            "https://data.fingrid.fi/api/datasets/265/data/latest"
        );
    }

    #[test]
    fn parses_the_latest_value() {
        let zones = parse(BODY.as_bytes()).unwrap();
        assert_eq!(zones[0].0, "FI");
        assert!((zones[0].1 - 72.5).abs() < f64::EPSILON);
        assert_eq!(zones[0].2.to_rfc3339(), "2026-10-20T12:24:00+00:00");
    }

    #[test]
    fn seven_fractional_digits_and_an_offset_are_accepted() {
        let body = r#"{"datasetId":266,"startTime":"2026-10-20T12:24:00.0000000+00:00","endTime":"2026-10-20T12:24:00.0000000+00:00","value":70}"#;
        assert!((parse(body.as_bytes()).unwrap()[0].1 - 70.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_missing_measurement_is_empty() {
        let body = r#"{"datasetId":266,"startTime":"2026-10-20T12:24:00.000Z","endTime":"2026-10-20T12:24:00.000Z","value":null}"#;
        assert!(matches!(parse(body.as_bytes()), Err(FeedError::Empty)));
    }

    #[test]
    fn an_error_body_is_a_parse_error() {
        let body =
            r#"{ "statusCode": 401, "message": "Access denied due to missing subscription key." }"#;
        assert!(matches!(parse(body.as_bytes()), Err(FeedError::Parse(_))));
    }
}
