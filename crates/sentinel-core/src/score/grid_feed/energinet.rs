//! Energinet Energi Data Service (Denmark): `CO2Emis`, CO2 per kWh consumed
//! in each bidding zone (`DK1`, `DK2`), every 5 minutes.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::scraper::{FeedError, check_value, parse_naive_utc};

/// The last hour, newest first: about a dozen slots per zone, so a feed
/// running late still answers and the 1-hour freshness limit applies.
const QUERY: &str = "dataset/CO2Emis?start=now-PT1H&sort=Minutes5UTC%20DESC";

#[derive(Deserialize)]
struct Response {
    records: Vec<Record>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Record {
    #[serde(rename = "Minutes5UTC")]
    minutes5_utc: String,
    price_area: String,
    #[serde(rename = "CO2Emission")]
    co2_emission: Option<f64>,
}

pub(super) fn url(endpoint: &str) -> String {
    format!("{endpoint}/{QUERY}")
}

/// Latest value per zone. The newest slot is picked in code: the order
/// of zones within a slot is not documented.
pub(super) fn parse(body: &[u8]) -> Result<Vec<(String, f64, DateTime<Utc>)>, FeedError> {
    let response: Response =
        serde_json::from_slice(body).map_err(|e| FeedError::Parse(e.to_string()))?;
    let mut latest: BTreeMap<String, (f64, DateTime<Utc>)> = BTreeMap::new();
    for record in response.records {
        let Ok(value) = check_value(record.co2_emission) else {
            continue;
        };
        // Naive timestamp documented as UTC, the start of the 5-minute slot.
        let at = parse_naive_utc(&record.minutes5_utc, "%Y-%m-%dT%H:%M:%S", "Minutes5UTC")?;
        if latest
            .get(&record.price_area)
            .is_none_or(|(_, seen)| at > *seen)
        {
            latest.insert(record.price_area, (value, at));
        }
    }
    if latest.is_empty() {
        return Err(FeedError::Empty);
    }
    Ok(latest
        .into_iter()
        .map(|(zone, (value, at))| (zone, value, at))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live answer, 2026-10-20 12:30 UTC.
    const BODY: &str = r#"{"total":6,"filters":"","sort":"Minutes5UTC DESC","dataset":"CO2Emis","records":[
      {"Minutes5UTC":"2026-10-20T12:25:00","Minutes5DK":"2026-10-20T14:25:00","PriceArea":"DK1","CO2Emission":123.000000},
      {"Minutes5UTC":"2026-10-20T12:25:00","Minutes5DK":"2026-10-20T14:25:00","PriceArea":"DK2","CO2Emission":85.000000},
      {"Minutes5UTC":"2026-10-20T12:20:00","Minutes5DK":"2026-10-20T14:20:00","PriceArea":"DK1","CO2Emission":123.000000},
      {"Minutes5UTC":"2026-10-20T12:20:00","Minutes5DK":"2026-10-20T14:20:00","PriceArea":"DK2","CO2Emission":85.000000},
      {"Minutes5UTC":"2026-10-20T12:15:00","Minutes5DK":"2026-10-20T14:15:00","PriceArea":"DK1","CO2Emission":127.000000},
      {"Minutes5UTC":"2026-10-20T12:15:00","Minutes5DK":"2026-10-20T14:15:00","PriceArea":"DK2","CO2Emission":85.000000}]}"#;

    #[test]
    fn url_asks_for_the_last_hour_newest_first() {
        let url = url("https://api.energidataservice.dk");
        assert_eq!(
            url,
            "https://api.energidataservice.dk/dataset/CO2Emis?start=now-PT1H&sort=Minutes5UTC%20DESC"
        );
        assert!(url.parse::<hyper::Uri>().is_ok());
    }

    #[test]
    fn keeps_the_newest_slot_of_each_zone() {
        let zones = parse(BODY.as_bytes()).unwrap();
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].0, "DK1");
        assert!((zones[0].1 - 123.0).abs() < f64::EPSILON);
        assert_eq!(zones[0].2.to_rfc3339(), "2026-10-20T12:25:00+00:00");
        assert_eq!(zones[1].0, "DK2");
        assert!((zones[1].1 - 85.0).abs() < f64::EPSILON);
    }

    #[test]
    fn order_within_the_answer_does_not_matter() {
        let body = r#"{"records":[
          {"Minutes5UTC":"2026-10-20T12:15:00","PriceArea":"DK1","CO2Emission":127},
          {"Minutes5UTC":"2026-10-20T12:25:00","PriceArea":"DK1","CO2Emission":110}]}"#;
        let zones = parse(body.as_bytes()).unwrap();
        assert!((zones[0].1 - 110.0).abs() < f64::EPSILON);
    }

    #[test]
    fn null_values_are_skipped_and_nothing_left_is_empty() {
        let body = r#"{"records":[
          {"Minutes5UTC":"2026-10-20T12:25:00","PriceArea":"DK1","CO2Emission":null},
          {"Minutes5UTC":"2026-10-20T12:20:00","PriceArea":"DK1","CO2Emission":120}]}"#;
        let zones = parse(body.as_bytes()).unwrap();
        assert!((zones[0].1 - 120.0).abs() < f64::EPSILON);
        assert!(matches!(
            parse(br#"{"total":0,"records":[]}"#),
            Err(FeedError::Empty)
        ));
    }

    #[test]
    fn a_rate_limit_body_is_a_parse_error() {
        let body = r#"{ "statusCode": 429, "message": "Rate limit is exceeded. Try again in 52 seconds." }"#;
        assert!(matches!(parse(body.as_bytes()), Err(FeedError::Parse(_))));
    }
}
