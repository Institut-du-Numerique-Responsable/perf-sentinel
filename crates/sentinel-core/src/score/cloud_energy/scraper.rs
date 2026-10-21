//! Cloud energy scraper: Prometheus JSON API client and energy
//! computation.
//!
//! Queries the Prometheus `/api/v1/query` endpoint once per distinct
//! CPU query, interpolates watts via the `SPECpower` table, and
//! publishes per-service energy-per-op coefficients to the shared
//! [`CloudEnergyState`]. Services reading one single-series query with
//! one power envelope share that node's watts by op count. A declared
//! database or broker with a `cpu_query` is a [`HostProbe`], billed into
//! its own state on the same ticks. A shared one's watts are taken out of
//! the node it runs on before the split.

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
use crate::score::alumet::DbEnergyState;
use crate::score::energy_state::split_by_ops;
use crate::score::host_static::{HostPlacement, StaticHostsConfig, Subsystem};

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
    /// A host CPU query must name one reading: the average CPU of the
    /// declared hosts.
    #[error("host CPU query '{query}' returned {count} series, expected one")]
    MultipleSeries { query: String, count: usize },
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
// Host CPU probes
// ---------------------------------------------------------------

/// The CPU query of a declared database or broker, priced on a
/// `SPECpower` envelope and billed into `state` each tick.
#[derive(Debug)]
pub struct HostProbe {
    pub subsystem: Subsystem,
    pub placement: HostPlacement,
    /// Trimmed at load.
    pub query: String,
    pub nodes: u32,
    pub idle_watts: f64,
    pub max_watts: f64,
    /// A shared host's node: the CPU query its listed group reads, the
    /// key its watts are deducted under. `None` otherwise.
    pub group_key: Option<String>,
    /// Read by the event loop as the subsystem's live source.
    pub state: Arc<DbEnergyState>,
    last_success_ms: u64,
    warned: bool,
}

impl HostProbe {
    /// A probe for a dedicated declaration with a `cpu_query`, `nodes`
    /// and an instance type that resolves to an envelope. `None` for any
    /// other declaration.
    #[must_use]
    pub fn dedicated(subsystem: Subsystem, cfg: &StaticHostsConfig) -> Option<Self> {
        if cfg.placement != HostPlacement::Dedicated {
            return None;
        }
        let envelope = cfg.node_power()?;
        let query = cfg.cpu_query.clone()?;
        Some(Self::new(
            subsystem,
            cfg.placement,
            query,
            cfg.nodes?,
            envelope,
            None,
        ))
    }

    /// A probe for a shared declaration: one node, priced on the envelope
    /// of the group it lists (one envelope, D7/B7), else on its instance
    /// type. `None` for any other declaration.
    #[must_use]
    pub fn shared(
        subsystem: Subsystem,
        cfg: &StaticHostsConfig,
        cloud: &CloudEnergyConfig,
    ) -> Option<Self> {
        if cfg.placement != HostPlacement::Shared {
            return None;
        }
        // Every listed service resolves the group's query (D7/B7).
        let listed = cfg
            .shared_with
            .first()
            .and_then(|name| cloud.services.get(name));
        let envelope = listed
            .map(|svc| cloud.service_power(svc))
            .or_else(|| cfg.node_power())?;
        let group_key = listed.map(|svc| cloud.service_cpu_query(svc).into_owned());
        let query = cfg.cpu_query.clone()?;
        Some(Self::new(
            subsystem,
            cfg.placement,
            query,
            1,
            envelope,
            group_key,
        ))
    }

    fn new(
        subsystem: Subsystem,
        placement: HostPlacement,
        query: String,
        nodes: u32,
        (idle_watts, max_watts): (f64, f64),
        group_key: Option<String>,
    ) -> Self {
        Self {
            subsystem,
            placement,
            query,
            nodes,
            idle_watts,
            max_watts,
            group_key,
            state: DbEnergyState::new(),
            last_success_ms: monotonic_ms(),
            warned: false,
        }
    }

    /// Watts above idle at `cpu` percent, clamped like the interpolation.
    fn dynamic_watts(&self, cpu: f64) -> f64 {
        table::interpolate_watts(self.idle_watts, self.max_watts, cpu) - self.idle_watts
    }

    fn age_gauge<'m>(&self, metrics: &'m MetricsState) -> &'m prometheus::Gauge {
        match self.subsystem {
            Subsystem::Database => &metrics.database_cpu_last_scrape_age_seconds,
            Subsystem::Broker => &metrics.broker_cpu_last_scrape_age_seconds,
        }
    }

    /// Advance the age gauge and warn on the first failure of a run of
    /// failures, debug after.
    fn record_failure(&mut self, metrics: &MetricsState, reason: &str) {
        #[allow(clippy::cast_precision_loss)]
        let age_secs = monotonic_ms().saturating_sub(self.last_success_ms) as f64 / 1000.0;
        self.age_gauge(metrics).set(age_secs);
        if self.warned {
            tracing::debug!(query = %self.query, reason, "Host CPU scrape failed");
            return;
        }
        tracing::warn!(
            subsystem = self.subsystem.noun(),
            query = %self.query,
            reason,
            "Host CPU scrape failed, so no host energy is billed from it this tick. \
             The query must return one series, the average CPU of the declared hosts. \
             Subsequent failures at debug level"
        );
        self.warned = true;
    }
}

/// Fetch one probe's CPU percent. A response with several series, an
/// error or a non-finite value is a failure.
async fn fetch_host_cpu(
    probe: &mut HostProbe,
    client: &HttpClient,
    uri: &hyper::Uri,
    auth: Option<&AuthHeader>,
    metrics: &MetricsState,
) -> Option<f64> {
    let fetched = fetch_cpu_percent(client, uri, &probe.query, auth).await;
    match single_series(fetched, &probe.query) {
        Ok(cpu) if cpu.is_finite() => {
            probe.last_success_ms = monotonic_ms();
            probe.warned = false;
            probe.age_gauge(metrics).set(0.0);
            Some(cpu)
        }
        Ok(cpu) => {
            probe.record_failure(metrics, &format!("non-finite CPU% {cpu}"));
            None
        }
        Err(e) => {
            probe.record_failure(metrics, &e.to_string());
            None
        }
    }
}

/// A host reading is one series: several fail with
/// [`CloudScraperError::MultipleSeries`].
fn single_series(
    fetched: Result<Reading, CloudScraperError>,
    query: &str,
) -> Result<f64, CloudScraperError> {
    match fetched {
        Ok((_, count)) if count > 1 => Err(CloudScraperError::MultipleSeries {
            query: query.to_string(),
            count,
        }),
        other => other.map(|(cpu, _)| cpu),
    }
}

/// Fetch every probe in order, one reading per probe.
async fn fetch_host_probes(
    probes: &mut [HostProbe],
    client: &HttpClient,
    uri: &hyper::Uri,
    auth: Option<&AuthHeader>,
    metrics: &MetricsState,
) -> Vec<Option<f64>> {
    let mut readings = Vec::with_capacity(probes.len());
    for probe in probes {
        readings.push(fetch_host_cpu(probe, client, uri, auth, metrics).await);
    }
    readings
}

/// Bill each probe with a reading over the interval, in probe order
/// (database first), and return the watts to take out of each listed
/// group's node, keyed by its query.
///
/// A dedicated probe bills `nodes × interpolate(idle, max, cpu)`. A shared
/// probe bills its dynamic share `(max - idle) × cpu / 100`: with an empty
/// `shared_with` nothing is deducted, otherwise it needs a one-series
/// reading of its group and is capped at what is left of the node's
/// dynamic watts after the probes before it.
fn bill_host_probes(
    probes: &[HostProbe],
    probe_cpu: &[Option<f64>],
    readings: &HashMap<String, Reading>,
    interval_secs: f64,
    now_ms: u64,
) -> HashMap<String, f64> {
    let mut deductions: HashMap<String, f64> = HashMap::new();
    for (probe, cpu) in probes.iter().zip(probe_cpu) {
        let Some(cpu) = *cpu else { continue };
        let Some(watts) = probe_watts(probe, cpu, readings, &mut deductions) else {
            continue;
        };
        probe
            .state
            .add_window_kwh(watts * interval_secs / 3_600_000.0, now_ms);
    }
    deductions
}

/// One probe's watts this tick, recording a listed shared probe's watts
/// in `deductions`. `None` when a listed group has no one-node reading.
fn probe_watts(
    probe: &HostProbe,
    cpu: f64,
    readings: &HashMap<String, Reading>,
    deductions: &mut HashMap<String, f64>,
) -> Option<f64> {
    if probe.placement == HostPlacement::Dedicated {
        return Some(
            f64::from(probe.nodes)
                * table::interpolate_watts(probe.idle_watts, probe.max_watts, cpu),
        );
    }
    let Some(group) = &probe.group_key else {
        return Some(probe.dynamic_watts(cpu));
    };
    let &(group_cpu, _) = readings.get(group).filter(|&&(_, series)| series == 1)?;
    let taken = deductions.entry(group.clone()).or_insert(0.0);
    let watts = probe
        .dynamic_watts(cpu)
        .min(probe.dynamic_watts(group_cpu) - *taken);
    *taken += watts;
    Some(watts)
}

// ---------------------------------------------------------------
// Scraper loop
// ---------------------------------------------------------------

/// Spawn the periodic cloud energy scraper task. `probes` are the host
/// CPU probes, database first, fetched and billed after the service
/// queries on every tick.
///
/// Returns a `JoinHandle` that the daemon captures and aborts on
/// Ctrl-C shutdown.
#[must_use]
pub fn spawn_cloud_scraper(
    cfg: CloudEnergyConfig,
    state: Arc<CloudEnergyState>,
    probes: Vec<HostProbe>,
    metrics: Arc<MetricsState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_cloud_scraper_loop(cfg, state, probes, metrics).await;
    })
}

#[allow(clippy::too_many_lines)] // Scraper orchestration: URI parse, auth parse, ticker, JoinSet fanout. Splitting would fragment the lifecycle without clarity gain.
async fn run_cloud_scraper_loop(
    cfg: CloudEnergyConfig,
    state: Arc<CloudEnergyState>,
    mut probes: Vec<HostProbe>,
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
        host_probes = probes.len(),
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
        let probe_cpu =
            fetch_host_probes(&mut probes, &client, &uri, parsed_auth.as_deref(), &metrics).await;
        let now_ms = monotonic_ms();
        // Whether or not a service reading landed: the hosts are billed on
        // their own query, and a shared one's watts leave its node first.
        let deductions =
            bill_host_probes(&probes, &probe_cpu, &cpu_readings, interval_secs, now_ms);

        if any_success {
            first_failure_warned = false;
            consecutive_failures = 0;
            apply_cloud_scrape(
                &state,
                &groups,
                &cpu_readings,
                &deltas,
                interval_secs,
                now_ms,
                &deductions,
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
/// envelope is one node: its interpolated watts, less the shared hosts'
/// `deductions` on it (never below idle), are split between the services
/// by the ops each emitted. Any other group with a reading prices each
/// service alone on its own envelope and takes no deduction. Services
/// with zero ops this tick, and groups without a reading, keep their
/// previous rows.
fn apply_cloud_scrape(
    state: &CloudEnergyState,
    groups: &[CloudGroup],
    cpu_readings: &HashMap<String, Reading>,
    op_deltas: &HashMap<String, u64>,
    interval_secs: f64,
    now_ms: u64,
    deductions: &HashMap<String, f64>,
) {
    let mut next = state.current_owned();
    let mut any_change = false;

    for group in groups {
        let Some(&(cpu_pct, series)) = cpu_readings.get(&group.query) else {
            continue; // no reading for this query this tick
        };
        any_change |= match group.envelope {
            Some((idle_watts, max_watts)) if series == 1 => {
                let hosts = deductions.get(&group.query).copied().unwrap_or(0.0);
                let watts = (table::interpolate_watts(idle_watts, max_watts, cpu_pct) - hosts)
                    .max(idle_watts);
                apply_group_split(&mut next, group, watts, op_deltas, interval_secs, now_ms)
            }
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
    watts: f64,
    op_deltas: &HashMap<String, u64>,
    interval_secs: f64,
    now_ms: u64,
) -> bool {
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
            &HashMap::new(),
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
            &HashMap::new(),
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
    // Host CPU probes
    // ------------------------------------------------------------------

    fn probe(nodes: u32, idle_watts: f64, max_watts: f64) -> HostProbe {
        let mut probe = HostProbe::new(
            Subsystem::Database,
            HostPlacement::Dedicated,
            "avg(db_cpu)".to_string(),
            nodes,
            (idle_watts, max_watts),
            None,
        );
        // Last success at boot, so a failure shows a positive age.
        probe.last_success_ms = 0;
        probe
    }

    /// A shared probe on 10/50 W, listing `group` when given.
    fn shared_probe(subsystem: Subsystem, group: Option<&str>) -> HostProbe {
        HostProbe::new(
            subsystem,
            HostPlacement::Shared,
            "host_cpu".to_string(),
            1,
            (10.0, 50.0),
            group.map(str::to_string),
        )
    }

    /// No group reading, for the probes that need none.
    fn no_readings() -> HashMap<String, Reading> {
        HashMap::new()
    }

    fn hosts(placement: HostPlacement, cpu_query: Option<&str>) -> StaticHostsConfig {
        StaticHostsConfig {
            placement,
            nodes: Some(2),
            instance_type: Some("m5.large".to_string()),
            region: None,
            cpu_query: cpu_query.map(str::to_string),
            shared_with: Vec::new(),
        }
    }

    #[test]
    fn a_probe_is_built_for_a_dedicated_cpu_query_only() {
        let p = HostProbe::dedicated(
            Subsystem::Broker,
            &hosts(HostPlacement::Dedicated, Some("avg(cpu)")),
        )
        .expect("dedicated with cpu_query");
        let envelope = table::lookup_instance_power("m5.large", "generic");
        assert_eq!((p.idle_watts, p.max_watts), envelope);
        assert_eq!((p.nodes, p.query.as_str()), (2, "avg(cpu)"));
        let without = hosts(HostPlacement::Dedicated, None);
        assert!(HostProbe::dedicated(Subsystem::Broker, &without).is_none());
        let shared = hosts(HostPlacement::Shared, Some("avg(cpu)"));
        assert!(HostProbe::dedicated(Subsystem::Broker, &shared).is_none());
    }

    #[test]
    fn a_dedicated_probe_bills_nodes_times_the_interpolated_watts() {
        let probes = [probe(2, 10.0, 50.0)];
        bill_host_probes(&probes, &[Some(25.0)], &no_readings(), INTERVAL, 1_000);
        // 25% on 10/50 W is 20 W per node, 40 W for two.
        let kwh = probes[0].state.take_window_kwh(1_000, 45_000).unwrap();
        let expected = 40.0 * INTERVAL / 3.6e6;
        assert!((kwh - expected).abs() <= expected * 1e-12, "{kwh} kWh");
        assert!(probes[0].state.has_recent_sample(1_000, 45_000));
    }

    #[test]
    fn a_probe_without_reading_bills_nothing() {
        let probes = [probe(2, 10.0, 50.0)];
        bill_host_probes(&probes, &[None], &no_readings(), INTERVAL, 1_000);
        assert_eq!(probes[0].state.take_window_kwh(1_000, 45_000), None);
        assert!(!probes[0].state.has_recent_sample(1_000, 45_000));
    }

    #[test]
    fn a_host_reading_must_hold_one_series() {
        assert_matches!(
            single_series(Ok((25.0, 2)), "q"),
            Err(CloudScraperError::MultipleSeries { count: 2, .. })
        );
        assert!((single_series(Ok((25.0, 1)), "q").unwrap() - 25.0).abs() < 1e-12);
    }

    #[test]
    fn a_dedicated_probe_leaves_the_application_rows_alone() {
        let cfg = config_with(&[
            ("a", manual(10.0, 50.0, Some("node_cpu"))),
            ("b", manual(10.0, 50.0, Some("node_cpu"))),
        ]);
        let state = CloudEnergyState::new();
        let snap = scrape(&cfg, &state, 50.0, 1, &ops(&[("a", 100), ("b", 300)]));
        let deductions = bill_host_probes(
            &[probe(2, 10.0, 50.0)],
            &[Some(90.0)],
            &no_readings(),
            INTERVAL,
            100,
        );
        assert!(deductions.is_empty(), "a dedicated host deducts nothing");
        assert_eq!(state.snapshot(200, 500), snap);
        let total = watts_of(snap["a"], 100) + watts_of(snap["b"], 300);
        assert!((total - 30.0).abs() <= 30.0 * 1e-9, "node watts {total}");
    }

    // ------------------------------------------------------------------
    // Shared host CPU probes
    // ------------------------------------------------------------------

    /// Two services on one 10/50 W node read by `node_cpu`.
    fn node_ab() -> CloudEnergyConfig {
        config_with(&[
            ("a", manual(10.0, 50.0, Some("node_cpu"))),
            ("b", manual(10.0, 50.0, Some("node_cpu"))),
        ])
    }

    /// Bill the probes, then apply the node reading with their
    /// deductions, and return the published coefficients.
    fn host_tick(
        state: &CloudEnergyState,
        probes: &[HostProbe],
        probe_cpu: &[Option<f64>],
        node: Option<Reading>,
        op_deltas: &HashMap<String, u64>,
    ) -> HashMap<String, f64> {
        let groups = cloud_groups(&node_ab());
        let readings: HashMap<String, Reading> = node
            .map(|r| ("node_cpu".to_string(), r))
            .into_iter()
            .collect();
        let deductions = bill_host_probes(probes, probe_cpu, &readings, INTERVAL, 100);
        apply_cloud_scrape(
            state,
            &groups,
            &readings,
            op_deltas,
            INTERVAL,
            100,
            &deductions,
        );
        state.snapshot(200, 500)
    }

    fn billed_watts(probe: &HostProbe) -> Option<f64> {
        probe
            .state
            .take_window_kwh(100, 45_000)
            .map(|kwh| kwh * 3.6e6 / INTERVAL)
    }

    fn published_watts(snap: &HashMap<String, f64>) -> f64 {
        watts_of(snap["a"], 100) + watts_of(snap["b"], 300)
    }

    fn close(actual: f64, expected: f64) -> bool {
        (actual - expected).abs() <= expected.abs().max(1.0) * 1e-9
    }

    #[test]
    fn shared_subtracts_exactly_what_it_bills() {
        let state = CloudEnergyState::new();
        let probes = [shared_probe(Subsystem::Database, Some("node_cpu"))];
        let ab = ops(&[("a", 100), ("b", 300)]);
        // Node at 60% is 34 W, the database at 20% takes 8 W of it.
        let snap = host_tick(&state, &probes, &[Some(20.0)], Some((60.0, 1)), &ab);
        assert!(close(published_watts(&snap), 26.0), "{snap:?}");
        assert!(close(billed_watts(&probes[0]).unwrap(), 8.0));
    }

    #[test]
    fn shared_caps_the_host_at_node_dynamic() {
        let state = CloudEnergyState::new();
        let probes = [shared_probe(Subsystem::Database, Some("node_cpu"))];
        let ab = ops(&[("a", 100), ("b", 300)]);
        let snap = host_tick(&state, &probes, &[Some(90.0)], Some((50.0, 1)), &ab);
        // The node's 20 W above idle all go to the host, idle stays once.
        assert!(close(billed_watts(&probes[0]).unwrap(), 20.0));
        assert!(close(published_watts(&snap), 10.0), "{snap:?}");
    }

    #[test]
    fn joint_budget_on_one_node() {
        let state = CloudEnergyState::new();
        let probes = [
            shared_probe(Subsystem::Database, Some("node_cpu")),
            shared_probe(Subsystem::Broker, Some("node_cpu")),
        ];
        let ab = ops(&[("a", 100), ("b", 300)]);
        let cpu = [Some(20.0), Some(30.0)];
        let snap = host_tick(&state, &probes, &cpu, Some((40.0, 1)), &ab);
        // 16 W above idle: the database takes 8, the broker what is left.
        let (db, broker) = (
            billed_watts(&probes[0]).unwrap(),
            billed_watts(&probes[1]).unwrap(),
        );
        assert!(close(db, 8.0) && close(broker, 8.0), "{db} {broker}");
        let published = published_watts(&snap);
        assert!(close(published, 10.0), "{snap:?}");
        assert!(close(published + db + broker, 26.0), "the node, once");
    }

    #[test]
    fn a_shared_host_without_a_one_node_reading_bills_nothing() {
        let ab = ops(&[("a", 100), ("b", 300)]);
        let cases: [(Option<f64>, Option<Reading>); 3] = [
            (None, Some((60.0, 1))),       // no probe reading
            (Some(20.0), None),            // no group reading
            (Some(20.0), Some((60.0, 2))), // the group names no node
        ];
        for (probe_cpu, node) in cases {
            let state = CloudEnergyState::new();
            let probes = [shared_probe(Subsystem::Database, Some("node_cpu"))];
            let snap = host_tick(&state, &probes, &[probe_cpu], node, &ab);
            assert_eq!(billed_watts(&probes[0]), None, "{probe_cpu:?} {node:?}");
            let without_host = host_tick(&CloudEnergyState::new(), &[], &[], node, &ab);
            assert_eq!(snap, without_host, "nothing subtracted");
        }
    }

    #[test]
    fn an_unlisted_shared_host_bills_its_dynamic_share_alone() {
        let state = CloudEnergyState::new();
        let probes = [shared_probe(Subsystem::Database, None)];
        let ab = ops(&[("a", 100), ("b", 300)]);
        let snap = host_tick(&state, &probes, &[Some(20.0)], Some((60.0, 1)), &ab);
        assert!(close(billed_watts(&probes[0]).unwrap(), 8.0));
        assert!(close(published_watts(&snap), 34.0), "no service changes");
    }

    #[test]
    fn a_member_without_ops_keeps_its_row_while_the_host_is_billed() {
        let state = CloudEnergyState::new();
        state.insert_for_test("b".into(), 1e-7, 150);
        let probes = [shared_probe(Subsystem::Database, Some("node_cpu"))];
        let only_a = ops(&[("a", 100), ("b", 0)]);
        let snap = host_tick(&state, &probes, &[Some(20.0)], Some((60.0, 1)), &only_a);
        assert!(close(billed_watts(&probes[0]).unwrap(), 8.0));
        assert!((snap["a"] - per_op(26.0, 100)).abs() < 1e-24);
        assert!((snap["b"] - 1e-7).abs() < 1e-20);
    }

    #[test]
    fn a_shared_probe_takes_its_group_envelope_and_query() {
        let cloud = config_with(&[
            ("a", manual(10.0, 50.0, Some(" node_cpu "))),
            ("b", manual(10.0, 50.0, Some("node_cpu"))),
        ]);
        let mut listed = hosts(HostPlacement::Shared, Some("pg_cpu"));
        listed.nodes = None;
        listed.instance_type = None;
        listed.shared_with = vec!["a".to_string(), "b".to_string()];
        let p = HostProbe::shared(Subsystem::Database, &listed, &cloud).expect("listed");
        assert_eq!((p.idle_watts, p.max_watts, p.nodes), (10.0, 50.0, 1));
        assert_eq!(p.group_key.as_deref(), Some("node_cpu"));
        assert_eq!(p.placement, HostPlacement::Shared);

        let unlisted = hosts(HostPlacement::Shared, Some("pg_cpu"));
        let p = HostProbe::shared(Subsystem::Database, &unlisted, &cloud).expect("typed");
        assert_eq!(
            (p.idle_watts, p.max_watts),
            table::lookup_instance_power("m5.large", "generic")
        );
        assert_eq!(p.group_key, None);
        let dedicated = hosts(HostPlacement::Dedicated, Some("pg_cpu"));
        assert!(HostProbe::shared(Subsystem::Database, &dedicated, &cloud).is_none());
    }

    #[tokio::test]
    async fn a_two_series_probe_response_fails_and_bills_nothing() {
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{"instance":"a"},"value":[1.0,"10"]},{"metric":{"instance":"b"},"value":[1.0,"90"]}]}}"#;
        let (endpoint, server) = spawn_one_shot_server(http_200_json(body)).await;
        let client = http_client::build_client();
        let uri = <hyper::Uri as std::str::FromStr>::from_str(&endpoint).unwrap();
        let metrics = MetricsState::new();
        let mut probes = [probe(2, 10.0, 50.0)];
        let cpu = fetch_host_probes(&mut probes, &client, &uri, None, &metrics).await;
        assert_eq!(cpu, vec![None]);
        assert!(probes[0].warned, "the first failure warns");
        bill_host_probes(&probes, &cpu, &no_readings(), INTERVAL, monotonic_ms());
        assert_eq!(
            probes[0].state.take_window_kwh(monotonic_ms(), 45_000),
            None
        );
        assert!(metrics.database_cpu_last_scrape_age_seconds.get() > 0.0);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_one_series_probe_response_resets_its_age_gauge() {
        let body = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1.0,"25"]}]}}"#;
        let (endpoint, server) = spawn_one_shot_server(http_200_json(body)).await;
        let client = http_client::build_client();
        let uri = <hyper::Uri as std::str::FromStr>::from_str(&endpoint).unwrap();
        let metrics = MetricsState::new();
        metrics.database_cpu_last_scrape_age_seconds.set(30.0);
        let mut probes = [probe(2, 10.0, 50.0)];
        probes[0].warned = true;
        let cpu = fetch_host_probes(&mut probes, &client, &uri, None, &metrics).await;
        assert_eq!(cpu, vec![Some(25.0)]);
        assert!(!probes[0].warned, "a success re-arms the warning");
        assert!(metrics.database_cpu_last_scrape_age_seconds.get().abs() < 1e-12);
        server.await.unwrap();
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
        let handle = spawn_cloud_scraper(cfg, state, Vec::new(), metrics);
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
        let handle = spawn_cloud_scraper(cfg, state, Vec::new(), metrics);
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
        let handle = spawn_cloud_scraper(cfg, state.clone(), Vec::new(), metrics.clone());
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
