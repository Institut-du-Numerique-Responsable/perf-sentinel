//! Daemon mode: streaming detection with OTLP and JSON ingestion.
//!
//! Runs an event loop that receives spans from multiple sources (OTLP gRPC,
//! OTLP HTTP, JSON socket), accumulates them in a `TraceWindow`, and emits
//! findings as NDJSON on stdout when traces expire, unless
//! `[daemon] findings_stdout` is off.

pub mod ack;
pub mod ack_toml_state;
pub mod archive;
pub mod findings_store;
pub mod health;
pub mod incidents;
pub mod query_api;
pub mod traces_store;

mod event_loop;
mod hub_correlation_export;
mod hub_export;
mod hub_incident_export;
mod hub_table;
#[cfg(unix)]
mod json_socket;
mod listeners;
mod mem_pressure;
mod rollup_writer;
mod sampling;
mod slow_window;
mod task_guard;
mod tls;

use std::sync::Arc;

use tokio::sync::{Mutex, RwLock, mpsc};

use crate::config::Config;
use crate::correlate::window::{TraceWindow, WindowConfig};
use crate::detect::DetectConfig;
use crate::event::SpanEvent;
use crate::report::GreenSummary;
use crate::report::metrics::MetricsState;
use crate::score::host_static::Subsystem;

use event_loop::{
    EnergyScraperHandles, EnergySources, EventLoopConfig, ListenerHandles, ShutdownTargets,
    SubsystemSources, run_event_loop,
};
use listeners::{
    init_ack_resources, setup_alumet_scraper, setup_cloud_scraper, setup_correlator,
    setup_emaps_scraper, setup_grid_feed_scrapers, setup_kepler_scraper, setup_redfish_scraper,
    setup_scaphandre_scraper, spawn_ack_toml_reload, spawn_listeners,
};

/// Bounded OTLP source context carried independently of normalized I/O events.
/// `endpoint` is present for an inbound entry span and absent for an
/// intermediate parent edge.
pub(crate) struct SourceEndpointUpdate {
    pub(crate) trace_id: String,
    pub(crate) service: Arc<str>,
    pub(crate) span_id: String,
    pub(crate) parent_span_id: Option<String>,
    pub(crate) endpoint: Option<String>,
    /// CONSUMER destination, read only where no route answers.
    pub(crate) consumer_endpoint: Option<String>,
}

/// One bounded daemon ingest message. JSON batches carry only `events`.
/// OTLP can additionally carry endpoint and parent context from spans that do
/// not themselves normalize into I/O events.
pub(crate) struct IngestBatch {
    pub(crate) events: Vec<SpanEvent>,
    pub(crate) source_endpoint_updates: Vec<SourceEndpointUpdate>,
}

impl From<Vec<SpanEvent>> for IngestBatch {
    fn from(events: Vec<SpanEvent>) -> Self {
        Self {
            events,
            source_endpoint_updates: Vec::new(),
        }
    }
}

/// Errors that can occur when running the daemon.
///
/// Marked `#[non_exhaustive]` so that adding future variants (e.g. a
/// new failure mode for a newly-integrated listener) stays a
/// SemVer-minor change. External consumers that `match` on this enum
/// must use a catch-all arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DaemonError {
    /// Listen address could not be parsed as a socket address.
    #[error("invalid listen address: {0}")]
    InvalidAddr(#[from] std::net::AddrParseError),
    /// HTTP listener failed to bind.
    #[error("failed to bind HTTP listener: {0}")]
    HttpBind(std::io::Error),
    /// gRPC listener failed to bind.
    #[error("failed to bind gRPC listener: {0}")]
    GrpcBind(std::io::Error),
    /// TLS configuration or certificate loading failed.
    #[error("TLS configuration error: {0}")]
    TlsConfig(#[source] TlsConfigError),
    /// Loading the CI ack TOML baseline failed at startup, and the
    /// path was operator-configured (so the failure escalates rather
    /// than silently downgrading to an empty baseline).
    #[error("failed to load acknowledgments TOML at '{path}'")]
    AckTomlLoad {
        /// Operator-configured path that failed to load.
        path: String,
        /// Underlying load error.
        #[source]
        source: crate::acknowledgments::AcknowledgmentLoadError,
    },
    /// Initializing the daemon ack JSONL store failed at startup, and
    /// the storage path was operator-configured.
    #[error("failed to initialize ack store at '{path}'")]
    AckStoreInit {
        /// Operator-configured path that failed to initialize.
        path: String,
        /// Underlying init error.
        #[source]
        source: ack::AckError,
    },
    /// Hub exporter could not read its API key file.
    #[error("failed to read PerfSentinelHub API key from '{path}'")]
    HubExportSecretRead {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// Hub exporter API key file was empty, short, or contained controls.
    #[error("PerfSentinelHub API key in '{path}' is invalid")]
    HubExportSecretInvalid { path: String },
    /// The validated Hub export endpoint could not be parsed as an HTTP URI.
    #[error("PerfSentinelHub export endpoint is invalid")]
    HubExportEndpoint,
    /// `[reporting] intent = "official"` is configured but the org-config
    /// is missing fields required for a publishable disclosure. Every
    /// missing or invalid field is listed in `errors`.
    #[error("[reporting] official intent is misconfigured:\n - {}", errors.join("\n - "))]
    ReportingValidation {
        /// All validation failures detected at startup.
        errors: Vec<String>,
    },
    /// Opening the per-window report archive file failed at startup.
    #[error("failed to open report archive at '{path}'")]
    ArchiveOpen {
        /// Operator-configured path that failed to open.
        path: String,
        /// Underlying open error.
        #[source]
        source: archive::ArchiveError,
    },
    /// Opening or reading back the incident archive file failed at startup.
    #[error("failed to open or read incident archive at '{path}'")]
    IncidentArchiveOpen {
        /// Operator-configured path that failed to open.
        path: String,
        /// Underlying open error.
        #[source]
        source: std::io::Error,
    },
    /// The single analysis worker stopped (e.g. a detector panicked) while
    /// the daemon was running. The daemon exits so a supervisor restarts it
    /// rather than staying up while silently analyzing nothing.
    #[error("analysis worker stopped unexpectedly")]
    AnalysisWorkerStopped,
}

/// Typed sub-enum for TLS configuration failures.
///
/// Five concrete cases let callers match on `TlsConfigError` and get
/// structured context instead of a `format!`-flattened string.
///
/// Marked `#[non_exhaustive]` so that adding future variants (e.g. a
/// handshake-specific failure or an unsupported protocol version) stays
/// a SemVer-minor change.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TlsConfigError {
    /// Could not read the PEM-encoded certificate chain from disk.
    #[error("failed to read TLS cert '{path}'")]
    ReadCert {
        /// Path to the cert file that could not be opened.
        path: String,
        /// Underlying I/O error (permissions, missing file, etc.).
        #[source]
        source: std::io::Error,
    },
    /// Could not read the PEM-encoded private key from disk.
    #[error("failed to read TLS key '{path}'")]
    ReadKey {
        /// Path to the key file that could not be opened.
        path: String,
        /// Underlying I/O error (permissions, missing file, etc.).
        #[source]
        source: std::io::Error,
    },
    /// The certificate chain PEM could not be parsed.
    #[error("failed to parse TLS cert chain")]
    ParseCerts(#[source] rustls::pki_types::pem::Error),
    /// The private key PEM could not be parsed.
    #[error("failed to parse TLS private key")]
    ParseKey(#[source] rustls::pki_types::pem::Error),
    /// `rustls::ServerConfig::with_single_cert` rejected the cert+key pair
    /// (e.g. mismatched key, unsupported algorithm).
    #[error("rustls server config rejected the cert+key pair")]
    ServerConfig(#[source] rustls::Error),
}

/// Publish the configured caps once at startup. They are constant for
/// the daemon's lifetime and pair with the live gauges (`active_traces`,
/// `analysis_queue_depth`, `stored_findings`) for the Grafana headroom
/// panels.
#[allow(clippy::cast_precision_loss)] // caps are small config values
fn publish_config_caps(metrics: &MetricsState, daemon: &crate::config::DaemonConfig) {
    metrics
        .max_active_traces
        .set(daemon.max_active_traces as f64);
    metrics
        .analysis_queue_capacity
        .set(daemon.analysis_queue_capacity as f64);
    metrics
        .max_retained_findings
        .set(daemon.max_retained_findings as f64);
}

/// Run the daemon: start all listeners and process events in a loop.
///
/// # Errors
///
/// Returns an error if the configured addresses are invalid or a listener fails to bind.
///
/// # Panics
///
/// Panics if `config.daemon.max_active_traces` is 0 (config validation prevents this).
// Daemon startup wiring: bind listeners, spawn scrapers + guards, run the
// loop, clean up. Linear orchestration, splitting would scatter the sequence.
#[allow(clippy::too_many_lines)]
pub async fn run(config: Config) -> Result<(), DaemonError> {
    validate_official_reporting(&config)?;
    warn_if_findings_kept_nowhere(&config.daemon);
    let (tx, mut rx) = mpsc::channel::<IngestBatch>(config.daemon.ingest_queue_capacity);
    let window = Arc::new(Mutex::new(TraceWindow::new(WindowConfig {
        max_events_per_trace: config.daemon.max_events_per_trace,
        trace_ttl_ms: config.daemon.trace_ttl_ms,
        max_active_traces: std::num::NonZeroUsize::new(config.daemon.max_active_traces)
            .expect("config validates max_active_traces >= 1"),
    })));
    let metrics = Arc::new(MetricsState::new());
    publish_config_caps(&metrics, &config.daemon);
    // Memory-pressure admission guard (opt-in): poll cgroup usage and flip
    // the flag the ingest listeners read. `None` when disabled. The guard
    // aborts the watcher on drop, covering the early `?` returns below
    // (listener bind, archive open) as well as the normal exit.
    let _memory_watch =
        mem_pressure::spawn_if_enabled(&metrics, config.daemon.memory_high_water_pct);
    let findings_store = Arc::new(findings_store::FindingsStore::new(
        config.daemon.max_retained_findings,
    ));
    // Shared with the Hub exporter: a pushed envelope must carry the same
    // acknowledgment annotation the query API adds to a polled one.
    let (toml_acks, ack_store) = init_ack_resources(&config).await?;
    // Re-reads the ack TOML in the background, so a team decision recorded in
    // a mounted ConfigMap applies without restarting the pod. Aborts on drop,
    // like the memory watcher above.
    let _ack_reload = spawn_ack_toml_reload(&config, toml_acks.clone());
    // Built before the exporter, which reads it to push correlation episodes.
    let correlator = setup_correlator(&config);
    let hub_export = hub_export::HubExporter::spawn(
        &config.daemon.hub_export,
        metrics.clone(),
        toml_acks.clone(),
        ack_store.clone(),
        config.daemon.incidents.enabled,
        correlator.clone(),
        &config.daemon.correlation,
    )?;
    let hub_export_buffer = hub_export.as_ref().map(hub_export::HubExporter::buffer);
    // Masked span trees for the findings the store keeps, so an exported
    // report still draws a tree once the correlation window has moved on.
    // Only /api/export/report reads it: with the API off or the findings
    // store disabled, retention would pin memory nothing can serve.
    let traces_capacity = if config.daemon.api_enabled && config.daemon.max_retained_findings > 0 {
        config.daemon.max_retained_traces
    } else {
        0
    };
    let traces_store = Arc::new(traces_store::TracesStore::new(
        traces_capacity,
        config.daemon.max_events_per_trace,
    ));
    // Shared cell mutated by the event loop after each batch and read
    // by the /api/export/report handler. Initialized to disabled(0):
    // the cold-start guard (`events_processed == 0 -> 503`) ensures
    // clients never observe the initial value.
    let green_summary_cell = Arc::new(RwLock::new(GreenSummary::disabled(0)));

    let incident_archive = open_incident_archive(&config, &metrics)?;
    let incident_store = load_incident_store(&config)?;

    let (grpc_handle, http_handle, json_socket_handle) = spawn_listeners(
        &config,
        tx.clone(),
        window.clone(),
        findings_store.clone(),
        traces_store.clone(),
        correlator.clone(),
        metrics.clone(),
        green_summary_cell.clone(),
        toml_acks,
        ack_store,
        incident_store,
        incident_archive.as_ref().map(|h| h.tx.clone()),
        hub_export
            .as_ref()
            .and_then(hub_export::HubExporter::incident_buffer),
    )
    .await?;

    let (alumet, alumet_db, alumet_broker) = setup_alumet_scraper(&config, &metrics);
    let database_ceilings = declared_ceilings(&config, Subsystem::Database);
    let broker_ceilings = declared_ceilings(&config, Subsystem::Broker);
    let scaphandre = setup_scaphandre_scraper(&config, &metrics);
    let kepler = setup_kepler_scraper(&config, &metrics);
    let redfish = setup_redfish_scraper(&config, &metrics);
    let (cloud, host_cpu) = setup_cloud_scraper(&config, &metrics);
    let emaps = setup_emaps_scraper(&config);
    let grid_feeds = setup_grid_feed_scrapers(&config);

    let archive_handle = match &config.daemon.archive {
        Some(cfg) => Some(archive::spawn(cfg, metrics.clone()).map_err(|source| {
            DaemonError::ArchiveOpen {
                path: cfg.path.clone(),
                source,
            }
        })?),
        None => None,
    };

    // Inject the host contexts here (not in `carbon_context()`): only the
    // daemon can deliver measured or declared window energy, batch keeps
    // both `None` and falls back to the estimated figures.
    let base_carbon_ctx = Arc::new({
        let mut ctx = config.carbon_context();
        ctx.db_energy = host_energy_context(&config, Subsystem::Database);
        ctx.broker_energy = host_energy_context(&config, Subsystem::Broker);
        ctx
    });
    let detect_config = DetectConfig::from(&config);
    let green_on = config.green.enabled;
    let database_sources = group_sources(
        green_on,
        alumet_db.as_deref().map(|s| (s, alumet.staleness_ms)),
        (&host_cpu.database, host_cpu.staleness_ms),
        &database_ceilings,
    );
    let broker_sources = group_sources(
        green_on,
        alumet_broker.as_deref().map(|s| (s, alumet.staleness_ms)),
        (&host_cpu.broker, host_cpu.staleness_ms),
        &broker_ceilings,
    );
    let energy_sources = EnergySources {
        base_carbon_ctx,
        alumet_state: alumet.state.as_deref(),
        database: &database_sources,
        broker: &broker_sources,
        alumet_staleness_ms: alumet.staleness_ms,
        scaphandre_state: scaphandre.state.as_deref(),
        scaphandre_staleness_ms: scaphandre.staleness_ms,
        kepler_state: kepler.state.as_deref(),
        kepler_staleness_ms: kepler.staleness_ms,
        redfish_state: redfish.state.as_deref(),
        redfish_staleness_ms: redfish.staleness_ms,
        cloud_state: cloud.state.as_deref(),
        cloud_staleness_ms: cloud.staleness_ms,
        emaps_state: emaps.state.as_deref(),
        emaps_staleness_ms: emaps.staleness_ms,
        grid_feeds: &grid_feeds,
    };
    let shutdown = ShutdownTargets {
        energy: EnergyScraperHandles {
            alumet: alumet.handle.as_ref(),
            scaphandre: scaphandre.handle.as_ref(),
            kepler: kepler.handle.as_ref(),
            redfish: redfish.handle.as_ref(),
            cloud: cloud.handle.as_ref(),
            emaps: emaps.handle.as_ref(),
            grid_feeds: &grid_feeds,
        },
        listeners: ListenerHandles {
            grpc: &grpc_handle,
            http: &http_handle,
            json_socket: json_socket_handle.as_ref(),
        },
    };

    // Capture the loop outcome but still run cleanup below before
    // propagating: a worker-death exit must release the socket and flush
    // the archive just like a graceful shutdown.
    let loop_result = run_event_loop(
        &mut rx,
        &window,
        metrics,
        findings_store,
        hub_export_buffer,
        traces_store,
        correlator,
        &detect_config,
        &energy_sources,
        shutdown,
        EventLoopConfig {
            green_enabled: config.green.enabled,
            sampling_rate: config.daemon.sampling_rate,
            evict_ms: config.daemon.trace_ttl_ms / 2,
            slow_window_ms: config
                .detection
                .slow_query_window_minutes
                .saturating_mul(60_000),
            confidence: config.confidence(),
            analysis_queue_capacity: config.daemon.analysis_queue_capacity,
            per_service_labels: config.daemon.per_service_labels,
            per_grouping_labels: config.daemon.per_grouping_labels,
            findings_stdout: config.daemon.findings_stdout,
            waste_sticky_ttl_ms: waste_sticky_ttl_ms(
                &config,
                alumet.staleness_ms,
                host_cpu.staleness_ms,
            ),
        },
        green_summary_cell,
        archive_handle.as_ref().map(|h| h.tx.clone()),
    )
    .await;

    // Drain the Hub exporter before the archive, for the same reason the
    // archive is drained at all: what the loop produced but has not shipped
    // is still owed to a consumer. The exporter goes first because its
    // budget is the one that can expire, and the archive is a local write
    // that will not.
    if let Some(mut exporter) = hub_export {
        exporter.shutdown().await;
    }

    if let Some(handle) = archive_handle {
        drop(handle.tx);
        let _ = handle.join.await;
    }

    if let Some(handle) = incident_archive {
        drop(handle.tx);
        let _ = handle.join.await;
    }

    #[cfg(unix)]
    {
        let _ = tokio::fs::remove_file(&config.daemon.json_socket).await;
    }
    loop_result
}

/// The billing state of each host group's declared ceiling, in
/// [`crate::score::host_static::StaticHostsConfig::groups`] order: a
/// dedicated group with `nodes` and `instance_type`, `None` otherwise. No
/// scraper, the state only needs a start marker to bill elapsed time from.
/// Its region is the subsystem's effective one. Empty without a section.
fn declared_ceilings(
    config: &Config,
    subsystem: Subsystem,
) -> Vec<Option<crate::score::broker_static::StaticBrokerState>> {
    let Some(cfg) = config.green.static_hosts(subsystem) else {
        return Vec::new();
    };
    let now = crate::score::scaphandre::monotonic_ms();
    cfg.groups()
        .iter()
        .map(|group| {
            group
                .ceiling()
                .map(|c| crate::score::broker_static::StaticBrokerState::new(now, &c))
        })
        .collect()
}

/// One subsystem's sources, one entry per host group, each pairing the
/// group's live state with its declared ceiling. `probes` are the groups'
/// CPU probe states with their shared staleness. D6/B6 refuse a `cpu_query`
/// next to the Alumet workload, and D10/B10 refuse hosts next to it, so
/// the Alumet state is the single group's live source when it exists.
fn group_sources<'a>(
    green_on: bool,
    alumet: Option<(&'a crate::score::alumet::DbEnergyState, u64)>,
    (probes, probe_staleness_ms): (&'a [Option<Arc<crate::score::alumet::DbEnergyState>>], u64),
    ceilings: &'a [Option<crate::score::broker_static::StaticBrokerState>],
) -> Vec<SubsystemSources<'a>> {
    let groups = ceilings
        .len()
        .max(probes.len())
        .max(usize::from(alumet.is_some()));
    (0..groups)
        .map(|i| {
            let live = alumet.filter(|_| i == 0).or_else(|| {
                probes
                    .get(i)
                    .and_then(Option::as_deref)
                    .map(|s| (s, probe_staleness_ms))
            });
            host_sources(green_on, live, ceilings.get(i).and_then(Option::as_ref))
        })
        .collect()
}

/// One subsystem's host sources, the live state (the Alumet workload or
/// the CPU probe) with its own staleness. Green scoring off means
/// `score_green` never runs: the loop must not consume (and thus destroy)
/// the accumulated host energy, so the bundle is empty.
fn host_sources<'a>(
    green_on: bool,
    live: Option<(&'a crate::score::alumet::DbEnergyState, u64)>,
    declared: Option<&'a crate::score::broker_static::StaticBrokerState>,
) -> SubsystemSources<'a> {
    if !green_on {
        return SubsystemSources::default();
    }
    SubsystemSources {
        live: live.map(|(state, _)| state),
        live_staleness_ms: live.map_or(0, |(_, staleness_ms)| staleness_ms),
        declared,
    }
}

/// Lifetime of the live cell's last host waste figure. 2x the staleness
/// window of each live scraper (Alumet, and the cloud scraper when it runs
/// a host CPU probe, `probe_staleness_ms`, 0 otherwise): flap-free between
/// scrapes, aged out shortly after a dead scraper's last reading. A
/// declared ceiling (database or broker) has no scraper, so it falls back
/// to the batch cadence: its figure is skipped on sub-second takes and
/// would otherwise flap with nothing to bridge it.
fn waste_sticky_ttl_ms(config: &Config, alumet_staleness_ms: u64, probe_staleness_ms: u64) -> u64 {
    let any_ceiling = [Subsystem::Database, Subsystem::Broker]
        .into_iter()
        .filter_map(|s| config.green.static_hosts(s))
        .flat_map(crate::score::host_static::StaticHostsConfig::groups)
        .any(|group| group.ceiling().is_some());
    let ceiling_ttl = if any_ceiling {
        config.daemon.trace_ttl_ms
    } else {
        0
    };
    alumet_staleness_ms
        .max(probe_staleness_ms)
        .saturating_mul(2)
        .max(ceiling_ttl)
}

/// The host context of one subsystem, built only when it has a source the
/// daemon bills: its Alumet workload, a dedicated declaration with a
/// `SPECpower` ceiling, or a declaration with a `cpu_query` (every shared
/// one, D5/B5). A ceiling-less declaration without one builds none, so the
/// estimated figure stays. `model` is the base tag, which a declared tick
/// replaces: `cloud_specpower` when the declaration has a `cpu_query` (its
/// live source), else the declared tag. A shared declaration has no
/// ceiling, so its tag never changes. `region` is the subsystem's
/// effective region.
fn host_energy_context(
    config: &Config,
    subsystem: Subsystem,
) -> Option<crate::score::carbon::DbEnergyContext> {
    let green = &config.green;
    let declared = green.static_hosts(subsystem);
    let model = if green.alumet_host(subsystem).is_some() {
        crate::score::carbon::CO2_MODEL_ALUMET
    } else {
        let groups = declared?.groups();
        if groups.iter().any(|g| g.cpu_query.is_some()) {
            crate::score::carbon::CO2_MODEL_CLOUD_SPECPOWER
        } else if groups.iter().any(|g| g.ceiling().is_some()) {
            subsystem.declared_tag()
        } else {
            return None;
        }
    };
    Some(crate::score::carbon::DbEnergyContext {
        window_kwh: 0.0,
        region: green.host_region(subsystem).map(str::to_string),
        model,
        placement: declared.map(|cfg| cfg.placement),
    })
}

/// The incident archive writer, when `[daemon.incidents]` is enabled and
/// names a path. Opened before the listeners so a bad path fails the
/// daemon rather than the first incident, and drained after the loop like
/// the report archive: a record handed over is owed to the file.
///
/// # Errors
///
/// The open error, which fails daemon startup.
fn open_incident_archive(
    config: &Config,
    metrics: &Arc<MetricsState>,
) -> Result<Option<incidents::ArchiveHandle>, DaemonError> {
    let incidents = &config.daemon.incidents;
    let Some(path) = incidents
        .archive_path
        .as_ref()
        .filter(|_| incidents.enabled)
    else {
        return Ok(None);
    };
    incidents::spawn_archive(path, metrics.clone())
        .map(Some)
        .map_err(|source| DaemonError::IncidentArchiveOpen {
            path: path.clone(),
            source,
        })
}

/// The incident ring, when `[daemon.incidents]` is enabled, seeded from
/// the archive when one is configured. Read before the listeners, so the
/// API never serves an empty ring the archive could have filled.
///
/// # Errors
///
/// A read error on the archive, which fails daemon startup.
fn load_incident_store(
    config: &Config,
) -> Result<Option<Arc<incidents::IncidentStore>>, DaemonError> {
    let incidents = &config.daemon.incidents;
    if !incidents.enabled {
        return Ok(None);
    }
    let loaded = match &incidents.archive_path {
        Some(path) => incidents::load_archive(std::path::Path::new(path), incidents.max_retained)
            .map_err(|source| DaemonError::IncidentArchiveOpen {
            path: path.clone(),
            source,
        })?,
        None => Vec::new(),
    };
    if !loaded.is_empty() {
        tracing::info!(count = loaded.len(), "Incidents reloaded from the archive");
    }
    Ok(Some(Arc::new(incidents::IncidentStore::with_incidents(
        incidents.max_retained,
        loaded,
    ))))
}

/// The startup warning for a daemon that keeps no lasting copy of its
/// findings (stdout off, no Hub export, no archive), `None` otherwise.
/// Without a served ring (`api_enabled = false` or
/// `max_retained_findings = 0`) nothing keeps them at all.
fn findings_kept_nowhere(daemon: &crate::config::DaemonConfig) -> Option<&'static str> {
    if daemon.findings_stdout || daemon.hub_export.enabled || daemon.archive.is_some() {
        return None;
    }
    Some(if daemon.api_enabled && daemon.max_retained_findings > 0 {
        "[daemon] findings_stdout = false with neither [daemon.hub_export] nor \
         [daemon.archive]: findings stay in the query API's bounded ring only, and are \
         lost once it rolls over or the daemon restarts"
    } else {
        "[daemon] findings_stdout = false with neither [daemon.hub_export] nor \
         [daemon.archive], and api_enabled = false or max_retained_findings = 0: \
         findings are kept nowhere, only /metrics counts them"
    })
}

fn warn_if_findings_kept_nowhere(daemon: &crate::config::DaemonConfig) {
    if let Some(message) = findings_kept_nowhere(daemon) {
        tracing::warn!(
            max_retained_findings = daemon.max_retained_findings,
            "{message}"
        );
    }
}

/// Daemon startup gate for `[reporting] intent = "official"`.
/// See `docs/design/08-PERIODIC-DISCLOSURE.md`.
fn validate_official_reporting(config: &Config) -> Result<(), DaemonError> {
    use crate::report::periodic::org_config;

    if config.reporting.intent.as_deref() == Some("audited") {
        return Err(DaemonError::ReportingValidation {
            errors: vec![
                "[reporting] intent = \"audited\" is not yet implemented, refusing to start daemon"
                    .to_string(),
            ],
        });
    }
    if config.reporting.intent.as_deref() != Some("official") {
        return Ok(());
    }

    let mut errors: Vec<String> = Vec::new();
    let loaded = match &config.reporting.org_config_path {
        None => {
            errors.push(
                "[reporting] org_config_path is required when intent = \"official\"".to_string(),
            );
            None
        }
        Some(path) => match org_config::load_from_path(path) {
            Ok(cfg) => Some(cfg),
            Err(err) => {
                errors.push(err.to_string());
                None
            }
        },
    };

    if let Some(cfg) = loaded {
        errors.extend(org_config::validate_for_official(&cfg));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(DaemonError::ReportingValidation { errors })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::host_static::HostPlacement;
    use core::assert_matches;

    #[test]
    fn findings_kept_nowhere_only_without_stdout_hub_export_or_archive() {
        let mut daemon = Config::default().daemon;
        assert!(
            findings_kept_nowhere(&daemon).is_none(),
            "stdout on by default"
        );
        daemon.findings_stdout = false;
        let ring = findings_kept_nowhere(&daemon).expect("ring only");
        assert!(ring.contains("bounded ring only"), "{ring}");
        warn_if_findings_kept_nowhere(&daemon);
        daemon.api_enabled = false;
        let none = findings_kept_nowhere(&daemon).expect("kept nowhere");
        assert!(none.contains("kept nowhere"), "no API, no ring: {none}");
        daemon.api_enabled = true;
        daemon.max_retained_findings = 0;
        let none = findings_kept_nowhere(&daemon).expect("kept nowhere");
        assert!(none.contains("kept nowhere"), "store disabled: {none}");
        daemon.hub_export.enabled = true;
        assert!(
            findings_kept_nowhere(&daemon).is_none(),
            "the Hub keeps them"
        );
        daemon.hub_export.enabled = false;
        daemon.archive = crate::config::load_from_str(
            "[daemon.archive]\npath = \"/var/lib/perf-sentinel/reports.ndjson\"\n",
        )
        .expect("config loads")
        .daemon
        .archive;
        assert!(daemon.archive.is_some());
        assert!(
            findings_kept_nowhere(&daemon).is_none(),
            "the archive keeps them"
        );
    }

    #[test]
    fn host_context_exists_only_for_a_billable_source() {
        let load = |toml: &str| crate::config::load_from_str(toml).expect("config loads");
        let ceiling = load(
            "[green]\ndefault_region = \"eu-west-3\"\n[green.broker_static]\n\
             placement = \"dedicated\"\nnodes = 3\ninstance_type = \"m5.large\"\n",
        );
        let ctx = host_energy_context(&ceiling, Subsystem::Broker).expect("ceiling bills");
        assert_eq!(ctx.model, crate::report::BROKER_WASTE_MODEL_SPECPOWER);
        assert_eq!(ctx.placement, Some(HostPlacement::Dedicated));
        assert_eq!(ctx.region.as_deref(), Some("eu-west-3"));
        assert!(host_energy_context(&ceiling, Subsystem::Database).is_none());

        let alumet = load(
            "[green]\ndefault_region = \"eu-west-3\"\n[green.alumet]\n\
             endpoint = \"http://localhost:9091/metrics\"\n\
             metric_name = \"attributed_energy_cpu_alumet\"\nlabel_key = \"name\"\n\
             [green.alumet.database]\nlabel_value = \"pg-pod\"\n",
        );
        let ctx = host_energy_context(&alumet, Subsystem::Database).expect("Alumet bills");
        assert_eq!(ctx.model, crate::score::carbon::CO2_MODEL_ALUMET);
        assert_eq!(ctx.placement, None, "no declared-hosts section");
        assert!(host_energy_context(&alumet, Subsystem::Broker).is_none());
        assert!(host_energy_context(&Config::default(), Subsystem::Broker).is_none());
    }

    const DATABASE_CEILING: &str = "[green]\ndefault_region = \"eu-west-3\"\n\
        [green.database_static]\nplacement = \"dedicated\"\nnodes = 2\n\
        instance_type = \"db.r6i.large\"\n";

    #[test]
    fn a_database_ceiling_builds_a_context_tagged_by_its_live_source() {
        let load = |toml: &str| crate::config::load_from_str(toml).expect("config loads");
        let ceiling = load(DATABASE_CEILING);
        let ctx = host_energy_context(&ceiling, Subsystem::Database).expect("ceiling bills");
        assert_eq!(ctx.model, crate::report::DB_WASTE_MODEL_SPECPOWER);
        assert_eq!(ctx.placement, Some(HostPlacement::Dedicated));
        assert_eq!(ctx.region.as_deref(), Some("eu-west-3"));
        assert_eq!(declared_ceilings(&ceiling, Subsystem::Database).len(), 1);
        assert!(declared_ceilings(&ceiling, Subsystem::Database)[0].is_some());
        assert!(declared_ceilings(&ceiling, Subsystem::Broker).is_empty());

        // A cpu_query is the live source: the base tag is its model, and
        // only a declared tick takes the declared tag.
        let probed = load(&format!(
            "{DATABASE_CEILING}cpu_query = 'avg(db_cpu)'\n\
             [green.cloud]\nprometheus_endpoint = \"http://prom:9090\"\n"
        ));
        let ctx = host_energy_context(&probed, Subsystem::Database).expect("ceiling bills");
        assert_eq!(ctx.model, crate::score::carbon::CO2_MODEL_CLOUD_SPECPOWER);

        // The broker twin: declared tag alone, the probe's tag with a cpu_query.
        let broker = DATABASE_CEILING
            .replace("database_static", "broker_static")
            .replace("db.r6i.large", "m5.large");
        let ctx = host_energy_context(&load(&broker), Subsystem::Broker).expect("ceiling bills");
        assert_eq!(ctx.model, crate::report::BROKER_WASTE_MODEL_SPECPOWER);
        let probed = load(&format!(
            "{broker}cpu_query = 'avg(broker_cpu)'\n\
             [green.cloud]\nprometheus_endpoint = \"http://prom:9090\"\n"
        ));
        let ctx = host_energy_context(&probed, Subsystem::Broker).expect("ceiling bills");
        assert_eq!(ctx.model, crate::score::carbon::CO2_MODEL_CLOUD_SPECPOWER);
    }

    #[test]
    fn a_shared_host_builds_a_cloud_context_without_a_ceiling() {
        let shared = crate::config::load_from_str(
            "[green]\ndefault_region = \"eu-west-3\"\n[green.broker_static]\n\
             placement = \"shared\"\ninstance_type = \"m5.large\"\ncpu_query = 'broker_cpu'\n\
             [green.cloud]\nprometheus_endpoint = \"http://prom:9090\"\n",
        )
        .expect("config loads");
        let ctx = host_energy_context(&shared, Subsystem::Broker).expect("its probe bills");
        assert_eq!(ctx.model, crate::score::carbon::CO2_MODEL_CLOUD_SPECPOWER);
        assert_eq!(ctx.placement, Some(HostPlacement::Shared));
        assert_eq!(ctx.region.as_deref(), Some("eu-west-3"));
        assert!(declared_ceilings(&shared, Subsystem::Broker)[0].is_none());
        assert!(host_energy_context(&shared, Subsystem::Database).is_none());
    }

    #[test]
    fn host_groups_pair_each_probe_with_its_own_ceiling() {
        let config = crate::config::load_from_str(
            "[green]\ndefault_region = \"eu-west-3\"\n\
             [green.cloud]\nprometheus_endpoint = \"http://prom:9090\"\n\
             [green.database_static]\nplacement = \"dedicated\"\n\
             [[green.database_static.hosts]]\nnodes = 1\ninstance_type = \"db.t4g.large\"\n\
             cpu_query = 'avg(writer_cpu)'\n\
             [[green.database_static.hosts]]\nnodes = 2\ninstance_type = \"db.t4g.small\"\n",
        )
        .expect("config loads");
        let ceilings = declared_ceilings(&config, Subsystem::Database);
        assert_eq!(ceilings.len(), 2);
        assert!(ceilings.iter().all(Option::is_some));
        let ctx = host_energy_context(&config, Subsystem::Database).expect("groups bill");
        assert_eq!(ctx.model, crate::score::carbon::CO2_MODEL_CLOUD_SPECPOWER);

        let live = crate::score::alumet::DbEnergyState::new();
        let probes = [Some(live.clone()), None];
        let sources = group_sources(true, None, (&probes, 30_000), &ceilings);
        assert_eq!(sources.len(), 2);
        assert!(sources[0].live.is_some() && sources[0].declared.is_some());
        assert_eq!(sources[0].live_staleness_ms, 30_000);
        assert!(sources[1].live.is_none() && sources[1].declared.is_some());
        let off = group_sources(false, None, (&probes, 30_000), &ceilings);
        assert!(off.iter().all(|s| s.live.is_none() && s.declared.is_none()));
        // The Alumet workload is the single group's live source.
        let alumet = group_sources(true, Some((&live, 10_000)), (&[], 0), &[]);
        assert_eq!(alumet.len(), 1);
        assert_eq!(alumet[0].live_staleness_ms, 10_000);
    }

    #[test]
    fn a_database_ceiling_alone_keeps_the_live_cell_across_batches() {
        let mut config = crate::config::load_from_str(DATABASE_CEILING).expect("config loads");
        config.daemon.trace_ttl_ms = 30_000;
        assert_eq!(waste_sticky_ttl_ms(&config, 0, 0), 30_000);
        assert_eq!(waste_sticky_ttl_ms(&Config::default(), 0, 0), 0);
        assert_eq!(waste_sticky_ttl_ms(&config, 20_000, 0), 40_000);
        // A CPU probe bridges two of its own staleness windows.
        assert_eq!(waste_sticky_ttl_ms(&config, 0, 45_000), 90_000);
        assert_eq!(waste_sticky_ttl_ms(&config, 20_000, 45_000), 90_000);
    }

    #[test]
    fn green_disabled_hands_the_loop_no_host_source() {
        let live = crate::score::alumet::DbEnergyState::new();
        let cfg = crate::score::broker_static::StaticBrokerConfig {
            nodes: 1,
            instance_type: "m5.large".to_string(),
            provider: "generic".to_string(),
            region: None,
        };
        let declared = crate::score::broker_static::StaticBrokerState::new(0, &cfg);
        let off = host_sources(false, Some((&live, 60_000)), Some(&declared));
        assert!(off.live.is_none() && off.declared.is_none());
        let on = host_sources(true, Some((&live, 60_000)), Some(&declared));
        assert!(on.live.is_some() && on.declared.is_some());
        assert_eq!(on.live_staleness_ms, 60_000);
    }

    #[tokio::test]
    async fn daemon_run_rejects_invalid_listen_address() {
        // Malformed listen_addr fails the `format!().parse()` call in
        // `run` before any listener binds. Covers the InvalidAddr path.
        let config = Config {
            daemon: crate::config::DaemonConfig {
                listen_addr: "not an address".to_string(),
                ..crate::config::DaemonConfig::default()
            },
            ..Config::default()
        };
        // Bogus port paths still reach .parse(), which fails.
        let err = run(config).await.expect_err("should fail");
        assert_matches!(err, DaemonError::InvalidAddr(_));
    }

    /// A default config whose `[reporting]` declares official intent.
    fn official_config(org_config_path: Option<String>) -> Config {
        Config {
            reporting: crate::config::ReportingConfig {
                intent: Some("official".to_string()),
                org_config_path,
                ..Default::default()
            },
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn daemon_run_refuses_official_without_org_config_path() {
        let config = official_config(None);
        let err = run(config).await.expect_err("must refuse");
        match err {
            DaemonError::ReportingValidation { errors } => {
                assert!(
                    errors.iter().any(|e| e.contains("org_config_path")),
                    "got {errors:?}"
                );
            }
            other => panic!("expected ReportingValidation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn daemon_run_refuses_official_with_unreadable_org_config() {
        let config = official_config(Some("/no/such/path/org.toml".to_string()));
        let err = run(config).await.expect_err("must refuse");
        assert_matches!(err, DaemonError::ReportingValidation { .. });
    }

    #[tokio::test]
    async fn daemon_run_refuses_official_when_org_config_misses_fields() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        // Incomplete: empty name, lowercase country, missing core patterns.
        writeln!(
            file,
            r#"
[organisation]
name = ""
country = "fr"

[methodology]
sci_specification = "ISO/IEC 21031:2024"
enabled_patterns = ["slow_sql"]
disabled_patterns = []
conformance = "partial"

[methodology.calibration]
carbon_intensity_source = "electricity_maps"
specpower_table_version = "2026-04-24"

[scope_manifest]
total_applications_declared = 1
"#
        )
        .unwrap();

        let config = official_config(Some(file.path().display().to_string()));
        let err = run(config).await.expect_err("must refuse");
        match err {
            DaemonError::ReportingValidation { errors } => {
                assert!(errors.iter().any(|e| e.contains("organisation.name")));
                assert!(errors.iter().any(|e| e.contains("country")));
                assert!(errors.iter().any(|e| e.contains("n_plus_one_sql")));
            }
            other => panic!("expected ReportingValidation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn daemon_run_refuses_official_when_specpower_version_drifts() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        // Publishable except for a table date the binary does not embed.
        writeln!(
            file,
            r#"
[organisation]
name = "Example SAS"
country = "FR"

[methodology]
sci_specification = "ISO/IEC 21031:2024"
enabled_patterns = ["n_plus_one_sql", "n_plus_one_http", "redundant_sql", "redundant_http"]
conformance = "core-required"

[methodology.calibration]
carbon_intensity_source = "static_tables"
specpower_table_version = "2023-05-01"

[scope_manifest]
total_applications_declared = 1
"#
        )
        .unwrap();

        let config = official_config(Some(file.path().display().to_string()));
        let err = run(config).await.expect_err("must refuse");
        match err {
            DaemonError::ReportingValidation { errors } => {
                assert_eq!(errors.len(), 1, "got {errors:?}");
                assert!(errors[0].contains("specpower_table_version"));
                assert!(errors[0].contains("does not match"));
            }
            other => panic!("expected ReportingValidation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn incident_store_is_seeded_from_the_archive_when_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("incidents.ndjson");
        std::fs::write(
            &path,
            r#"{"id":"a","service":"svc","kind":"restart","at_ms":1000,"window_from_ms":0,"window_to_ms":2000,"findings":[]}"#,
        )
        .unwrap();
        let config = |enabled, archive_path| Config {
            daemon: crate::config::DaemonConfig {
                incidents: crate::config::DaemonIncidentsConfig {
                    enabled,
                    archive_path,
                    ..Default::default()
                },
                ..crate::config::DaemonConfig::default()
            },
            ..Config::default()
        };
        let archive = Some(path.display().to_string());

        assert!(
            load_incident_store(&config(false, archive.clone()))
                .unwrap()
                .is_none()
        );
        let seeded = load_incident_store(&config(true, archive))
            .unwrap()
            .unwrap();
        assert!(seeded.contains("a").await);
        let empty = load_incident_store(&config(true, None)).unwrap().unwrap();
        assert!(!empty.contains("a").await);
    }

    #[test]
    fn daemon_error_display_is_informative() {
        // Smoke test for thiserror messages on every variant. Operators
        // should be able to tell "bad port number" from "port already in
        // use" at a glance.
        let e1: DaemonError = "not a socket"
            .parse::<std::net::SocketAddr>()
            .unwrap_err()
            .into();
        assert!(format!("{e1}").contains("invalid listen address"));
        let e2 = DaemonError::HttpBind(std::io::Error::other("boom"));
        assert!(format!("{e2}").contains("HTTP listener"));
        let e3 = DaemonError::GrpcBind(std::io::Error::other("boom"));
        assert!(format!("{e3}").contains("gRPC listener"));
        let e4 = DaemonError::AnalysisWorkerStopped;
        assert!(format!("{e4}").contains("analysis worker stopped"));
    }

    // ------------------------------------------------------------------
    // daemon::run end-to-end on ephemeral ports
    // ------------------------------------------------------------------
    //
    // Spins up the daemon on ephemeral TCP ports (port 0, OS-assigned)
    // and a tempdir-scoped Unix socket, sends one NDJSON line, then polls
    // the HTTP /metrics endpoint and asserts the daemon processed the
    // event. Ctrl-C is never sent. The test aborts the JoinHandle
    // instead, so the shutdown branch is not covered here (validated
    // separately by manual testing).

    #[cfg(unix)]
    #[tokio::test]
    async fn daemon_run_ingests_json_socket_events_and_exposes_metrics() {
        use std::fmt::Write as _;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream, UnixStream};
        use tokio::time::Duration;

        // Grab ephemeral ports via TCP binds, then release them so the
        // daemon can rebind. There is a brief race window between drop
        // and rebind. We compensate below with a retry loop on the
        // Unix-socket client connect, the first externally observable
        // effect of a successful bind.
        let l1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let l2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_port = l1.local_addr().unwrap().port();
        let grpc_port = l2.local_addr().unwrap().port();
        drop(l1);
        drop(l2);

        let (_dir, socket_path) = json_socket::unique_socket_dir_and_path("daemon-run");
        let socket_path_str = socket_path.to_string_lossy().into_owned();
        let config = Config {
            daemon: crate::config::DaemonConfig {
                listen_addr: "127.0.0.1".to_string(),
                listen_port: http_port,
                listen_port_grpc: grpc_port,
                json_socket: socket_path_str,
                trace_ttl_ms: 200, // fast eviction so the ticker fires during test
                max_active_traces: 10,
                ..crate::config::DaemonConfig::default()
            },
            ..Config::default()
        };

        let daemon_handle = tokio::spawn(async move {
            let _ = run(config).await;
        });

        // Poll for the Unix socket to appear (with retries) instead of
        // a fixed sleep. Gives slow CI runners more headroom without
        // hard-coding a conservative wait on fast dev machines.
        let mut client = None;
        for _ in 0..40 {
            if let Ok(s) = UnixStream::connect(&socket_path).await {
                client = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let mut client = client.expect("daemon Unix socket must bind within 1s");

        // Send 6 N+1-worthy events so the detector has something to
        // flag, which guarantees `findings_total` increments.
        let mut payload = String::from("[");
        for i in 1..=6 {
            if i > 1 {
                payload.push(',');
            }
            // Hand-written JSON (not serde) so the test has zero deps
            // on the event types' serde layout evolving over time.
            let _ = write!(
                payload,
                r#"{{"timestamp":"2025-07-10T14:32:01.{i:03}Z","trace_id":"daemon-t1","span_id":"s{i}","service":"svc-e2e","type":"sql","operation":"SELECT","target":"SELECT * FROM users WHERE id = {i}","duration_us":100,"source":{{"endpoint":"GET /test","method":"m"}}}}"#
            );
        }
        payload.push(']');
        client.write_all(payload.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.shutdown().await.unwrap();

        // Poll the HTTP /metrics endpoint with retries. The daemon
        // flushes processed traces on every eviction tick (ttl/2 = 100ms),
        // so we give it up to 1.5s to update the counters. Using a
        // minimal hand-rolled HTTP/1.0 GET avoids pulling hyper into the
        // test just for this.
        let metrics_addr = format!("127.0.0.1:{http_port}");
        let mut observed_events = false;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            let Ok(mut stream) = TcpStream::connect(&metrics_addr).await else {
                continue;
            };
            let req = "GET /metrics HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n";
            if stream.write_all(req.as_bytes()).await.is_err() {
                continue;
            }
            let mut buf = Vec::with_capacity(8192);
            if stream.read_to_end(&mut buf).await.is_err() {
                continue;
            }
            let body = String::from_utf8_lossy(&buf);
            // Look for any non-zero events counter. The metric text is
            // `perf_sentinel_events_processed_total <value>` (OpenMetrics).
            if body.contains("perf_sentinel_events_processed_total")
                && body.lines().any(|l| {
                    l.starts_with("perf_sentinel_events_processed_total")
                        && l.split_whitespace()
                            .last()
                            .and_then(|v| v.parse::<f64>().ok())
                            .is_some_and(|v| v > 0.0)
                })
            {
                observed_events = true;
                break;
            }
        }

        // Abort the daemon BEFORE the assert so the test cleans up on
        // both pass and fail paths.
        daemon_handle.abort();
        let _ = daemon_handle.await;
        // _dir drops here, removing the socket and parent tempdir.

        assert!(
            observed_events,
            "daemon should have processed the 6 events and surfaced a \
             non-zero `perf_sentinel_events_processed_total` on /metrics"
        );
    }

    // ------------------------------------------------------------------
    // /health is always exposed, even when the query API is disabled.
    // ------------------------------------------------------------------
    //
    // Liveness probes (K8s, load balancers, systemd) must not depend on
    // `daemon.api_enabled`. This test spins up a daemon with the query
    // API off, then hits `/health` over TCP to verify both the 200 OK
    // and the JSON body shape.

    #[cfg(unix)]
    #[tokio::test]
    async fn daemon_exposes_health_endpoint_even_when_query_api_disabled() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};
        use tokio::time::Duration;

        let l1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let l2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_port = l1.local_addr().unwrap().port();
        let grpc_port = l2.local_addr().unwrap().port();
        drop(l1);
        drop(l2);

        let (_dir, socket_path) = json_socket::unique_socket_dir_and_path("daemon-health");
        let config = Config {
            daemon: crate::config::DaemonConfig {
                listen_addr: "127.0.0.1".to_string(),
                listen_port: http_port,
                listen_port_grpc: grpc_port,
                json_socket: socket_path.to_string_lossy().into_owned(),
                api_enabled: false, // proves /health is not gated by the query API toggle
                ..crate::config::DaemonConfig::default()
            },
            ..Config::default()
        };

        let daemon_handle = tokio::spawn(async move {
            let _ = run(config).await;
        });

        let addr = format!("127.0.0.1:{http_port}");
        let mut body = String::new();
        let mut ok = false;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            let Ok(mut stream) = TcpStream::connect(&addr).await else {
                continue;
            };
            let req = "GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n";
            if stream.write_all(req.as_bytes()).await.is_err() {
                continue;
            }
            let mut buf = Vec::with_capacity(1024);
            if stream.read_to_end(&mut buf).await.is_err() {
                continue;
            }
            body = String::from_utf8_lossy(&buf).into_owned();
            if body.starts_with("HTTP/1.") && body.contains(" 200 ") {
                ok = true;
                break;
            }
        }

        daemon_handle.abort();
        let _ = daemon_handle.await;

        assert!(ok, "/health should return 200 OK; got response: {body}");
        assert!(
            body.contains("\"status\":\"ok\""),
            "response body should include status=ok; got: {body}"
        );
        assert!(
            body.contains(env!("CARGO_PKG_VERSION")),
            "response body should include current version; got: {body}"
        );
    }
}
