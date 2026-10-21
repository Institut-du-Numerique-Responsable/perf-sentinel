//! REE e·sios (Spain): indicator 10355, CO2 of peninsular generation in
//! real time, every 5 minutes, behind a free personal token.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::scraper::{FeedError, check_value, parse_rfc3339};

/// Sent by every e·sios client: the API versions its answers by it.
pub(super) const ACCEPT: &str = "application/json; application/vnd.esios-api-v1+json";
/// Peninsular Spain, the only geo of indicator 10355.
const PENINSULA: u32 = 8741;
const TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";
/// Enough points to tell a stalled indicator from an empty one.
const LOOKBACK: chrono::TimeDelta = chrono::TimeDelta::hours(3);

#[derive(Deserialize)]
struct Response {
    indicator: Indicator,
}

#[derive(Deserialize)]
struct Indicator {
    values: Vec<Point>,
}

#[derive(Deserialize)]
struct Point {
    value: Option<f64>,
    datetime_utc: String,
    geo_id: u32,
}

/// The last three hours up to now. Moving bounds also miss the CDN cache,
/// which answers cached URLs without checking the token.
pub(super) fn url(endpoint: &str, now: DateTime<Utc>) -> String {
    let from = (now - LOOKBACK).format(TIME_FORMAT);
    let to = now.format(TIME_FORMAT);
    format!("{endpoint}/indicators/10355?start_date={from}&end_date={to}")
}

/// The newest peninsular point. The unit reads `tCO2/MW` but the value is
/// tCO2 per `MWh` generated, so times 1000 gives gCO2/kWh.
pub(super) fn parse(body: &[u8]) -> Result<Vec<(String, f64, DateTime<Utc>)>, FeedError> {
    let response: Response =
        serde_json::from_slice(body).map_err(|e| FeedError::Parse(e.to_string()))?;
    let mut latest: Option<(f64, DateTime<Utc>)> = None;
    for point in response.indicator.values {
        // e·sios writes 0 for a missing point, never a carbon-free grid.
        let Some(value) = point.value.filter(|v| *v > 0.0) else {
            continue;
        };
        if point.geo_id != PENINSULA {
            continue;
        }
        let at = parse_rfc3339(&point.datetime_utc, "datetime_utc")?;
        if latest.is_none_or(|(_, seen)| at > seen) {
            latest = Some((value, at));
        }
    }
    let (value, at) = latest.ok_or(FeedError::Empty)?;
    Ok(vec![(
        "ES".to_string(),
        check_value(Some(value * 1000.0))?,
        at,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// Live answer, 2026-10-20 13:00 UTC, earlier points elided.
    const BODY: &str = r#"{"indicator":{"name":"CO2 Asociado Generación T.Real","short_name":"CO2 T.Real","id":10355,"composited":true,"step_type":"linear","disaggregated":false,"magnitud":[{"name":"tCO2/MW","id":217}],"tiempo":[{"name":"Cinco minutos","id":219}],"geos":[{"geo_id":8741,"geo_name":"Península"}],"values_updated_at":"2026-10-20T14:58:58.000+02:00","values":[
      {"value":0.04381374304835517,"datetime":"2026-10-20T14:45:00.000+02:00","datetime_utc":"2026-10-20T12:45:00Z","tz_time":"2026-10-20T12:45:00.000Z","geo_id":8741,"geo_name":"Península"},
      {"value":0.04250966695771486,"datetime":"2026-10-20T14:50:00.000+02:00","datetime_utc":"2026-10-20T12:50:00Z","tz_time":"2026-10-20T12:50:00.000Z","geo_id":8741,"geo_name":"Península"}]}}"#;

    #[test]
    fn url_asks_for_the_last_three_hours() {
        let url = url("https://api.esios.ree.es", at("2026-10-20T13:07:55Z"));
        assert_eq!(
            url,
            "https://api.esios.ree.es/indicators/10355?start_date=2026-10-20T10:07:55Z&end_date=2026-10-20T13:07:55Z"
        );
        assert!(url.parse::<hyper::Uri>().is_ok());
    }

    #[test]
    fn takes_the_newest_point_in_grams_per_kwh() {
        let zones = parse(BODY.as_bytes()).unwrap();
        assert_eq!(zones[0].0, "ES");
        assert!((zones[0].1 - 42.509_666_957_714_86).abs() < 1e-9);
        assert_eq!(zones[0].2, at("2026-10-20T12:50:00Z"));
    }

    #[test]
    fn zeros_and_other_geos_are_not_readings() {
        let body = r#"{"indicator":{"values":[
          {"value":0.05,"datetime_utc":"2026-10-20T12:45:00Z","geo_id":8741},
          {"value":0,"datetime_utc":"2026-10-20T12:50:00Z","geo_id":8741},
          {"value":0.6,"datetime_utc":"2026-10-20T12:55:00Z","geo_id":8795}]}}"#;
        let zones = parse(body.as_bytes()).unwrap();
        assert!((zones[0].1 - 50.0).abs() < 1e-9);
        assert_eq!(zones[0].2, at("2026-10-20T12:45:00Z"));
    }

    #[test]
    fn no_value_is_empty_and_a_refusal_is_a_parse_error() {
        assert!(matches!(
            parse(br#"{"indicator":{"values":[]}}"#),
            Err(FeedError::Empty)
        ));
        assert!(matches!(
            parse(br#"{"Status":403,"message":"Forbidden"}"#),
            Err(FeedError::Parse(_))
        ));
    }
}
