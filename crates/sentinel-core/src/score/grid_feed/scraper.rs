//! Polling loop shared by every grid operator feed. Each feed only differs
//! by its requests, its headers and its response parser.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};

use super::config::{GridFeedConfig, GridFeedKind};
use super::state::GridFeedState;
use super::{em6, energinet, esios, fingrid, mavir, neso, open_electricity, rte};
use crate::http_client;
use crate::ingest::auth_header::AuthHeader;
use crate::score::scaphandre::state::monotonic_ms;

/// Each answer is a handful of records, this leaves room for a verbose error body.
const MAX_BODY_BYTES: usize = 64 * 1024;
/// Three hours of 5-minute points per region and technology: about 150 KiB
/// for the NEM.
const MAX_OPEN_ELECTRICITY_BODY_BYTES: usize = 512 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Names the project to operators that publish for people, not robots.
const USER_AGENT: &str =
    "perf-sentinel/grid-feed (+https://github.com/Institut-du-Numerique-Responsable/perf-sentinel)";
/// Consecutive failed ticks before the one-shot fallback warning.
const FAILURE_THRESHOLD: u32 = 3;
/// A record dated further ahead than clock skew explains is not a measurement.
const MAX_CLOCK_SKEW: chrono::TimeDelta = chrono::TimeDelta::minutes(15);

#[derive(Debug, thiserror::Error)]
pub(crate) enum FeedError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("fetch failed: {0}")]
    Fetch(#[from] http_client::FetchError),
    #[error("invalid response: {0}")]
    Parse(String),
    #[error("no record with a carbon intensity in the response")]
    Empty,
    #[error("the carbon intensity is not a finite non-negative number")]
    InvalidValue,
    #[error("latest record is {0} minutes old, the feed looks stalled")]
    Stale(i64),
    #[error("latest record is dated {0} minutes in the future")]
    Future(i64),
}

/// Oldest record still read as live. Each operator publishes with its own
/// lag: RTE up to an hour or so, Open Electricity up to 100 minutes for
/// rooftop solar on the WEM, NESO and em6 per half hour, MAVIR per quarter
/// hour, Fingrid, Energinet and e·sios every few minutes.
const fn max_reading_age(kind: GridFeedKind) -> chrono::TimeDelta {
    match kind {
        GridFeedKind::RteEco2mix | GridFeedKind::OpenElectricity => chrono::TimeDelta::hours(3),
        GridFeedKind::Neso | GridFeedKind::Em6 => chrono::TimeDelta::hours(2),
        GridFeedKind::Fingrid(_)
        | GridFeedKind::Energinet
        | GridFeedKind::ReeEsios
        | GridFeedKind::Mavir => chrono::TimeDelta::hours(1),
    }
}

pub(super) fn check_value(value: Option<f64>) -> Result<f64, FeedError> {
    let value = value.ok_or(FeedError::Empty)?;
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(FeedError::InvalidValue)
    }
}

pub(super) fn parse_rfc3339(raw: &str, field: &str) -> Result<DateTime<Utc>, FeedError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|e| FeedError::Parse(format!("{field}: {e}")))
}

/// A timestamp written without offset that the API documents as UTC.
pub(super) fn parse_naive_utc(
    raw: &str,
    format: &str,
    field: &str,
) -> Result<DateTime<Utc>, FeedError> {
    NaiveDateTime::parse_from_str(raw, format)
        .map(|at| at.and_utc())
        .map_err(|e| FeedError::Parse(format!("{field}: {e}")))
}

/// The requests of a feed at `now`, each with the wanted zones its answer
/// serves. Only Open Electricity splits, one call per network.
fn requests<'a>(
    cfg: &GridFeedConfig,
    wanted: &[&'a str],
    now: DateTime<Utc>,
) -> Result<Vec<(hyper::Uri, Vec<&'a str>)>, FeedError> {
    let single = |url: String| vec![(url, wanted.to_vec())];
    let calls = match cfg.kind {
        GridFeedKind::RteEco2mix => single(rte::url(&cfg.endpoint)),
        GridFeedKind::Neso => single(neso::url(&cfg.endpoint, now)),
        GridFeedKind::Fingrid(dataset) => single(fingrid::url(&cfg.endpoint, dataset)),
        GridFeedKind::Energinet => single(energinet::url(&cfg.endpoint)),
        GridFeedKind::ReeEsios => single(esios::url(&cfg.endpoint, now)),
        GridFeedKind::Em6 => single(em6::url(&cfg.endpoint)),
        GridFeedKind::Mavir => single(mavir::url(&cfg.endpoint, now)),
        GridFeedKind::OpenElectricity => open_electricity::requests(&cfg.endpoint, wanted, now),
    };
    calls
        .into_iter()
        .map(|(url, zones)| {
            url.parse()
                .map(|uri| (uri, zones))
                .map_err(|e: hyper::http::uri::InvalidUri| FeedError::InvalidRequest(e.to_string()))
        })
        .collect()
}

/// The key, as a bearer token for Open Electricity and `x-api-key`
/// elsewhere, plus the `Accept` e·sios expects and the language cookie
/// MAVIR's headers depend on.
fn request_headers(cfg: &GridFeedConfig) -> Result<Vec<AuthHeader>, FeedError> {
    let mut raw = Vec::new();
    if let Some(key) = cfg.api_key.as_deref() {
        raw.push(match cfg.kind {
            GridFeedKind::OpenElectricity => format!("Authorization: Bearer {key}"),
            _ => format!("x-api-key: {key}"),
        });
    }
    match cfg.kind {
        GridFeedKind::ReeEsios => raw.push(format!("Accept: {}", esios::ACCEPT)),
        GridFeedKind::Mavir => raw.push(format!("Cookie: {}", mavir::COOKIE)),
        _ => {}
    }
    raw.iter()
        .map(|header| {
            AuthHeader::parse(header).map_err(|e| FeedError::InvalidRequest(e.to_string()))
        })
        .collect()
}

/// Live values of the zones served here, and why each other zone got none.
type ZoneSplit = (Vec<(String, f64)>, Vec<(String, FeedError)>);

/// Split the parsed records into the live values of the zones served here
/// and, for every other zone served here, why it got none this tick.
fn fresh_zones(
    kind: GridFeedKind,
    wanted: &[&str],
    parsed: Vec<(String, f64, DateTime<Utc>)>,
    now: DateTime<Utc>,
) -> ZoneSplit {
    let mut fresh = Vec::new();
    let mut failed: Vec<(String, FeedError)> = Vec::new();
    for (zone, value, at) in parsed {
        if !wanted.contains(&zone.as_str()) {
            continue;
        }
        let age = now.signed_duration_since(at);
        if age > max_reading_age(kind) {
            failed.push((zone, FeedError::Stale(age.num_minutes())));
        } else if age < -MAX_CLOCK_SKEW {
            failed.push((zone, FeedError::Future(-age.num_minutes())));
        } else {
            fresh.push((zone, value));
        }
    }
    for zone in wanted {
        let seen = |(z, _): &(String, _)| z == zone;
        if !fresh.iter().any(|(z, _)| z == zone) && !failed.iter().any(seen) {
            failed.push(((*zone).to_string(), FeedError::Empty));
        }
    }
    (fresh, failed)
}

async fn fetch(
    client: &http_client::HttpClient,
    cfg: &GridFeedConfig,
    headers: &[AuthHeader],
    uri: &hyper::Uri,
    wanted: &[&str],
    now: DateTime<Utc>,
) -> Result<ZoneSplit, FeedError> {
    let max_bytes = if cfg.kind == GridFeedKind::OpenElectricity {
        MAX_OPEN_ELECTRICITY_BODY_BYTES
    } else {
        MAX_BODY_BYTES
    };
    let body = http_client::fetch_get_with_headers(
        client,
        uri,
        USER_AGENT,
        REQUEST_TIMEOUT,
        headers,
        max_bytes,
    )
    .await?;
    let parsed = match cfg.kind {
        GridFeedKind::RteEco2mix => rte::parse(&body)?,
        GridFeedKind::Neso => neso::parse(&body)?,
        GridFeedKind::Fingrid(_) => fingrid::parse(&body)?,
        GridFeedKind::Energinet => energinet::parse(&body)?,
        GridFeedKind::ReeEsios => esios::parse(&body)?,
        GridFeedKind::OpenElectricity => open_electricity::parse(&body)?,
        GridFeedKind::Mavir => mavir::parse(&body)?,
        GridFeedKind::Em6 => em6::parse(&body)?,
    };
    Ok(fresh_zones(cfg.kind, wanted, parsed, now))
}

/// Publish the live values of one answer and count the failures of the
/// zones it serves.
fn tally(
    state: &GridFeedState,
    failures: &mut BTreeMap<String, u32>,
    cfg: &GridFeedConfig,
    zones: &[&str],
    result: Result<ZoneSplit, FeedError>,
) {
    let name = cfg.kind.provider().display_name();
    match result {
        Ok((fresh, failed)) => {
            if !fresh.is_empty() {
                state.publish(&fresh, monotonic_ms());
            }
            for (zone, _) in &fresh {
                failures.remove(zone);
            }
            for (zone, e) in &failed {
                tracing::debug!(zone, error = %e, "{name} returned no live value");
                record_failure(failures, name, zone, &e.to_string());
            }
        }
        Err(e) => {
            tracing::debug!(
                endpoint = %http_client::redact_endpoint_str(&cfg.endpoint),
                error = %e,
                "{name} fetch failed"
            );
            let error = e.to_string();
            for zone in zones {
                record_failure(failures, name, zone, &error);
            }
        }
    }
}

/// Count consecutive failures per zone, so a dead zone of a feed whose
/// other zones still answer is reported too, once, at the threshold.
fn record_failure(failures: &mut BTreeMap<String, u32>, name: &str, zone: &str, error: &str) {
    let count = failures.entry(zone.to_string()).or_insert(0);
    *count = count.saturating_add(1);
    if *count == FAILURE_THRESHOLD {
        tracing::warn!(
            "{name} {zone}: {FAILURE_THRESHOLD} consecutive failures ({error}), \
             falling back to embedded profiles"
        );
    }
}

/// Spawn the poller of one feed. The first tick fires immediately.
#[must_use]
pub fn spawn_grid_feed_scraper(
    config: GridFeedConfig,
    state: Arc<GridFeedState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut wanted: Vec<&str> = config.regions.values().map(String::as_str).collect();
        wanted.sort_unstable();
        wanted.dedup();
        let headers = match request_headers(&config)
            .and_then(|headers| requests(&config, &wanted, Utc::now()).map(|_| headers))
        {
            Ok(headers) => headers,
            Err(e) => {
                let name = config.kind.provider().display_name();
                tracing::warn!("{name}: {e}, the feed is disabled");
                return;
            }
        };
        let client = http_client::build_client();
        let mut ticker = tokio::time::interval(config.poll_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut failures: BTreeMap<String, u32> = BTreeMap::new();
        loop {
            ticker.tick().await;
            let now = Utc::now();
            match requests(&config, &wanted, now) {
                Ok(calls) => {
                    for (uri, zones) in &calls {
                        let result = fetch(&client, &config, &headers, uri, zones, now).await;
                        tally(&state, &mut failures, &config, zones, result);
                    }
                }
                Err(e) => tally(&state, &mut failures, &config, &wanted, Err(e)),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{http_200_text, http_status, spawn_one_shot_server};
    use std::collections::BTreeMap;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn rte_config(endpoint: String) -> GridFeedConfig {
        GridFeedConfig {
            kind: GridFeedKind::RteEco2mix,
            endpoint,
            poll_interval: Duration::from_mins(15),
            regions: BTreeMap::from([("eu-west-3".to_string(), "FR".to_string())]),
            api_key: None,
        }
    }

    fn only_error(result: ZoneSplit) -> FeedError {
        assert!(result.0.is_empty(), "unexpected fresh {:?}", result.0);
        result.1.into_iter().next().unwrap().1
    }

    #[test]
    fn stalled_and_future_records_are_rejected() {
        let record = |at: &str| vec![("FR".to_string(), 21.0, self::at(at))];
        let now = at("2026-10-20T12:00:00Z");
        let kind = GridFeedKind::RteEco2mix;
        assert!(matches!(
            only_error(fresh_zones(
                kind,
                &["FR"],
                record("2026-10-20T08:15:00Z"),
                now
            )),
            FeedError::Stale(225)
        ));
        assert!(matches!(
            only_error(fresh_zones(
                kind,
                &["FR"],
                record("2026-10-20T13:15:00Z"),
                now
            )),
            FeedError::Future(75)
        ));
        // Within the clock-skew allowance.
        let (fresh, failed) = fresh_zones(kind, &["FR"], record("2026-10-20T12:05:00Z"), now);
        assert_eq!(fresh.len(), 1);
        assert!(failed.is_empty());
    }

    #[test]
    fn zones_the_config_does_not_map_are_dropped() {
        let now = at("2026-10-20T12:00:00Z");
        let parsed = vec![
            ("DK1".to_string(), 120.0, now),
            ("DK2".to_string(), 80.0, now),
        ];
        let (fresh, failed) = fresh_zones(GridFeedKind::Energinet, &["DK2"], parsed, now);
        assert_eq!(fresh, vec![("DK2".to_string(), 80.0)]);
        assert!(failed.is_empty());
    }

    #[test]
    fn a_zone_missing_from_the_answer_is_reported_as_failed() {
        let now = at("2026-10-20T12:00:00Z");
        let parsed = vec![("DK1".to_string(), 120.0, now)];
        let (fresh, failed) = fresh_zones(GridFeedKind::Energinet, &["DK1", "DK2"], parsed, now);
        assert_eq!(fresh, vec![("DK1".to_string(), 120.0)]);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, "DK2");
        assert!(matches!(failed[0].1, FeedError::Empty));
    }

    #[test]
    fn failures_are_counted_per_zone_and_reset_by_a_fresh_value() {
        let mut failures = BTreeMap::new();
        for _ in 0..FAILURE_THRESHOLD {
            record_failure(&mut failures, "Energinet", "DK2", "empty");
        }
        record_failure(&mut failures, "Energinet", "DK1", "empty");
        assert_eq!(failures["DK2"], FAILURE_THRESHOLD);
        assert_eq!(failures["DK1"], 1);
    }

    #[test]
    fn freshness_limits_follow_each_operator() {
        let now = at("2026-10-20T12:00:00Z");
        let ninety_minutes_ago = vec![("GB".to_string(), 120.0, at("2026-10-20T10:30:00Z"))];
        let (fresh, _) = fresh_zones(GridFeedKind::Neso, &["GB"], ninety_minutes_ago, now);
        assert_eq!(fresh.len(), 1);
        let ninety_minutes_ago = vec![("DK1".to_string(), 120.0, at("2026-10-20T10:30:00Z"))];
        assert!(matches!(
            only_error(fresh_zones(
                GridFeedKind::Energinet,
                &["DK1"],
                ninety_minutes_ago,
                now
            )),
            FeedError::Stale(90)
        ));
    }

    #[test]
    fn the_api_key_travels_as_a_sensitive_header() {
        let mut cfg = rte_config("https://data.fingrid.fi/api".to_string());
        cfg.kind = GridFeedKind::Fingrid(super::super::config::FingridDataset::Production);
        cfg.api_key = Some("k3y".to_string());
        let headers = request_headers(&cfg).unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name.as_str(), "x-api-key");
        assert!(headers[0].value.is_sensitive());
        cfg.kind = GridFeedKind::OpenElectricity;
        let headers = request_headers(&cfg).unwrap();
        assert_eq!(headers[0].name.as_str(), "authorization");
        assert_eq!(headers[0].value.to_str().unwrap(), "Bearer k3y");
        cfg.api_key = None;
        assert!(request_headers(&cfg).unwrap().is_empty());
    }

    #[test]
    fn esios_and_mavir_get_the_headers_their_answers_depend_on() {
        let mut cfg = rte_config("https://api.esios.ree.es".to_string());
        cfg.kind = GridFeedKind::ReeEsios;
        cfg.api_key = Some("t0k".to_string());
        let headers = request_headers(&cfg).unwrap();
        let names: Vec<&str> = headers.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["x-api-key", "accept"]);
        assert_eq!(headers[1].value.to_str().unwrap(), esios::ACCEPT);
        cfg.kind = GridFeedKind::Mavir;
        cfg.api_key = None;
        let headers = request_headers(&cfg).unwrap();
        assert_eq!(headers[0].name.as_str(), "cookie");
        assert_eq!(headers[0].value.to_str().unwrap(), mavir::COOKIE);
    }

    #[test]
    fn the_neso_url_moves_with_time() {
        let mut cfg = rte_config("https://api.carbonintensity.org.uk".to_string());
        cfg.kind = GridFeedKind::Neso;
        let calls = requests(&cfg, &["GB"], at("2026-10-20T12:59:00Z")).unwrap();
        assert_eq!(
            calls[0].0.path(),
            "/intensity/2026-10-20T09:59Z/2026-10-20T12:59Z"
        );
        assert_eq!(calls[0].1, vec!["GB"]);
    }

    #[test]
    fn open_electricity_calls_only_the_networks_it_serves() {
        let mut cfg = rte_config("https://api.openelectricity.org.au/v4".to_string());
        cfg.kind = GridFeedKind::OpenElectricity;
        let calls = requests(&cfg, &["AU-WA"], at("2026-10-20T13:05:00Z")).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.path(), "/v4/data/network/WEM");
        assert_eq!(calls[0].1, vec!["AU-WA"]);
    }

    #[tokio::test]
    async fn fetch_reads_the_rate_from_the_server() {
        let (base, server) = spawn_one_shot_server(http_200_text(
            "application/json; charset=utf-8",
            rte::tests::BODY,
        ))
        .await;
        let cfg = rte_config(base);
        let client = http_client::build_client();
        let now = at("2026-10-20T09:00:00Z");
        let calls = requests(&cfg, &["FR"], now).unwrap();
        let (fresh, failed) = fetch(&client, &cfg, &[], &calls[0].0, &["FR"], now)
            .await
            .unwrap();
        assert_eq!(fresh, vec![("FR".to_string(), 21.0)]);
        assert!(failed.is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn fetch_surfaces_http_errors() {
        let (base, server) = spawn_one_shot_server(http_status(429, "Too Many Requests")).await;
        let cfg = rte_config(base);
        let client = http_client::build_client();
        let now = at("2026-10-20T09:00:00Z");
        let calls = requests(&cfg, &["FR"], now).unwrap();
        let err = fetch(&client, &cfg, &[], &calls[0].0, &["FR"], now)
            .await
            .unwrap_err();
        assert!(matches!(err, FeedError::Fetch(_)), "got {err:?}");
        server.await.unwrap();
    }
}
