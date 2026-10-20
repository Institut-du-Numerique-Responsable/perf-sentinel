//! Background poller for the RTE `éCO2mix` `eco2mix-national-tr` dataset.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::config::RteEco2mixConfig;
use super::state::RteEco2mixState;
use crate::http_client;
use crate::score::scaphandre::state::monotonic_ms;

/// One record is a few dozen bytes, this leaves room for a verbose error body.
const MAX_BODY_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Consecutive failed ticks before the one-shot fallback warning.
const FAILURE_THRESHOLD: u32 = 3;
/// The latest non-null `taux_co2` lags real time by up to an hour or so;
/// an older one means the feed stalled and the value is no longer live.
const MAX_READING_AGE: chrono::TimeDelta = chrono::TimeDelta::hours(3);

/// Latest record with a CO2 rate, newest first, two fields only.
const RECORDS_QUERY: &str = "catalog/datasets/eco2mix-national-tr/records\
    ?select=taux_co2%2Cdate_heure\
    &where=taux_co2%20is%20not%20null\
    &order_by=date_heure%20desc\
    &limit=1";

#[derive(Debug, thiserror::Error)]
pub(crate) enum RteScraperError {
    #[error("invalid request URI: {0}")]
    InvalidUri(String),
    #[error("fetch failed: {0}")]
    Fetch(#[from] http_client::FetchError),
    #[error("invalid response: {0}")]
    Parse(String),
    #[error("no record with a CO2 rate in the response")]
    Empty,
    #[error("taux_co2 is not a finite non-negative number")]
    InvalidValue,
    #[error("latest record is {0} minutes old, the feed looks stalled")]
    Stale(i64),
}

#[derive(Deserialize)]
struct RecordsResponse {
    results: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    taux_co2: Option<f64>,
    date_heure: String,
}

/// Spawn the poller. The first tick fires immediately.
#[must_use]
pub fn spawn_rte_eco2mix_scraper(
    config: RteEco2mixConfig,
    state: Arc<RteEco2mixState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = http_client::build_client();
        let url = records_url(&config.endpoint);
        let mut ticker = tokio::time::interval(config.poll_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut consecutive_failures: u32 = 0;
        loop {
            ticker.tick().await;
            match fetch_intensity(&client, &url, Utc::now()).await {
                Ok(gco2) => {
                    state.publish(gco2, monotonic_ms());
                    consecutive_failures = 0;
                }
                Err(e) => {
                    tracing::debug!(
                        endpoint = %http_client::redact_endpoint_str(&url),
                        error = %e,
                        "RTE éCO2mix fetch failed"
                    );
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    if consecutive_failures == FAILURE_THRESHOLD {
                        tracing::warn!(
                            "RTE éCO2mix: {FAILURE_THRESHOLD} consecutive failures ({e}), \
                             falling back to embedded profiles"
                        );
                    }
                }
            }
        }
    })
}

fn records_url(endpoint: &str) -> String {
    format!("{endpoint}/{RECORDS_QUERY}")
}

async fn fetch_intensity(
    client: &http_client::HttpClient,
    url: &str,
    now: DateTime<Utc>,
) -> Result<f64, RteScraperError> {
    let uri: hyper::Uri = url
        .parse()
        .map_err(|e: hyper::http::uri::InvalidUri| RteScraperError::InvalidUri(e.to_string()))?;
    let body = http_client::fetch_get_limited(
        client,
        &uri,
        "perf-sentinel",
        REQUEST_TIMEOUT,
        None,
        MAX_BODY_BYTES,
    )
    .await?;
    parse_latest(&body, now)
}

/// Extract the newest `taux_co2` (gCO2/kWh) and check it is still live.
fn parse_latest(body: &[u8], now: DateTime<Utc>) -> Result<f64, RteScraperError> {
    let response: RecordsResponse =
        serde_json::from_slice(body).map_err(|e| RteScraperError::Parse(e.to_string()))?;
    let record = response
        .results
        .into_iter()
        .next()
        .ok_or(RteScraperError::Empty)?;
    let value = record.taux_co2.ok_or(RteScraperError::Empty)?;
    if !value.is_finite() || value < 0.0 {
        return Err(RteScraperError::InvalidValue);
    }
    let at = DateTime::parse_from_rfc3339(&record.date_heure)
        .map_err(|e| RteScraperError::Parse(format!("date_heure: {e}")))?;
    let age = now.signed_duration_since(at);
    if age > MAX_READING_AGE {
        return Err(RteScraperError::Stale(age.num_minutes()));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{http_200_text, http_status, spawn_one_shot_server};

    /// Shape of an Explore API v2.1 `records` answer for this query.
    const BODY: &str = r#"{"total_count": 1, "results": [{"taux_co2": 21.0, "date_heure": "2026-10-20T08:15:00+00:00"}]}"#;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn url_targets_the_national_real_time_dataset() {
        let url = records_url("https://odre.opendatasoft.com/api/explore/v2.1");
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
        let value = parse_latest(BODY.as_bytes(), at("2026-10-20T09:00:00Z")).unwrap();
        assert!((value - 21.0).abs() < f64::EPSILON);
    }

    #[test]
    fn integer_rate_and_paris_offset_are_accepted() {
        let body = r#"{"total_count": 1, "results": [{"taux_co2": 34, "date_heure": "2026-10-20T10:15:00+02:00"}]}"#;
        let value = parse_latest(body.as_bytes(), at("2026-10-20T09:00:00Z")).unwrap();
        assert!((value - 34.0).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_results_are_rejected() {
        let body = r#"{"total_count": 0, "results": []}"#;
        assert!(matches!(
            parse_latest(body.as_bytes(), at("2026-10-20T09:00:00Z")),
            Err(RteScraperError::Empty)
        ));
    }

    #[test]
    fn null_or_negative_rate_is_rejected() {
        let null =
            r#"{"results": [{"taux_co2": null, "date_heure": "2026-10-20T08:15:00+00:00"}]}"#;
        assert!(matches!(
            parse_latest(null.as_bytes(), at("2026-10-20T09:00:00Z")),
            Err(RteScraperError::Empty)
        ));
        let negative =
            r#"{"results": [{"taux_co2": -1, "date_heure": "2026-10-20T08:15:00+00:00"}]}"#;
        assert!(matches!(
            parse_latest(negative.as_bytes(), at("2026-10-20T09:00:00Z")),
            Err(RteScraperError::InvalidValue)
        ));
    }

    #[test]
    fn stalled_feed_is_rejected() {
        assert!(matches!(
            parse_latest(BODY.as_bytes(), at("2026-10-20T12:00:00Z")),
            Err(RteScraperError::Stale(225))
        ));
    }

    #[test]
    fn malformed_body_is_a_parse_error() {
        assert!(matches!(
            parse_latest(b"<html>", at("2026-10-20T09:00:00Z")),
            Err(RteScraperError::Parse(_))
        ));
    }

    #[tokio::test]
    async fn fetch_reads_the_rate_from_the_server() {
        let (base, server) =
            spawn_one_shot_server(http_200_text("application/json; charset=utf-8", BODY)).await;
        let client = http_client::build_client();
        let value = fetch_intensity(&client, &records_url(&base), at("2026-10-20T09:00:00Z"))
            .await
            .unwrap();
        assert!((value - 21.0).abs() < f64::EPSILON);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn fetch_surfaces_http_errors() {
        let (base, server) = spawn_one_shot_server(http_status(429, "Too Many Requests")).await;
        let client = http_client::build_client();
        let err = fetch_intensity(&client, &records_url(&base), at("2026-10-20T09:00:00Z"))
            .await
            .unwrap_err();
        assert!(matches!(err, RteScraperError::Fetch(_)), "got {err:?}");
        server.await.unwrap();
    }
}
