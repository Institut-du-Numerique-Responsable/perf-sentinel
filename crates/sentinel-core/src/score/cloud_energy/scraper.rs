//! Cloud energy scraper: Prometheus JSON API client and energy
//! computation.
//!
//! Queries the Prometheus `/api/v1/query` endpoint once per distinct
//! CPU query, interpolates watts via the `SPECpower` table, and
//! publishes per-service energy-per-op coefficients to the shared
//! [`CloudEnergyState`]. Services reading one single-series query with
//! one power envelope share that node's watts by op count.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::http_client::{self, HttpClient};
use crate::ingest::auth_header::{AuthHeader, ScraperAuthOutcome, parse_scraper_auth_header};
use crate::report::metrics::MetricsState;
use crate::score::ops_snapshot_diff::OpsSnapshotDiff;

use super::config::CloudEnergyConfig;
use super::state::{CloudEnergyState, ServiceEnergy, monotonic_ms};
use super::table;
use crate::score::energy_state::split_by_ops;

/// Number of consecutive scrape failures before a diagnostic warning.
const FAILURE_THRESHOLD: u32 = 3;

// ---------------------------------------------------------------
// Error type
// ---------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub(super) enum CloudScraperError {
    #[error("invalid Prometheus endpoint URI '{endpoint}'")]
    InvalidUri {
        endpoint: String,
        #[source]
        source: hyper::http::uri::InvalidUri,
    },
    /// HTTP fetch failed. Delegates to the shared
    /// [`http_client::FetchError`].
    #[error("Prometheus fetch failed")]
    Fetch(#[source] http_client::FetchError),
    #[error("Prometheus response parse error: {0}")]
    JsonParse(String),
    #[error("Prometheus query returned no result for '{query}'")]
    EmptyResult { query: String },
    #[error("Prometheus query returned error status: {0}")]
    QueryError(String),
}

// ---------------------------------------------------------------
// Prometheus JSON API response types
// ---------------------------------------------------------------

#[derive(serde::Deserialize)]
struct PromResponse {
    status: String,
    data: Option<PromData>,
    error: Option<String>,
}

#[derive(serde::Deserialize)]
struct PromData {
    result: Vec<PromResult>,
}

/// A single Prometheus instant query result.
/// `value` is `[timestamp, "scalar_string"]`.
#[derive(serde::Deserialize)]
struct PromResult {
    value: (f64, String),
}

// ---------------------------------------------------------------
// Fetch helpers
// ---------------------------------------------------------------

/// Fetch a single `PromQL` instant query and return the first series'
/// value with the number of series in the response.
async fn fetch_cpu_percent(
    client: &HttpClient,
    base_uri: &hyper::Uri,
    query: &str,
    auth: Option<&AuthHeader>,
) -> Result<(f64, usize), CloudScraperError> {
    let encoded_query = percent_encode_query(query);
    let path_and_query = format!("/api/v1/query?query={encoded_query}");

    let uri = hyper::Uri::builder()
        .scheme(base_uri.scheme_str().unwrap_or("http"))
        .authority(
            base_uri
                .authority()
                .map_or("localhost:9090", |a| a.as_str()),
        )
        .path_and_query(path_and_query)
        .build()
        .map_err(|e| CloudScraperError::Fetch(http_client::FetchError::RequestBuild(e)))?;

    let bytes = http_client::fetch_get(
        client,
        &uri,
        "perf-sentinel/cloud-energy-scraper",
        Duration::from_secs(5),
        auth,
    )
    .await
    .map_err(CloudScraperError::Fetch)?;

    let body_str = std::str::from_utf8(&bytes).map_err(|_| {
        CloudScraperError::Fetch(http_client::FetchError::BodyRead(
            "Prometheus response was not valid UTF-8".to_string(),
        ))
    })?;

    parse_prom_scalar(body_str, query)
}

/// Parse a Prometheus instant query JSON response and extract the
/// scalar value from the first result, with the number of results.
fn parse_prom_scalar(body: &str, query: &str) -> Result<(f64, usize), CloudScraperError> {
    let resp: PromResponse =
        serde_json::from_str(body).map_err(|e| CloudScraperError::JsonParse(format!("{e}")))?;

    if resp.status != "success" {
        return Err(CloudScraperError::QueryError(
            resp.error.unwrap_or(resp.status),
        ));
    }

    let data = resp.data.ok_or_else(|| CloudScraperError::EmptyResult {
        query: query.to_string(),
    })?;

    let first = data
        .result
        .first()
        .ok_or_else(|| CloudScraperError::EmptyResult {
            query: query.to_string(),
        })?;

    let value = first
        .value
        .1
        .parse::<f64>()
        .map_err(|e| CloudScraperError::JsonParse(format!("value parse: {e}")))?;
    Ok((value, data.result.len()))
}

/// Minimal percent-encoding for `PromQL` query parameters.
/// Encodes characters that are not URL-safe in a query string.
fn percent_encode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for b in input.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'*'
            | b':'
            | b','
            | b'/'
            | b'@' => out.push(b as char),
            _ => {
                out.push('%');
                out.push(HEX_UPPER[(b >> 4) as usize] as char);
                out.push(HEX_UPPER[(b & 0xF) as usize] as char);
            }
        }
    }
    out
}

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

// ---------------------------------------------------------------
// Query groups
// ---------------------------------------------------------------

/// The services reading one resolved CPU query, with their power
/// envelopes `(idle_watts, max_watts)`.
#[derive(Debug)]
struct CloudGroup {
    query: String,
    /// Sorted service names, parallel to `envelopes`.
    services: Vec<String>,
    envelopes: Vec<(f64, f64)>,
    /// `Some` when every member resolves the same envelope.
    envelope: Option<(f64, f64)>,
}

/// A CPU reading for one query: the first series' value and the number
/// of series the response held.
type Reading = (f64, usize);

fn cloud_groups(cfg: &CloudEnergyConfig) -> Vec<CloudGroup> {
    cfg.cpu_query_groups()
        .into_iter()
        .map(|(query, services)| {
            let envelopes: Vec<(f64, f64)> = services
                .iter()
                .map(|name| cfg.service_power(&cfg.services[name]))
                .collect();
            let envelope = envelopes
                .first()
                .copied()
                .filter(|first| envelopes.iter().all(|e| e == first));
            CloudGroup {
                query,
                services,
                envelopes,
                envelope,
            }
        })
        .collect()
}

/// Warn once per query for the daemon's lifetime when a group's query
/// returned several series, debug after. Returns whether this call
/// warned.
fn warn_multi_series(group: &CloudGroup, count: usize, warned: &mut HashSet<String>) -> bool {
    if warned.contains(&group.query) {
        tracing::debug!(
            query = %group.query,
            series = count,
            "Cloud energy: CPU query still returns several series"
        );
        return false;
    }
    tracing::warn!(
        query = %group.query,
        series = count,
        services = ?group.services,
        "Cloud energy: CPU query returned several series, so it names no single node. \
         Each service keeps its own reading of the first series. Give the services of \
         each node a cpu_query filtered to that node so they share its watts"
    );
    warned.insert(group.query.clone());
    true
}

// ---------------------------------------------------------------
// Scraper loop
// ---------------------------------------------------------------

/// Spawn the periodic cloud energy scraper task.
///
/// Returns a `JoinHandle` that the daemon captures and aborts on
/// Ctrl-C shutdown.
#[must_use]
pub fn spawn_cloud_scraper(
    cfg: CloudEnergyConfig,
    state: Arc<CloudEnergyState>,
    metrics: Arc<MetricsState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_cloud_scraper_loop(cfg, state, metrics).await;
    })
}

#[allow(clippy::too_many_lines)] // Scraper orchestration: URI parse, auth parse, ticker, JoinSet fanout. Splitting would fragment the lifecycle without clarity gain.
async fn run_cloud_scraper_loop(
    cfg: CloudEnergyConfig,
    state: Arc<CloudEnergyState>,
    metrics: Arc<MetricsState>,
) {
    use std::str::FromStr;

    let uri = match hyper::Uri::from_str(&cfg.prometheus_endpoint) {
        Ok(u) => u,
        Err(e) => {
            let err = CloudScraperError::InvalidUri {
                endpoint: cfg.prometheus_endpoint.clone(),
                source: e,
            };
            tracing::error!(error = %err, "Cloud energy scraper aborting on invalid endpoint");
            return;
        }
    };
    let redacted = http_client::redact_endpoint(&uri);

    // Parse the optional auth header once at startup. A parse failure
    // logs and aborts the task, because silent retries would only spam
    // warn logs.
    // Option<Arc<_>> so the no-auth path pays zero refcount cost in the
    // per-tick JoinSet fanout below.
    let parsed_auth: Option<Arc<AuthHeader>> = match parse_scraper_auth_header(
        cfg.auth_header.as_deref(),
        &cfg.prometheus_endpoint,
        &redacted,
        "cloud_energy",
    ) {
        ScraperAuthOutcome::Invalid => return,
        ScraperAuthOutcome::None => None,
        ScraperAuthOutcome::Some(h) => Some(Arc::new(h)),
    };

    let client = http_client::build_client();

    let mut ticker = tokio::time::interval(cfg.scrape_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // skip first immediate tick

    let mut snapshot_diff = OpsSnapshotDiff::default();
    let mut first_failure_warned = false;
    let mut consecutive_failures: u32 = 0;
    // Anchors the staleness gauge on the failure branch, mirroring the
    // Scaphandre/Kepler/Redfish scrapers: without it a failing scraper
    // keeps the gauge frozen at its last healthy value.
    let mut last_success_ms = monotonic_ms();
    let mut unreachable_warned = false;

    let groups = cloud_groups(&cfg);
    let interval_secs = cfg.scrape_interval.as_secs_f64();
    let mut multi_series_warned: HashSet<String> = HashSet::new();

    tracing::info!(
        endpoint = %redacted,
        scrape_interval_secs = cfg.scrape_interval.as_secs(),
        services = cfg.services.len(),
        queries = groups.len(),
        "Cloud energy scraper started"
    );

    loop {
        ticker.tick().await;

        let current_ops = metrics.snapshot_service_io_ops();
        let deltas = snapshot_diff.delta_and_advance(current_ops);

        let tick_start = std::time::Instant::now();

        // Query CPU% once per distinct query in parallel via a
        // `JoinSet`, collecting results as they return. A sequential
        // loop costs N × request_time, which at 1024 queries × 20ms
        // exceeds the 15s scrape interval and backs up the ticker. The
        // hyper client holds an Arc internally so `.clone()` is cheap.
        let mut set: tokio::task::JoinSet<(String, Result<Reading, CloudScraperError>)> =
            tokio::task::JoinSet::new();
        for group in &groups {
            let query = group.query.clone();
            let client_clone = client.clone();
            let uri_clone = uri.clone();
            // Option::clone is a no-op on None and one Arc bump on Some.
            let auth_clone = parsed_auth.clone();
            set.spawn(async move {
                let result =
                    fetch_cpu_percent(&client_clone, &uri_clone, &query, auth_clone.as_deref())
                        .await;
                (query, result)
            });
        }

        let cpu_readings = collect_readings(set, groups.len()).await;
        warn_multi_series_groups(&groups, &cpu_readings, &mut multi_series_warned);
        let any_success = !cpu_readings.is_empty();

        if any_success {
            first_failure_warned = false;
            consecutive_failures = 0;
            let now_ms = monotonic_ms();
            apply_cloud_scrape(
                &state,
                &groups,
                &cpu_readings,
                &deltas,
                interval_secs,
                now_ms,
            );
            metrics.cloud_energy_last_scrape_age_seconds.set(0.0);
            last_success_ms = now_ms;
            tracing::debug!(
                readings = cpu_readings.len(),
                elapsed_ms = tick_start.elapsed().as_millis() as u64,
                "Cloud energy scrape succeeded"
            );
        } else if !cfg.services.is_empty() {
            consecutive_failures = consecutive_failures.saturating_add(1);
            // Advance the staleness gauge so hung-scraper alerts (and
            // /api/energy) see the failure, as the Scaphandre and Kepler
            // scrapers do.
            #[allow(clippy::cast_precision_loss)]
            let age_secs = monotonic_ms().saturating_sub(last_success_ms) as f64 / 1000.0;
            metrics.cloud_energy_last_scrape_age_seconds.set(age_secs);
            log_scrape_failure(
                &redacted,
                &mut first_failure_warned,
                &mut unreachable_warned,
                consecutive_failures,
            );
        }
        warn_if_slow(&cfg, tick_start.elapsed());
    }
}

fn log_scrape_failure(
    redacted: &str,
    first_warned: &mut bool,
    unreachable_warned: &mut bool,
    consecutive: u32,
) {
    if *first_warned {
        tracing::debug!("Cloud energy scrape failed for all services");
    } else {
        tracing::warn!(
            endpoint = %redacted,
            "Cloud energy scrape failed for all services; subsequent failures at debug level"
        );
        *first_warned = true;
    }
    if !*unreachable_warned && consecutive >= FAILURE_THRESHOLD {
        tracing::warn!(
            endpoint = %redacted,
            consecutive,
            "Prometheus endpoint unreachable for {FAILURE_THRESHOLD} consecutive scrapes. \
             The daemon is falling back to the proxy model for all cloud-configured services."
        );
        *unreachable_warned = true;
    }
}

fn warn_if_slow(cfg: &CloudEnergyConfig, elapsed: Duration) {
    let warn_threshold = cfg.scrape_interval.mul_f64(0.8);
    if elapsed > warn_threshold {
        tracing::warn!(
            elapsed_ms = elapsed.as_millis() as u64,
            interval_ms = cfg.scrape_interval.as_millis() as u64,
            services = cfg.services.len(),
            "Cloud energy scrape took > 80% of the scrape interval"
        );
    }
}

/// Drain the fetch tasks into one reading per query, finite values only.
async fn collect_readings(
    mut set: tokio::task::JoinSet<(String, Result<Reading, CloudScraperError>)>,
    capacity: usize,
) -> HashMap<String, Reading> {
    let mut readings = HashMap::with_capacity(capacity);
    while let Some(join_result) = set.join_next().await {
        if let Some((query, reading)) = finite_reading(join_result) {
            readings.insert(query, reading);
        }
    }
    readings
}

fn warn_multi_series_groups(
    groups: &[CloudGroup],
    readings: &HashMap<String, Reading>,
    warned: &mut HashSet<String>,
) {
    for group in groups {
        if let Some(&(_, count)) = readings.get(&group.query)
            && count > 1
        {
            warn_multi_series(group, count, warned);
        }
    }
}

/// Keep a finished fetch's reading when it is finite, logging the
/// skipped cases at debug level.
fn finite_reading(
    join_result: Result<(String, Result<Reading, CloudScraperError>), tokio::task::JoinError>,
) -> Option<(String, Reading)> {
    match join_result {
        Ok((query, Ok(reading))) if reading.0.is_finite() => Some((query, reading)),
        Ok((query, Ok((cpu_pct, _)))) => {
            tracing::debug!(
                query = %query,
                value = %cpu_pct,
                "Cloud energy: Prometheus returned non-finite CPU%, skipping"
            );
            None
        }
        Ok((query, Err(e))) => {
            tracing::debug!(
                query = %query,
                error = %e,
                "Cloud energy: failed to fetch CPU% for query"
            );
            None
        }
        Err(e) => {
            // Task panicked or was cancelled. Neither should happen in
            // normal operation but don't bring the scraper loop down.
            tracing::debug!(
                error = %e,
                "Cloud energy: CPU% fetch task panicked or was cancelled"
            );
            None
        }
    }
}

/// Apply CPU% readings to the cloud energy state.
///
/// A group whose reading holds one series and whose services share one
/// envelope is one node: its interpolated watts are split between the
/// services by the ops each emitted. Any other group with a reading
/// prices each service alone on its own envelope. Services with zero
/// ops this tick, and groups without a reading, keep their previous
/// rows.
fn apply_cloud_scrape(
    state: &CloudEnergyState,
    groups: &[CloudGroup],
    cpu_readings: &HashMap<String, Reading>,
    op_deltas: &HashMap<String, u64>,
    interval_secs: f64,
    now_ms: u64,
) {
    let mut next = state.current_owned();
    let mut any_change = false;

    for group in groups {
        let Some(&(cpu_pct, series)) = cpu_readings.get(&group.query) else {
            continue; // no reading for this query this tick
        };
        any_change |= match group.envelope {
            Some(envelope) if series == 1 => apply_group_split(
                &mut next,
                group,
                envelope,
                cpu_pct,
                op_deltas,
                interval_secs,
                now_ms,
            ),
            _ => {
                apply_group_per_service(&mut next, group, cpu_pct, op_deltas, interval_secs, now_ms)
            }
        };
    }

    if any_change {
        state.publish(next);
    }
}

fn apply_group_split(
    next: &mut HashMap<String, ServiceEnergy>,
    group: &CloudGroup,
    (idle_watts, max_watts): (f64, f64),
    cpu_pct: f64,
    op_deltas: &HashMap<String, u64>,
    interval_secs: f64,
    now_ms: u64,
) -> bool {
    let watts = table::interpolate_watts(idle_watts, max_watts, cpu_pct);
    split_by_ops(next, &group.services, op_deltas, now_ms, |ops| {
        table::compute_cloud_energy_per_op_kwh(watts, interval_secs, ops)
    })
}

fn apply_group_per_service(
    next: &mut HashMap<String, ServiceEnergy>,
    group: &CloudGroup,
    cpu_pct: f64,
    op_deltas: &HashMap<String, u64>,
    interval_secs: f64,
    now_ms: u64,
) -> bool {
    let mut changed = false;
    for (service, &(idle_watts, max_watts)) in group.services.iter().zip(&group.envelopes) {
        let watts = table::interpolate_watts(idle_watts, max_watts, cpu_pct);
        changed |= split_by_ops(
            next,
            std::slice::from_ref(service),
            op_deltas,
            now_ms,
            |ops| table::compute_cloud_energy_per_op_kwh(watts, interval_secs, ops),
        );
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::super::config::ServiceCloudConfig;
    use super::*;
    use core::assert_matches;

    // ------------------------------------------------------------------
    // Prometheus JSON parsing
    // ------------------------------------------------------------------

    #[test]
    fn parse_valid_prom_response() {
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1234567890.123,"42.5"]}]}}"#;
        let (val, series) = parse_prom_scalar(body, "test_query").unwrap();
        assert!((val - 42.5).abs() < 1e-10);
        assert_eq!(series, 1);
    }

    #[test]
    fn parse_prom_scalar_counts_series() {
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{"instance":"a"},"value":[1.0,"10"]},{"metric":{"instance":"b"},"value":[1.0,"90"]}]}}"#;
        let (val, series) = parse_prom_scalar(body, "q").unwrap();
        assert!((val - 10.0).abs() < 1e-10, "first series wins");
        assert_eq!(series, 2);
    }

    #[test]
    fn parse_empty_result() {
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#;
        let err = parse_prom_scalar(body, "test_query").unwrap_err();
        assert!(
            matches!(err, CloudScraperError::EmptyResult { .. }),
            "expected EmptyResult, got {err:?}"
        );
    }

    #[test]
    fn parse_error_status() {
        let body = r#"{"status":"error","errorType":"bad_data","error":"invalid query"}"#;
        let err = parse_prom_scalar(body, "test_query").unwrap_err();
        assert_matches!(err, CloudScraperError::QueryError(_));
    }

    #[test]
    fn parse_malformed_json() {
        let err = parse_prom_scalar("not json", "test_query").unwrap_err();
        assert_matches!(err, CloudScraperError::JsonParse(_));
    }

    #[test]
    fn parse_nan_value_returns_nan() {
        // Prometheus returns "NaN" for absent series. parse_prom_scalar
        // returns Ok(NaN), but the scraper loop filters non-finite
        // values before inserting into cpu_readings.
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1234567890.123,"NaN"]}]}}"#;
        let (val, _) = parse_prom_scalar(body, "test_query").unwrap();
        assert!(val.is_nan());
    }

    // ------------------------------------------------------------------
    // percent_encode_query
    // ------------------------------------------------------------------

    #[test]
    fn encode_simple_metric() {
        assert_eq!(
            percent_encode_query("aws_ec2_cpuutilization_average"),
            "aws_ec2_cpuutilization_average"
        );
    }

    #[test]
    fn encode_promql_with_braces_and_quotes() {
        let input = r#"rate(node_cpu{mode="idle"}[5m])"#;
        let encoded = percent_encode_query(input);
        assert!(encoded.contains("%7B")); // {
        assert!(encoded.contains("%7D")); // }
        assert!(encoded.contains("%22")); // "
        assert!(encoded.contains("%5B")); // [
        assert!(encoded.contains("%5D")); // ]
    }

    // ------------------------------------------------------------------
    // apply_cloud_scrape
    // ------------------------------------------------------------------

    const INTERVAL: f64 = 15.0;

    fn manual(idle_watts: f64, max_watts: f64, cpu_query: Option<&str>) -> ServiceCloudConfig {
        ServiceCloudConfig::ManualWatts {
            idle_watts,
            max_watts,
            cpu_query: cpu_query.map(str::to_string),
        }
    }

    fn config_with(services: &[(&str, ServiceCloudConfig)]) -> CloudEnergyConfig {
        CloudEnergyConfig {
            prometheus_endpoint: "http://localhost:9090".into(),
            scrape_interval: Duration::from_secs(15),
            default_provider: None,
            default_instance_type: None,
            cpu_metric: None,
            services: services
                .iter()
                .map(|(name, svc)| ((*name).to_string(), svc.clone()))
                .collect(),
            auth_header: None,
        }
    }

    fn ops(pairs: &[(&str, u64)]) -> HashMap<String, u64> {
        pairs.iter().map(|(s, n)| ((*s).to_string(), *n)).collect()
    }

    /// Every group's query reads `cpu` with `series` series.
    fn readings(groups: &[CloudGroup], cpu: f64, series: usize) -> HashMap<String, Reading> {
        groups
            .iter()
            .map(|g| (g.query.clone(), (cpu, series)))
            .collect()
    }

    /// Run one scrape and return the published coefficients.
    fn scrape(
        cfg: &CloudEnergyConfig,
        state: &CloudEnergyState,
        cpu: f64,
        series: usize,
        op_deltas: &HashMap<String, u64>,
    ) -> HashMap<String, f64> {
        let groups = cloud_groups(cfg);
        apply_cloud_scrape(
            state,
            &groups,
            &readings(&groups, cpu, series),
            op_deltas,
            INTERVAL,
            100,
        );
        state.snapshot(200, 500)
    }

    fn watts_of(e: f64, ops: u64) -> f64 {
        e * ops as f64 * 3.6e6 / INTERVAL
    }

    fn per_op(watts: f64, ops: u64) -> f64 {
        table::compute_cloud_energy_per_op_kwh(watts, INTERVAL, ops).unwrap()
    }

    #[test]
    fn services_on_one_query_split_the_node() {
        let cfg = config_with(&[
            ("a", manual(10.0, 50.0, Some("node_cpu"))),
            ("b", manual(10.0, 50.0, Some("node_cpu"))),
        ]);
        let state = CloudEnergyState::new();
        let snap = scrape(&cfg, &state, 50.0, 1, &ops(&[("a", 100), ("b", 300)]));
        // 50% on 10/50 W is 30 W for the node, once.
        let total = watts_of(snap["a"], 100) + watts_of(snap["b"], 300);
        assert!((total - 30.0).abs() <= 30.0 * 1e-9, "node watts {total}");
        assert!((snap["a"] - snap["b"]).abs() < 1e-24, "one coefficient");
    }

    #[test]
    fn single_service_group_matches_previous_formula() {
        let cfg = config_with(&[("svc-a", manual(2.0, 20.0, None))]);
        let state = CloudEnergyState::new();
        let snap = scrape(&cfg, &state, 50.0, 1, &ops(&[("svc-a", 1000)]));
        assert_eq!(snap.len(), 1);
        // 50% CPU on 2/20W = 11W. energy = (11/1000) * (15/3600) / 1000
        let expected = (11.0 / 1000.0) * (15.0 / 3600.0) / 1000.0;
        assert!(
            (snap["svc-a"] - expected).abs() < 1e-15,
            "expected {expected}, got {}",
            snap["svc-a"]
        );
    }

    #[test]
    fn distinct_queries_do_not_split() {
        let cfg = config_with(&[
            ("a", manual(10.0, 50.0, Some("cpu{node=\"1\"}"))),
            ("b", manual(10.0, 50.0, Some("cpu{node=\"2\"}"))),
        ]);
        let state = CloudEnergyState::new();
        let snap = scrape(&cfg, &state, 50.0, 1, &ops(&[("a", 100), ("b", 300)]));
        assert!((snap["a"] - per_op(30.0, 100)).abs() < 1e-24);
        assert!((snap["b"] - per_op(30.0, 300)).abs() < 1e-24);
    }

    #[test]
    fn multi_series_reading_keeps_per_service_attribution() {
        let cfg = config_with(&[
            ("a", manual(10.0, 50.0, None)),
            ("b", manual(10.0, 50.0, None)),
        ]);
        let state = CloudEnergyState::new();
        let snap = scrape(&cfg, &state, 50.0, 2, &ops(&[("a", 100), ("b", 300)]));
        assert!((snap["a"] - per_op(30.0, 100)).abs() < 1e-24);
        assert!((snap["b"] - per_op(30.0, 300)).abs() < 1e-24);
    }

    #[test]
    fn mixed_envelopes_keep_per_service_attribution() {
        let cfg = config_with(&[
            ("a", manual(10.0, 50.0, None)),
            ("b", manual(20.0, 80.0, None)),
        ]);
        let groups = cloud_groups(&cfg);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].envelope, None);
        let state = CloudEnergyState::new();
        let snap = scrape(&cfg, &state, 50.0, 1, &ops(&[("a", 100), ("b", 300)]));
        assert!((snap["a"] - per_op(30.0, 100)).abs() < 1e-24);
        assert!((snap["b"] - per_op(50.0, 300)).abs() < 1e-24);
    }

    #[test]
    fn zero_ops_member_keeps_previous_row() {
        let cfg = config_with(&[
            ("a", manual(10.0, 50.0, None)),
            ("b", manual(10.0, 50.0, None)),
        ]);
        let state = CloudEnergyState::new();
        state.insert_for_test("b".into(), 1e-7, 150);
        let snap = scrape(&cfg, &state, 50.0, 1, &ops(&[("a", 100), ("b", 0)]));
        // The busy member takes the whole node, the idle one keeps its row.
        assert!((snap["a"] - per_op(30.0, 100)).abs() < 1e-24);
        assert!((snap["b"] - 1e-7).abs() < 1e-20);
    }

    #[test]
    fn group_without_reading_keeps_all_rows() {
        let cfg = config_with(&[
            ("a", manual(10.0, 50.0, None)),
            ("b", manual(10.0, 50.0, None)),
        ]);
        let state = CloudEnergyState::new();
        state.insert_for_test("a".into(), 1e-7, 150);
        state.insert_for_test("b".into(), 2e-7, 150);
        let groups = cloud_groups(&cfg);
        apply_cloud_scrape(
            &state,
            &groups,
            &HashMap::new(),
            &ops(&[("a", 100), ("b", 300)]),
            INTERVAL,
            100,
        );
        let snap = state.snapshot(200, 500);
        assert!((snap["a"] - 1e-7).abs() < 1e-20);
        assert!((snap["b"] - 2e-7).abs() < 1e-20);
    }

    #[test]
    fn service_without_ops_delta_is_not_inserted() {
        let cfg = config_with(&[("svc-a", manual(2.0, 20.0, None))]);
        let state = CloudEnergyState::new();
        assert!(scrape(&cfg, &state, 50.0, 1, &HashMap::new()).is_empty());
        assert!(scrape(&cfg, &state, 50.0, 1, &ops(&[("svc-a", 0)])).is_empty());
    }

    #[test]
    fn warn_multi_series_latches_once_per_query() {
        let cfg = config_with(&[("a", manual(10.0, 50.0, None))]);
        let groups = cloud_groups(&cfg);
        let mut warned = HashSet::new();
        assert!(warn_multi_series(&groups[0], 2, &mut warned));
        assert!(!warn_multi_series(&groups[0], 3, &mut warned));
        assert!(warned.contains(&groups[0].query));
    }

    // ------------------------------------------------------------------
    // warn_if_slow
    // ------------------------------------------------------------------

    #[test]
    fn warn_if_slow_does_not_panic_when_elapsed_under_threshold() {
        let cfg = CloudEnergyConfig {
            prometheus_endpoint: "http://localhost:9090".into(),
            scrape_interval: Duration::from_secs(15),
            default_provider: None,
            default_instance_type: None,
            cpu_metric: None,
            services: HashMap::new(),
            auth_header: None,
        };
        warn_if_slow(&cfg, Duration::from_millis(100));
    }

    #[test]
    fn warn_if_slow_does_not_panic_when_elapsed_exceeds_threshold() {
        // 13s > 15s * 0.8 = 12s, so it emits a tracing::warn but must not panic.
        let cfg = CloudEnergyConfig {
            prometheus_endpoint: "http://localhost:9090".into(),
            scrape_interval: Duration::from_secs(15),
            default_provider: None,
            default_instance_type: None,
            cpu_metric: None,
            services: HashMap::new(),
            auth_header: None,
        };
        warn_if_slow(&cfg, Duration::from_secs(13));
    }

    // ------------------------------------------------------------------
    // log_scrape_failure
    // ------------------------------------------------------------------

    #[test]
    fn log_scrape_failure_first_invocation_flips_first_warned() {
        let mut first_warned = false;
        let mut unreachable_warned = false;
        log_scrape_failure("http://fake", &mut first_warned, &mut unreachable_warned, 1);
        assert!(first_warned, "first invocation must flip first_warned");
        assert!(!unreachable_warned, "unreachable threshold not reached yet");
    }

    #[test]
    fn log_scrape_failure_threshold_sets_unreachable() {
        let mut first_warned = true;
        let mut unreachable_warned = false;
        log_scrape_failure(
            "http://fake",
            &mut first_warned,
            &mut unreachable_warned,
            FAILURE_THRESHOLD,
        );
        assert!(
            unreachable_warned,
            "threshold reached must flip unreachable_warned"
        );
    }

    #[test]
    fn log_scrape_failure_is_idempotent_after_unreachable_flagged() {
        let mut first_warned = true;
        let mut unreachable_warned = true;
        log_scrape_failure(
            "http://fake",
            &mut first_warned,
            &mut unreachable_warned,
            10,
        );
        // Both flags stay true, no panic.
        assert!(first_warned && unreachable_warned);
    }

    // ------------------------------------------------------------------
    // Integration tests with a mock HTTP server on an ephemeral port
    // ------------------------------------------------------------------
    //
    // Shared helpers from `crate::test_helpers`, same pattern used by
    // scaphandre, electricity_maps, and tempo tests.

    use crate::test_helpers::{http_200_text, http_status, spawn_one_shot_server};

    /// Wrap the shared `http_200_text` with the JSON content type.
    fn http_200_json(body: &str) -> Vec<u8> {
        http_200_text("application/json", body)
    }

    #[tokio::test]
    async fn fetch_cpu_percent_happy_path() {
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1234567890.0,"42.5"]}]}}"#;
        let (endpoint, server) = spawn_one_shot_server(http_200_json(body)).await;

        let client = http_client::build_client();
        let uri = <hyper::Uri as std::str::FromStr>::from_str(&endpoint).unwrap();
        let val = fetch_cpu_percent(&client, &uri, "test_query", None)
            .await
            .expect("valid response should parse");
        assert_eq!(val.1, 1);
        assert!((val.0 - 42.5).abs() < 1e-10);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn fetch_cpu_percent_non_200_surfaces_http_status() {
        let (endpoint, server) =
            spawn_one_shot_server(http_status(503, "Service Unavailable")).await;

        let client = http_client::build_client();
        let uri = <hyper::Uri as std::str::FromStr>::from_str(&endpoint).unwrap();
        let err = fetch_cpu_percent(&client, &uri, "q", None)
            .await
            .expect_err("503 must error");
        match err {
            CloudScraperError::Fetch(http_client::FetchError::HttpStatus(503)) => {}
            other => panic!("expected Fetch(HttpStatus(503)), got {other:?}"),
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn fetch_cpu_percent_malformed_json_surfaces_json_parse() {
        let (endpoint, server) = spawn_one_shot_server(http_200_json("not json")).await;

        let client = http_client::build_client();
        let uri = <hyper::Uri as std::str::FromStr>::from_str(&endpoint).unwrap();
        let err = fetch_cpu_percent(&client, &uri, "q", None)
            .await
            .expect_err("malformed JSON must error");
        assert_matches!(err, CloudScraperError::JsonParse(_));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn fetch_cpu_percent_empty_result_surfaces_empty_result_error() {
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#;
        let (endpoint, server) = spawn_one_shot_server(http_200_json(body)).await;

        let client = http_client::build_client();
        let uri = <hyper::Uri as std::str::FromStr>::from_str(&endpoint).unwrap();
        let err = fetch_cpu_percent(&client, &uri, "query-alpha", None)
            .await
            .expect_err("empty result must error");
        match err {
            CloudScraperError::EmptyResult { query } => {
                assert_eq!(query, "query-alpha");
            }
            other => panic!("expected EmptyResult, got {other:?}"),
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn spawn_cloud_scraper_invalid_endpoint_exits_cleanly() {
        // `from_str` on the invalid URI returns Err, which makes
        // run_cloud_scraper_loop emit an error and return immediately.
        // The spawned task should complete without panic.
        let cfg = CloudEnergyConfig {
            prometheus_endpoint: "not a uri :: bad".into(),
            scrape_interval: Duration::from_millis(50),
            default_provider: None,
            default_instance_type: None,
            cpu_metric: None,
            services: HashMap::new(),
            auth_header: None,
        };
        let state = CloudEnergyState::new();
        let metrics = Arc::new(MetricsState::new());
        let handle = spawn_cloud_scraper(cfg, state, metrics);
        // The task returns almost immediately on the InvalidUri branch.
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("invalid URI task must exit within 2s")
            .expect("task must not panic");
    }

    #[tokio::test]
    async fn spawn_cloud_scraper_empty_services_never_fails() {
        // With no services configured, each tick runs to completion
        // without querying anything. `any_success == false` but the
        // `!cfg.services.is_empty()` guard skips `log_scrape_failure`.
        // Verify the task stays alive and we can abort cleanly.
        let cfg = CloudEnergyConfig {
            prometheus_endpoint: "http://127.0.0.1:1".into(), // unreachable
            scrape_interval: Duration::from_millis(50),
            default_provider: None,
            default_instance_type: None,
            cpu_metric: None,
            services: HashMap::new(),
            auth_header: None,
        };
        let state = CloudEnergyState::new();
        let metrics = Arc::new(MetricsState::new());
        let handle = spawn_cloud_scraper(cfg, state, metrics);
        // Let a couple of ticks fire.
        tokio::time::sleep(Duration::from_millis(180)).await;
        handle.abort();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn spawn_cloud_scraper_unreachable_endpoint_keeps_running() {
        // Configured services but the endpoint refuses connections.
        // The scraper must stay alive, go through log_scrape_failure
        // and eventually bump consecutive_failures past the threshold.
        let mut services = HashMap::new();
        services.insert(
            "svc-a".to_string(),
            ServiceCloudConfig::ManualWatts {
                idle_watts: 2.0,
                max_watts: 20.0,
                cpu_query: None,
            },
        );
        let cfg = CloudEnergyConfig {
            prometheus_endpoint: "http://127.0.0.1:1".into(),
            scrape_interval: Duration::from_millis(30),
            default_provider: None,
            default_instance_type: None,
            cpu_metric: None,
            services,
            auth_header: None,
        };
        let state = CloudEnergyState::new();
        let metrics = Arc::new(MetricsState::new());
        let handle = spawn_cloud_scraper(cfg, state.clone(), metrics.clone());
        // Enough ticks to step past the FAILURE_THRESHOLD.
        tokio::time::sleep(Duration::from_millis(220)).await;
        handle.abort();
        let _ = handle.await;

        // State stays empty since nothing ever succeeded.
        assert!(state.snapshot(monotonic_ms(), 5_000).is_empty());
        // The staleness gauge must advance on the failure branch, not
        // stay frozen at its pre-registered 0.
        assert!(
            metrics.cloud_energy_last_scrape_age_seconds.get() > 0.0,
            "failed scrapes must advance the staleness gauge"
        );
    }

    #[test]
    fn cloud_scraper_error_display_messages_are_informative() {
        use crate::http_client::FetchError;
        let e1 = CloudScraperError::Fetch(FetchError::BodyRead("oops".to_string()));
        let e2 = CloudScraperError::Fetch(FetchError::HttpStatus(503));
        let e3 = CloudScraperError::Fetch(FetchError::Timeout);
        let e4 = CloudScraperError::JsonParse("bad".to_string());
        let e5 = CloudScraperError::EmptyResult {
            query: "q".to_string(),
        };
        let e6 = CloudScraperError::QueryError("err".to_string());
        assert!(format!("{e1}").contains("fetch failed"));
        assert!(format!("{e2}").contains("fetch failed"));
        assert!(format!("{e3}").contains("fetch failed"));
        assert!(format!("{e4}").contains("parse"));
        assert!(format!("{e5}").contains("no result"));
        assert!(format!("{e6}").contains("error status"));
    }

    /// End-to-end check that a configured `auth_header` lands on the
    /// Prometheus request wire when `fetch_cpu_percent` is invoked.
    /// Mirrors the shape of `search_sends_auth_header_on_wire` in
    /// `jaeger_query.rs`.
    #[tokio::test]
    async fn cloud_scraper_sends_auth_header_on_wire() {
        use crate::ingest::auth_header::AuthHeader;

        let body = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1234567890.0,"42.5"]}]}}"#;
        let response = http_200_json(body);
        let (endpoint, mut rx, server) = crate::test_helpers::spawn_capture_server(response).await;

        let client = http_client::build_client();
        let base_uri = <hyper::Uri as std::str::FromStr>::from_str(&endpoint).unwrap();
        let auth = AuthHeader::parse("Authorization: Bearer topsecret").expect("valid");
        let val = fetch_cpu_percent(&client, &base_uri, "up", Some(&auth))
            .await
            .expect("fetch_cpu_percent must succeed");
        assert!((val.0 - 42.5).abs() < 1e-10);

        let captured = rx.recv().await.expect("captured request");
        let text = std::str::from_utf8(&captured).expect("utf8");
        assert!(
            text.contains("authorization: Bearer topsecret")
                || text.contains("Authorization: Bearer topsecret"),
            "auth header missing from request, got:\n{text}"
        );
        assert!(
            text.contains("/api/v1/query?query=up"),
            "PromQL query path missing, got:\n{text}"
        );
        server.await.expect("server join");
    }
}
