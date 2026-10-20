//! em6 (New Zealand): carbon intensity of grid-connected generation per
//! 30-minute trading period, on a free endpoint without key.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::scraper::{FeedError, check_value, parse_rfc3339};

#[derive(Deserialize)]
struct Response {
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    timestamp: String,
    nz_carbon_gkwh: Option<f64>,
}

pub(super) fn url(endpoint: &str) -> String {
    format!("{endpoint}/current_carbon_intensity/")
}

/// The newest of the last three trading periods, dated by the start of its
/// period: em6 publishes a period a few minutes after it ends.
pub(super) fn parse(body: &[u8]) -> Result<Vec<(String, f64, DateTime<Utc>)>, FeedError> {
    let response: Response =
        serde_json::from_slice(body).map_err(|e| FeedError::Parse(e.to_string()))?;
    let mut latest: Option<(f64, DateTime<Utc>)> = None;
    for item in response.items {
        let Ok(value) = check_value(item.nz_carbon_gkwh) else {
            continue;
        };
        let at = parse_rfc3339(&item.timestamp, "timestamp")?;
        if latest.is_none_or(|(_, seen)| at > seen) {
            latest = Some((value, at));
        }
    }
    let (value, at) = latest.ok_or(FeedError::Empty)?;
    Ok(vec![("NZ".to_string(), value, at)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live answer, 2026-10-20 13:03 UTC, third period elided.
    const BODY: &str = r#"{"items":[
      {"trading_date":"2026-10-20T11:00:00Z","trading_period":4,"timestamp":"2026-10-20T12:30:00Z","nz_carbon_t":61.72,"nz_carbon_gkwh":31.74,"nz_carbon_gkwh_prev":31.84,"nz_carbon_change_gkwh":-0.1,"nz_renewable":98.58,"max_24hrs_gkwh":54.39,"min_24hrs_gkwh":28.54,"current_month_avg_gkwh":33.4,"current_year_avg_gkwh":45.71,"pct_current_year_gkwh":25.82},
      {"trading_date":"2026-10-20T11:00:00Z","trading_period":3,"timestamp":"2026-10-20T12:00:00Z","nz_carbon_t":61.69,"nz_carbon_gkwh":31.84,"nz_carbon_gkwh_prev":30.92,"nz_carbon_change_gkwh":0.92,"nz_renewable":98.57,"max_24hrs_gkwh":54.39,"min_24hrs_gkwh":28.54,"current_month_avg_gkwh":33.4,"current_year_avg_gkwh":45.71,"pct_current_year_gkwh":25.82}],
      "hasMore":false,"limit":100,"offset":0,"count":2,"links":[]}"#;

    #[test]
    fn url_reads_the_current_intensity() {
        let url = url("https://api.em6.co.nz/ords/em6/data_api");
        assert_eq!(
            url,
            "https://api.em6.co.nz/ords/em6/data_api/current_carbon_intensity/"
        );
        assert!(url.parse::<hyper::Uri>().is_ok());
    }

    #[test]
    fn takes_the_newest_period() {
        let zones = parse(BODY.as_bytes()).unwrap();
        assert_eq!(zones[0].0, "NZ");
        assert!((zones[0].1 - 31.74).abs() < f64::EPSILON);
        assert_eq!(zones[0].2.to_rfc3339(), "2026-10-20T12:30:00+00:00");
    }

    #[test]
    fn a_null_period_is_skipped_and_nothing_left_is_empty() {
        let body = r#"{"items":[
          {"timestamp":"2026-10-20T12:30:00Z","nz_carbon_gkwh":null},
          {"timestamp":"2026-10-20T12:00:00Z","nz_carbon_gkwh":31.84}]}"#;
        assert!((parse(body.as_bytes()).unwrap()[0].1 - 31.84).abs() < f64::EPSILON);
        assert!(matches!(parse(br#"{"items":[]}"#), Err(FeedError::Empty)));
    }

    #[test]
    fn an_error_body_is_a_parse_error() {
        assert!(matches!(
            parse(br#"{"message":"Unauthorized"}"#),
            Err(FeedError::Parse(_))
        ));
    }
}
