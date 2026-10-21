//! Fold archived per-window [`Report`] envelopes into a
//! [`PeriodicReport`] builder. Wire format and per-service attribution
//! policy: `docs/design/08-PERIODIC-DISCLOSURE.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::detect::Finding;
use crate::report::Report;
use crate::score::carbon::ENERGY_PER_IO_OP_KWH;

use super::errors::AggregationError;
use super::schema::{
    Aggregate, CarbonBreakdown, DatabaseWasteAggregate, Period, TemporalCoverage, WasteTier,
};

pub const UNATTRIBUTED_SERVICE: &str = "_unattributed";

/// Cardinality cap on services tracked by the aggregator. Caps the
/// `Builder.per_service` map so that a tampered archive carrying an
/// unbounded number of distinct service strings cannot exhaust memory.
/// Overflow is folded into `UNATTRIBUTED_SERVICE`.
const MAX_SERVICES: usize = 4096;

/// Cardinality cap on distinct `energy_model` strings tracked in
/// `Builder.energy_source_models`. Overflow entries are silently dropped.
const MAX_ENERGY_MODELS: usize = 64;

/// Per-string length cap for `energy_model` entries collected from
/// archive lines. Longer values are rejected (dropped, never inserted).
const MAX_ENERGY_MODEL_LEN: usize = super::schema::MODEL_TAG_MAX_LEN;

/// Cardinality cap on distinct `binary_version` strings tracked in
/// `Builder.binary_versions`. Overflow entries are silently dropped.
/// Sized for multi-team async-release environments where a quarter can
/// span more than a dozen patch versions. The worst case,
/// 256 × 64 bytes = 16 KB, is a negligible memory budget.
const MAX_BINARY_VERSIONS: usize = 256;

/// Per-string length cap on `binary_version` entries.
const MAX_BINARY_VERSION_LEN: usize = 64;

/// Matches the JSON Schema pattern `^[A-Za-z0-9._+-]+$` for `binary_version`
/// without pulling in a regex. Rejects empty input and any byte outside the
/// allowed alphabet so a tampered archive cannot inject control chars or
/// arbitrary UTF-8 into the periodic report.
fn is_valid_binary_version(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

#[derive(Debug, Default)]
pub struct AggregateInputs {
    pub aggregate: Aggregate,
    pub per_service: BTreeMap<String, ServiceAccumulator>,
    pub windows_aggregated: u64,
    pub source_files: Vec<String>,
    pub malformed_lines_skipped: u64,
    /// Windows carrying no `disclosure_waste`, archived before canonical
    /// disclosure. Their waste fed the operational tier only, so the
    /// canonical tier omits those windows.
    pub legacy_waste_windows: u64,
    pub first_seen: BTreeMap<(String, String), DateTime<Utc>>,
    pub last_seen: BTreeMap<(String, String), DateTime<Utc>>,
    /// Distinct `energy_model` tags (without `+cal` suffix) observed
    /// across the folded windows. Empty when every window predates
    /// per-service carbon attribution.
    pub energy_source_models: BTreeSet<String>,
    /// Number of windows that carried runtime-calibrated per-service
    /// data. Together with `fallback_windows`, surfaces the share of
    /// the period that benefits from runtime attribution vs. the proxy.
    pub runtime_windows: u64,
    /// Number of windows that fell back to the I/O proxy path. Each
    /// archive file emits at most one `tracing::warn!` when its first
    /// fallback window is folded.
    pub fallback_windows: u64,
    /// `true` if at least one folded window applied calibration: its
    /// `energy_calibrated` flag, or a `+cal` suffix on its `energy_model`
    /// for archives written before the flag. Surfaced via
    /// `CalibrationInputs.calibration_applied`.
    pub calibration_applied: bool,
    /// Archive integrity: windows whose chain verified, windows written
    /// before chaining existed, and detected breaks. Published so a
    /// reader sees which part of the period is still attestable.
    pub chain_verified: u64,
    pub chain_unchained: u64,
    pub chain_breaks: u64,
    /// Breaks in the same files but outside the period. One rolling
    /// archive can cover several periods, and this report only answers
    /// for its own, so they are counted apart rather than folded in.
    pub chain_breaks_outside: u64,
    /// Windows the daemon produced but could not archive, derived from
    /// the cumulative `drops` counter on the archive lines. `None` when
    /// no line carried the counter (pre-v1.7 archives).
    pub windows_dropped: Option<u64>,
    /// Times the drop counter went backwards (daemon restarts). Each
    /// makes `windows_dropped` a lower bound over the gap it spans.
    pub drop_counter_resets: Option<u64>,
    /// Coefficient sets observed over the period, as `key=value` strings.
    pub scoring_coefficients: BTreeSet<String>,
    /// SCI methodology tags observed. Current windows use the `+transport`
    /// variant. The legacy tag can remain when a period spans older windows.
    pub carbon_methodologies: BTreeSet<String>,
    /// Oldest window found in the archive files, in or out of the period.
    /// Later than the period's first day means the archive does not reach
    /// back to it, see [`archive_reaches_period_start`].
    pub earliest_archived_window: Option<DateTime<Utc>>,
    /// Three of the five terms of the published total, in gCO2eq. The
    /// in-total database and broker host terms are in `carbon_breakdown`.
    /// Only `operational` carries an avoidable share: embodied hardware
    /// and network transport are irreducible by fixing an anti-pattern.
    pub embodied_gco2_total: f64,
    pub operational_gco2_total: f64,
    pub transport_gco2_total: f64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ServiceAccumulator {
    pub total_requests: u64,
    pub total_io_ops: u64,
    pub energy_kwh: f64,
    pub carbon_kgco2eq: f64,
    pub anti_patterns: BTreeMap<String, AntiPatternAccumulator>,
    pub endpoints_seen: BTreeSet<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AntiPatternAccumulator {
    pub occurrences: u64,
    /// Estimated avoidable I/O ops attributed to this pattern. For
    /// avoidable types (`n_plus_one_*`, `redundant_*`), sums
    /// `pattern.occurrences - 1` across findings, zero for non-avoidable
    /// types. Drives both per-service efficiency and the per-pattern
    /// `estimated_waste_*` values surfaced by `disclose`.
    pub avoidable_io_ops: u64,
}

#[derive(Debug, Deserialize)]
struct ArchivedReport {
    ts: DateTime<Utc>,
    report: Report,
}

/// Walk `paths` (files and/or directories), fold every in-period
/// archived report into a single [`AggregateInputs`].
///
/// # Errors
///
/// - [`AggregationError::InvalidInput`] if a path is neither a file nor
///   a directory.
/// - [`AggregationError::Io`] on read errors.
/// - [`AggregationError::NoWindowsInPeriod`] if zero archived windows
///   fall inside `period`.
/// - [`AggregationError::UnattributedWindow`] when `strict_attribution`
///   is set and a window has no per-service offenders.
pub fn aggregate_from_paths(
    paths: &[PathBuf],
    period: &Period,
    strict_attribution: bool,
) -> Result<AggregateInputs, AggregationError> {
    let files = resolve_files(paths)?;
    let source_files: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();

    let mut builder = Builder::default();
    let mut window_files = Vec::new();
    for path in &files {
        if is_rollup_file(path)? {
            builder.process_rollup_file(path, period, strict_attribution)?;
        } else {
            window_files.push(path);
        }
    }
    for path in window_files {
        builder.process_file(path, period, strict_attribution)?;
    }

    if builder.state.windows_aggregated == 0 {
        return Err(AggregationError::NoWindowsInPeriod);
    }

    Ok(builder.finalize(source_files, period))
}

/// Inclusive `(earliest, latest)` window timestamp covered by an archive.
pub type ArchiveTimeRange = (DateTime<Utc>, DateTime<Utc>);

/// Scan the archive `paths` for the earliest and latest window timestamp,
/// without folding the (heavy) report bodies. Each NDJSON line is parsed
/// for its `ts` field only. Returns `None` when no parseable window is
/// found. Used by the interactive `disclose --tui` preview to pick a
/// sensible default period and show the archive's covered range. The
/// canonical aggregation stays in [`aggregate_from_paths`].
///
/// # Errors
///
/// Same path-resolution and I/O errors as [`aggregate_from_paths`].
pub fn archive_time_range(paths: &[PathBuf]) -> Result<Option<ArchiveTimeRange>, AggregationError> {
    #[derive(Deserialize)]
    struct TsOnly {
        ts: DateTime<Utc>,
    }
    let mut range: Option<ArchiveTimeRange> = None;
    for path in &resolve_files(paths)? {
        let file = File::open(path).map_err(|source| AggregationError::Io {
            path: path.display().to_string(),
            source,
        })?;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|source| AggregationError::Io {
                path: path.display().to_string(),
                source,
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Malformed lines are silently skipped here (diagnostics are
            // the aggregation path's job). This scan only needs the time
            // bounds.
            if let Ok(TsOnly { ts }) = serde_json::from_str::<TsOnly>(trimmed) {
                range = Some(match range {
                    None => (ts, ts),
                    Some((lo, hi)) => (lo.min(ts), hi.max(ts)),
                });
            }
        }
    }
    Ok(range)
}

/// Per-window scalars extracted up front so `process_window` and its
/// helpers can pass a single value around instead of re-reading the
/// `Report` everywhere. Fields are derived from `green_summary` and
/// `analysis.traces_analyzed` only, never mutated downstream.
struct WindowMetrics {
    carbon_kg: f64,
    avoidable_kg: f64,
    total_io: u64,
    avoidable_io: u64,
    traces: u64,
    /// Window energy: the application part plus `host_kwh`.
    energy_kwh: f64,
    /// Database and broker host energy inside `energy_kwh`, and its carbon
    /// inside `carbon_kg`. Never attributed to an application.
    host_kwh: f64,
    host_kg: f64,
    runtime_attribution: bool,
}

impl WindowMetrics {
    fn application_energy_kwh(&self) -> f64 {
        sanitize_f64(self.energy_kwh - self.host_kwh)
    }

    fn application_carbon_kg(&self) -> f64 {
        sanitize_f64(self.carbon_kg - self.host_kg)
    }
}

/// Period-summed avoidable energy/carbon for one N+1 threshold tier.
/// `threshold` reconciled by `max` across windows. `avoidable_kg` in kg.
#[derive(Default, Serialize, Deserialize)]
struct WasteTierAccumulator {
    n_plus_one_threshold: u32,
    avoidable_io_ops: u64,
    avoidable_kwh: f64,
    avoidable_kg: f64,
}

/// Fold one window's waste block into a running accumulator. An
/// out-of-spec provenance tag drops the whole block: a figure whose
/// provenance cannot be published must not reach the sums either.
fn fold_waste_block(acc: &mut DbWasteAccumulator, block: &crate::report::DisclosureDbWaste) {
    let crate::report::DisclosureDbWaste {
        model,
        energy_kwh,
        operational_waste_kwh,
        operational_waste_gco2,
        canonical_waste_kwh,
        canonical_waste_gco2,
        energy_gco2,
        in_totals,
        placement,
    } = block;
    if !super::schema::is_valid_model_tag(model) {
        return;
    }
    let energy = sanitize_f64(*energy_kwh);
    acc.energy_kwh += energy;
    acc.operational_kwh += sanitize_f64(*operational_waste_kwh);
    acc.canonical_kwh += sanitize_f64(*canonical_waste_kwh);
    // Keep None-vs-zero: sums stay None until a window carried a carbon
    // conversion.
    add_sanitized(&mut acc.operational_g, *operational_waste_gco2);
    add_sanitized(&mut acc.canonical_g, *canonical_waste_gco2);
    // Estimated subsystem carbon is already attributed inside the service
    // total, and an in-total host figure is inside the window total. Only
    // the remaining measured or declared figures sit beside it.
    if model != crate::report::DB_WASTE_MODEL_ESTIMATED && !*in_totals {
        add_sanitized(&mut acc.energy_g, *energy_gco2);
    }
    acc.windows = acc.windows.saturating_add(1);
    fold_provenance(acc, model, energy);
    fold_host_scope(acc, model, *in_totals, *placement, energy);
    if operational_waste_gco2.is_some() || canonical_waste_gco2.is_some() {
        acc.windows_with_carbon = acc.windows_with_carbon.saturating_add(1);
    }
    // Same cap as the sibling energy-model collector.
    if acc.models.len() < MAX_BINARY_VERSIONS || acc.models.contains(model) {
        acc.models.insert(model.clone());
    }
}

/// Four provenance classes, four buckets, see
/// `docs/design/08-PERIODIC-DISCLOSURE.md`.
fn fold_provenance(acc: &mut DbWasteAccumulator, model: &str, energy: f64) {
    if model == crate::report::DB_WASTE_MODEL_ESTIMATED {
        acc.estimated_windows = acc.estimated_windows.saturating_add(1);
    } else if crate::report::is_declared_model(model) {
        acc.declared_windows = acc.declared_windows.saturating_add(1);
        acc.declared_energy_kwh += energy;
    } else if model == crate::score::carbon::CO2_MODEL_CLOUD_SPECPOWER {
        acc.modeled_windows = acc.modeled_windows.saturating_add(1);
        acc.modeled_energy_kwh += energy;
    } else {
        acc.measured_windows = acc.measured_windows.saturating_add(1);
        acc.measured_energy_kwh += energy;
    }
}

/// Whether a window's host block is a host term inside its totals. An
/// estimated figure is the application-side share, never a host term,
/// whatever its flag says, and an out-of-spec tag never reaches the sums.
fn is_in_total_host(in_totals: bool, model: &str) -> bool {
    in_totals
        && model != crate::report::DB_WASTE_MODEL_ESTIMATED
        && super::schema::is_valid_model_tag(model)
}

/// The in-total share and the declared placement.
fn fold_host_scope(
    acc: &mut DbWasteAccumulator,
    model: &str,
    in_totals: bool,
    placement: Option<crate::score::host_static::HostPlacement>,
    energy: f64,
) {
    if is_in_total_host(in_totals, model) {
        acc.in_totals_windows = acc.in_totals_windows.saturating_add(1);
        acc.in_totals_energy_kwh += energy;
    }
    if let Some(p) = placement {
        acc.placements.insert(p.as_str().to_string());
    }
}

/// Add a sanitized optional figure, leaving `slot` `None` while every
/// value seen so far was absent.
fn add_sanitized(slot: &mut Option<f64>, value: Option<f64>) {
    if let Some(v) = value {
        *slot = Some(slot.unwrap_or(0.0) + sanitize_f64(v));
    }
}

/// Struct-level `default`: a rollup line written before a field existed
/// still deserializes.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct DbWasteAccumulator {
    energy_kwh: f64,
    measured_energy_kwh: f64,
    modeled_energy_kwh: f64,
    in_totals_energy_kwh: f64,
    operational_kwh: f64,
    /// `None` until a window carried a carbon conversion, so an absent
    /// conversion is not published as an affirmative zero.
    operational_g: Option<f64>,
    canonical_kwh: f64,
    canonical_g: Option<f64>,
    /// Carbon of the measured or declared figures that stayed beside the
    /// totals: windows archived before host accounting, or without carbon.
    energy_g: Option<f64>,
    models: BTreeSet<String>,
    windows: u64,
    measured_windows: u64,
    declared_energy_kwh: f64,
    declared_windows: u64,
    estimated_windows: u64,
    modeled_windows: u64,
    in_totals_windows: u64,
    placements: BTreeSet<String>,
    windows_with_carbon: u64,
}

/// What the windows of a period measured, independent of how they were
/// read. Folded window by window, and merged whole when a daily rollup
/// carries it, see `rollup.rs`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PeriodState {
    per_service: BTreeMap<String, ServiceAccumulator>,
    windows_aggregated: u64,
    malformed_lines_skipped: u64,
    legacy_waste_windows: u64,
    #[serde(with = "pair_key_map")]
    first_seen: BTreeMap<(String, String), DateTime<Utc>>,
    #[serde(with = "pair_key_map")]
    last_seen: BTreeMap<(String, String), DateTime<Utc>>,
    total_requests: u64,
    total_io_ops: u64,
    total_carbon_kgco2eq: f64,
    /// Avoidable tiers from each window's `Report.disclosure_waste`.
    canonical_waste: WasteTierAccumulator,
    operational_waste: WasteTierAccumulator,
    /// Database-waste sums from each window's `disclosure_waste.database`.
    /// Windows predating the block are not folded (no canonical figure),
    /// so both tiers stay consistent.
    db_waste: DbWasteAccumulator,
    msg_waste: DbWasteAccumulator,
    /// Sum of runtime-calibrated `energy_kwh` for windows that carry it.
    runtime_energy_kwh: f64,
    /// Distinct energy model strings collected across all windows. The
    /// `+cal` suffix is stripped so consumers see the bare source tag.
    energy_source_models: BTreeSet<String>,
    /// Windows that carried `green_summary.energy_kwh > 0` or non-empty
    /// per-service runtime maps.
    runtime_windows: u64,
    /// Windows that fell back to the I/O proxy path. Used by tests and
    /// surfaced via [`AggregateInputs`] for operator diagnostics.
    fallback_windows: u64,
    /// Distinct `binary_version` values observed across the folded
    /// windows. Empty when every window predates the field.
    binary_versions: BTreeSet<String>,
    /// Set when at least one window's `energy_model` carried the `+cal`
    /// suffix, indicating operator calibration was active for that window.
    calibration_applied: bool,
    /// SCI methodology tags observed, bounded by `MAX_ENERGY_MODELS`.
    carbon_methodologies: BTreeSet<String>,
    /// Distinct coefficient sets observed, as `"key=value"` strings. A
    /// set that changed mid-period yields more than one entry.
    scoring_coefficients: BTreeSet<String>,
    /// Set once a window contributed transport under a coefficient that
    /// is not the fixed one, or under none this binary can read.
    transport_coefficient_uncertain: bool,
    /// Running sums of the first three terms of the published total, in
    /// gCO2eq.
    embodied_gco2_total: f64,
    operational_gco2_total: f64,
    transport_gco2_total: f64,
    /// In-total database and broker host carbon, in gCO2eq, the fourth and
    /// fifth terms of the published total.
    database_gco2_total: f64,
    messaging_gco2_total: f64,
    /// Per-service set of distinct energy model tags accumulated across
    /// the period's windows. The `+cal` suffix is stripped before
    /// insertion. Service cardinality is bounded by `MAX_SERVICES`,
    /// each inner set by `MAX_ENERGY_MODELS`.
    per_service_energy_models: BTreeMap<String, BTreeSet<String>>,
    /// Sum and count of per-window `per_service_measured_ratio` values,
    /// keyed by service. Finalized to a per-service mean in `finalize`.
    per_service_measured_ratio_sums: BTreeMap<String, (f64, u32)>,
    /// Distinct UTC calendar days that carried >= 1 folded window. Bounded
    /// by the period length (<= 366 for a calendar year), no cap needed.
    /// Drives the v1.2 temporal-coverage continuity signal.
    observed_days: BTreeSet<NaiveDate>,
    /// Earliest window folded into the unattributed bucket. A rollup is
    /// folded without `--strict-attribution`, so the reader needs this to
    /// refuse it later when the flag is set.
    first_unattributed: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct Builder {
    state: PeriodState,
    /// Archive lines whose hash chain checked out, carried no chain at
    /// all (pre-chaining archives), or failed to verify.
    chain_verified: u64,
    chain_unchained: u64,
    chain_breaks: u64,
    chain_breaks_outside: u64,
    /// Sum of in-period deltas of the cumulative `drops` counter, and
    /// whether any line carried it at all (pre-v1.7 archives carry none,
    /// and `0` must stay distinguishable from "not measured").
    windows_dropped: u64,
    drop_counter_resets: u64,
    drops_observed: bool,
    /// Previous cumulative `drops` value per archive family, keyed by
    /// the family stem. The counter is daemon-lifetime, so the baseline
    /// must survive a rotation, and it must NOT cross into an unrelated
    /// archive: `disclose` takes a list of paths, and diffing one host's
    /// counter against another's would invent resets.
    last_drops: BTreeMap<String, u64>,
    /// Oldest well-formed window across every input line, period or not.
    earliest_archived: Option<DateTime<Utc>>,
    /// Windows the rollup lines already folded, as `[first, last]` window
    /// timestamps per archive family (the window archive a rollup file
    /// sits beside). A window line inside one is read for its chain and
    /// drop counter only, so no window counts twice. Windows outside every
    /// span, archived before the upgrade or after a crash's last snapshot,
    /// still count from their window lines.
    rollup_spans: BTreeMap<String, Vec<WindowSpan>>,
    /// Chain anchor per rollup family. Unlike window files, a rollup chain
    /// runs across its monthly files, so a removed month shows as a break.
    rollup_anchors: BTreeMap<String, ChainAnchor>,
    /// Oldest well-formed window whose chain verified. When any line is
    /// chained, the reach check reads this one: a line without a hash put
    /// in front of a chained archive must not stand in for pruned windows.
    earliest_chained: Option<DateTime<Utc>>,
}

/// The chain anchor: the previous line's hash and its sequence number.
type ChainAnchor = Option<(String, u64)>;

/// First and last window timestamps a rollup line folded.
type WindowSpan = (DateTime<Utc>, DateTime<Utc>);

/// One archive line handed to [`Builder::fold_window`]. Grouped like
/// [`ChainStep`]: the mutable `warned_fallback` has to travel with the
/// read-only per-line context, and seven loose arguments would sit on
/// clippy's ceiling.
struct WindowStep<'a> {
    parsed: Option<serde_json::Value>,
    trimmed: &'a str,
    period: &'a Period,
    strict: bool,
    path: &'a Path,
    line_no: usize,
    warned_fallback: &'a mut bool,
    /// A rollup line already folded this window.
    covered: bool,
    /// The line's own hash and its `prev` both checked out.
    chain_verified: bool,
}

/// One line's worth of chain state, threaded through [`Builder::walk_chain_line`].
struct ChainStep<'a> {
    parsed: Option<&'a mut serde_json::Value>,
    expected: &'a mut ChainAnchor,
    chain_started: &'a mut bool,
    in_scope: bool,
    previous_in_scope: bool,
    next_seq: &'a dyn Fn(&ChainAnchor) -> u64,
    path: &'a Path,
    line_no: usize,
    warned_break: &'a mut bool,
    /// Windows this line attests: one for a window line, the folded count
    /// for a rollup line.
    weight: u64,
}

impl Builder {
    /// Advance the integrity chain by one archive line. Split out of
    /// `process_file` so that loop stays under the complexity gate.
    /// Returns whether the line verified.
    fn walk_chain_line(&mut self, step: ChainStep<'_>) -> bool {
        // An unparseable line is a crash-truncated fragment, not an edit.
        // The anchor is kept, so a destroyed window still surfaces as a
        // break through its successor's `prev`.
        let outcome = step.parsed.map_or(ChainOutcome::Malformed, |value| {
            verify_chain_value(value, step.expected.as_ref())
        });
        let verified = matches!(outcome, ChainOutcome::Verified(_));
        match outcome {
            // The typed fold counts it under malformed_lines_skipped.
            ChainOutcome::Malformed => {}
            ChainOutcome::Verified(hash) => {
                *step.chain_started = true;
                if step.in_scope {
                    self.chain_verified += step.weight;
                }
                let seq = (step.next_seq)(step.expected);
                *step.expected = Some((hash, seq));
            }
            // Unchained is benign only before the file's chain starts:
            // those lines predate chaining. Once a line has verified, a
            // later one without a `hash` had the field removed, an edit
            // the chain exists to catch.
            ChainOutcome::Unchained if !*step.chain_started => {
                if step.in_scope {
                    self.chain_unchained += step.weight;
                }
            }
            ChainOutcome::Unchained => {
                self.count_break(step.in_scope || step.previous_in_scope);
                // No hash to chain onto, so the anchor is dropped and the
                // next chained line re-establishes it.
                *step.expected = None;
                warn_break(step.path, step.line_no, step.warned_break);
            }
            ChainOutcome::Break(hash) => {
                *step.chain_started = true;
                // The current line reveals a removed predecessor. If that
                // predecessor was the last in-period line, the break
                // affects this report even when the revealing line itself
                // is just outside the boundary.
                self.count_break(step.in_scope || step.previous_in_scope);
                // Resynchronise on this line's own hash and seq, so one
                // edit reports one break rather than poisoning the tail.
                let seq = (step.next_seq)(step.expected);
                *step.expected = Some((hash, seq));
                warn_break(step.path, step.line_no, step.warned_break);
            }
        }
        verified
    }
}

impl Builder {
    fn process_file(
        &mut self,
        path: &Path,
        period: &Period,
        strict: bool,
    ) -> Result<(), AggregationError> {
        let file = File::open(path).map_err(|source| AggregationError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let reader = BufReader::new(file);
        let mut warned_fallback = false;
        // Chain state is per file: a rotated file restarts from the seed.
        // `None` means the walk lost its anchor and adopts whatever the
        // next chained line declares, so one damaged line costs one break.
        let mut expected: Option<(String, u64)> =
            Some((super::hasher::ARCHIVE_CHAIN_SEED.to_string(), 0));
        let mut warned_break = false;
        let mut chain_started = false;
        let mut previous_in_scope = false;
        let family = archive_family(path);
        for (line_no, line) in reader.lines().enumerate() {
            let line = line.map_err(|source| AggregationError::Io {
                path: path.display().to_string(),
                source,
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Parsed once: the chain check and the typed fold share this
            // value, instead of running two full JSON parses per line.
            let mut parsed: Option<serde_json::Value> = serde_json::from_str(trimmed).ok();
            let parsed_seq = parsed
                .as_ref()
                .and_then(|v| v.get("seq"))
                .and_then(serde_json::Value::as_u64);
            // Every line is walked, including those outside the period, or
            // an edit just outside the window would go unseen. The line's
            // own timestamp decides which counter it lands in: one
            // rolling archive can span several periods, and a 2024 edit
            // must not be published as this quarter's break.
            let covered = self.rollup_covers(&family, parsed.as_ref());
            let in_scope = line_in_period(parsed.as_ref(), period) && !covered;
            if let Some(drops) = parsed
                .as_ref()
                .and_then(|v| v.get("drops"))
                .and_then(serde_json::Value::as_u64)
            {
                self.fold_drops(&family, drops, in_scope);
            }
            let next_seq = |expected: &Option<(String, u64)>| {
                parsed_seq
                    .unwrap_or_else(|| expected.as_ref().map_or(0, |(_, seq)| *seq))
                    .saturating_add(1)
            };
            let chain_verified = self.walk_chain_line(ChainStep {
                parsed: parsed.as_mut(),
                expected: &mut expected,
                chain_started: &mut chain_started,
                in_scope,
                previous_in_scope,
                next_seq: &next_seq,
                path,
                line_no,
                warned_break: &mut warned_break,
                weight: 1,
            });
            if parsed.is_some() {
                previous_in_scope = in_scope;
            }
            self.fold_window(WindowStep {
                parsed,
                trimmed,
                period,
                strict,
                path,
                line_no,
                warned_fallback: &mut warned_fallback,
                covered,
                chain_verified,
            })?;
        }
        Ok(())
    }

    /// Fold one archive line into the period, or count it as malformed.
    /// Split out of `process_file` for the same reason `walk_chain_line`
    /// is: the loop sits on the cognitive-complexity gate, and this branch
    /// carried the largest share of it.
    fn fold_window(&mut self, step: WindowStep<'_>) -> Result<(), AggregationError> {
        // Reuse the value the chain walk already parsed when there is one,
        // instead of parsing the same line a second time.
        let typed = step.parsed.map_or_else(
            || serde_json::from_str::<ArchivedReport>(step.trimmed),
            serde_json::from_value::<ArchivedReport>,
        );
        let envelope = match typed {
            Ok(envelope) => envelope,
            Err(err) => {
                self.state.malformed_lines_skipped += 1;
                tracing::warn!(
                    path = %step.path.display(),
                    line = step.line_no + 1,
                    error = %err,
                    "skipping malformed archive line",
                );
                return Ok(());
            }
        };
        self.earliest_archived = Some(
            self.earliest_archived
                .map_or(envelope.ts, |seen| seen.min(envelope.ts)),
        );
        if step.chain_verified {
            self.earliest_chained = Some(
                self.earliest_chained
                    .map_or(envelope.ts, |seen| seen.min(envelope.ts)),
            );
        }
        if !in_period(envelope.ts, step.period) || step.covered {
            return Ok(());
        }
        let used_fallback =
            self.state
                .process_window(&envelope.report, envelope.ts, step.strict)?;
        if used_fallback && !*step.warned_fallback {
            *step.warned_fallback = true;
            tracing::warn!(
                path = %step.path.display(),
                "archive predates per-service carbon attribution; \
                 falling back to I/O share proxy for this file",
            );
        }
        Ok(())
    }

    /// Fold one line's cumulative `drops` counter. The delta between two
    /// consecutive carrying lines is the loss between them, attributed to
    /// the later line's period. A decrease is a daemon restart: the count
    /// restarts from the new value (its drops happened after the restart)
    /// and the reset is surfaced so the figure reads as a lower bound.
    /// The first carrying line of an archive family sets that family's
    /// baseline without contributing: pruned rotations may hide arbitrary
    /// history behind its absolute value. The baseline then persists
    /// across that family's rotations, so a rotation boundary keeps its
    /// delta, and never leaks into another family's counter.
    ///
    /// `drops_observed` moves only on in-period lines: an out-of-period
    /// carrying line must not turn "not measured" into "measured zero".
    fn fold_drops(&mut self, family: &str, drops: u64, in_scope: bool) {
        if in_scope {
            self.drops_observed = true;
        }
        let Some(slot) = self.last_drops.get_mut(family) else {
            // First carrying line of this family: baseline only.
            self.last_drops.insert(family.to_string(), drops);
            return;
        };
        let prev = std::mem::replace(slot, drops);
        if !in_scope {
            return;
        }
        if drops < prev {
            self.drop_counter_resets += 1;
            self.windows_dropped = self.windows_dropped.saturating_add(drops);
        } else {
            self.windows_dropped = self
                .windows_dropped
                .saturating_add(drops.saturating_sub(prev));
        }
    }

    fn count_break(&mut self, in_scope: bool) {
        if in_scope {
            self.chain_breaks += 1;
        } else {
            self.chain_breaks_outside += 1;
        }
    }
}

impl PeriodState {
    pub(crate) fn process_window(
        &mut self,
        report: &Report,
        ts: DateTime<Utc>,
        strict: bool,
    ) -> Result<bool, AggregationError> {
        let Some(m) = self.compute_window_metrics(report, ts) else {
            return Ok(false);
        };

        self.fold_global_counters(&m);
        // Count the day only once the window is committed (after the
        // non-finite-carbon guard), keeping observed_days aligned with
        // windows_aggregated.
        self.observed_days.insert(ts.date_naive());
        self.fold_disclosure_waste(report, &m);
        self.fold_binary_version(&report.binary_version);
        self.fold_window_energy_model(
            &report.green_summary.energy_model,
            report.green_summary.energy_calibrated,
        );
        self.fold_carbon_methodology(&report.green_summary);
        self.fold_transport_coefficient(
            report.green_summary.co2.as_ref(),
            report.green_summary.scoring_config.as_ref(),
        );
        self.fold_scoring_coefficients(report.green_summary.scoring_config.as_ref());
        self.fold_per_service_measured_ratio(&report.green_summary.per_service_measured_ratio);
        self.fold_per_service_energy_models(&report.green_summary.per_service_energy_model);

        let per_service_io = service_io_distribution(&report.per_endpoint_io_ops);
        let unattributed = per_service_io.is_empty() && !m.runtime_attribution;
        if unattributed && strict {
            return Err(AggregationError::UnattributedWindow {
                ts: ts.to_rfc3339(),
            });
        }
        if unattributed {
            self.first_unattributed = Some(self.first_unattributed.map_or(ts, |seen| seen.min(ts)));
        }

        self.attribute_window(report, &m, &per_service_io, unattributed);
        self.route_findings(&report.findings, ts, unattributed);

        Ok(!m.runtime_attribution)
    }

    /// Validate, then capture the per-window scalars the rest of
    /// `process_window` needs. Returns `None` (and bumps the malformed
    /// counter) when the carbon fields are non-finite, signalling the
    /// caller to skip the window.
    fn compute_window_metrics(
        &mut self,
        report: &Report,
        ts: DateTime<Utc>,
    ) -> Option<WindowMetrics> {
        let carbon_kg = report
            .green_summary
            .co2
            .as_ref()
            .map_or(0.0, |c| c.total.mid / 1000.0);
        let avoidable_kg = report
            .green_summary
            .co2
            .as_ref()
            .map_or(0.0, |c| c.avoidable.mid / 1000.0);
        if !carbon_kg.is_finite() || !avoidable_kg.is_finite() {
            self.malformed_lines_skipped += 1;
            tracing::warn!(ts = %ts, "skipping window with non-finite carbon");
            return None;
        }
        let (host_kwh, host_kg) = in_total_host_terms(&report.green_summary);
        // Sanitize against `+Inf` from tampered archives. NaN / -Inf /
        // negative inputs fall through the `> 0.0` check to the proxy
        // path. The post-clamp catches the remaining `+Inf` case. The proxy
        // prices the application part only, the host energy is kept.
        let application = report.green_summary.energy_kwh - host_kwh;
        let raw_application = if application > 0.0 {
            application
        } else {
            (report.green_summary.total_io_ops as f64) * ENERGY_PER_IO_OP_KWH
        };
        Some(WindowMetrics {
            carbon_kg,
            avoidable_kg,
            total_io: report.green_summary.total_io_ops as u64,
            avoidable_io: report.green_summary.avoidable_io_ops as u64,
            traces: report.analysis.traces_analyzed as u64,
            energy_kwh: sanitize_f64(raw_application) + host_kwh,
            host_kwh,
            host_kg,
            runtime_attribution: !report.green_summary.per_service_carbon_kgco2eq.is_empty()
                && !report.green_summary.per_service_energy_kwh.is_empty(),
        })
    }

    fn fold_global_counters(&mut self, m: &WindowMetrics) {
        self.windows_aggregated += 1;
        self.total_requests = self.total_requests.saturating_add(m.traces);
        self.total_io_ops = self.total_io_ops.saturating_add(m.total_io);
        self.total_carbon_kgco2eq += m.carbon_kg;
        self.runtime_energy_kwh += m.energy_kwh;
    }

    /// Accumulate the canonical and operational avoidable tiers. A legacy
    /// archive (no `disclosure_waste`) has no canonical figure, so it feeds
    /// only the operational tier (best-effort from `green_summary`). The
    /// canonical tier is left untouched rather than contaminated with
    /// operator-threshold data, so an all-legacy period fails official
    /// validation instead of presenting legacy data as canonical.
    fn fold_disclosure_waste(&mut self, report: &Report, m: &WindowMetrics) {
        if let Some(dw) = &report.disclosure_waste {
            fold_tier(&mut self.canonical_waste, &dw.canonical);
            fold_tier(&mut self.operational_waste, &dw.operational);
            if let Some(db) = &dw.database {
                fold_waste_block(&mut self.db_waste, db);
                self.record_in_total_model(db);
            }
            if let Some(mw) = &dw.messaging {
                fold_waste_block(&mut self.msg_waste, mw);
                self.record_in_total_model(mw);
            }
        } else {
            self.legacy_waste_windows += 1;
            // accounted_io_ops is not serialized, so the legacy energy share
            // uses total_io as the denominator (clamped). Threshold stays 0.
            let ratio = if m.total_io == 0 {
                0.0
            } else {
                (m.avoidable_io as f64 / m.total_io as f64).min(1.0)
            };
            self.operational_waste.avoidable_io_ops = self
                .operational_waste
                .avoidable_io_ops
                .saturating_add(m.avoidable_io);
            self.operational_waste.avoidable_kwh += m.energy_kwh * ratio;
            self.operational_waste.avoidable_kg += m.avoidable_kg;
        }
    }

    /// An in-total host block's energy is part of `total_energy_kwh`, so
    /// its model is one of the period's energy sources.
    fn record_in_total_model(&mut self, block: &crate::report::DisclosureDbWaste) {
        if is_in_total_host(block.in_totals, &block.model) {
            self.record_energy_model_tag(&block.model);
        }
    }

    fn fold_binary_version(&mut self, bv: &str) {
        if bv.is_empty() || bv.len() > MAX_BINARY_VERSION_LEN || !is_valid_binary_version(bv) {
            return;
        }
        if self.binary_versions.len() < MAX_BINARY_VERSIONS || self.binary_versions.contains(bv) {
            self.binary_versions.insert(bv.to_string());
        }
    }

    /// Whether the low/high bracket can be published: it only frames the
    /// fixed coefficient, so any window carrying transport under another
    /// value, or under none we can read, disqualifies the whole period.
    /// Absent is disqualifying, not default: windows archived before
    /// 0.9.25 record no coefficient at all and could hold any value.
    fn fold_transport_coefficient(
        &mut self,
        co2: Option<&crate::score::carbon::CarbonReport>,
        cfg: Option<&crate::score::carbon::ScoringConfig>,
    ) {
        let contributed = co2.is_some_and(|c| c.transport_gco2.unwrap_or(0.0) > 0.0);
        if !contributed {
            return;
        }
        let applied = cfg.and_then(|c| c.network_energy_per_byte_kwh);
        let is_default = applied.is_some_and(|v| {
            (v - crate::score::carbon::DEFAULT_NETWORK_ENERGY_PER_BYTE_KWH).abs() < f64::EPSILON
        });
        if !is_default {
            self.transport_coefficient_uncertain = true;
        }
    }

    /// Record the coefficients one window was scored with. They scale the
    /// published figures and appear nowhere else, so a period that changed
    /// them shows both values rather than one.
    fn fold_scoring_coefficients(&mut self, cfg: Option<&crate::score::carbon::ScoringConfig>) {
        let Some(cfg) = cfg else { return };
        let mut push = |entry: String| {
            if self.scoring_coefficients.len() < MAX_ENERGY_MODELS
                || self.scoring_coefficients.contains(&entry)
            {
                self.scoring_coefficients.insert(entry);
            }
        };
        if let Some(v) = cfg.embodied_per_request_gco2 {
            push(format!("embodied_gco2_per_request={v}"));
        }
        if let Some(v) = cfg.network_energy_per_byte_kwh {
            push(format!("network_kwh_per_byte={v}"));
        }
        if let Some(v) = cfg.per_operation_coefficients {
            push(format!("per_operation_coefficients={v}"));
        }
        if let Some(v) = cfg.use_hourly_profiles {
            push(format!("use_hourly_profiles={v}"));
        }
    }

    /// Collect the methodology tag and the terms of one window's total:
    /// operational, embodied, transport, and the in-total database and
    /// broker hosts. The split tells a reader how much of the published
    /// total is reducible at all.
    fn fold_carbon_methodology(&mut self, gs: &crate::report::GreenSummary) {
        let Some(co2) = gs.co2.as_ref() else { return };
        self.embodied_gco2_total += sanitize_f64(co2.embodied_gco2);
        self.operational_gco2_total += sanitize_f64(co2.operational_gco2);
        self.transport_gco2_total += sanitize_f64(co2.transport_gco2.unwrap_or(0.0));
        let (database_gco2, messaging_gco2) = in_total_host_gco2(gs);
        self.database_gco2_total += database_gco2;
        self.messaging_gco2_total += messaging_gco2;
        let tag = co2.total.methodology.as_str();
        if tag.is_empty() || tag.len() > MAX_ENERGY_MODEL_LEN {
            return;
        }
        if self.carbon_methodologies.len() < MAX_ENERGY_MODELS
            || self.carbon_methodologies.contains(tag)
        {
            self.carbon_methodologies.insert(tag.to_string());
        }
    }

    /// `calibrated` is the window's `energy_calibrated` flag: a measured
    /// or real-time window tag drops `+cal`, the flag does not.
    fn fold_window_energy_model(&mut self, model: &str, calibrated: bool) {
        self.calibration_applied |= calibrated;
        if model.is_empty() || model.len() > MAX_ENERGY_MODEL_LEN {
            return;
        }
        self.record_energy_model_tag(model);
    }

    /// Strip the `+cal` suffix, flip the calibration flag if present,
    /// and insert the bare tag into `energy_source_models` subject to
    /// the model-set cap.
    fn record_energy_model_tag(&mut self, raw: &str) {
        let bare = raw.strip_suffix("+cal").unwrap_or(raw);
        if raw.len() != bare.len() {
            self.calibration_applied = true;
        }
        if self.energy_source_models.len() < MAX_ENERGY_MODELS
            || self.energy_source_models.contains(bare)
        {
            self.energy_source_models.insert(bare.to_string());
        }
    }

    fn fold_per_service_measured_ratio(&mut self, map: &BTreeMap<String, f64>) {
        for (service, ratio) in map {
            // Symmetric clamp: `sanitize_f64` maps NaN/Inf/negative to
            // 0.0, `.min(1.0)` maps overshoots to 1.0. Both are treated
            // as "out of spec" rather than dropped, so the period mean
            // stays defined.
            let ratio = sanitize_f64(*ratio).min(1.0);
            let entry =
                if let Some(existing) = self.per_service_measured_ratio_sums.get_mut(service) {
                    existing
                } else if self.per_service_measured_ratio_sums.len() >= MAX_SERVICES {
                    continue;
                } else {
                    self.per_service_measured_ratio_sums
                        .entry(service.clone())
                        .or_insert((0.0, 0))
                };
            entry.0 += ratio;
            entry.1 = entry.1.saturating_add(1);
        }
    }

    fn fold_per_service_energy_models(&mut self, map: &BTreeMap<String, String>) {
        for (service, raw_model) in map {
            if raw_model.is_empty() || raw_model.len() > MAX_ENERGY_MODEL_LEN {
                continue;
            }
            self.record_energy_model_tag(raw_model);
            let bare = raw_model.strip_suffix("+cal").unwrap_or(raw_model);
            let set = if let Some(existing) = self.per_service_energy_models.get_mut(service) {
                existing
            } else if self.per_service_energy_models.len() >= MAX_SERVICES {
                continue;
            } else {
                self.per_service_energy_models
                    .entry(service.clone())
                    .or_default()
            };
            if set.len() < MAX_ENERGY_MODELS || set.contains(bare) {
                set.insert(bare.to_string());
            }
        }
    }

    fn attribute_window(
        &mut self,
        report: &Report,
        m: &WindowMetrics,
        per_service_io: &BTreeMap<String, u64>,
        unattributed: bool,
    ) {
        if m.runtime_attribution {
            self.attribute_runtime(report, m, per_service_io);
        } else if unattributed {
            self.attribute_unattributed(m);
        } else {
            self.attribute_proxy_share(report, m, per_service_io);
        }
    }

    fn attribute_runtime(
        &mut self,
        report: &Report,
        m: &WindowMetrics,
        per_service_io: &BTreeMap<String, u64>,
    ) {
        self.runtime_windows += 1;
        for (service, carbon) in &report.green_summary.per_service_carbon_kgco2eq {
            let carbon = sanitize_f64(*carbon);
            let energy = sanitize_f64(
                report
                    .green_summary
                    .per_service_energy_kwh
                    .get(service)
                    .copied()
                    .unwrap_or(0.0),
            );
            let Some(bucket) = bounded_entry(&mut self.per_service, service) else {
                continue;
            };
            bucket.carbon_kgco2eq += carbon;
            bucket.energy_kwh += energy;
            if let Some(io) = per_service_io.get(service) {
                bucket.total_io_ops += *io;
                let share = if m.total_io == 0 {
                    0.0
                } else {
                    *io as f64 / m.total_io as f64
                };
                bucket.total_requests += scale_u64(m.traces, share);
            }
        }
        collect_endpoints_seen(&mut self.per_service, &report.per_endpoint_io_ops);
    }

    fn attribute_unattributed(&mut self, m: &WindowMetrics) {
        self.fallback_windows += 1;
        let bucket = self
            .per_service
            .entry(UNATTRIBUTED_SERVICE.to_string())
            .or_default();
        bucket.total_requests += m.traces;
        bucket.total_io_ops += m.total_io;
        bucket.energy_kwh += m.application_energy_kwh();
        bucket.carbon_kgco2eq += m.application_carbon_kg();
    }

    fn attribute_proxy_share(
        &mut self,
        report: &Report,
        m: &WindowMetrics,
        per_service_io: &BTreeMap<String, u64>,
    ) {
        self.fallback_windows += 1;
        let total_window_io: u64 = per_service_io.values().sum();
        let (energy, carbon) = (m.application_energy_kwh(), m.application_carbon_kg());
        for (service, io) in per_service_io {
            let share = if total_window_io == 0 {
                0.0
            } else {
                *io as f64 / total_window_io as f64
            };
            let Some(bucket) = bounded_entry(&mut self.per_service, service) else {
                continue;
            };
            bucket.total_io_ops += *io;
            bucket.total_requests += scale_u64(m.traces, share);
            bucket.energy_kwh += energy * share;
            bucket.carbon_kgco2eq += carbon * share;
        }
        collect_endpoints_seen(&mut self.per_service, &report.per_endpoint_io_ops);
    }

    fn route_findings(&mut self, findings: &[Finding], ts: DateTime<Utc>, unattributed: bool) {
        // Route findings to the unattributed bucket when the window had
        // no per-service offenders or runtime maps, so a service never
        // publishes efficiency=100 alongside non-zero
        // anti_patterns_detected_count.
        let fold = unattributed.then_some(UNATTRIBUTED_SERVICE);
        for finding in findings {
            let pattern: &'static str = finding.finding_type.as_str();
            if !finding.finding_type.is_avoidable_io() {
                self.credit(fold.unwrap_or(&finding.service), pattern, ts, 1, 0);
                continue;
            }
            // The finding counts once, on its owner. Its avoidable ops go
            // to the services whose spans they are, the owner first so a
            // refused owner drops the finding whole.
            for (service, ops) in finding.avoidable_by_service() {
                let counted = u64::from(service == finding.service);
                let admitted =
                    self.credit(fold.unwrap_or(service), pattern, ts, counted, ops as u64);
                if !admitted && counted == 1 {
                    break;
                }
            }
        }
    }

    /// Add to one `(service, pattern)` row. False when the service cap
    /// refuses a new service.
    fn credit(
        &mut self,
        service: &str,
        pattern: &str,
        ts: DateTime<Utc>,
        occurrences: u64,
        avoidable: u64,
    ) -> bool {
        let Some(bucket) = bounded_entry(&mut self.per_service, service) else {
            return false;
        };
        let ap = bucket.anti_patterns.entry(pattern.to_string()).or_default();
        ap.occurrences += occurrences;
        ap.avoidable_io_ops = ap.avoidable_io_ops.saturating_add(avoidable);
        self.update_seen_timestamps(service, pattern, ts);
        true
    }

    fn update_seen_timestamps(&mut self, service_key: &str, pattern: &str, ts: DateTime<Utc>) {
        let key = (service_key.to_string(), pattern.to_string());
        self.first_seen
            .entry(key.clone())
            .and_modify(|prev| {
                if ts < *prev {
                    *prev = ts;
                }
            })
            .or_insert(ts);
        self.last_seen
            .entry(key)
            .and_modify(|prev| {
                if ts > *prev {
                    *prev = ts;
                }
            })
            .or_insert(ts);
    }

    /// Prefer the sum of runtime-calibrated `energy_kwh` accumulated from
    /// each window. Falls back to per-service energy, already proxy when
    /// no runtime data exists.
    fn total_energy_kwh(&self) -> f64 {
        if self.runtime_energy_kwh > 0.0 {
            self.runtime_energy_kwh
        } else {
            self.per_service.values().map(|s| s.energy_kwh).sum()
        }
    }
}

impl PeriodState {
    pub(crate) fn first_unattributed(&self) -> Option<DateTime<Utc>> {
        self.first_unattributed
    }

    pub(crate) fn windows_aggregated(&self) -> u64 {
        self.windows_aggregated
    }

    /// Restrict the observed days to `day`, the only one a rollup line may
    /// answer for. An empty state keeps no day.
    pub(crate) fn keep_only_day(&mut self, day: NaiveDate) {
        self.observed_days.clear();
        if self.windows_aggregated > 0 {
            self.observed_days.insert(day);
        }
    }

    #[cfg(test)]
    pub(crate) fn insert_observed_day_for_test(&mut self, day: NaiveDate) {
        self.observed_days.insert(day);
    }

    /// Fold another state in. Each field merges by the operation its
    /// window fold uses, and every guard that path applies to untrusted
    /// archive input applies again: a rollup line is read from a file.
    pub(crate) fn merge(&mut self, other: PeriodState) {
        let PeriodState {
            per_service,
            windows_aggregated,
            malformed_lines_skipped,
            legacy_waste_windows,
            first_seen,
            last_seen,
            total_requests,
            total_io_ops,
            total_carbon_kgco2eq,
            canonical_waste,
            operational_waste,
            db_waste,
            msg_waste,
            runtime_energy_kwh,
            energy_source_models,
            runtime_windows,
            fallback_windows,
            binary_versions,
            calibration_applied,
            carbon_methodologies,
            scoring_coefficients,
            transport_coefficient_uncertain,
            embodied_gco2_total,
            operational_gco2_total,
            transport_gco2_total,
            database_gco2_total,
            messaging_gco2_total,
            per_service_energy_models,
            per_service_measured_ratio_sums,
            observed_days,
            first_unattributed,
        } = other;
        self.windows_aggregated = self.windows_aggregated.saturating_add(windows_aggregated);
        self.malformed_lines_skipped = self
            .malformed_lines_skipped
            .saturating_add(malformed_lines_skipped);
        self.legacy_waste_windows = self
            .legacy_waste_windows
            .saturating_add(legacy_waste_windows);
        self.total_requests = self.total_requests.saturating_add(total_requests);
        self.total_io_ops = self.total_io_ops.saturating_add(total_io_ops);
        self.runtime_windows = self.runtime_windows.saturating_add(runtime_windows);
        self.fallback_windows = self.fallback_windows.saturating_add(fallback_windows);
        self.total_carbon_kgco2eq += sanitize_f64(total_carbon_kgco2eq);
        self.runtime_energy_kwh += sanitize_f64(runtime_energy_kwh);
        self.embodied_gco2_total += sanitize_f64(embodied_gco2_total);
        self.operational_gco2_total += sanitize_f64(operational_gco2_total);
        self.transport_gco2_total += sanitize_f64(transport_gco2_total);
        self.database_gco2_total += sanitize_f64(database_gco2_total);
        self.messaging_gco2_total += sanitize_f64(messaging_gco2_total);
        self.calibration_applied |= calibration_applied;
        self.transport_coefficient_uncertain |= transport_coefficient_uncertain;
        merge_tier(&mut self.canonical_waste, &canonical_waste);
        merge_tier(&mut self.operational_waste, &operational_waste);
        merge_waste_block(&mut self.db_waste, db_waste);
        merge_waste_block(&mut self.msg_waste, msg_waste);
        self.merge_services(per_service);
        self.merge_seen(first_seen, last_seen);
        self.merge_tags(binary_versions, energy_source_models, carbon_methodologies);
        self.merge_coefficients(scoring_coefficients);
        self.merge_per_service_maps(per_service_energy_models, per_service_measured_ratio_sums);
        self.observed_days.extend(observed_days);
        if let Some(ts) = first_unattributed {
            self.first_unattributed = Some(self.first_unattributed.map_or(ts, |seen| seen.min(ts)));
        }
    }

    fn merge_services(&mut self, incoming: BTreeMap<String, ServiceAccumulator>) {
        for (service, acc) in incoming {
            let Some(bucket) = bounded_entry(&mut self.per_service, &service) else {
                continue;
            };
            bucket.total_requests = bucket.total_requests.saturating_add(acc.total_requests);
            bucket.total_io_ops = bucket.total_io_ops.saturating_add(acc.total_io_ops);
            bucket.energy_kwh += sanitize_f64(acc.energy_kwh);
            bucket.carbon_kgco2eq += sanitize_f64(acc.carbon_kgco2eq);
            bucket.endpoints_seen.extend(acc.endpoints_seen);
            for (pattern, ap) in acc.anti_patterns {
                if !is_known_pattern(&pattern) {
                    continue;
                }
                let slot = bucket.anti_patterns.entry(pattern).or_default();
                slot.occurrences = slot.occurrences.saturating_add(ap.occurrences);
                slot.avoidable_io_ops = slot.avoidable_io_ops.saturating_add(ap.avoidable_io_ops);
            }
        }
    }

    /// Only rows whose `(service, pattern)` survived `merge_services`, the
    /// pairing `credit` keeps on the window path.
    fn merge_seen(
        &mut self,
        first: BTreeMap<(String, String), DateTime<Utc>>,
        last: BTreeMap<(String, String), DateTime<Utc>>,
    ) {
        for (key, ts) in first {
            if self.admits(&key) {
                let slot = self.first_seen.entry(key).or_insert(ts);
                *slot = (*slot).min(ts);
            }
        }
        for (key, ts) in last {
            if self.admits(&key) {
                let slot = self.last_seen.entry(key).or_insert(ts);
                *slot = (*slot).max(ts);
            }
        }
    }

    fn admits(&self, (service, pattern): &(String, String)) -> bool {
        self.per_service
            .get(service)
            .is_some_and(|bucket| bucket.anti_patterns.contains_key(pattern))
    }

    fn merge_tags(
        &mut self,
        binary_versions: BTreeSet<String>,
        energy_models: BTreeSet<String>,
        methodologies: BTreeSet<String>,
    ) {
        for version in binary_versions {
            self.fold_binary_version(&version);
        }
        for model in energy_models {
            if !model.is_empty() && model.len() <= MAX_ENERGY_MODEL_LEN {
                insert_capped(&mut self.energy_source_models, model, MAX_ENERGY_MODELS);
            }
        }
        for tag in methodologies {
            if !tag.is_empty() && tag.len() <= MAX_ENERGY_MODEL_LEN {
                insert_capped(&mut self.carbon_methodologies, tag, MAX_ENERGY_MODELS);
            }
        }
    }

    fn merge_coefficients(&mut self, coefficients: BTreeSet<String>) {
        for entry in coefficients {
            if entry.len() <= MAX_COEFFICIENT_LEN {
                insert_capped(&mut self.scoring_coefficients, entry, MAX_ENERGY_MODELS);
            }
        }
    }

    fn merge_per_service_maps(
        &mut self,
        models: BTreeMap<String, BTreeSet<String>>,
        ratios: BTreeMap<String, (f64, u32)>,
    ) {
        for (service, set) in models {
            for model in set {
                if model.is_empty() || model.len() > MAX_ENERGY_MODEL_LEN {
                    continue;
                }
                let known = self.per_service_energy_models.contains_key(&service);
                if !known && self.per_service_energy_models.len() >= MAX_SERVICES {
                    break;
                }
                let slot = self
                    .per_service_energy_models
                    .entry(service.clone())
                    .or_default();
                insert_capped(slot, model, MAX_ENERGY_MODELS);
            }
        }
        for (service, (sum, count)) in ratios {
            let known = self.per_service_measured_ratio_sums.contains_key(&service);
            if !known && self.per_service_measured_ratio_sums.len() >= MAX_SERVICES {
                continue;
            }
            let slot = self
                .per_service_measured_ratio_sums
                .entry(service)
                .or_insert((0.0, 0));
            // Each folded ratio is clamped to [0, 1], so a sum above its count is forged.
            slot.0 += sanitize_f64(sum).min(f64::from(count));
            slot.1 = slot.1.saturating_add(count);
        }
    }
}

/// Bound on one `key=value` coefficient entry read back from a rollup.
const MAX_COEFFICIENT_LEN: usize = 128;

fn insert_capped(set: &mut BTreeSet<String>, value: String, cap: usize) {
    if set.len() < cap || set.contains(&value) {
        set.insert(value);
    }
}

/// A pattern name the window path could have produced: a `FindingType`.
fn is_known_pattern(name: &str) -> bool {
    serde_json::from_value::<crate::detect::FindingType>(serde_json::Value::String(
        name.to_string(),
    ))
    .is_ok_and(|ft| ft.as_str() == name)
}

fn merge_tier(acc: &mut WasteTierAccumulator, other: &WasteTierAccumulator) {
    acc.n_plus_one_threshold = acc.n_plus_one_threshold.max(other.n_plus_one_threshold);
    acc.avoidable_io_ops = acc.avoidable_io_ops.saturating_add(other.avoidable_io_ops);
    acc.avoidable_kwh += sanitize_f64(other.avoidable_kwh);
    acc.avoidable_kg += sanitize_f64(other.avoidable_kg);
}

fn merge_waste_block(acc: &mut DbWasteAccumulator, other: DbWasteAccumulator) {
    acc.energy_kwh += sanitize_f64(other.energy_kwh);
    acc.measured_energy_kwh += sanitize_f64(other.measured_energy_kwh);
    acc.operational_kwh += sanitize_f64(other.operational_kwh);
    acc.canonical_kwh += sanitize_f64(other.canonical_kwh);
    acc.declared_energy_kwh += sanitize_f64(other.declared_energy_kwh);
    acc.modeled_energy_kwh += sanitize_f64(other.modeled_energy_kwh);
    acc.in_totals_energy_kwh += sanitize_f64(other.in_totals_energy_kwh);
    add_sanitized(&mut acc.operational_g, other.operational_g);
    add_sanitized(&mut acc.canonical_g, other.canonical_g);
    add_sanitized(&mut acc.energy_g, other.energy_g);
    acc.windows = acc.windows.saturating_add(other.windows);
    acc.measured_windows = acc.measured_windows.saturating_add(other.measured_windows);
    acc.declared_windows = acc.declared_windows.saturating_add(other.declared_windows);
    acc.estimated_windows = acc
        .estimated_windows
        .saturating_add(other.estimated_windows);
    acc.modeled_windows = acc.modeled_windows.saturating_add(other.modeled_windows);
    acc.in_totals_windows = acc
        .in_totals_windows
        .saturating_add(other.in_totals_windows);
    acc.windows_with_carbon = acc
        .windows_with_carbon
        .saturating_add(other.windows_with_carbon);
    // A rollup line is read back from disk: only the two TOML spellings.
    acc.placements.extend(
        other
            .placements
            .into_iter()
            .filter(|p| super::schema::is_known_placement(p)),
    );
    for model in other.models {
        if super::schema::is_valid_model_tag(&model) {
            insert_capped(&mut acc.models, model, MAX_BINARY_VERSIONS);
        }
    }
}

/// `(service, pattern) -> ts` as a JSON list of `[service, pattern, ts]`.
mod pair_key_map {
    use std::collections::BTreeMap;

    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    type Map = BTreeMap<(String, String), DateTime<Utc>>;

    pub(super) fn serialize<S: Serializer>(map: &Map, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(
            map.iter()
                .map(|((service, pattern), ts)| (service, pattern, ts)),
        )
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Map, D::Error> {
        let rows = Vec::<(String, String, DateTime<Utc>)>::deserialize(d)?;
        Ok(rows
            .into_iter()
            .map(|(service, pattern, ts)| ((service, pattern), ts))
            .collect())
    }
}

impl Builder {
    fn process_rollup_file(
        &mut self,
        path: &Path,
        period: &Period,
        strict: bool,
    ) -> Result<(), AggregationError> {
        let file = File::open(path).map_err(|source| AggregationError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let owner = rollup_owner(path);
        let family = format!("{owner}\u{1}daily");
        // A family starts unanchored: retention deletes whole months from
        // the front, so the oldest surviving line is a start, not a break.
        let mut expected = self.rollup_anchors.remove(&family).unwrap_or(None);
        // Rollups were chained from their first line, so a line without a
        // hash is always an edit, never a pre-chain line.
        let mut chain_started = true;
        let mut previous_in_scope = false;
        let mut warned_break = false;
        for (line_no, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|source| AggregationError::Io {
                path: path.display().to_string(),
                source,
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            self.rollup_line(RollupStep {
                trimmed,
                period,
                strict,
                path,
                line_no,
                family: &family,
                owner: &owner,
                expected: &mut expected,
                chain_started: &mut chain_started,
                previous_in_scope: &mut previous_in_scope,
                warned_break: &mut warned_break,
            })?;
        }
        self.rollup_anchors.insert(family, expected);
        Ok(())
    }

    fn rollup_line(&mut self, step: RollupStep<'_>) -> Result<(), AggregationError> {
        let RollupStep {
            trimmed,
            period,
            strict,
            path,
            line_no,
            family,
            owner,
            expected,
            chain_started,
            previous_in_scope,
            warned_break,
        } = step;
        let mut parsed: Option<serde_json::Value> = serde_json::from_str(trimmed).ok();
        let line = parsed
            .clone()
            .and_then(|value| serde_json::from_value::<super::rollup::RollupLine>(value).ok());
        let in_scope = line.as_ref().is_none_or(|l| period_has_day(period, l.day));
        if let Some(drops) = parsed
            .as_ref()
            .and_then(|v| v.get("drops"))
            .and_then(serde_json::Value::as_u64)
        {
            self.fold_drops(family, drops, in_scope);
        }
        let parsed_seq = parsed
            .as_ref()
            .and_then(|v| v.get("seq"))
            .and_then(serde_json::Value::as_u64);
        let next_seq = |expected: &ChainAnchor| {
            parsed_seq
                .unwrap_or_else(|| expected.as_ref().map_or(0, |(_, seq)| *seq))
                .saturating_add(1)
        };
        let chain_verified = self.walk_chain_line(ChainStep {
            parsed: parsed.as_mut(),
            expected,
            chain_started,
            in_scope,
            previous_in_scope: *previous_in_scope,
            next_seq: &next_seq,
            path,
            line_no,
            warned_break,
            weight: line.as_ref().map_or(1, |l| l.state.windows_aggregated()),
        });
        if parsed.is_some() {
            *previous_in_scope = in_scope;
        }
        let Some(line) = line else {
            self.state.malformed_lines_skipped += 1;
            return Ok(());
        };
        if chain_verified {
            self.earliest_chained = Some(
                self.earliest_chained
                    .map_or(line.ts, |seen| seen.min(line.ts)),
            );
        }
        self.fold_rollup(line, period, strict, owner)
    }

    fn fold_rollup(
        &mut self,
        line: super::rollup::RollupLine,
        period: &Period,
        strict: bool,
        owner: &str,
    ) -> Result<(), AggregationError> {
        self.earliest_archived = Some(
            self.earliest_archived
                .map_or(line.ts, |seen| seen.min(line.ts)),
        );
        if !period_has_day(period, line.day) {
            return Ok(());
        }
        let mut state = line.state;
        if strict && let Some(ts) = state.first_unattributed() {
            return Err(AggregationError::UnattributedWindow {
                ts: ts.to_rfc3339(),
            });
        }
        // A rollup answers for its own day only.
        state.keep_only_day(line.day);
        self.rollup_spans
            .entry(owner.to_string())
            .or_default()
            .push((line.ts, line.last_ts));
        self.state.merge(state);
        Ok(())
    }

    /// Whether a rollup line of the same archive already folded this
    /// window line.
    fn rollup_covers(&self, family: &str, value: Option<&serde_json::Value>) -> bool {
        let Some(spans) = self.rollup_spans.get(family) else {
            return false;
        };
        value
            .and_then(|v| v.get("ts"))
            .and_then(serde_json::Value::as_str)
            .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
            .map(|ts| ts.with_timezone(&Utc))
            .is_some_and(|ts| {
                spans
                    .iter()
                    .any(|(first, last)| *first <= ts && ts <= *last)
            })
    }
}

/// One rollup line's worth of reading state, threaded through
/// [`Builder::rollup_line`].
struct RollupStep<'a> {
    trimmed: &'a str,
    period: &'a Period,
    strict: bool,
    path: &'a Path,
    line_no: usize,
    family: &'a str,
    /// The window family the rollup file sits beside.
    owner: &'a str,
    expected: &'a mut ChainAnchor,
    chain_started: &'a mut bool,
    previous_in_scope: &'a mut bool,
    warned_break: &'a mut bool,
}

fn period_has_day(period: &Period, day: NaiveDate) -> bool {
    period.from_date <= day && day <= period.to_date
}

/// Rollup files are told apart by content, so a renamed or copied one is
/// still read as a rollup. The first line that parses decides: a crash
/// fragment ahead of it does not demote the whole file.
fn is_rollup_file(path: &Path) -> Result<bool, AggregationError> {
    let io = |source| AggregationError::Io {
        path: path.display().to_string(),
        source,
    };
    let file = File::open(path).map_err(io)?;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return Ok(super::rollup::is_rollup_value(&value));
        }
    }
    Ok(false)
}

/// The window family a rollup file belongs to, the same key
/// [`archive_family`] gives the window archive beside it. Its monthly files
/// share it, so their drop counter and their chain run on across months,
/// and their spans only cover that archive's windows.
fn rollup_owner(path: &Path) -> String {
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let stem = super::rollup::parse_rollup_file_name(&name).map_or_else(
        || {
            path.file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
        },
        |(stem, _, _)| stem.to_string(),
    );
    format!(
        "{}\u{1}{stem}",
        path.parent().unwrap_or(Path::new("")).display()
    )
}

impl Builder {
    fn finalize(self, source_files: Vec<String>, period: &Period) -> AggregateInputs {
        let total_energy_kwh = self.state.total_energy_kwh();
        let state = self.state;
        let total_requests = state.total_requests;
        let total_carbon = state.total_carbon_kgco2eq;
        // Flat avoidable fields alias the canonical (non-manipulable) tier.
        let canonical_waste = make_waste_tier(&state.canonical_waste, state.total_io_ops);
        let operational_waste = make_waste_tier(&state.operational_waste, state.total_io_ops);
        let anti_patterns_count: u64 = state
            .per_service
            .values()
            .flat_map(|s| s.anti_patterns.values())
            .map(|ap| ap.occurrences)
            .sum();

        let total_windows = state.runtime_windows + state.fallback_windows;
        let period_coverage = if total_windows == 0 {
            1.0
        } else {
            state.runtime_windows as f64 / total_windows as f64
        };

        let temporal_coverage = compute_temporal_coverage(&state.observed_days, period);
        // An archive written before chaining has no verified line, so its
        // oldest line still counts.
        let earliest_archived_window = self.earliest_chained.or(self.earliest_archived);

        AggregateInputs {
            aggregate: Aggregate {
                total_requests,
                total_energy_kwh,
                total_carbon_kgco2eq: total_carbon,
                carbon_breakdown: build_carbon_breakdown(&state),
                aggregate_efficiency_score: canonical_waste.efficiency_score,
                aggregate_waste_ratio: canonical_waste.waste_ratio,
                anti_patterns_detected_count: anti_patterns_count,
                estimated_optimization_potential_kgco2eq: canonical_waste.carbon_kgco2eq,
                canonical_waste,
                operational_waste,
                period_coverage,
                binary_versions: state.binary_versions,
                runtime_windows_count: state.runtime_windows,
                fallback_windows_count: state.fallback_windows,
                database_waste: waste_aggregate(state.db_waste),
                messaging_waste: waste_aggregate(state.msg_waste),
                per_service_energy_models: state.per_service_energy_models,
                per_service_measured_ratio: state
                    .per_service_measured_ratio_sums
                    .into_iter()
                    .map(|(svc, (sum, count))| {
                        let mean = if count == 0 {
                            0.0
                        } else {
                            sum / f64::from(count)
                        };
                        (svc, mean)
                    })
                    .collect(),
                temporal_coverage,
            },
            per_service: state.per_service,
            windows_aggregated: state.windows_aggregated,
            source_files,
            malformed_lines_skipped: state.malformed_lines_skipped,
            legacy_waste_windows: state.legacy_waste_windows,
            first_seen: state.first_seen,
            last_seen: state.last_seen,
            energy_source_models: state.energy_source_models,
            runtime_windows: state.runtime_windows,
            fallback_windows: state.fallback_windows,
            calibration_applied: state.calibration_applied,
            windows_dropped: self.drops_observed.then_some(self.windows_dropped),
            drop_counter_resets: self.drops_observed.then_some(self.drop_counter_resets),
            chain_verified: self.chain_verified,
            chain_unchained: self.chain_unchained,
            chain_breaks: self.chain_breaks,
            chain_breaks_outside: self.chain_breaks_outside,
            carbon_methodologies: state.carbon_methodologies,
            scoring_coefficients: state.scoring_coefficients,
            embodied_gco2_total: state.embodied_gco2_total,
            operational_gco2_total: state.operational_gco2_total,
            transport_gco2_total: state.transport_gco2_total,
            earliest_archived_window,
        }
    }
}

/// Whether the archive reaches back to the period's first day. A pruned
/// archive, or one switched on after the period began, does not: its
/// oldest surviving window falls later and the windows before it are gone.
/// A quiet start does not trip it, since older lines still precede the
/// period.
#[must_use]
pub fn archive_reaches_period_start(earliest: Option<DateTime<Utc>>, period: &Period) -> bool {
    earliest.is_some_and(|ts| ts.date_naive() <= period.from_date)
}

/// One waste block, database or broker, emitted only once a window
/// carried a figure.
fn waste_aggregate(w: DbWasteAccumulator) -> Option<DatabaseWasteAggregate> {
    (w.windows > 0).then(|| DatabaseWasteAggregate {
        energy_kwh: w.energy_kwh,
        measured_energy_kwh: w.measured_energy_kwh,
        modeled_energy_kwh: w.modeled_energy_kwh,
        declared_energy_kwh: w.declared_energy_kwh,
        in_totals_energy_kwh: w.in_totals_energy_kwh,
        models: w.models,
        placements: w.placements,
        windows_with_figure: w.windows,
        measured_windows: w.measured_windows,
        modeled_windows: w.modeled_windows,
        declared_windows: w.declared_windows,
        estimated_windows: w.estimated_windows,
        in_totals_windows: w.in_totals_windows,
        windows_with_carbon: w.windows_with_carbon,
        operational_waste_kwh: w.operational_kwh,
        operational_waste_kgco2eq: w.operational_g.map(|g| g / 1000.0),
        canonical_waste_kwh: w.canonical_kwh,
        canonical_waste_kgco2eq: w.canonical_g.map(|g| g / 1000.0),
    })
}

/// Split the period total into its five terms, in kgCO2eq, then the host
/// carbon that stayed beside it. One rule for every term: absent when zero,
/// so unmeasured never reads as zero. Transport is linear in its
/// coefficient, so low/high are mid rescaled, and omitted when a window
/// used another coefficient than the fixed one.
fn build_carbon_breakdown(state: &PeriodState) -> Option<CarbonBreakdown> {
    use crate::score::carbon::{
        DEFAULT_NETWORK_ENERGY_PER_BYTE_KWH, NETWORK_ENERGY_PER_BYTE_KWH_HIGH,
        NETWORK_ENERGY_PER_BYTE_KWH_LOW,
    };
    let kg = |g: f64| (g > 0.0).then_some(g / 1000.0);
    let fixed_coefficient = !state.transport_coefficient_uncertain;
    let transport = kg(state.transport_gco2_total);
    let breakdown = CarbonBreakdown {
        operational_kgco2eq: kg(state.operational_gco2_total),
        embodied_kgco2eq: kg(state.embodied_gco2_total),
        database_kgco2eq: kg(state.database_gco2_total),
        messaging_kgco2eq: kg(state.messaging_gco2_total),
        database_kgco2eq_out_of_total: state.db_waste.energy_g.map(|g| g / 1000.0),
        messaging_kgco2eq_out_of_total: state.msg_waste.energy_g.map(|g| g / 1000.0),
        transport_kgco2eq: transport,
        transport_kgco2eq_low: transport
            .filter(|_| fixed_coefficient)
            .map(|t| t * (NETWORK_ENERGY_PER_BYTE_KWH_LOW / DEFAULT_NETWORK_ENERGY_PER_BYTE_KWH)),
        transport_kgco2eq_high: transport
            .filter(|_| fixed_coefficient)
            .map(|t| t * (NETWORK_ENERGY_PER_BYTE_KWH_HIGH / DEFAULT_NETWORK_ENERGY_PER_BYTE_KWH)),
    };
    (breakdown.operational_kgco2eq.is_some()
        || breakdown.embodied_kgco2eq.is_some()
        || breakdown.transport_kgco2eq.is_some()
        || breakdown.database_kgco2eq.is_some()
        || breakdown.messaging_kgco2eq.is_some()
        || breakdown.database_kgco2eq_out_of_total.is_some()
        || breakdown.messaging_kgco2eq_out_of_total.is_some())
    .then_some(breakdown)
}

/// In-total database and broker host energy (kWh) and carbon (kg) of one
/// window, each term sanitized on its own. The kWh follows the block
/// rule of [`is_in_total_host`], so the application part keeps what the
/// in-total share leaves out.
fn in_total_host_terms(gs: &crate::report::GreenSummary) -> (f64, f64) {
    let kwh = gs
        .database_waste
        .as_ref()
        .filter(|w| is_in_total_host(w.in_totals, &w.model))
        .map_or(0.0, |w| sanitize_f64(w.energy_kwh))
        + gs.messaging_waste
            .as_ref()
            .filter(|w| is_in_total_host(w.in_totals, &w.model))
            .map_or(0.0, |w| sanitize_f64(w.energy_kwh));
    let (database_gco2, messaging_gco2) = in_total_host_gco2(gs);
    (kwh, (database_gco2 + messaging_gco2) / 1000.0)
}

/// In-total database and broker host carbon (g) of one window, under the
/// same [`is_in_total_host`] rule as the kWh: a dropped block takes its
/// carbon out of the host perimeter too.
fn in_total_host_gco2(gs: &crate::report::GreenSummary) -> (f64, f64) {
    let Some(co2) = gs.co2.as_ref() else {
        return (0.0, 0.0);
    };
    let term = |tag: Option<(bool, &str)>, gco2: Option<f64>| match tag {
        Some((in_totals, model)) if is_in_total_host(in_totals, model) => {
            sanitize_f64(gco2.unwrap_or(0.0))
        }
        _ => 0.0,
    };
    (
        term(
            gs.database_waste
                .as_ref()
                .map(|w| (w.in_totals, w.model.as_str())),
            co2.database_gco2,
        ),
        term(
            gs.messaging_waste
                .as_ref()
                .map(|w| (w.in_totals, w.model.as_str())),
            co2.messaging_gco2,
        ),
    )
}

/// One warning per file, whatever the number of broken lines.
fn warn_break(path: &Path, line_no: usize, warned: &mut bool) {
    if *warned {
        return;
    }
    *warned = true;
    tracing::warn!(
        path = %path.display(),
        line = line_no + 1,
        "archive integrity chain broken: a window was edited, removed or \
         reordered after it was written",
    );
}

/// Verdict for one archive line against the running chain.
enum ChainOutcome {
    /// Hash recomputes and `prev` points at the previous line. Carries the
    /// line's own hash, which the next line must reference.
    Verified(String),
    /// No `hash` field: written before archives were chained. Not a break,
    /// though not attestable.
    Unchained,
    /// Edited, removed or reordered. Carries this line's own hash so the
    /// walk can resynchronise.
    Break(String),
    /// Not JSON at all: a crash-truncated fragment, never chained.
    Malformed,
}

/// Removes the line's `hash` field in place: the chain hashes the body
/// without it, and the typed fold that consumes the value ignores it.
fn verify_chain_value(
    value: &mut serde_json::Value,
    expected: Option<&(String, u64)>,
) -> ChainOutcome {
    let Some(stated) = value
        .as_object_mut()
        .and_then(|obj| obj.remove("hash"))
        .as_ref()
        .and_then(serde_json::Value::as_str)
        .map(String::from)
    else {
        return ChainOutcome::Unchained;
    };
    let body = &*value;
    if super::hasher::archive_chain_hash(body).ok().as_deref() != Some(stated.as_str()) {
        return ChainOutcome::Break(stated);
    }
    // Without an anchor the walk cannot say where this line belongs, so it
    // adopts it: the break was already counted on the line that lost it.
    let Some((expected_prev, expected_seq)) = expected else {
        return ChainOutcome::Verified(stated);
    };
    let prev_ok = body
        .get("prev")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|p| p == expected_prev);
    // A `seq` that skips means lines are missing between this one and the
    // last. Absent on the first chained format, treated as in sequence.
    let seq_ok = body
        .get("seq")
        .and_then(serde_json::Value::as_u64)
        .is_none_or(|s| s == *expected_seq);
    if prev_ok && seq_ok {
        ChainOutcome::Verified(stated)
    } else {
        ChainOutcome::Break(stated)
    }
}

/// Whether an archive line's own timestamp falls inside the period.
/// A line the reader cannot parse counts as in-period: it was read from a
/// file the operator pointed at, and dropping it would hide a break.
fn line_in_period(value: Option<&serde_json::Value>, period: &Period) -> bool {
    value
        .and_then(|v| v.get("ts"))
        .and_then(serde_json::Value::as_str)
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .is_none_or(|ts| in_period(ts.with_timezone(&Utc), period))
}

fn service_io_distribution(
    per_endpoint: &[crate::report::PerEndpointIoOps],
) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for entry in per_endpoint {
        *out.entry(entry.service.clone()).or_insert(0) += entry.io_ops as u64;
    }
    out
}

/// Fold one window's avoidable tier into the period accumulator, sanitizing
/// the energy/carbon against tampered archives.
fn fold_tier(acc: &mut WasteTierAccumulator, tier: &crate::report::AvoidableTier) {
    // saturating_add: the counts come from untrusted archive JSON. A wrapping
    // sum would be a silent under-reporting primitive in a release binary.
    acc.avoidable_io_ops = acc
        .avoidable_io_ops
        .saturating_add(tier.avoidable_io_ops as u64);
    acc.avoidable_kwh += sanitize_f64(tier.avoidable_kwh);
    acc.avoidable_kg += sanitize_f64(tier.avoidable_gco2) / 1000.0;
    acc.n_plus_one_threshold = acc.n_plus_one_threshold.max(tier.n_plus_one_threshold);
}

/// Derive a [`WasteTier`] from a period accumulator. `waste_ratio` and
/// `efficiency_score` are computed against the period's total I/O ops.
fn make_waste_tier(acc: &WasteTierAccumulator, total_io_ops: u64) -> WasteTier {
    // An accumulator that received no data (threshold 0 and no avoidable ops,
    // i.e. an all-legacy canonical tier) is the all-zero default, not "100%
    // efficient". Returning the default lets `skip_serializing_if` omit it,
    // signalling "no data".
    if acc.n_plus_one_threshold == 0 && acc.avoidable_io_ops == 0 {
        return WasteTier::default();
    }
    let waste_ratio = if total_io_ops == 0 {
        0.0
    } else {
        acc.avoidable_io_ops as f64 / total_io_ops as f64
    };
    WasteTier {
        n_plus_one_threshold: acc.n_plus_one_threshold,
        energy_kwh: acc.avoidable_kwh,
        carbon_kgco2eq: acc.avoidable_kg,
        waste_ratio: waste_ratio.clamp(0.0, 1.0),
        efficiency_score: (100.0 - waste_ratio * 100.0).clamp(0.0, 100.0),
    }
}

/// Strip non-finite and negative values from any `f64` field read out
/// of archive JSON (top-level energy, per-service energy, per-service
/// carbon). Tampered or corrupted archives can carry `NaN`, `+Inf`, or
/// negative numbers which would otherwise poison every downstream sum.
fn sanitize_f64(value: f64) -> f64 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// Record each `(service, endpoint)` pair into the matching service
/// bucket's `endpoints_seen` set. Services absent from the bucket map
/// (filtered out by the cap or never inserted) are skipped.
fn collect_endpoints_seen(
    per_service: &mut BTreeMap<String, ServiceAccumulator>,
    entries: &[crate::report::PerEndpointIoOps],
) {
    for entry in entries {
        if let Some(bucket) = per_service.get_mut(&entry.service) {
            bucket.endpoints_seen.insert(entry.endpoint.clone());
        }
    }
}

/// Bounded `entry()`-equivalent for the per-service map. Returns a
/// mutable handle to the bucket when the cap has room, `None` once the
/// cap is reached for a previously unseen service.
fn bounded_entry<'a>(
    per_service: &'a mut BTreeMap<String, ServiceAccumulator>,
    service: &str,
) -> Option<&'a mut ServiceAccumulator> {
    if per_service.contains_key(service) {
        return per_service.get_mut(service);
    }
    if per_service.len() >= MAX_SERVICES {
        return None;
    }
    Some(per_service.entry(service.to_string()).or_default())
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn scale_u64(value: u64, factor: f64) -> u64 {
    let scaled = value as f64 * factor;
    if scaled.is_finite() && scaled >= 0.0 {
        scaled.round() as u64
    } else {
        0
    }
}

fn in_period(ts: DateTime<Utc>, period: &Period) -> bool {
    // Half-open [from, to+1d) so that envelopes at any sub-second offset
    // inside `to_date` (e.g. `2026-03-31T23:59:59.500Z`) are included.
    let from = naive_to_utc_start(period.from_date);
    let to_exclusive = period
        .to_date
        .succ_opt()
        .map_or_else(|| naive_to_utc_start(period.to_date), naive_to_utc_start);
    ts >= from && ts < to_exclusive
}

fn naive_to_utc_start(d: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("00:00:00 is valid"))
}

/// Build the v1.2 temporal-continuity signal from the set of distinct
/// observed days and the declared period. `observed_days` only ever holds
/// in-period days (the `in_period` filter runs before a window is folded),
/// so the ratio cannot exceed 1. It is clamped defensively anyway.
///
/// This measures days with OBSERVED TRAFFIC, not daemon uptime: archiving is
/// traffic-gated, so legitimately quiet days lower it. See
/// [`TemporalCoverage`].
fn compute_temporal_coverage(observed: &BTreeSet<NaiveDate>, period: &Period) -> TemporalCoverage {
    let days_in_period = period.days_covered;
    let observed_days = u32::try_from(observed.len()).unwrap_or(u32::MAX);
    let temporal_coverage = if days_in_period == 0 {
        0.0
    } else {
        (f64::from(observed_days) / f64::from(days_in_period)).clamp(0.0, 1.0)
    };
    TemporalCoverage {
        temporal_coverage,
        observed_days,
        days_in_period,
        largest_gap_days: largest_gap_days(observed, period),
    }
}

/// Longest run of consecutive in-period calendar days with zero windows.
///
/// Walks the sorted `observed` set (`O(observed_days)`) rather than every day in
/// the declared span, so the cost is bounded by archive content, not by an
/// operator-chosen `from`/`to` range. `observed` holds only in-period days, so
/// the leading/trailing edges and the between-day gaps cover the whole period.
fn largest_gap_days(observed: &BTreeSet<NaiveDate>, period: &Period) -> u32 {
    // Inclusive day-count between two dates as a saturating u32 (>= 0).
    let span = |a: NaiveDate, b: NaiveDate| -> u32 {
        u32::try_from((b - a).num_days().max(0)).unwrap_or(u32::MAX)
    };
    let Some(&first) = observed.iter().next() else {
        // No observed day: the whole period is one gap.
        return if period.to_date >= period.from_date {
            span(period.from_date, period.to_date).saturating_add(1)
        } else {
            0
        };
    };
    // Leading gap: days before the first observed day.
    let mut max = span(period.from_date, first);
    // Between consecutive observed days a and b: (b - a) - 1 empty days.
    let mut prev = first;
    for &day in observed.iter().skip(1) {
        max = max.max(span(prev, day).saturating_sub(1));
        prev = day;
    }
    // Trailing gap: days after the last observed day.
    max.max(span(prev, period.to_date))
}

/// The archive family a file belongs to: its directory plus its stem
/// with the rotation stamp stripped, so `archive.ndjson` and
/// `archive-20260110T000000000Z.ndjson` in one directory share a key
/// while two hosts' `archive.ndjson` do not. The daemon writes the
/// stamp as `{stem}-%Y%m%dT%H%M%S%fZ` (see `daemon::archive::rotate`).
fn archive_family(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let base = match stem.rsplit_once('-') {
        Some((head, stamp)) if is_rotation_stamp(stamp) && !head.is_empty() => head,
        _ => stem.as_str(),
    };
    format!(
        "{}\u{1}{base}",
        path.parent().unwrap_or(Path::new("")).display()
    )
}

/// `%Y%m%dT%H%M%S%fZ`: eight digits, `T`, digits, `Z`.
fn is_rotation_stamp(candidate: &str) -> bool {
    let Some(body) = candidate.strip_suffix('Z') else {
        return false;
    };
    let Some((date, time)) = body.split_once('T') else {
        return false;
    };
    date.len() == 8
        && date.bytes().all(|b| b.is_ascii_digit())
        && !time.is_empty()
        && time.bytes().all(|b| b.is_ascii_digit())
}

fn resolve_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>, AggregationError> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        let meta = stat_no_follow(path)?;
        if meta.is_file() {
            push_unique(&mut out, &mut seen, path.clone());
        } else if meta.is_dir() {
            collect_dir_ndjson(path, &mut out, &mut seen)?;
        } else {
            return Err(AggregationError::InvalidInput(path.display().to_string()));
        }
    }
    out.sort();
    Ok(out)
}

/// `symlink_metadata` plus an explicit symlink rejection. The
/// `resolve_files` caller wants `is_file()` / `is_dir()` semantics
/// without following links.
fn stat_no_follow(path: &Path) -> Result<std::fs::Metadata, AggregationError> {
    let meta = std::fs::symlink_metadata(path).map_err(|source| AggregationError::Io {
        path: path.display().to_string(),
        source,
    })?;
    if meta.file_type().is_symlink() {
        return Err(AggregationError::SymlinkRefused {
            path: path.display().to_string(),
        });
    }
    Ok(meta)
}

fn collect_dir_ndjson(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut BTreeSet<PathBuf>,
) -> Result<(), AggregationError> {
    let entries = std::fs::read_dir(dir).map_err(|source| AggregationError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| AggregationError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        let p = entry.path();
        // Symlink rejection scoped to `.ndjson` candidates only. A
        // symlinked README or sibling file in the same archive
        // directory is not our concern.
        if p.extension().and_then(std::ffi::OsStr::to_str) != Some("ndjson") {
            continue;
        }
        stat_no_follow(&p)?;
        push_unique(out, seen, p);
    }
    Ok(())
}

fn push_unique(out: &mut Vec<PathBuf>, seen: &mut BTreeSet<PathBuf>, path: PathBuf) {
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if seen.insert(canonical) {
        out.push(path);
    }
}

#[cfg(test)]
mod tests;
