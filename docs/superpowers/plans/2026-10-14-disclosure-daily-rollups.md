# Disclosure Daily Rollups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The daemon writes one chained rollup line per UTC day beside its window archive, and `disclose` aggregates rollups and windows into the same figures, so a quarter fits in a few MB.

**Architecture:** The measuring half of the aggregator's `Builder` becomes a serializable, mergeable `PeriodState` that stays in `aggregator.rs`. A new core module `report/periodic/rollup.rs` holds the line format and a pure day accumulator. A new daemon module `daemon/rollup_writer.rs` owns the monthly files, the never-reset chain, the crash snapshot and retention. The aggregator reads rollup files first, then skips window lines on days a rollup already covers.

**Tech Stack:** Rust 2024, serde, chrono, tokio (blocking writer thread), prometheus `IntGauge`, proptest, the existing archive hash chain (`hasher::archive_chain_hash`).

**Spec:** `docs/superpowers/specs/2026-10-14-disclosure-archive-holds-a-period-design.md`

**Base branch:** cut `feat/disclosure-daily-rollups` from `main` once PR 193 (release 0.28.0) is merged, then cherry-pick the spec and plan commits from `docs/disclosure-archive-design`. The plan relies on `earliest_archived` and `archive_reaches_period_start`, which only exist from 0.28.0.

## Global Constraints

- `sentinel-core` is `#![forbid(unsafe_code)]`. No FFI in this work.
- English only in code, comments, errors and docs. French only in `docs/FR/`, committed with its English source.
- Prometheus metrics added here carry no labels.
- SonarCloud `rust:S3776` threshold is 15 per function. Split before reaching it, never `#[allow]`.
- Large modules keep tests in a sibling `tests.rs`: aggregator tests go to `report/periodic/aggregator/tests.rs`, daemon archive tests stay in `daemon/archive.rs`'s own test module.
- Production files keep their path. The fold code stays in `aggregator.rs`.
- No new dependency. `proptest` is already a dev-dependency of `sentinel-core`.
- Defaults: `[daemon.archive] windows = true`, `rollup_retention_days = 400`. The chart does not change.
- Rollup files: `<stem>-daily-YYYY-MM.ndjson` in the directory of `[daemon.archive] path`. Partial snapshot: `<stem>-daily.partial.json` in the same directory.
- Line format tag: `"format": "perf-sentinel-rollup/v1"`.
- `DaemonArchiveConfig` gains two public fields, so the next release is a minor bump. The CHANGELOG says so.
- Commits: Conventional Commits, subject line only, no trailer, `cargo fmt --all` first.
- Before each commit touching Rust: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings`.

## Review Focus

1. **A clock stepped back across midnight.** A window stamped on an already closed day opens a second line for that day. The reader merges both lines, it never keeps only one. Test owned by Task 3.
2. **SIGKILL after a snapshot, then two restarts.** The snapshot is appended exactly once, never zero times and never twice. Test owned by Task 6.
3. **A period spanning the upgrade.** Days before the upgrade exist only as windows, and one day holds both windows and a rollup. Every window is counted once, and the rollup wins on the shared day. Test owned by Task 4.
4. **A tampered rollup line.** It may carry an unknown pattern name, a NaN energy, 5,000 services, observed days outside its own day, or a malformed binary version. Each guard the window path applies applies again on merge. Tests owned by Tasks 2 and 4.
5. **A directory input mixing everything.** It holds window files, monthly rollup files, a renamed rollup file and the partial snapshot. The snapshot is ignored, and rollup files are recognised by content, not by name. Test owned by Task 4.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/sentinel-core/src/report/periodic/aggregator.rs` | Modify | `PeriodState` split out of `Builder`, `merge`, rollup reading, rollup-wins rule |
| `crates/sentinel-core/src/report/periodic/aggregator/tests.rs` | Modify | Merge guards, equivalence property, mixed inputs, tamper cases |
| `crates/sentinel-core/src/report/periodic/rollup.rs` | Create | Line format, file naming, `DayAccumulator`, serialisation, partial snapshot format |
| `crates/sentinel-core/src/report/periodic/mod.rs` | Modify | `pub(crate) mod rollup;` |
| `crates/sentinel-core/src/config/mod.rs`, `config/raw.rs`, `config/validate.rs` | Modify | `windows`, `rollup_retention_days` |
| `crates/sentinel-core/src/daemon/rollup_writer.rs` | Create | Monthly files, chain resume, snapshot recovery, retention, horizon gauges |
| `crates/sentinel-core/src/daemon/archive.rs` | Modify | Window writing extracted into `WindowSink`, rollup writer wired in, `windows = false` |
| `crates/sentinel-core/src/daemon/mod.rs` | Modify | `mod rollup_writer;` |
| `crates/sentinel-core/src/report/metrics.rs` | Modify | Three label-free gauges |
| `crates/sentinel-core/src/daemon/query_api/mod.rs` | Modify | `disclosure_archive` in `/api/status` |
| `docs/CONFIGURATION.md`, `docs/REPORTING.md`, `docs/METRICS.md`, `docs/QUERY-API.md`, `docs/design/08-PERIODIC-DISCLOSURE.md` and their `docs/FR/` mirrors, `CHANGELOG.md` | Modify | Operator-facing description |
| Lab: `scenarios/chart-disclose-persistence/verify.sh`, `README.md` | Modify | Rollups survive a restart and disclose over them alone matches the windows |

---

### Task 1: Split `PeriodState` out of `Builder`

Pure refactor. Every existing aggregator test must pass unchanged.

**Files:**
- Modify: `crates/sentinel-core/src/report/periodic/aggregator.rs`

**Interfaces:**
- Produces: `pub(crate) struct PeriodState` (fields below) with `pub(crate) fn process_window(&mut self, report: &Report, ts: DateTime<Utc>, strict: bool) -> Result<bool, AggregationError>`. `Builder` gains `state: PeriodState`.

- [ ] **Step 1: Run the aggregator tests to record the baseline**

Run: `cargo test -p perf-sentinel-core report::periodic`
Expected: PASS. Note the test count.

- [ ] **Step 2: Declare `PeriodState` and shrink `Builder`**

Move every `Builder` field that measures the period into `PeriodState`. `Builder` keeps only what describes how lines were read.

```rust
/// What the windows of a period measured, independent of how they were
/// read. Folded window by window, and merged whole when a daily rollup
/// carries it, see `rollup.rs`.
#[derive(Default)]
pub(crate) struct PeriodState {
    per_service: BTreeMap<String, ServiceAccumulator>,
    windows_aggregated: u64,
    malformed_lines_skipped: u64,
    legacy_waste_windows: u64,
    first_seen: BTreeMap<(String, String), DateTime<Utc>>,
    last_seen: BTreeMap<(String, String), DateTime<Utc>>,
    total_requests: u64,
    total_io_ops: u64,
    total_carbon_kgco2eq: f64,
    canonical_waste: WasteTierAccumulator,
    operational_waste: WasteTierAccumulator,
    db_waste: DbWasteAccumulator,
    msg_waste: DbWasteAccumulator,
    runtime_energy_kwh: f64,
    energy_source_models: BTreeSet<String>,
    runtime_windows: u64,
    fallback_windows: u64,
    binary_versions: BTreeSet<String>,
    calibration_applied: bool,
    carbon_methodologies: BTreeSet<String>,
    scoring_coefficients: BTreeSet<String>,
    transport_coefficient_uncertain: bool,
    embodied_gco2_total: f64,
    operational_gco2_total: f64,
    transport_gco2_total: f64,
    per_service_energy_models: BTreeMap<String, BTreeSet<String>>,
    per_service_measured_ratio_sums: BTreeMap<String, (f64, u32)>,
    observed_days: BTreeSet<NaiveDate>,
}

#[derive(Default)]
struct Builder {
    state: PeriodState,
    chain_verified: u64,
    chain_unchained: u64,
    chain_breaks: u64,
    chain_breaks_outside: u64,
    windows_dropped: u64,
    drop_counter_resets: u64,
    drops_observed: bool,
    last_drops: BTreeMap<String, u64>,
    earliest_archived: Option<DateTime<Utc>>,
}
```

Keep each field's existing doc comment, moved with the field.

- [ ] **Step 3: Move the fold methods onto `PeriodState`**

Change the `impl` header, not the bodies, for: `process_window`, `compute_window_metrics`, `fold_global_counters`, `fold_disclosure_waste`, `fold_database_block`, `fold_messaging_block`, `fold_binary_version`, `fold_transport_coefficient`, `fold_scoring_coefficients`, `fold_carbon_methodology`, `fold_window_energy_model`, `record_energy_model_tag`, `fold_per_service_measured_ratio`, `fold_per_service_energy_models`, `attribute_window`, `attribute_runtime`, `attribute_unattributed`, `attribute_proxy_share`, `route_findings`, `credit`, `update_seen_timestamps`, `total_energy_kwh`. Put them in one `impl PeriodState` block placed where the old `impl Builder` fold block was.

Change `process_window` to borrow the report:

```rust
pub(crate) fn process_window(
    &mut self,
    report: &Report,
    ts: DateTime<Utc>,
    strict: bool,
) -> Result<bool, AggregationError> {
    let Some(m) = self.compute_window_metrics(report, ts) else {
        return Ok(false);
    };
```

Inside it, replace each `&report` by `report` and `&report.x` stays as is. Delete the two lines `let ts = envelope.ts;` and `let report = envelope.report;`.

- [ ] **Step 4: Rewire `Builder`**

In `Builder::fold_window`, the malformed counter and the fold become:

```rust
            Err(err) => {
                self.state.malformed_lines_skipped += 1;
```

```rust
        let used_fallback = self
            .state
            .process_window(&envelope.report, envelope.ts, step.strict)?;
```

In `aggregate_from_paths`, `builder.windows_aggregated` becomes `builder.state.windows_aggregated`.

`Builder::finalize` destructures the state first, then keeps its current body with `self.` replaced by `state.` for every moved field:

```rust
    fn finalize(self, source_files: Vec<String>, period: &Period) -> AggregateInputs {
        let total_energy_kwh = self.state.total_energy_kwh();
        let state = self.state;
        let total_requests = state.total_requests;
        let total_carbon = state.total_carbon_kgco2eq;
```

The chain, drop and `earliest_archived` fields keep reading from `self.`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p perf-sentinel-core report::periodic`
Expected: PASS, same count as Step 1.

Run: `cargo clippy -p perf-sentinel-core --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/report/periodic/aggregator.rs
git commit -m "refactor(disclose): split the period's measurements out of the archive reader"
```

---

### Task 2: Make `PeriodState` serializable and mergeable

**Files:**
- Modify: `crates/sentinel-core/src/report/periodic/aggregator.rs`
- Test: `crates/sentinel-core/src/report/periodic/aggregator/tests.rs`

**Interfaces:**
- Consumes: `PeriodState` from Task 1.
- Produces: `PeriodState: Serialize + Deserialize` (with `#[serde(default)]`), `pub(crate) fn merge(&mut self, other: PeriodState)`, field `first_unattributed: Option<DateTime<Utc>>`, and `pub(crate) fn first_unattributed(&self) -> Option<DateTime<Utc>>`. `ServiceAccumulator` and `AntiPatternAccumulator` derive `Serialize, Deserialize`.

- [ ] **Step 1: Write the failing tests**

Append to `aggregator/tests.rs`:

```rust
fn folded(lines: &[(DateTime<Utc>, Report)]) -> PeriodState {
    let mut state = PeriodState::default();
    for (ts, report) in lines {
        state.process_window(report, *ts, false).unwrap();
    }
    state
}

fn day(d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, d, h, 0, 0).unwrap()
}

#[test]
fn a_state_survives_a_json_round_trip() {
    let finding = make_finding("svc-a", FindingType::NPlusOneSql, "SELECT ?");
    let state = folded(&[(
        day(10, 9),
        make_report(10, 100, 10, &[("svc-a", "/api", 100)], vec![finding]),
    )]);
    let json = serde_json::to_string(&state).unwrap();
    let back: PeriodState = serde_json::from_str(&json).unwrap();
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
}

#[test]
fn merging_two_days_counts_like_folding_them() {
    let a = (day(10, 9), plain_window());
    let b = (day(11, 9), plain_window());
    let mut merged = folded(&[a.clone()]);
    merged.merge(folded(&[b.clone()]));
    let whole = folded(&[a, b]);
    assert_eq!(merged.windows_aggregated, whole.windows_aggregated);
    assert_eq!(merged.total_io_ops, whole.total_io_ops);
    assert_eq!(merged.observed_days, whole.observed_days);
    assert_eq!(
        merged.per_service["svc-a"].total_requests,
        whole.per_service["svc-a"].total_requests
    );
}

#[test]
fn merge_drops_unknown_patterns_and_sanitizes_figures() {
    let mut incoming = folded(&[(day(10, 9), plain_window())]);
    let bucket = incoming.per_service.get_mut("svc-a").unwrap();
    bucket.energy_kwh = f64::NAN;
    bucket
        .anti_patterns
        .insert("not_a_pattern".to_string(), AntiPatternAccumulator::default());
    incoming
        .first_seen
        .insert(("svc-a".into(), "not_a_pattern".into()), day(10, 9));
    incoming.binary_versions.insert("bad version!".to_string());
    incoming.total_carbon_kgco2eq = f64::INFINITY;

    let mut state = PeriodState::default();
    state.merge(incoming);
    let svc = &state.per_service["svc-a"];
    assert!(svc.energy_kwh.is_finite());
    assert!(!svc.anti_patterns.contains_key("not_a_pattern"));
    assert!(state.first_seen.is_empty());
    assert!(state.binary_versions.is_empty());
    assert!(state.total_carbon_kgco2eq.is_finite());
}

#[test]
fn merge_honours_the_service_cap() {
    let mut incoming = PeriodState::default();
    for i in 0..(MAX_SERVICES + 10) {
        incoming
            .per_service
            .insert(format!("svc-{i}"), ServiceAccumulator::default());
    }
    let mut state = PeriodState::default();
    state.merge(incoming);
    assert_eq!(state.per_service.len(), MAX_SERVICES);
}

#[test]
fn an_unattributed_window_is_remembered_for_strict_mode() {
    let empty = make_report(10, 100, 10, &[], vec![]);
    let state = folded(&[(day(12, 8), empty.clone()), (day(10, 8), empty)]);
    assert_eq!(state.first_unattributed(), Some(day(10, 8)));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p perf-sentinel-core report::periodic::aggregator::tests::merge`
Expected: compile errors, `merge`, `first_unattributed` and `Serialize` missing.

- [ ] **Step 3: Add the derives and the pair-keyed map codec**

Add `Serialize` to the `serde` import. Derive `Serialize, Deserialize` on `ServiceAccumulator`, `AntiPatternAccumulator`, `WasteTierAccumulator`, `DbWasteAccumulator` and `PeriodState`. On `PeriodState` add `#[serde(default)]` at the struct level, and on `first_seen` and `last_seen` add `#[serde(with = "pair_key_map")]`. Add the field:

```rust
    /// Earliest window folded into the unattributed bucket. A rollup is
    /// folded without `--strict-attribution`, so the reader needs this to
    /// refuse it later when the flag is set.
    first_unattributed: Option<DateTime<Utc>>,
```

JSON object keys must be strings, so the `(service, pattern)` maps travel as a list:

```rust
/// `(service, pattern) -> ts` as a JSON list of `[service, pattern, ts]`.
mod pair_key_map {
    use std::collections::BTreeMap;

    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    type Map = BTreeMap<(String, String), DateTime<Utc>>;

    pub(super) fn serialize<S: Serializer>(map: &Map, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(map.iter().map(|((service, pattern), ts)| (service, pattern, ts)))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Map, D::Error> {
        let rows = Vec::<(String, String, DateTime<Utc>)>::deserialize(d)?;
        Ok(rows
            .into_iter()
            .map(|(service, pattern, ts)| ((service, pattern), ts))
            .collect())
    }
}
```

- [ ] **Step 4: Record the first unattributed window**

In `PeriodState::process_window`, right after the strict check:

```rust
        if unattributed {
            self.first_unattributed =
                Some(self.first_unattributed.map_or(ts, |seen| seen.min(ts)));
        }
```

And the accessor:

```rust
    pub(crate) fn first_unattributed(&self) -> Option<DateTime<Utc>> {
        self.first_unattributed
    }
```

- [ ] **Step 5: Implement `merge` with the window path's guards**

The destructuring makes a new field a compile error until it is merged.

```rust
impl PeriodState {
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
            per_service_energy_models,
            per_service_measured_ratio_sums,
            observed_days,
            first_unattributed,
        } = other;
        self.windows_aggregated = self.windows_aggregated.saturating_add(windows_aggregated);
        self.malformed_lines_skipped =
            self.malformed_lines_skipped.saturating_add(malformed_lines_skipped);
        self.legacy_waste_windows = self.legacy_waste_windows.saturating_add(legacy_waste_windows);
        self.total_requests = self.total_requests.saturating_add(total_requests);
        self.total_io_ops = self.total_io_ops.saturating_add(total_io_ops);
        self.runtime_windows = self.runtime_windows.saturating_add(runtime_windows);
        self.fallback_windows = self.fallback_windows.saturating_add(fallback_windows);
        self.total_carbon_kgco2eq += sanitize_f64(total_carbon_kgco2eq);
        self.runtime_energy_kwh += sanitize_f64(runtime_energy_kwh);
        self.embodied_gco2_total += sanitize_f64(embodied_gco2_total);
        self.operational_gco2_total += sanitize_f64(operational_gco2_total);
        self.transport_gco2_total += sanitize_f64(transport_gco2_total);
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
                let slot = self.per_service_energy_models.entry(service.clone()).or_default();
                insert_capped(slot, model, MAX_ENERGY_MODELS);
            }
        }
        for (service, (sum, count)) in ratios {
            let known = self.per_service_measured_ratio_sums.contains_key(&service);
            if !known && self.per_service_measured_ratio_sums.len() >= MAX_SERVICES {
                continue;
            }
            let slot = self.per_service_measured_ratio_sums.entry(service).or_insert((0.0, 0));
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
    let add_opt = |slot: &mut Option<f64>, value: Option<f64>| {
        if let Some(g) = value {
            *slot = Some(slot.unwrap_or(0.0) + sanitize_f64(g));
        }
    };
    acc.energy_kwh += sanitize_f64(other.energy_kwh);
    acc.measured_energy_kwh += sanitize_f64(other.measured_energy_kwh);
    acc.operational_kwh += sanitize_f64(other.operational_kwh);
    acc.canonical_kwh += sanitize_f64(other.canonical_kwh);
    acc.declared_energy_kwh += sanitize_f64(other.declared_energy_kwh);
    add_opt(&mut acc.operational_g, other.operational_g);
    add_opt(&mut acc.canonical_g, other.canonical_g);
    add_opt(&mut acc.energy_g, other.energy_g);
    acc.windows = acc.windows.saturating_add(other.windows);
    acc.measured_windows = acc.measured_windows.saturating_add(other.measured_windows);
    acc.declared_windows = acc.declared_windows.saturating_add(other.declared_windows);
    acc.estimated_windows = acc.estimated_windows.saturating_add(other.estimated_windows);
    acc.windows_with_carbon = acc.windows_with_carbon.saturating_add(other.windows_with_carbon);
    for model in other.models {
        if super::schema::is_valid_model_tag(&model) {
            insert_capped(&mut acc.models, model, MAX_BINARY_VERSIONS);
        }
    }
}
```

If `merge` alone exceeds 15 on SonarCloud's cognitive complexity, move the scalar sums into a `merge_scalars` helper. Straight-line code scores near zero, so it should not.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p perf-sentinel-core report::periodic`
Expected: PASS, including the five new tests.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/report/periodic/aggregator.rs crates/sentinel-core/src/report/periodic/aggregator/tests.rs
git commit -m "feat(disclose): merge period states under the window path's guards"
```

---

### Task 3: Rollup line format and day accumulator

**Files:**
- Create: `crates/sentinel-core/src/report/periodic/rollup.rs`
- Modify: `crates/sentinel-core/src/report/periodic/mod.rs`

**Interfaces:**
- Consumes: `PeriodState::process_window`, `PeriodState::merge`, `hasher::archive_chain_hash`.
- Produces:
  - `pub(crate) const ROLLUP_FORMAT: &str`
  - `pub(crate) fn rollup_file_name(stem: &str, day: NaiveDate) -> String`
  - `pub(crate) fn parse_rollup_file_name(name: &str) -> Option<(&str, i32, u32)>`
  - `pub(crate) fn partial_file_name(stem: &str) -> String`
  - `pub(crate) struct ClosedDay { pub day: NaiveDate, pub first_ts: DateTime<Utc>, pub last_ts: DateTime<Utc>, pub state: PeriodState }`
  - `pub(crate) struct DayAccumulator` with `observe(&mut self, ts, &Report) -> Option<ClosedDay>`, `take(&mut self) -> Option<ClosedDay>`, `peek(&self) -> Option<&ClosedDay>`
  - `pub(crate) fn serialize_rollup(day: &ClosedDay, prev: &str, seq: u64, drops: u64) -> Result<String, serde_json::Error>`
  - `pub(crate) fn serialize_partial(day: &ClosedDay) -> Result<String, serde_json::Error>`
  - `pub(crate) fn parse_partial(text: &str) -> Option<ClosedDay>`
  - `pub(crate) fn is_rollup_value(value: &serde_json::Value) -> bool`
  - `#[derive(Deserialize)] pub(crate) struct RollupLine { pub ts: DateTime<Utc>, pub day: NaiveDate, pub last_ts: DateTime<Utc>, pub state: PeriodState }`

- [ ] **Step 1: Write the failing tests at the bottom of the new file**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Report;
    use chrono::TimeZone;

    fn at(d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, d, h, 0, 0).unwrap()
    }

    fn window() -> Report {
        serde_json::from_str(include_str!("../../../../../tests/fixtures/periodic/window-report.json"))
            .unwrap()
    }

    #[test]
    fn a_new_day_closes_the_previous_one() {
        let mut acc = DayAccumulator::default();
        assert!(acc.observe(at(10, 9), &window()).is_none());
        assert!(acc.observe(at(10, 23), &window()).is_none());
        let closed = acc.observe(at(11, 0), &window()).unwrap();
        assert_eq!(closed.day, at(10, 0).date_naive());
        assert_eq!((closed.first_ts, closed.last_ts), (at(10, 9), at(10, 23)));
        assert_eq!(closed.state.windows_aggregated(), 2);
        assert_eq!(acc.peek().unwrap().day, at(11, 0).date_naive());
    }

    #[test]
    fn a_clock_stepped_back_across_midnight_opens_a_second_line_for_the_earlier_day() {
        let mut acc = DayAccumulator::default();
        acc.observe(at(10, 23), &window());
        let first = acc.observe(at(11, 0), &window()).unwrap();
        let second = acc.observe(at(10, 23), &window()).unwrap();
        assert_eq!(first.day, at(10, 0).date_naive());
        assert_eq!(second.day, at(11, 0).date_naive());
        assert_eq!(acc.take().unwrap().day, at(10, 0).date_naive());
    }

    #[test]
    fn rollup_file_names_round_trip() {
        let name = rollup_file_name("archive", at(9, 0).date_naive());
        assert_eq!(name, "archive-daily-2026-01.ndjson");
        assert_eq!(parse_rollup_file_name(&name), Some(("archive", 2026, 1)));
        assert_eq!(parse_rollup_file_name("archive-20260110T000000000Z.ndjson"), None);
        assert_eq!(parse_rollup_file_name("archive-daily-2026-13.ndjson"), None);
        assert_eq!(parse_rollup_file_name("-daily-2026-01.ndjson"), None);
    }

    #[test]
    fn a_rollup_line_verifies_with_the_archive_chain() {
        let mut acc = DayAccumulator::default();
        acc.observe(at(10, 9), &window());
        let day = acc.take().unwrap();
        let line = serialize_rollup(&day, "seed", 0, 3).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(is_rollup_value(&value));
        let stated = value.as_object_mut().unwrap().remove("hash").unwrap();
        let recomputed = crate::report::periodic::hasher::archive_chain_hash(&value).unwrap();
        assert_eq!(stated.as_str(), Some(recomputed.as_str()));
        let parsed: RollupLine = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.day, day.day);
    }

    #[test]
    fn a_partial_snapshot_round_trips_and_is_not_a_rollup_line() {
        let mut acc = DayAccumulator::default();
        acc.observe(at(10, 9), &window());
        let text = serialize_partial(acc.peek().unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(!is_rollup_value(&value));
        let back = parse_partial(&text).unwrap();
        assert_eq!(back.first_ts, at(10, 9));
        assert_eq!(back.state.windows_aggregated(), 1);
    }
}
```

The fixture `tests/fixtures/periodic/window-report.json` is a `Report` as `perf-sentinel analyze --format json` prints it. Create it in this step:

```bash
mkdir -p tests/fixtures/periodic
cargo run -q -p perf-sentinel -- analyze --format json --input tests/fixtures/n_plus_one_sql.json > tests/fixtures/periodic/window-report.json
```

Add `pub(crate) fn windows_aggregated(&self) -> u64 { self.windows_aggregated }` to `PeriodState` in `aggregator.rs`.

- [ ] **Step 2: Declare the module and run the tests to see them fail**

In `report/periodic/mod.rs` add `pub(crate) mod rollup;` next to the other module declarations.

Run: `cargo test -p perf-sentinel-core report::periodic::rollup`
Expected: compile errors, items missing.

- [ ] **Step 3: Write the module**

```rust
//! Daily rollups of the disclosure archive. One chained NDJSON line per
//! UTC day carries the [`PeriodState`] the aggregator would build from
//! that day's windows, so a quarter is 92 lines instead of 500,000. See
//! `docs/design/08-PERIODIC-DISCLOSURE.md`.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;

use super::aggregator::PeriodState;
use super::hasher::archive_chain_hash;
use crate::report::Report;

/// Tag every rollup line carries, inside the hashed body.
pub(crate) const ROLLUP_FORMAT: &str = "perf-sentinel-rollup/v1";

/// `<stem>-daily-YYYY-MM.ndjson`: one file per month, so retention deletes
/// whole files and never rewrites a chained one.
pub(crate) fn rollup_file_name(stem: &str, day: NaiveDate) -> String {
    format!("{stem}-daily-{}.ndjson", day.format("%Y-%m"))
}

/// The `(stem, year, month)` a rollup file name encodes.
pub(crate) fn parse_rollup_file_name(name: &str) -> Option<(&str, i32, u32)> {
    let body = name.strip_suffix(".ndjson")?;
    let (stem, year_month) = body.rsplit_once("-daily-")?;
    let (year, month) = year_month.split_once('-')?;
    if stem.is_empty() || year.len() != 4 || month.len() != 2 {
        return None;
    }
    let year: i32 = year.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    (1..=12).contains(&month).then_some((stem, year, month))
}

/// The crash snapshot of the open day. `.json`, so a directory passed to
/// `disclose --input` never picks it up.
pub(crate) fn partial_file_name(stem: &str) -> String {
    format!("{stem}-daily.partial.json")
}

/// One UTC day's windows, folded.
pub(crate) struct ClosedDay {
    pub day: NaiveDate,
    pub first_ts: DateTime<Utc>,
    pub last_ts: DateTime<Utc>,
    pub state: PeriodState,
}

/// Folds windows into the open day and hands the day back once a window
/// lands on another one.
#[derive(Default)]
pub(crate) struct DayAccumulator {
    open: Option<ClosedDay>,
}

impl DayAccumulator {
    /// Fold one window. A window on another UTC day closes the open one,
    /// in either direction: a clock stepped back across midnight closes it
    /// too, and the earlier day gets a second line that merges with its
    /// first.
    pub(crate) fn observe(&mut self, ts: DateTime<Utc>, report: &Report) -> Option<ClosedDay> {
        let day = ts.date_naive();
        let closed = self.open.take_if(|open| open.day != day);
        let open = self.open.get_or_insert_with(|| ClosedDay {
            day,
            first_ts: ts,
            last_ts: ts,
            state: PeriodState::default(),
        });
        open.first_ts = open.first_ts.min(ts);
        open.last_ts = open.last_ts.max(ts);
        // Never strict here: `--strict-attribution` is the reader's call,
        // made from `first_unattributed`.
        if let Err(err) = open.state.process_window(report, ts, false) {
            tracing::warn!(error = %err, "window not folded into the daily rollup");
        }
        closed
    }

    pub(crate) fn take(&mut self) -> Option<ClosedDay> {
        self.open.take()
    }

    pub(crate) fn peek(&self) -> Option<&ClosedDay> {
        self.open.as_ref()
    }
}

fn body(day: &ClosedDay) -> serde_json::Value {
    serde_json::json!({
        "format": ROLLUP_FORMAT,
        "ts": day.first_ts,
        "day": day.day,
        "last_ts": day.last_ts,
        "state": &day.state,
    })
}

/// Serialise one day and chain it to `prev`, the way the window writer
/// chains a window. `ts` is the day's first window, so the reader's
/// period and reach checks work on it unchanged.
pub(crate) fn serialize_rollup(
    day: &ClosedDay,
    prev: &str,
    seq: u64,
    drops: u64,
) -> Result<String, serde_json::Error> {
    let mut line = body(day);
    if let Some(obj) = line.as_object_mut() {
        obj.insert("prev".into(), prev.into());
        obj.insert("seq".into(), seq.into());
        obj.insert("drops".into(), drops.into());
    }
    let hash = archive_chain_hash(&line)?;
    if let Some(obj) = line.as_object_mut() {
        obj.insert("hash".into(), hash.into());
    }
    serde_json::to_string(&line)
}

/// The open day, unchained and without the format tag, so it can never
/// pass for a rollup line.
pub(crate) fn serialize_partial(day: &ClosedDay) -> Result<String, serde_json::Error> {
    let mut snapshot = body(day);
    if let Some(obj) = snapshot.as_object_mut() {
        obj.remove("format");
    }
    serde_json::to_string(&snapshot)
}

pub(crate) fn parse_partial(text: &str) -> Option<ClosedDay> {
    let line: RollupLine = serde_json::from_str(text).ok()?;
    Some(ClosedDay {
        day: line.day,
        first_ts: line.ts,
        last_ts: line.last_ts,
        state: line.state,
    })
}

pub(crate) fn is_rollup_value(value: &serde_json::Value) -> bool {
    value.get("format").and_then(serde_json::Value::as_str) == Some(ROLLUP_FORMAT)
}

/// The fields the reader folds. `prev`, `seq`, `drops` and `hash` are
/// read by the chain walk on the raw value.
#[derive(Deserialize)]
pub(crate) struct RollupLine {
    pub ts: DateTime<Utc>,
    pub day: NaiveDate,
    pub last_ts: DateTime<Utc>,
    pub state: PeriodState,
}
```

`Option::take_if` is stable since Rust 1.80, below the workspace MSRV.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p perf-sentinel-core report::periodic::rollup`
Expected: PASS.

Run: `cargo check -p perf-sentinel-core --no-default-features --all-targets`
Expected: clean, the module needs no feature.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/report/periodic/rollup.rs crates/sentinel-core/src/report/periodic/mod.rs crates/sentinel-core/src/report/periodic/aggregator.rs tests/fixtures/periodic/window-report.json
git commit -m "feat(disclose): define the daily rollup line and its day accumulator"
```

---

### Task 4: Aggregate rollup files, rollups win their day

**Files:**
- Modify: `crates/sentinel-core/src/report/periodic/aggregator.rs`
- Test: `crates/sentinel-core/src/report/periodic/aggregator/tests.rs`

**Interfaces:**
- Consumes: everything from Task 3.
- Produces: `aggregate_from_paths` accepts rollup files in `paths` or in a directory, recognised by content. Behaviour only, no new public item.

- [ ] **Step 1: Write the failing tests**

Append to `aggregator/tests.rs`:

```rust
use super::super::rollup::{self, DayAccumulator};
use proptest::prelude::*;

/// The daemon's shape: one chained rollup line per closed day, in monthly
/// files, the chain carried across files.
fn write_rollups(dir: &Path, stem: &str, lines: &[(DateTime<Utc>, Report)]) -> Vec<PathBuf> {
    let mut acc = DayAccumulator::default();
    let mut days = Vec::new();
    for (ts, report) in lines {
        days.extend(acc.observe(*ts, report));
    }
    days.extend(acc.take());
    let mut prev = super::super::hasher::ARCHIVE_CHAIN_SEED.to_string();
    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (seq, day) in days.iter().enumerate() {
        let line = rollup::serialize_rollup(day, &prev, seq as u64, 0).unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        prev = value["hash"].as_str().unwrap().to_string();
        let path = dir.join(rollup::rollup_file_name(stem, day.day));
        let buf = files.entry(path).or_default();
        buf.push_str(&line);
        buf.push('\n');
    }
    for (path, text) in &files {
        std::fs::write(path, text).unwrap();
    }
    files.into_keys().collect()
}

fn window_pool() -> Vec<Report> {
    let tier = crate::report::AvoidableTier {
        n_plus_one_threshold: 2,
        avoidable_io_ops: 10,
        avoidable_kwh: 0.1,
        avoidable_gco2: 1.0,
    };
    let mut with_waste = make_report(40, 400, 30, &[("svc-a", "/api", 300), ("svc-b", "/b", 100)], vec![]);
    with_waste.disclosure_waste = Some(crate::report::DisclosureWaste {
        canonical: tier.clone(),
        operational: tier,
        database: Some(waste_block(1.0, "alumet_rapl")),
        messaging: Some(waste_block(0.5, "estimated")),
    });
    with_waste.binary_version = "0.29.0".to_string();
    vec![
        plain_window(),
        make_report(
            25,
            250,
            40,
            &[("svc-a", "/api", 150), ("svc-c", "/c", 100)],
            vec![
                make_finding("svc-a", FindingType::NPlusOneSql, "SELECT ?"),
                make_finding("svc-c", FindingType::RedundantSql, "SELECT 1"),
            ],
        ),
        make_runtime_report(
            &[("svc-a", "/api", 60), ("svc-b", "/b", 40)],
            &[("svc-a", 0.003), ("svc-b", 0.001)],
            &[("svc-a", 0.02), ("svc-b", 0.01)],
            &[("svc-a", "eu-west-3"), ("svc-b", "eu-west-3")],
            0.03,
            "scaphandre_rapl+cal",
        ),
        make_report(5, 50, 0, &[], vec![]),
        with_waste,
    ]
}

/// Floats may differ in their last bits: a rollup sums a day first, so
/// the additions run in another order than window by window.
fn approx_json(a: &serde_json::Value, b: &serde_json::Value, at: &str) {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            let tolerance = 1e-9 * x.abs().max(y.abs()).max(1e-3);
            assert!((x - y).abs() <= tolerance, "{at}: {x} != {y}");
        }
        (Value::Object(x), Value::Object(y)) => {
            assert_eq!(x.keys().collect::<Vec<_>>(), y.keys().collect::<Vec<_>>(), "{at}");
            for (key, value) in x {
                approx_json(value, &y[key], &format!("{at}.{key}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "{at}");
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                approx_json(p, q, &format!("{at}[{i}]"));
            }
        }
        _ => assert_eq!(a, b, "{at}"),
    }
}

fn assert_same_figures(windows: &AggregateInputs, rollups: &AggregateInputs) {
    approx_json(
        &serde_json::to_value(&windows.aggregate).unwrap(),
        &serde_json::to_value(&rollups.aggregate).unwrap(),
        "aggregate",
    );
    approx_json(
        &serde_json::to_value(&windows.per_service).unwrap(),
        &serde_json::to_value(&rollups.per_service).unwrap(),
        "per_service",
    );
    assert_eq!(windows.windows_aggregated, rollups.windows_aggregated);
    assert_eq!(windows.first_seen, rollups.first_seen);
    assert_eq!(windows.last_seen, rollups.last_seen);
    assert_eq!(windows.energy_source_models, rollups.energy_source_models);
    assert_eq!(windows.carbon_methodologies, rollups.carbon_methodologies);
    assert_eq!(windows.scoring_coefficients, rollups.scoring_coefficients);
    assert_eq!(windows.legacy_waste_windows, rollups.legacy_waste_windows);
    assert_eq!(windows.calibration_applied, rollups.calibration_applied);
    assert_eq!(windows.earliest_archived_window, rollups.earliest_archived_window);
    assert_eq!(windows.chain_verified, rollups.chain_verified);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn rollups_publish_the_figures_the_windows_do(
        picks in prop::collection::vec((0usize..5, 0i64..(3 * 86_400)), 1..40)
    ) {
        let pool = window_pool();
        let base = Utc.with_ymd_and_hms(2026, 1, 30, 0, 0, 0).unwrap();
        let mut lines: Vec<(DateTime<Utc>, Report)> = picks
            .iter()
            .map(|(i, s)| (base + chrono::Duration::seconds(*s), pool[*i].clone()))
            .collect();
        lines.sort_by_key(|(ts, _)| *ts);
        let (_wdir, window_path) = write_chained_archive(&lines);
        let rdir = TempDir::new().unwrap();
        let rollup_paths = write_rollups(rdir.path(), "archive", &lines);
        let from_windows = aggregate_from_paths(&[window_path], &q1_2026(), false).unwrap();
        let from_rollups = aggregate_from_paths(&rollup_paths, &q1_2026(), false).unwrap();
        assert_same_figures(&from_windows, &from_rollups);
    }
}

#[test]
fn a_day_with_a_rollup_ignores_that_days_windows() {
    let pool = window_pool();
    let before = vec![(day(10, 9), pool[0].clone()), (day(11, 9), pool[1].clone())];
    let mut after = before.clone();
    after.push((day(11, 15), pool[2].clone()));
    after.push((day(12, 9), pool[3].clone()));
    // Windows cover days 10 to 12, with an extra window on day 11 the
    // rollup does not hold. Rollups cover days 10 and 11.
    let (_wdir, window_path) = write_chained_archive(&after);
    let rdir = TempDir::new().unwrap();
    let mut paths = write_rollups(rdir.path(), "archive", &before);
    paths.push(window_path);
    let mixed = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();

    let mut expected_lines = before;
    expected_lines.push((day(12, 9), pool[3].clone()));
    let (_edir, expected_path) = write_chained_archive(&expected_lines);
    let expected = aggregate_from_paths(&[expected_path], &q1_2026(), false).unwrap();
    assert_eq!(mixed.windows_aggregated, expected.windows_aggregated);
    approx_json(
        &serde_json::to_value(&mixed.aggregate).unwrap(),
        &serde_json::to_value(&expected.aggregate).unwrap(),
        "aggregate",
    );
}

#[test]
fn two_lines_for_one_day_both_count() {
    let pool = window_pool();
    let dir = TempDir::new().unwrap();
    let lines = vec![
        (day(10, 23), pool[0].clone()),
        (day(11, 0), pool[0].clone()),
        (day(10, 22), pool[0].clone()),
    ];
    let paths = write_rollups(dir.path(), "archive", &lines);
    let agg = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();
    assert_eq!(agg.windows_aggregated, 3);
}

#[test]
fn a_removed_rollup_day_breaks_the_chain() {
    let pool = window_pool();
    let dir = TempDir::new().unwrap();
    let lines: Vec<_> = (10..14).map(|d| (day(d, 9), pool[0].clone())).collect();
    let paths = write_rollups(dir.path(), "archive", &lines);
    let text = std::fs::read_to_string(&paths[0]).unwrap();
    let kept: Vec<&str> = text.lines().enumerate().filter(|(i, _)| *i != 1).map(|(_, l)| l).collect();
    std::fs::write(&paths[0], kept.join("\n") + "\n").unwrap();
    let agg = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();
    assert_eq!(agg.chain_breaks, 1);
    assert_eq!(agg.windows_aggregated, 3);
}

#[test]
fn the_rollup_chain_runs_across_monthly_files() {
    let pool = window_pool();
    let dir = TempDir::new().unwrap();
    let lines = vec![(day(30, 9), pool[0].clone()), (day(31, 9), pool[0].clone()), (
        Utc.with_ymd_and_hms(2026, 2, 1, 9, 0, 0).unwrap(),
        pool[0].clone(),
    )];
    let paths = write_rollups(dir.path(), "archive", &lines);
    assert_eq!(paths.len(), 2);
    let agg = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();
    assert_eq!((agg.chain_breaks, agg.chain_verified), (0, 3));
}

#[test]
fn strict_attribution_refuses_a_rollup_holding_an_unattributed_window() {
    let dir = TempDir::new().unwrap();
    let paths = write_rollups(dir.path(), "archive", &[(day(10, 9), make_report(5, 50, 0, &[], vec![]))]);
    let err = aggregate_from_paths(&paths, &q1_2026(), true).unwrap_err();
    assert_matches!(err, AggregationError::UnattributedWindow { .. });
}

#[test]
fn a_rollup_cannot_claim_days_other_than_its_own() {
    let dir = TempDir::new().unwrap();
    let mut acc = DayAccumulator::default();
    acc.observe(day(10, 9), &plain_window());
    let mut closed = acc.take().unwrap();
    closed.state.insert_observed_day_for_test(day(20, 0).date_naive());
    let line = rollup::serialize_rollup(&closed, "seed", 0, 0).unwrap();
    let path = dir.path().join("archive-daily-2026-01.ndjson");
    std::fs::write(&path, line + "\n").unwrap();
    let agg = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(agg.aggregate.temporal_coverage.observed_days, 1);
}

#[test]
fn a_directory_mixes_windows_rollups_a_renamed_rollup_and_the_snapshot() {
    let pool = window_pool();
    let dir = TempDir::new().unwrap();
    write_rollups(dir.path(), "archive", &[(day(10, 9), pool[0].clone())]);
    let renamed = write_rollups(dir.path(), "other", &[(day(11, 9), pool[0].clone())]);
    std::fs::rename(&renamed[0], dir.path().join("copied.ndjson")).unwrap();
    let windows = dir.path().join("archive.ndjson");
    let mut file = File::create(&windows).unwrap();
    let envelope = serde_json::json!({ "ts": day(12, 9), "report": pool[0] });
    writeln!(file, "{envelope}").unwrap();
    std::fs::write(dir.path().join(rollup::partial_file_name("archive")), "{}").unwrap();
    let agg = aggregate_from_paths(&[dir.path().to_path_buf()], &q1_2026(), false).unwrap();
    assert_eq!(agg.windows_aggregated, 3);
    assert_eq!(agg.malformed_lines_skipped, 0);
}
```

Add to `PeriodState`, under `#[cfg(test)]`:

```rust
    #[cfg(test)]
    pub(crate) fn insert_observed_day_for_test(&mut self, day: NaiveDate) {
        self.observed_days.insert(day);
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p perf-sentinel-core report::periodic::aggregator::tests`
Expected: the new tests FAIL. Rollup lines are counted as malformed and the windows of covered days are folded too.

- [ ] **Step 3: Teach `Builder` about rollups**

Add to `Builder`:

```rust
    /// Days a rollup line already folded. A window line on one of them is
    /// read for its chain and drop counter only, so a day is never counted
    /// twice when an archive holds both shapes.
    rollup_days: BTreeSet<NaiveDate>,
    /// Chain anchor per rollup family. Unlike window files, a rollup chain
    /// runs across its monthly files, so a removed month shows as a break.
    rollup_anchors: BTreeMap<String, ChainAnchor>,
```

Give `ChainStep` a `weight: u64` field. In `walk_chain_line`, the verified and unchained branches add `step.weight` instead of `1`. The window call site passes `weight: 1`.

`aggregate_from_paths` reads rollups first:

```rust
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
```

A file is a rollup file when its first non-empty line carries the format tag:

```rust
/// Rollup files are told apart by content, so a renamed or copied one is
/// still read as a rollup.
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
        return Ok(serde_json::from_str::<serde_json::Value>(trimmed)
            .is_ok_and(|value| super::rollup::is_rollup_value(&value)));
    }
    Ok(false)
}

/// Monthly rollup files of one archive share a family, so their drop
/// counter and their chain run on across months.
fn rollup_family(path: &Path) -> String {
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let stem = super::rollup::parse_rollup_file_name(&name).map_or_else(
        || path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        |(stem, _, _)| stem.to_string(),
    );
    format!("{}\u{1}{stem}\u{1}daily", path.parent().unwrap_or(Path::new("")).display())
}
```

The rollup reader mirrors `process_file`, one helper per line to stay under the complexity gate:

```rust
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
        let family = rollup_family(path);
        // A family starts unanchored: retention deletes whole months from
        // the front, so the oldest surviving line is a start, not a break.
        let mut expected = self.rollup_anchors.remove(&family).unwrap_or(None);
        let mut chain_started = expected.is_some();
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
                expected: &mut expected,
                chain_started: &mut chain_started,
                warned_break: &mut warned_break,
            })?;
        }
        self.rollup_anchors.insert(family, expected);
        Ok(())
    }

    fn rollup_line(&mut self, step: RollupStep<'_>) -> Result<(), AggregationError> {
        let mut parsed: Option<serde_json::Value> = serde_json::from_str(step.trimmed).ok();
        let line = parsed
            .clone()
            .and_then(|value| serde_json::from_value::<super::rollup::RollupLine>(value).ok());
        let in_scope = line.as_ref().is_none_or(|l| period_has_day(step.period, l.day));
        if let Some(drops) = parsed.as_ref().and_then(|v| v.get("drops")).and_then(serde_json::Value::as_u64) {
            self.fold_drops(step.family, drops, in_scope);
        }
        let parsed_seq = parsed.as_ref().and_then(|v| v.get("seq")).and_then(serde_json::Value::as_u64);
        let next_seq = |expected: &ChainAnchor| {
            parsed_seq
                .unwrap_or_else(|| expected.as_ref().map_or(0, |(_, seq)| *seq))
                .saturating_add(1)
        };
        self.walk_chain_line(ChainStep {
            parsed: parsed.as_mut(),
            expected: step.expected,
            chain_started: step.chain_started,
            in_scope,
            previous_in_scope: in_scope,
            next_seq: &next_seq,
            path: step.path,
            line_no: step.line_no,
            warned_break: step.warned_break,
            weight: line.as_ref().map_or(1, |l| l.state.windows_aggregated()),
        });
        let Some(line) = line else {
            self.state.malformed_lines_skipped += 1;
            return Ok(());
        };
        self.fold_rollup(line, step.period, step.strict)
    }

    fn fold_rollup(
        &mut self,
        line: super::rollup::RollupLine,
        period: &Period,
        strict: bool,
    ) -> Result<(), AggregationError> {
        self.earliest_archived = Some(self.earliest_archived.map_or(line.ts, |seen| seen.min(line.ts)));
        if !period_has_day(period, line.day) {
            return Ok(());
        }
        let mut state = line.state;
        if strict && let Some(ts) = state.first_unattributed() {
            return Err(AggregationError::UnattributedWindow { ts: ts.to_rfc3339() });
        }
        // A rollup answers for its own day only.
        state.keep_only_day(line.day);
        self.rollup_days.insert(line.day);
        self.state.merge(state);
        Ok(())
    }
}

struct RollupStep<'a> {
    trimmed: &'a str,
    period: &'a Period,
    strict: bool,
    path: &'a Path,
    line_no: usize,
    family: &'a str,
    expected: &'a mut ChainAnchor,
    chain_started: &'a mut bool,
    warned_break: &'a mut bool,
}

fn period_has_day(period: &Period, day: NaiveDate) -> bool {
    period.from_date <= day && day <= period.to_date
}
```

`walk_chain_line` already counts the line's own break, which is the right semantics for rollups, so `previous_in_scope` equals `in_scope` here.

On `PeriodState`:

```rust
    /// Restrict the observed days to `day`, the only one a rollup line may
    /// answer for. An empty state keeps no day.
    pub(crate) fn keep_only_day(&mut self, day: NaiveDate) {
        self.observed_days.clear();
        if self.windows_aggregated > 0 {
            self.observed_days.insert(day);
        }
    }
```

- [ ] **Step 4: Skip window lines on rollup days**

In `Builder::process_file`, the scope test becomes:

```rust
            let in_scope = line_in_period(parsed.as_ref(), period)
                && !self.rollup_covers(parsed.as_ref());
```

In `Builder::fold_window`, right after updating `earliest_archived`:

```rust
        if !in_period(envelope.ts, step.period)
            || self.rollup_days.contains(&envelope.ts.date_naive())
        {
            return Ok(());
        }
```

And:

```rust
    /// Whether a rollup already folded this window line's day.
    fn rollup_covers(&self, value: Option<&serde_json::Value>) -> bool {
        value
            .and_then(|v| v.get("ts"))
            .and_then(serde_json::Value::as_str)
            .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
            .is_some_and(|ts| self.rollup_days.contains(&ts.with_timezone(&Utc).date_naive()))
    }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p perf-sentinel-core report::periodic`
Expected: PASS, the property test included. If the property test fails on a float, read the path in the message. A gap above 1e-9 relative is a fold bug, not a tolerance problem.

Run: `cargo test -p perf-sentinel --test e2e -- disclose`
Expected: PASS, the CLI paths are unchanged.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/report/periodic/aggregator.rs crates/sentinel-core/src/report/periodic/aggregator/tests.rs
git commit -m "feat(disclose): aggregate daily rollups beside window archives"
```

---

### Task 5: `[daemon.archive] windows` and `rollup_retention_days`

**Files:**
- Modify: `crates/sentinel-core/src/config/mod.rs:98-117`
- Modify: `crates/sentinel-core/src/config/raw.rs` (`ArchiveSection`, `convert_archive_section`)
- Modify: `crates/sentinel-core/src/config/validate.rs`
- Test: the config test module that already covers `[daemon.archive]` (find it with `grep -rn 'max_files' crates/sentinel-core/src/config/`)

**Interfaces:**
- Produces: `DaemonArchiveConfig { path, max_size_mb, max_files, windows: bool, rollup_retention_days: u32 }`, defaults `true` and `400`.

- [ ] **Step 1: Write the failing tests**

In the config tests next to the existing `[daemon.archive]` cases:

```rust
#[test]
fn archive_section_defaults_keep_windows_and_400_days_of_rollups() {
    let cfg = parse_config(
        "[daemon.archive]\npath = \"/var/lib/perf-sentinel/archive.ndjson\"\n",
    );
    let archive = cfg.daemon.archive.unwrap();
    assert!(archive.windows);
    assert_eq!(archive.rollup_retention_days, 400);
}

#[test]
fn archive_section_reads_windows_and_retention() {
    let cfg = parse_config(
        "[daemon.archive]\npath = \"/a/archive.ndjson\"\nwindows = false\nrollup_retention_days = 120\n",
    );
    let archive = cfg.daemon.archive.unwrap();
    assert!(!archive.windows);
    assert_eq!(archive.rollup_retention_days, 120);
}

#[test]
fn a_zero_day_rollup_retention_is_rejected() {
    let err = validate_config(
        "[daemon.archive]\npath = \"/a/archive.ndjson\"\nrollup_retention_days = 0\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("rollup_retention_days"), "{err}");
}
```

Use the parse and validate helpers that module already defines. If their names differ, keep the assertions and adapt the call.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p perf-sentinel-core config::`
Expected: compile errors, fields missing.

- [ ] **Step 3: Add the fields**

`config/mod.rs`:

```rust
#[derive(Debug, Clone)]
pub struct DaemonArchiveConfig {
    pub path: String,
    pub max_size_mb: u64,
    pub max_files: u32,
    /// Write one line per scoring window to `path`. `false` keeps only
    /// the daily rollups beside it.
    pub windows: bool,
    /// Age past which a monthly rollup file is deleted, counted from the
    /// newest closed day.
    pub rollup_retention_days: u32,
}

impl Default for DaemonArchiveConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            max_size_mb: 100,
            max_files: 12,
            windows: true,
            rollup_retention_days: 400,
        }
    }
}
```

`config/raw.rs`: add `windows: Option<bool>` and `rollup_retention_days: Option<u32>` to `ArchiveSection`, then in `convert_archive_section`:

```rust
        windows: raw.windows.unwrap_or(defaults.windows),
        rollup_retention_days: raw
            .rollup_retention_days
            .unwrap_or(defaults.rollup_retention_days),
```

`config/validate.rs`, beside the other `[daemon.archive]` checks:

```rust
    if let Some(archive) = &config.daemon.archive
        && archive.rollup_retention_days == 0
    {
        return Err(ConfigError::Invalid(
            "[daemon.archive] rollup_retention_days must be at least 1".to_string(),
        ));
    }
```

Use the error variant the neighbouring checks use.

- [ ] **Step 4: Fix the struct literals the new fields break**

Run: `cargo check --workspace --all-targets --features perf-sentinel-core/daemon`
Every `DaemonArchiveConfig { .. }` literal it reports, in `daemon/archive.rs` tests and elsewhere, gains `..DaemonArchiveConfig::default()`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p perf-sentinel-core config::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/config crates/sentinel-core/src/daemon/archive.rs
git commit -m "feat(config): add the archive's windows switch and rollup retention"
```

---

### Task 6: Daemon rollup writer

**Files:**
- Create: `crates/sentinel-core/src/daemon/rollup_writer.rs`
- Modify: `crates/sentinel-core/src/daemon/mod.rs` (`mod rollup_writer;`)
- Modify: `crates/sentinel-core/src/daemon/archive.rs` (make `open_append` and `write_line` `pub(super)`)
- Modify: `crates/sentinel-core/src/report/metrics.rs` (three gauges)

**Interfaces:**
- Consumes: `rollup::*` from Task 3, `DaemonArchiveConfig` from Task 5, `archive::open_append`, `archive::write_line`.
- Produces: `pub(super) struct RollupWriter` with `open(archive_path: &Path, retention_days: u32, drops: u64, metrics: &MetricsState) -> Self`, `observe(&mut self, ts: DateTime<Utc>, report: &Report, drops: u64, metrics: &MetricsState)`, `shutdown(self, drops: u64, metrics: &MetricsState)`. `MetricsState` fields `archive_oldest_day_seconds`, `archive_rollup_days`, `archive_last_closed_day_seconds`, all `IntGauge`.

- [ ] **Step 1: Add the gauges**

In `report/metrics.rs`, beside `archive_windows_dropped_total`, three fields on `MetricsState` and their registration, following the pattern of `analysis_queue_depth`:

```rust
    /// Oldest UTC day the daily rollups still hold, as epoch seconds at
    /// 00:00. Zero until a day has closed.
    pub archive_oldest_day_seconds: IntGauge,
    /// Distinct days held in the daily rollups.
    pub archive_rollup_days: IntGauge,
    /// Newest closed day in the daily rollups, as epoch seconds at 00:00.
    pub archive_last_closed_day_seconds: IntGauge,
```

```rust
        let archive_oldest_day_seconds = register_int_gauge(
            &registry,
            "perf_sentinel_archive_oldest_day_timestamp_seconds",
            "Oldest UTC day held in the disclosure archive's daily rollups, epoch seconds at 00:00",
        );
        let archive_rollup_days = register_int_gauge(
            &registry,
            "perf_sentinel_archive_rollup_days",
            "Distinct UTC days held in the disclosure archive's daily rollups",
        );
        let archive_last_closed_day_seconds = register_int_gauge(
            &registry,
            "perf_sentinel_archive_last_closed_day_timestamp_seconds",
            "Newest closed UTC day in the disclosure archive's daily rollups, epoch seconds at 00:00",
        );
```

Use the gauge registration helper `metrics.rs` already uses. If `MetricsState` can be built with a struct literal outside the crate, note the new fields in the CHANGELOG with the config fields.

- [ ] **Step 2: Write the failing tests**

At the bottom of the new `daemon/rollup_writer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn at(m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, m, d, h, 0, 0).unwrap()
    }

    fn window() -> Report {
        serde_json::from_str(include_str!("../../../../tests/fixtures/periodic/window-report.json"))
            .unwrap()
    }

    fn lines(dir: &Path) -> Vec<serde_json::Value> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.to_string_lossy().contains("-daily-"))
            .collect();
        names.sort();
        names
            .iter()
            .flat_map(|p| std::fs::read_to_string(p).unwrap().lines().map(String::from).collect::<Vec<_>>())
            .map(|l| serde_json::from_str(&l).unwrap())
            .collect()
    }

    #[test]
    fn a_day_closes_on_the_next_days_first_window_and_shutdown_writes_the_rest() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(1, 10, 9), &window(), 0, &metrics);
        w.observe(at(1, 11, 9), &window(), 0, &metrics);
        assert_eq!(lines(dir.path()).len(), 1);
        assert_eq!(metrics.archive_rollup_days.get(), 1);
        w.shutdown(0, &metrics);
        let all = lines(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[1]["seq"], 1);
        assert_eq!(all[1]["prev"], all[0]["hash"]);
        assert!(!dir.path().join(rollup::partial_file_name("archive")).exists());
    }

    #[test]
    fn the_chain_resumes_across_a_restart_and_a_month() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(1, 31, 9), &window(), 0, &metrics);
        w.shutdown(0, &metrics);
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        w.observe(at(2, 1, 9), &window(), 0, &metrics);
        w.shutdown(0, &metrics);
        let all = lines(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[1]["prev"], all[0]["hash"]);
        assert_eq!(all[1]["seq"], 1);
    }

    #[test]
    fn a_snapshot_left_by_a_crash_is_appended_exactly_once() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        for _ in 0..SNAPSHOT_EVERY {
            w.observe(at(1, 10, 9), &window(), 0, &metrics);
        }
        assert!(dir.path().join(rollup::partial_file_name("archive")).exists());
        drop(w); // SIGKILL: no shutdown
        let w = RollupWriter::open(&archive, 400, 0, &metrics);
        drop(w); // killed again before any window
        let _w = RollupWriter::open(&archive, 400, 0, &metrics);
        let all = lines(dir.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0]["state"]["windows_aggregated"], SNAPSHOT_EVERY);
    }

    #[test]
    fn a_snapshot_the_last_line_covers_is_discarded() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 400, 0, &metrics);
        for _ in 0..SNAPSHOT_EVERY {
            w.observe(at(1, 10, 9), &window(), 0, &metrics);
        }
        let snapshot = std::fs::read(dir.path().join(rollup::partial_file_name("archive"))).unwrap();
        w.shutdown(0, &metrics);
        // A crash between the shutdown append and the snapshot removal.
        std::fs::write(dir.path().join(rollup::partial_file_name("archive")), snapshot).unwrap();
        let _w = RollupWriter::open(&archive, 400, 0, &metrics);
        assert_eq!(lines(dir.path()).len(), 1);
    }

    #[test]
    fn retention_deletes_whole_months_past_the_horizon() {
        let dir = TempDir::new().unwrap();
        let metrics = MetricsState::new();
        let archive = dir.path().join("archive.ndjson");
        let mut w = RollupWriter::open(&archive, 30, 0, &metrics);
        w.observe(at(1, 10, 9), &window(), 0, &metrics);
        w.observe(at(2, 20, 9), &window(), 0, &metrics);
        w.observe(at(3, 25, 9), &window(), 0, &metrics);
        // The March window closes 2026-02-20: January's last day is 20
        // days back, kept. Shutdown closes 2026-03-25: January's last day
        // is then 53 days back, deleted, and February's 25, kept.
        w.shutdown(0, &metrics);
        assert!(!dir.path().join("archive-daily-2026-01.ndjson").exists());
        assert!(dir.path().join("archive-daily-2026-02.ndjson").exists());
        assert_eq!(metrics.archive_rollup_days.get(), 2);
        assert_eq!(
            metrics.archive_oldest_day_seconds.get(),
            at(2, 20, 0).timestamp()
        );
    }
}
```

- [ ] **Step 3: Run them to see them fail**

Run: `cargo test -p perf-sentinel-core --features daemon daemon::rollup_writer`
Expected: compile errors.

- [ ] **Step 4: Write the module**

```rust
//! Daily rollup files beside the window archive. One chained line per
//! UTC day in `<stem>-daily-YYYY-MM.ndjson`, the chain carried across
//! months and restarts, a partial snapshot so a crash costs at most
//! [`SNAPSHOT_EVERY`] windows, and retention by age. Runs on the archive
//! writer's blocking thread. See `docs/design/08-PERIODIC-DISCLOSURE.md`.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::report::Report;
use crate::report::metrics::MetricsState;
use crate::report::periodic::hasher::ARCHIVE_CHAIN_SEED;
use crate::report::periodic::rollup::{self, ClosedDay, DayAccumulator};

/// Windows between two snapshots of the open day: about five minutes at
/// the default 15-second window.
pub(super) const SNAPSHOT_EVERY: u64 = 20;

pub(super) struct RollupWriter {
    dir: PathBuf,
    stem: String,
    retention_days: u32,
    acc: DayAccumulator,
    prev: String,
    seq: u64,
    since_snapshot: u64,
}

impl RollupWriter {
    pub(super) fn open(
        archive_path: &Path,
        retention_days: u32,
        drops: u64,
        metrics: &MetricsState,
    ) -> Self {
        let dir = archive_path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let stem = archive_path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("archive")
            .to_string();
        let mut writer = Self {
            dir,
            stem,
            retention_days,
            acc: DayAccumulator::default(),
            prev: ARCHIVE_CHAIN_SEED.to_string(),
            seq: 0,
            since_snapshot: 0,
        };
        let last_ts = writer.resume();
        writer.recover_snapshot(last_ts, drops);
        writer.refresh_horizon(metrics);
        writer
    }

    pub(super) fn observe(
        &mut self,
        ts: DateTime<Utc>,
        report: &Report,
        drops: u64,
        metrics: &MetricsState,
    ) {
        if let Some(day) = self.acc.observe(ts, report) {
            self.close(&day, drops, metrics);
        }
        self.since_snapshot += 1;
        if self.since_snapshot >= SNAPSHOT_EVERY {
            self.snapshot();
        }
    }

    /// Graceful stop: the day so far becomes a line, and the snapshot it
    /// supersedes goes. A restart the same day opens a second line for it.
    pub(super) fn shutdown(mut self, drops: u64, metrics: &MetricsState) {
        if let Some(day) = self.acc.take() {
            self.close(&day, drops, metrics);
        }
        let _ = std::fs::remove_file(self.partial_path());
    }

    fn close(&mut self, day: &ClosedDay, drops: u64, metrics: &MetricsState) {
        self.append(day, drops);
        self.prune(day.day);
        self.refresh_horizon(metrics);
    }

    fn partial_path(&self) -> PathBuf {
        self.dir.join(rollup::partial_file_name(&self.stem))
    }

    /// Rollup files of this archive, oldest month first.
    fn files(&self) -> Vec<(i32, u32, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                let (stem, year, month) = rollup::parse_rollup_file_name(&name)?;
                (stem == self.stem).then(|| (year, month, entry.path()))
            })
            .collect();
        files.sort();
        files
    }

    /// Continue the chain from the newest line on disk. Returns that
    /// line's `ts`, which identifies the day state it closed.
    fn resume(&mut self) -> Option<DateTime<Utc>> {
        let (_, _, newest) = self.files().pop()?;
        let file = std::fs::File::open(newest).ok()?;
        let last = BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
            .filter(rollup::is_rollup_value)
            .last()?;
        let hash = last.get("hash")?.as_str()?.to_string();
        let seq = last.get("seq")?.as_u64()?;
        (self.prev, self.seq) = (hash, seq.saturating_add(1));
        last.get("ts")?
            .as_str()
            .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
            .map(|ts| ts.with_timezone(&Utc))
    }

    /// A snapshot whose day state the last line already closed (same
    /// first window) is stale. Any other is the only copy of its windows.
    fn recover_snapshot(&mut self, last_ts: Option<DateTime<Utc>>, drops: u64) {
        let path = self.partial_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        if let Some(day) = rollup::parse_partial(&text)
            && Some(day.first_ts) != last_ts
        {
            self.append(&day, drops);
        }
        let _ = std::fs::remove_file(path);
    }

    fn append(&mut self, day: &ClosedDay, drops: u64) {
        let line = match rollup::serialize_rollup(day, &self.prev, self.seq, drops) {
            Ok(line) => line,
            Err(err) => {
                tracing::warn!(error = %err, day = %day.day, "daily rollup not serialised, the day falls back to its window lines");
                return;
            }
        };
        let path = self.dir.join(rollup::rollup_file_name(&self.stem, day.day));
        let written = super::archive::open_append(&path)
            .map_err(|err| std::io::Error::other(err.to_string()))
            .and_then(|mut file| super::archive::write_line(&mut file, &line));
        match written {
            Ok(()) => {
                if let Some(hash) = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v.get("hash")?.as_str().map(String::from))
                {
                    self.prev = hash;
                }
                self.seq = self.seq.saturating_add(1);
            }
            Err(err) => {
                tracing::warn!(error = %err, day = %day.day, "daily rollup not written, the day falls back to its window lines");
            }
        }
    }

    /// Overwrite the snapshot atomically: a temporary file, then a rename.
    fn snapshot(&mut self) {
        self.since_snapshot = 0;
        let Some(open) = self.acc.peek() else { return };
        let Ok(text) = rollup::serialize_partial(open) else { return };
        let target = self.partial_path();
        let tmp = target.with_extension("json.tmp");
        let written = super::archive::open_truncate(&tmp)
            .and_then(|mut file| file.write_all(text.as_bytes()).and_then(|()| file.sync_all()))
            .and_then(|()| std::fs::rename(&tmp, &target));
        if let Err(err) = written {
            tracing::warn!(error = %err, "daily rollup snapshot not written");
        }
    }

    /// Delete each month whose last day is more than `retention_days`
    /// before `newest`.
    fn prune(&self, newest: NaiveDate) {
        let cutoff = newest - Duration::days(i64::from(self.retention_days));
        for (year, month, path) in self.files() {
            if last_day_of_month(year, month).is_some_and(|last| last < cutoff) {
                if let Err(err) = std::fs::remove_file(&path) {
                    tracing::warn!(error = %err, path = %path.display(), "expired daily rollup file not removed");
                }
            }
        }
    }

    fn refresh_horizon(&self, metrics: &MetricsState) {
        let mut days = std::collections::BTreeSet::new();
        for (_, _, path) in self.files() {
            let Ok(file) = std::fs::File::open(path) else { continue };
            days.extend(
                BufReader::new(file)
                    .lines()
                    .map_while(Result::ok)
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
                    .filter(rollup::is_rollup_value)
                    .filter_map(|v| v.get("day")?.as_str()?.parse::<NaiveDate>().ok()),
            );
        }
        let seconds = |day: &NaiveDate| day.and_hms_opt(0, 0, 0).map_or(0, |t| t.and_utc().timestamp());
        metrics.archive_rollup_days.set(i64::try_from(days.len()).unwrap_or(i64::MAX));
        metrics.archive_oldest_day_seconds.set(days.first().map_or(0, seconds));
        metrics.archive_last_closed_day_seconds.set(days.last().map_or(0, seconds));
    }
}

fn last_day_of_month(year: i32, month: u32) -> Option<NaiveDate> {
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)?.pred_opt()
}
```

In `daemon/archive.rs`, make `open_append` and `write_line` `pub(super)`, and add the snapshot opener next to `open_append`:

```rust
/// Owner-only, no-follow, truncating open for the rollup snapshot's
/// temporary file.
pub(super) fn open_truncate(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    no_follow(&mut options);
    options.open(path)
}
```

Declare the module in `daemon/mod.rs`: `mod rollup_writer;`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p perf-sentinel-core --features daemon daemon::rollup_writer`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/daemon crates/sentinel-core/src/report/metrics.rs
git commit -m "feat(daemon): write the disclosure archive's daily rollups"
```

---

### Task 7: Wire the rollup writer into the archive task

**Files:**
- Modify: `crates/sentinel-core/src/daemon/archive.rs` (`spawn`, `run_writer`)

**Interfaces:**
- Consumes: `RollupWriter` from Task 6, `DaemonArchiveConfig.windows` and `rollup_retention_days` from Task 5.
- Produces: behaviour only. With `windows = false` no window file is opened or written.

- [ ] **Step 1: Write the failing tests**

In `archive.rs`'s test module:

```rust
#[tokio::test]
async fn every_window_also_lands_in_the_daily_rollup() {
    let dir = TempDir::new().unwrap();
    let metrics = Arc::new(MetricsState::new());
    let handle = spawn(&cfg(&dir, 100, 12), metrics.clone()).unwrap();
    for _ in 0..3 {
        handle.tx.send(sample_archive()).await.unwrap();
    }
    drop(handle.tx);
    handle.join.await.unwrap();
    let daily: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("-daily-"))
        .collect();
    assert_eq!(daily.len(), 1);
    let line = std::fs::read_to_string(daily[0].path()).unwrap();
    let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(value["state"]["windows_aggregated"], 3);
}

#[tokio::test]
async fn windows_off_writes_rollups_only() {
    let dir = TempDir::new().unwrap();
    let metrics = Arc::new(MetricsState::new());
    let config = DaemonArchiveConfig {
        windows: false,
        ..cfg(&dir, 100, 12)
    };
    let handle = spawn(&config, metrics).unwrap();
    handle.tx.send(sample_archive()).await.unwrap();
    drop(handle.tx);
    handle.join.await.unwrap();
    assert!(!dir.path().join("archive.ndjson").exists());
    assert!(std::fs::read_dir(dir.path())
        .unwrap()
        .any(|e| e.unwrap().file_name().to_string_lossy().contains("-daily-")));
}
```

Use the module's existing window builder in place of `sample_archive()` if it has another name.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p perf-sentinel-core --features daemon daemon::archive`
Expected: the two new tests FAIL.

- [ ] **Step 3: Extract the window writing into `WindowSink`**

`run_writer` already sits near the complexity gate. Its window body moves into a struct, and the loop gains the rollup call:

```rust
/// The window file and its chain head.
struct WindowSink {
    path: PathBuf,
    file: File,
    bytes_written: u64,
    cap_bytes: u64,
    max_files: u32,
    prev: String,
    seq: u64,
}

impl WindowSink {
    fn write(&mut self, archive: &OwnedArchive, metrics: &MetricsState) {
        // The body of the former loop, unchanged, with `file`,
        // `bytes_written`, `prev`, `seq`, `cap_bytes`, `max_files` and
        // `path` read from `self`, and each `continue` turned into `return`.
    }
}

fn run_writer(
    mut rx: Receiver<OwnedArchive>,
    mut windows: Option<WindowSink>,
    mut rollups: RollupWriter,
    metrics: &Arc<MetricsState>,
) {
    while let Some(archive) = rx.blocking_recv() {
        let drops = metrics.archive_drops_total();
        rollups.observe(archive.ts, &archive.report, drops, metrics);
        if let Some(sink) = windows.as_mut() {
            sink.write(&archive, metrics);
        }
    }
    rollups.shutdown(metrics.archive_drops_total(), metrics);
}
```

Move the body by cut and paste. Keep every comment of the former loop with its line.

`spawn` opens the window file only when `cfg.windows` is set, and the rollup writer on the blocking thread:

```rust
pub fn spawn(
    cfg: &DaemonArchiveConfig,
    metrics: Arc<MetricsState>,
) -> Result<ArchiveHandle, ArchiveError> {
    let path = PathBuf::from(&cfg.path);
    refuse_symlink(&path)?;
    let windows = if cfg.windows {
        let mut file = open_append(&path)?;
        terminate_incomplete_line(&mut file).map_err(|source| ArchiveError::Open {
            path: path.display().to_string(),
            source,
        })?;
        let (prev, seq) = resume_chain(&path);
        Some(WindowSink {
            bytes_written: metadata_len(&path),
            path: path.clone(),
            file,
            cap_bytes: cfg.max_size_mb.saturating_mul(1_048_576),
            max_files: cfg.max_files,
            prev,
            seq,
        })
    } else {
        None
    };
    let retention_days = cfg.rollup_retention_days;
    let (tx, rx) = mpsc::channel::<OwnedArchive>(CHANNEL_CAPACITY);
    let join = tokio::task::spawn_blocking(move || {
        let rollups =
            RollupWriter::open(&path, retention_days, metrics.archive_drops_total(), &metrics);
        run_writer(rx, windows, rollups, &metrics);
    });
    Ok(ArchiveHandle { tx, join })
}
```

`resume_chain` moves from the start of `run_writer` into `spawn`, where the sink is built.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p perf-sentinel-core --features daemon daemon::`
Expected: PASS, every existing archive test included.

Run: `cargo check --workspace --no-default-features`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/daemon/archive.rs
git commit -m "feat(daemon): fold every archived window into its daily rollup"
```

---

### Task 8: Horizon in `/api/status`

**Files:**
- Modify: `crates/sentinel-core/src/daemon/query_api/mod.rs` (`StatusResponse`, `handle_status`)
- Test: `crates/sentinel-core/src/daemon/query_api/tests.rs`

**Interfaces:**
- Consumes: the three gauges from Task 6.
- Produces: `"disclosure_archive": {"oldest_day": "YYYY-MM-DD", "days": N, "last_closed_day": "YYYY-MM-DD"}`, absent until a day has closed.

- [ ] **Step 1: Write the failing tests**

In `query_api/tests.rs`, next to the existing `/api/status` test, using its state builder:

```rust
#[tokio::test]
async fn status_reports_the_rollup_horizon_once_a_day_closed() {
    let state = test_state();
    let app = query_api_router(state.clone());
    let body = get_json(&app, "/api/status").await;
    assert!(body.get("disclosure_archive").is_none());

    state.metrics.archive_rollup_days.set(2);
    state.metrics.archive_oldest_day_seconds.set(1_767_225_600); // 2026-01-01
    state.metrics.archive_last_closed_day_seconds.set(1_767_312_000); // 2026-01-02
    let body = get_json(&app, "/api/status").await;
    assert_eq!(
        body["disclosure_archive"],
        serde_json::json!({"oldest_day": "2026-01-01", "days": 2, "last_closed_day": "2026-01-02"})
    );
}
```

Use the helpers the module already has for building state and reading JSON.

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -p perf-sentinel-core --features daemon query_api::tests::status_reports`
Expected: FAIL, the field is missing.

- [ ] **Step 3: Add the field**

```rust
    /// Days the disclosure archive's daily rollups hold, absent until one
    /// has closed. A period starting before `oldest_day` cannot be
    /// disclosed officially from this archive. Additive since 0.29.0.
    #[serde(skip_serializing_if = "Option::is_none")]
    disclosure_archive: Option<DisclosureArchiveStatus>,
```

```rust
#[derive(Serialize)]
struct DisclosureArchiveStatus {
    oldest_day: chrono::NaiveDate,
    days: u64,
    last_closed_day: chrono::NaiveDate,
}

fn disclosure_archive_status(metrics: &MetricsState) -> Option<DisclosureArchiveStatus> {
    let days = u64::try_from(metrics.archive_rollup_days.get()).ok().filter(|d| *d > 0)?;
    let day = |seconds: i64| chrono::DateTime::from_timestamp(seconds, 0).map(|t| t.date_naive());
    Some(DisclosureArchiveStatus {
        oldest_day: day(metrics.archive_oldest_day_seconds.get())?,
        days,
        last_closed_day: day(metrics.archive_last_closed_day_seconds.get())?,
    })
}
```

In `handle_status`: `disclosure_archive: disclosure_archive_status(&state.metrics),`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p perf-sentinel-core --features daemon query_api::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/daemon/query_api
git commit -m "feat(daemon): report the disclosure archive's horizon in the status API"
```

---

### Task 9: Documentation and CHANGELOG

**Files:**
- Modify: `docs/CONFIGURATION.md` and `docs/FR/CONFIGURATION-FR.md` (the `[daemon.archive]` table, find it with `grep -n 'max_files' docs/CONFIGURATION.md`)
- Modify: `docs/REPORTING.md` and `docs/FR/REPORTING-FR.md`
- Modify: `docs/METRICS.md` and `docs/FR/METRICS-FR.md`
- Modify: `docs/QUERY-API.md` and `docs/FR/QUERY-API-FR.md`
- Modify: `docs/design/08-PERIODIC-DISCLOSURE.md` and `docs/FR/design/08-PERIODIC-DISCLOSURE-FR.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Configuration**

Add two rows to the `[daemon.archive]` table:

```markdown
| `windows`               | bool | `true` | Write one line per scoring window to `path`. `false` keeps only the daily rollups. |
| `rollup_retention_days` | int  | `400`  | A monthly rollup file is deleted once its last day is this many days older than the newest closed day. Must be at least 1. |
```

Then one paragraph under the table: the daemon also writes `<stem>-daily-YYYY-MM.ndjson` beside `path`, one line per UTC day, about 30 KB per day. With the chart's persistence on, they land on the same volume with no chart change.

- [ ] **Step 2: Reporting**

In `docs/REPORTING.md`, beside the 0.28.0 section on the archive reaching the period start, add a section titled "Daily rollups hold a period (0.29.0)". It says:
- what a rollup line is and where it lives;
- that `disclose --input` takes window files, rollup files or both, and a directory holding both;
- that a day present as a rollup is read from the rollup, its window lines are read for their chain only;
- that figures from rollups match figures from windows up to floating-point rounding, so the same period disclosed from each can differ in its last digits and in its `content_hash`;
- that `--strict-attribution` refuses a rollup holding an unattributed window;
- that the published chain counters count the windows a verified rollup line holds, and one break per broken rollup line;
- how retention, the chain across months, and the 0.28.0 reach check fit together.

- [ ] **Step 3: Metrics and status**

`docs/METRICS.md`: three rows for the gauges, no labels, plus an alert example:

```yaml
- alert: PerfSentinelDailyRollupStalled
  expr: perf_sentinel_archive_rollup_days > 0 and time() - perf_sentinel_archive_last_closed_day_timestamp_seconds > 2 * 86400
  for: 1h
```

`docs/QUERY-API.md`: the `disclosure_archive` row in the status table and in the example response.

- [ ] **Step 4: Design note 08**

Add a "Daily rollups" section: the line format with its fields, the fold shared with the reader, the chain across months, the snapshot and its dedup rule by first window, retention, and the rollup-wins rule.

- [ ] **Step 5: French mirrors**

Mirror every change of Steps 1 to 4 in the matching `docs/FR/` file, with French accents, straight quotes, no em-dash and no semicolon in prose.

- [ ] **Step 6: CHANGELOG**

Under `[Unreleased]`:

```markdown
### Added

- **Daily rollups for periodic disclosure.** With `[daemon.archive]` on, the daemon also writes one chained line per UTC day to `<stem>-daily-YYYY-MM.ndjson` beside the window archive, about 30 KB per day, kept `rollup_retention_days` (400 by default). `disclose` reads rollup files, window files or both, and a quarter now fits in a few MB.
- `[daemon.archive] windows = false` keeps only the daily rollups.
- `/api/status` reports `disclosure_archive` (oldest day, days held, last closed day), backed by `perf_sentinel_archive_oldest_day_timestamp_seconds`, `perf_sentinel_archive_rollup_days` and `perf_sentinel_archive_last_closed_day_timestamp_seconds`.

### Changed

- `DaemonArchiveConfig`, published on crates.io and not `#[non_exhaustive]`, gains the public fields `windows` and `rollup_retention_days`. Code building it with a struct literal must add them or use `..Default::default()`. Minor bump.
- `ServiceAccumulator` and `AntiPatternAccumulator` derive `Serialize` and `Deserialize`.
```

Add the `MetricsState` note there too if Task 6 Step 1 found it constructible outside the crate.

- [ ] **Step 7: Commit**

```bash
git add docs CHANGELOG.md
git commit -m "docs: describe the disclosure archive's daily rollups"
```

---

### Task 10: Full verification

- [ ] **Step 1: Run every gate**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings
cargo check --workspace --no-default-features
cargo check -p perf-sentinel-core --no-default-features --all-targets
cargo test --workspace
```

Expected: all clean and green. Fix any failure in the task that owns the code and amend nothing: add a commit.

- [ ] **Step 2: Refresh the knowledge graph**

Run: `graphify update .`

---

### Task 11: Lab scenario

Lab commits go straight to the lab's `main`. Build and pin a daemon image from the branch first with the lab's `seed-daemon-local`, and never commit the manifest pin.

**Files:**
- Modify: `~/IdeaProjects/perf-sentinel-simulation-lab/scenarios/chart-disclose-persistence/verify.sh`
- Modify: `~/IdeaProjects/perf-sentinel-simulation-lab/scenarios/chart-disclose-persistence/README.md`

- [ ] **Step 1: Read the rollups beside the archive**

In `read_archive`, the reader pod also prints the monthly rollup files after the window archive:

```bash
  out="$(kubectl -n "${NS}" exec "${SCENARIO}-reader" -- sh -c \
    'if [ -f /data/archive.ndjson ]; then cat /data/archive.ndjson; fi; echo __ARCHIVE_OK__; for f in /data/archive-daily-*.ndjson; do if [ -f "$f" ]; then cat "$f"; fi; done; echo __ROLLUPS_OK__' 2>/dev/null)"
  rc=$?
  kubectl -n "${NS}" delete pod "${SCENARIO}-reader" --grace-period=1 --wait=true >/dev/null 2>&1
  if [ "${rc}" -ne 0 ] || [ "$(printf '%s\n' "${out}" | tail -1)" != "__ROLLUPS_OK__" ]; then
    ARCHIVE_STATUS="readfail"
    return 0
  fi

  printf '%s\n' "${out}" | sed '/^__ARCHIVE_OK__$/,$d' > "${out_file}"
  printf '%s\n' "${out}" | sed '1,/^__ARCHIVE_OK__$/d;$d' > "${out_file%.ndjson}-rollups.ndjson"
```

- [ ] **Step 2: Disclose over the rollups alone**

After step 7, a step 8 runs the same `disclose` over `post-restart-rollups.ndjson` and compares the window and anti-pattern counts with step 7's:

```bash
step "8. Run disclose against the daily rollups alone"
if [ ! -s "${TMP_DIR}/post-restart-rollups.ndjson" ]; then
  assert_fail "disclose-rollups" "no daily rollup line on the PVC after a graceful drain"
elif [ -z "${WINDOWS:-}" ]; then
  record "disclose-rollups" "SKIP - step 7 produced no window figures to compare against"
else
  if ${DOCKER_ENV_PREFIX[@]+"${DOCKER_ENV_PREFIX[@]}"} docker run --rm -u "$(id -u):$(id -g)" \
    -v "$(docker_mount_path "${TMP_DIR}"):/data" \
    "${IMAGE}" disclose \
    --intent internal --confidentiality internal \
    --period-type custom --from "${FROM_DATE}" --to "${TODAY}" \
    --input /data/post-restart-rollups.ndjson \
    --output /data/report-rollups.json \
    --org-config /data/org-config.toml >"${TMP_DIR}/disclose-rollups.log" 2>&1; then
    ROLLUP_STATS="$(python3 -c '
import json, sys
a = json.load(open(sys.argv[1])).get("aggregate", {})
print(a.get("runtime_windows_count", 0) + a.get("fallback_windows_count", 0), a.get("anti_patterns_detected_count", 0))
' "${TMP_DIR}/report-rollups.json" 2>/dev/null)"
    R_WINDOWS="$(printf '%s' "${ROLLUP_STATS}" | awk '{print $1}')"
    R_PATTERNS="$(printf '%s' "${ROLLUP_STATS}" | awk '{print $2}')"
    if [ "${R_WINDOWS:-x}" = "${WINDOWS}" ] && [ "${R_PATTERNS:-x}" = "${PATTERNS}" ]; then
      assert_pass "disclose-rollups" "rollups alone give ${R_WINDOWS} window(s) and ${R_PATTERNS} anti-pattern(s), as the windows do"
    else
      assert_fail "disclose-rollups" "rollups give ${R_WINDOWS:-?} window(s) and ${R_PATTERNS:-?} anti-pattern(s), windows gave ${WINDOWS} and ${PATTERNS}"
    fi
  else
    tail -20 "${TMP_DIR}/disclose-rollups.log"
    assert_fail "disclose-rollups" "disclose over the rollups exited non-zero, see ${TMP_DIR}/disclose-rollups.log"
  fi
fi
```

Initialise `WINDOWS=""` and `PATTERNS=""` before step 7 so step 8 can tell "skipped" from "zero".

- [ ] **Step 3: Document and run**

Add a "disclose-rollups" row to the scenario README's table. Run the scenario against the pinned image:

```bash
cd ~/IdeaProjects/perf-sentinel-simulation-lab && DAEMON_IMAGE=<pinned image> ./scenarios/chart-disclose-persistence/verify.sh
```

Expected: every row PASS, `disclose-rollups` included.

- [ ] **Step 4: Commit on the lab's main**

```bash
git -C ~/IdeaProjects/perf-sentinel-simulation-lab add scenarios/chart-disclose-persistence
git -C ~/IdeaProjects/perf-sentinel-simulation-lab commit -m "feat(chart-disclose-persistence): disclose from the daily rollups alone"
```

---

## Out of scope

- The Hub showing the horizon per source. It reads `/api/status` once this ships.
- `disclose --tui` reading rollups: its range scan reads `ts`, so it already shows a rollup's first window. Its end bound can fall short of a day's last window by some hours.
- A CLI end-to-end test over rollup files: the line format is crate-internal, and the core tests drive `aggregate_from_paths` directly.
- The chart: rollups sit beside the archive on the same volume, so no value or template changes.
