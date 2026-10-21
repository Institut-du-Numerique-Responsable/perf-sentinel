//! RTE `éCO2mix`: `taux_co2` of the `eco2mix-national-tr` dataset on the
//! ODRÉ portal, an estimate of the direct emissions of French production.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::scraper::{FeedError, check_value, parse_rfc3339};

/// Latest record with a CO2 rate, newest first, two fields only.
const RECORDS_QUERY: &str = "catalog/datasets/eco2mix-national-tr/records\
    ?select=taux_co2%2Cdate_heure\
    &where=taux_co2%20is%20not%20null\
    &order_by=date_heure%20desc\
    &limit=1";

#[derive(Deserialize)]
struct RecordsResponse {
    results: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    taux_co2: Option<f64>,
    date_heure: String,
}

pub(super) fn url(endpoint: &str) -> String {
    format!("{endpoint}/{RECORDS_QUERY}")
}

pub(super) fn parse(body: &[u8]) -> Result<Vec<(String, f64, DateTime<Utc>)>, FeedError> {
    let response: RecordsResponse =
        serde_json::from_slice(body).map_err(|e| FeedError::Parse(e.to_string()))?;
    let record = response
        .results
        .into_iter()
        .next()
        .ok_or(FeedError::Empty)?;
    let value = check_value(record.taux_co2)?;
    let at = parse_rfc3339(&record.date_heure, "date_heure")?;
    Ok(vec![("FR".to_string(), value, at)])
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Shape of an Explore API v2.1 `records` answer for this query.
    pub(in crate::score::grid_feed) const BODY: &str = r#"{"total_count": 1, "results": [{"taux_co2": 21.0, "date_heure": "2026-10-20T08:15:00+00:00"}]}"#;

    #[test]
    fn url_targets_the_national_real_time_dataset() {
        let url = url("https://odre.opendatasoft.com/api/explore/v2.1");
        assert!(url.starts_with(
            "https://odre.opendatasoft.com/api/explore/v2.1/catalog/datasets/eco2mix-national-tr/records?"
        ));
        assert!(url.contains("select=taux_co2%2Cdate_heure"));
        assert!(url.contains("order_by=date_heure%20desc"));
        assert!(url.contains("limit=1"));
        assert!(url.parse::<hyper::Uri>().is_ok());
    }

    #[test]
    fn parses_latest_rate() {
        let zones = parse(BODY.as_bytes()).unwrap();
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].0, "FR");
        assert!((zones[0].1 - 21.0).abs() < f64::EPSILON);
    }

    #[test]
    fn integer_rate_and_paris_offset_are_accepted() {
        let body = r#"{"total_count": 1, "results": [{"taux_co2": 34, "date_heure": "2026-10-20T10:15:00+02:00"}]}"#;
        let zones = parse(body.as_bytes()).unwrap();
        assert!((zones[0].1 - 34.0).abs() < f64::EPSILON);
        assert_eq!(zones[0].2.to_rfc3339(), "2026-10-20T08:15:00+00:00");
    }

    #[test]
    fn empty_results_or_null_rate_are_rejected() {
        let empty = r#"{"total_count": 0, "results": []}"#;
        assert!(matches!(parse(empty.as_bytes()), Err(FeedError::Empty)));
        let null =
            r#"{"results": [{"taux_co2": null, "date_heure": "2026-10-20T08:15:00+00:00"}]}"#;
        assert!(matches!(parse(null.as_bytes()), Err(FeedError::Empty)));
    }

    #[test]
    fn negative_rate_is_rejected() {
        let negative =
            r#"{"results": [{"taux_co2": -1, "date_heure": "2026-10-20T08:15:00+00:00"}]}"#;
        assert!(matches!(
            parse(negative.as_bytes()),
            Err(FeedError::InvalidValue)
        ));
    }

    #[test]
    fn malformed_body_is_a_parse_error() {
        assert!(matches!(parse(b"<html>"), Err(FeedError::Parse(_))));
    }
}
