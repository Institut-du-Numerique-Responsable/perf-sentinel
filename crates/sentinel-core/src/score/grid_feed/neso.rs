//! NESO Carbon Intensity API (Great Britain): national intensity per half
//! hour, direct CO2 of generation plus imports per kWh consumed.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::scraper::{FeedError, check_value, parse_naive_utc};

/// Minutes only, no seconds: "2026-10-20T12:00Z".
const TIME_FORMAT: &str = "%Y-%m-%dT%H:%MZ";
/// Windows read per poll: enough to see an estimate stop moving.
const LOOKBACK: chrono::TimeDelta = chrono::TimeDelta::hours(3);
/// The same estimate on this many consecutive windows while the forecast
/// moved means NESO stopped updating it and copies it forward.
const FROZEN_WINDOWS: usize = 4;

#[derive(Deserialize)]
struct Response {
    data: Vec<Period>,
}

#[derive(Deserialize)]
struct Period {
    from: String,
    intensity: Intensity,
}

#[derive(Deserialize)]
struct Intensity {
    actual: Option<f64>,
    forecast: Option<f64>,
}

/// The last three hours of half-hour windows, up to now.
pub(super) fn url(endpoint: &str, now: DateTime<Utc>) -> String {
    let from = (now - LOOKBACK).format(TIME_FORMAT);
    let to = now.format(TIME_FORMAT);
    format!("{endpoint}/intensity/{from}/{to}")
}

/// `actual` is NESO's estimate from metered generation, published once a
/// window ends; the window in progress only has a forecast. Take the newest
/// estimate, dated by its own window start. When it has stopped moving
/// while the forecast moves, or when no window has one, take the newest
/// window's forecast instead.
pub(super) fn parse(body: &[u8]) -> Result<Vec<(String, f64, DateTime<Utc>)>, FeedError> {
    let response: Response =
        serde_json::from_slice(body).map_err(|e| FeedError::Parse(e.to_string()))?;
    let mut windows = Vec::with_capacity(response.data.len());
    for period in response.data {
        let at = parse_naive_utc(&period.from, TIME_FORMAT, "from")?;
        windows.push((at, period.intensity));
    }
    windows.sort_by_key(|(at, _)| *at);
    let estimated: Vec<&(DateTime<Utc>, Intensity)> =
        windows.iter().filter(|(_, i)| i.actual.is_some()).collect();
    let frozen = estimated.len() >= FROZEN_WINDOWS && {
        let tail = &estimated[estimated.len() - FROZEN_WINDOWS..];
        tail.windows(2).all(|w| w[0].1.actual == w[1].1.actual)
            && tail.windows(2).any(|w| w[0].1.forecast != w[1].1.forecast)
    };
    let (at, value) = match estimated.last() {
        Some((at, intensity)) if !frozen => (*at, intensity.actual),
        _ => {
            let (at, intensity) = windows.last().ok_or(FeedError::Empty)?;
            (*at, intensity.forecast)
        }
    };
    Ok(vec![("GB".to_string(), check_value(value)?, at)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn body(windows: &[(&str, Option<f64>, Option<f64>)]) -> String {
        let data: Vec<String> = windows
            .iter()
            .map(|(from, forecast, actual)| {
                let n = |v: &Option<f64>| v.map_or("null".to_string(), |v| v.to_string());
                format!(
                    r#"{{"from":"{from}","to":"x","intensity":{{"forecast":{},"actual":{},"index":"moderate"}}}}"#,
                    n(forecast),
                    n(actual)
                )
            })
            .collect();
        format!(r#"{{"data":[{}]}}"#, data.join(","))
    }

    #[test]
    fn url_asks_for_the_last_three_hours() {
        assert_eq!(
            url(
                "https://api.carbonintensity.org.uk",
                at("2026-10-20T12:59:31Z")
            ),
            "https://api.carbonintensity.org.uk/intensity/2026-10-20T09:59Z/2026-10-20T12:59Z"
        );
    }

    #[test]
    fn takes_the_newest_estimate_dated_by_its_own_window() {
        // Live shape: the window in progress has a forecast only.
        let body = body(&[
            ("2026-10-20T11:30Z", Some(124.0), Some(118.0)),
            ("2026-10-20T12:00Z", Some(117.0), Some(122.0)),
            ("2026-10-20T12:30Z", Some(115.0), None),
        ]);
        let zones = parse(body.as_bytes()).unwrap();
        assert_eq!(zones[0].0, "GB");
        assert!((zones[0].1 - 122.0).abs() < f64::EPSILON);
        assert_eq!(zones[0].2, at("2026-10-20T12:00:00Z"));
    }

    #[test]
    fn a_frozen_estimate_gives_way_to_the_current_forecast() {
        // Live on 2026-10-20: actual stuck at 121 while the forecast moved.
        let body = body(&[
            ("2026-10-20T10:30Z", Some(126.0), Some(121.0)),
            ("2026-10-20T11:00Z", Some(130.0), Some(121.0)),
            ("2026-10-20T11:30Z", Some(124.0), Some(121.0)),
            ("2026-10-20T12:00Z", Some(117.0), Some(121.0)),
            ("2026-10-20T12:30Z", Some(115.0), None),
        ]);
        let zones = parse(body.as_bytes()).unwrap();
        assert!((zones[0].1 - 115.0).abs() < f64::EPSILON);
        assert_eq!(zones[0].2, at("2026-10-20T12:30:00Z"));
    }

    #[test]
    fn a_steady_grid_is_not_mistaken_for_a_frozen_estimate() {
        let body = body(&[
            ("2026-10-20T02:00Z", Some(90.0), Some(91.0)),
            ("2026-10-20T02:30Z", Some(90.0), Some(91.0)),
            ("2026-10-20T03:00Z", Some(90.0), Some(91.0)),
            ("2026-10-20T03:30Z", Some(90.0), Some(91.0)),
        ]);
        assert!((parse(body.as_bytes()).unwrap()[0].1 - 91.0).abs() < f64::EPSILON);
    }

    #[test]
    fn no_estimate_at_all_falls_back_to_the_newest_forecast() {
        let body = body(&[
            ("2026-10-20T12:30Z", Some(115.0), None),
            ("2026-10-20T12:00Z", Some(117.0), None),
        ]);
        let zones = parse(body.as_bytes()).unwrap();
        assert!((zones[0].1 - 115.0).abs() < f64::EPSILON);
    }

    #[test]
    fn no_value_or_no_window_is_empty() {
        let body = body(&[("2026-10-20T12:30Z", None, None)]);
        assert!(matches!(parse(body.as_bytes()), Err(FeedError::Empty)));
        assert!(matches!(parse(br#"{"data": []}"#), Err(FeedError::Empty)));
    }

    #[test]
    fn an_error_body_is_a_parse_error() {
        let body =
            r#"{"error": {"code": "400 Bad Request", "message": "Please enter a valid datetime"}}"#;
        assert!(matches!(parse(body.as_bytes()), Err(FeedError::Parse(_))));
    }

    #[test]
    fn a_timestamp_with_seconds_is_rejected_rather_than_misread() {
        let body = body(&[("2026-10-20T12:00:00Z", Some(117.0), Some(122.0))]);
        assert!(matches!(parse(body.as_bytes()), Err(FeedError::Parse(_))));
    }
}
