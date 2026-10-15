# Correlation push, engine side, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A perf-sentinel daemon with `[daemon.hub_export]` and `[daemon.correlation] enabled = true` cuts each correlated pair's activity into episodes and pushes them to PerfSentinelHub's `POST /api/import/correlations`, and the findings dashboard draws them in its History (Hub) row.

**Architecture:** The incident half of the Hub exporter is first generalised into a shared latest-value table and route half (`hub_table.rs`), so the correlation half reuses it instead of copying a hundred lines. A new `hub_correlation_export.rs` holds the episode identity (`pair_key`, episode `id`), an `EpisodeTracker` that reads `active_correlations()` from the shared correlator each round and opens, updates and closes episodes from `last_seen`, and a `CorrelationExport` half with its own reading clock and send backoff. `run_exporter` gains a third half through a small `Halves` struct, and a graceful drain closes every open episode before it empties the three tables. The correlator does not change.

**Tech Stack:** Rust 2024 (MSRV 1.99.0), tokio, hyper via `crate::http_client`, serde_json, sha2, prometheus, axum (test fake Hub). Grafana 10 JSON with the Infinity datasource, Python 3 stdlib for the French dashboard generator.

**Spec:** `docs/superpowers/specs/2026-10-15-correlation-history-in-hub-design.md`. This plan covers its **Engine** and **Grafana** sections and the engine lines of **Documentation** and **Testing**. The Hub side (storage, import and read routes, launcher) and the `hub-correlations-push` lab scenario belong to another plan, and the Hub ships first.

**Repository:** `/Users/robintrassard/RustroverProjects/perf-sentinel`, branch `feat/correlation-push` (holds the spec commit).

## Global Constraints

- Activation: `[daemon.hub_export]` plus `[daemon.correlation] enabled = true`, which is exactly when `setup_correlator` returns `Some`. No new configuration key.
- `pair_key`: 32 hex characters of a SHA-256 over the canonical source and target endpoints. Stable across restarts and daemons.
- Episode `id`: 32 hex characters of a SHA-256 over `pair_key` and `started_at_ms`. Stable across the pushes of one episode.
- Activity is read from `last_seen`, not from presence. Episode gap: `min(window_ms, 30 min)`, 10 minutes by default.
- Open: a pair in the snapshot with no open episode, whose `last_seen` is newer than any episode the tracker closed for it. `started_at_ms` is the correlator's `first_seen` for a pair the tracker has never seen, and the first `last_seen` observed after the gap otherwise.
- Update: an open episode whose pair's `last_seen` advanced. It keeps `last_seen_ms`, the peaks of confidence and co-occurrences, the latest confidence, co-occurrences and median lag, and the two sample trace ids.
- Close: an open episode whose `last_seen` has not advanced for the episode gap, or whose pair left the snapshot for that long. `ended_at_ms` is its last `last_seen`. Graceful shutdown closes every open episode at its `last_seen` before the drain.
- Queue triggers: open, peak rise (confidence or co-occurrences), `last_seen` advanced by at least 60 seconds since the last queued copy, close. Round-to-round drift of the rolling values is not sent.
- Table: latest value per episode id with revisions, bounded by `max_pending`. Evicting counts on `perf_sentinel_hub_export_correlations_dropped_total`.
- Send: a third half in the exporter task, on its own backoff and pace. Batches of at most 100 episodes and 1 MiB of JSON. An episode alone over 1 MiB is dropped and counted.
- Body: `{"producer_version": "<CARGO_PKG_VERSION>", "episodes": [<Episode>...]}`, each episode in the spec's wire format, `gap_ms` included.
- URL: `endpoint` with its `/api/import/findings` suffix replaced by `/api/import/correlations`, plus the same `?source_id=`, same `X-API-Key`.
- A 404 or 405 stops correlation pushes until restart: one `WARN`, the table cleared and counted as dropped. Findings and incidents keep flowing.
- The graceful drain empties the three tables within the existing 10 s budget (`SHUTDOWN_DRAIN_BUDGET`).
- Metrics, both label-free: `perf_sentinel_hub_export_correlations_pending` (gauge) and `perf_sentinel_hub_export_correlations_dropped_total` (counter).
- Grafana: a State timeline (one line per pair, one segment per episode) and a table under it in the "History (Hub)" row of `examples/grafana-findings-dashboard.json`, Infinity on `GET /api/correlations?from=${__from}&to=${__to}`, plus the Hub's incidents as annotations. The Hub dependency it names is PerfSentinelHub 0.6.0. The daemon floor does not move. French variants regenerated with `scripts/translate-dashboards-fr.py`.
- Versions: perf-sentinel 0.30.0, chart in lockstep (the release bump is a separate step, as in 0.29.0). PerfSentinelHub 0.6.0 ships first.
- Out of scope: acknowledging a correlation, a Findings screen in the launcher, filling `correlations` in the NDJSON archive, a confidence series per episode.
- `sentinel-core` stays `#![forbid(unsafe_code)]`. Clean: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings` (pedantic), `cargo check --workspace --no-default-features`, `cargo fmt --all`. Sonar cognitive complexity at most 15 per function. Prometheus label values from bounded compile-time sets only.
- Unit tests next to the code, or in a sibling `tests.rs` behind `mod tests;` (`foo.rs` + `foo/tests.rs`).
- English only in code, comments and docs. French only under `docs/FR/` and `examples/FR/`. Every `docs/X.md` change ships with `docs/FR/X-FR.md` in the same commit. Prose: no em dashes, no semicolons, straight quotes.
- Commits: Conventional Commits, subject line only, no body, no Co-Authored-By, no process words, signed (`-S`). Each commit is chained with `&&` after its checks, so a failing check blocks it.

## Review Focus

- A pair that drops out of the snapshot (below the thresholds) and comes back with new activity while the correlator still holds its old `first_seen`: the new episode must start at its new `last_seen`, never at that `first_seen`, or it overlaps the episode already closed. Pinned in Task 3.
- A pair the tracker first meets with a `last_seen` already older than the gap (it crossed the thresholds because the source total decayed, with no new co-occurrence): it must open nothing, not an episode born closed in the past. Pinned in Task 3.
- Two pairs identical except for `grouping_value` (the same services in two namespaces): distinct `pair_key`s, so the tracker never merges them into one episode. Pinned in Task 2.
- A correlation route that keeps failing (5xx, 401 from an older Hub) backs off up to five minutes: the tracker must keep reading the correlator every flush interval meanwhile, so episode starts and ends stay precise to one interval. Pinned in Task 5.
- A daemon with `[daemon.hub_export]` on and `[daemon.correlation]` off: no correlation table, nothing sent to the route, the gauge stays at 0. Pinned in Task 6.

## Rulings

Places where the spec was silent or the code disagreed with it. Each is applied in the task named.

1. **`pair_key` covers `grouping_value` too** (Task 2). The spec lists `(finding_type, service, template, grouping_key)`, but the correlator keys a pair on five fields and refuses to pair across `grouping_value` (`pairable` in `correlate_cross.rs`). Hashing four would merge the same pair from two namespaces into one tracker entry. Fields are length-prefixed with a presence byte, so `None` and `""` differ.
2. **Endpoints travel in the correlator's own JSON shape** (Task 2). `Episode.source` and `.target` are `CorrelationEndpoint`, serialized as `GET /api/correlations` already does: `grouping_key` and `grouping_value` are omitted when absent (the spec example writes `null`) and `grouping_value` travels when set. The Hub plan must read an absent field as null and store `grouping_value` beside `grouping_key`.
3. **No episode opens on a `last_seen` older than the gap** (Task 3). The spec's open rule says nothing about recency, but its "activity is read from `last_seen`" decision does. Without it, a pair crossing the thresholds long after its last co-occurrence would open and close a past episode in one round.
4. **A closed pair is remembered for one `window_ms` after its last `last_seen`** (Task 3). Past that, the correlator has dropped the pair too (it retains pairs with `last_seen >= now - window_ms`), so a returning pair carries a fresh `first_seen`. Forgetting earlier would let a returning pair start at a stale `first_seen` (Review Focus line 1). Memory stays bounded by the pairs active within one window, which the correlator itself bounds by `max_tracked_pairs`.
5. **The tracker reads the correlator on its own clock** (Task 5). `CorrelationExport` keeps `observe_due` (every `flush_interval`) apart from the send half's `due`, which follows the backoff. Otherwise a five-minute backoff would blur episode starts by five minutes.
6. **The incident half is generalised before the correlation half is added** (Task 1). Copying `IncidentExportBuffer` and `IncidentExport` would add about a hundred duplicated lines that Sonar's duplication gate on new code blocks. The incident log lines stay byte-identical. `hub_incident_export.rs` keeps its path.
7. **The episode table is bounded by `max_pending`, not capped like incidents** (Task 6). The spec says `max_pending`. An episode weighs about a kilobyte, unlike an incident and its thousand frozen findings.
8. **`run_exporter` moves its halves into a `Halves` struct** (Task 6). It sits at about 15 cognitive complexity today, and a third half pushes it to about 18.
9. **The dashboard has no `source` variable** (Task 7). The spec names one. The plan adds `hub_source`, listed from the `source` label of the Hub's `perf_sentinel_hub_source_last_import_seconds` (only daemons that push appear, which are the only ones with episodes), with All sending a single space as the other variables do.
10. **The Hub incidents annotation is off by default** (Task 7). `DS_HUB` is optional, and an annotation runs on every dashboard load, collapsed row or not. Enabled by default it would raise an error toast for every user without a Hub, breaking the variable's "nothing else changes" promise. It is a toggle at the top of the dashboard, and the timeline description says to turn it on.
11. **"The compatibility badge's Hub dependency" is the Hub version each History (Hub) panel description names** (Task 7). `docs/RELEASE-PROCEDURE.md` says so: the `Compatibility` stat reads the daemon only, and a History (Hub) change never moves its regex. The new panels name PerfSentinelHub 0.6.0. "Findings history" keeps its 0.3.0.
12. **Hub read contract the dashboard assumes** (Task 7), to be honoured by the Hub plan: `GET /api/correlations` returns a bare JSON array of episodes in the push shape plus `source_id` and `source_name` (as `GET /api/incidents` does), an open episode has `ended_at_ms` null or absent, a filter given as whitespace only reads as absent, and the launcher accepts `#/correlations?pair_key=<key>`.
13. **`docs/RUNBOOK.md` gains one sentence** (Task 8), though the spec's documentation list omits it: its "Cross-trace correlations cannot be recovered after the fact" paragraph becomes false for a daemon that pushes to a 0.6.0 Hub.

---

### Task 1: Share the incident half's table and route half

Pure refactor. The incident table and half become instances of a generic `LatestTable<V>` and `RouteExport<V>` in a new `hub_table.rs`, so Task 5 adds the correlation half without duplicating them.

**Files:**
- Create: `crates/sentinel-core/src/daemon/hub_table.rs`
- Modify: `crates/sentinel-core/src/daemon/hub_incident_export.rs` (lines 1-284 replaced, tests kept)
- Modify: `crates/sentinel-core/src/daemon/mod.rs:18` (add `mod hub_table;`)
- Modify: `crates/sentinel-core/src/daemon/query_api/tests.rs:1056` (one field rename)

**Interfaces:**
- Consumes: `hub_export::{ExportOutcome, bounded_body, classify, evict_by_order, next_delay, post_batch, snapshot_by_order}` (already `pub(super)`).
- Produces, in `daemon::hub_table`:
  - `pub(crate) trait Keyed { fn key(&self) -> &str; }`
  - `pub(crate) struct Pending<V> { pub(crate) value: Arc<V>, revision: u64 }` (`Clone` for any `V`)
  - `pub(crate) struct LatestTable<V>` with, for `V: Keyed + Clone`: `new(max_pending: usize) -> Self`, `push(&self, value: &V) -> u64` (evicted count), `snapshot(&self, limit: usize) -> Vec<Pending<V>>`, `acknowledge(&self, sent: &[Pending<V>])`, `mark_unsupported(&self) -> u64`, `is_unsupported(&self) -> bool`, `len(&self) -> usize`
  - `pub(crate) struct Route<V: 'static> { max_batch, max_body, body: fn(&[Pending<V>]) -> serde_json::Result<Vec<u8>>, pending: fn(&MetricsState) -> &Gauge, dropped: fn(&MetricsState) -> &IntCounter, rejected: &'static str, retrying: &'static str, missing: &'static str }`
  - `pub(crate) fn import_route(findings_endpoint: &str, path: &str) -> String`
  - `pub(crate) struct RouteExport<V: 'static>` with `pub(super) due: tokio::time::Instant`, `with_route(route: &'static Route<V>, buffer: Arc<LatestTable<V>>, uri: http_client::Uri) -> Self`, `async fn round(&mut self, client: &http_client::HttpClientWithBody, api_key: &str, metrics: &MetricsState, flush_interval: Duration) -> bool`
- Produces, in `daemon::hub_incident_export` (names unchanged for every caller): `pub(crate) type IncidentExportBuffer = LatestTable<Incident>`, `pub(crate) type PendingIncident = Pending<Incident>`, `pub(crate) type IncidentExport = RouteExport<Incident>`, `IncidentExport::new(buffer, uri)`, `incident_capacity`, `incidents_endpoint`, `MAX_INCIDENT_BATCH`, `MAX_INCIDENT_BODY_BYTES`. The one visible change: `PendingIncident.incident` is now `PendingIncident.value`.

- [ ] **Step 1: Run the exporter tests as the baseline**

Run: `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_ daemon::query_api::tests`
Expected: PASS. Note the count, Step 4 must match it.

- [ ] **Step 2: Create `hub_table.rs`**

```rust
//! What the Hub exporter's incident and correlation halves share: a
//! latest-value table keyed by id, and the half that drains one table to
//! its own import route.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prometheus::{Gauge, IntCounter};

use super::hub_export::{
    ExportOutcome, bounded_body, classify, evict_by_order, next_delay, post_batch,
    snapshot_by_order,
};
use crate::http_client;
use crate::report::metrics::MetricsState;

/// A value its table keys by its own id.
pub(crate) trait Keyed {
    fn key(&self) -> &str;
}

pub(crate) struct Pending<V> {
    // Shared, so a snapshot copies pointers rather than payloads.
    pub(crate) value: Arc<V>,
    revision: u64,
}

impl<V> Clone for Pending<V> {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
            revision: self.revision,
        }
    }
}

struct Queue<V> {
    entries: HashMap<String, Pending<V>>,
    order: VecDeque<(String, u64)>,
    next_revision: u64,
}

/// The latest state of each value waiting for one Hub route, bounded and
/// revisioned, so a late acknowledgment never removes a newer state.
pub(crate) struct LatestTable<V> {
    inner: Mutex<Queue<V>>,
    max_pending: usize,
    // Set once a Hub answered that it has no route for this table. Until
    // restart, queueing would only grow a table nothing can drain.
    unsupported: AtomicBool,
}

impl<V: Keyed + Clone> LatestTable<V> {
    pub(crate) fn new(max_pending: usize) -> Self {
        Self {
            inner: Mutex::new(Queue {
                entries: HashMap::new(),
                order: VecDeque::new(),
                next_revision: 0,
            }),
            max_pending,
            unsupported: AtomicBool::new(false),
        }
    }

    /// Queue the value's current state, replacing an older one still
    /// pending. Every change is a new revision, so the acknowledgment of a
    /// batch already in flight cannot remove it. Returns how many values
    /// were evicted to stay within `max_pending`.
    pub(crate) fn push(&self, value: &V) -> u64 {
        let mut state = self.lock();
        // Read under the lock `mark_unsupported` sets it under, so a push
        // racing a 404 cannot land after the table is cleared.
        if self.is_unsupported() {
            return 0;
        }
        state.next_revision = state.next_revision.wrapping_add(1);
        let revision = state.next_revision;
        let key = value.key().to_string();
        state.entries.insert(
            key.clone(),
            Pending {
                value: Arc::new(value.clone()),
                revision,
            },
        );
        state.order.push_back((key, revision));
        let Queue { entries, order, .. } = &mut *state;
        evict_by_order(entries, order, self.max_pending, |entry| entry.revision)
    }

    /// Up to `limit` pending values, least recently changed first.
    pub(crate) fn snapshot(&self, limit: usize) -> Vec<Pending<V>> {
        let state = self.lock();
        snapshot_by_order(&state.entries, &state.order, limit, |entry| entry.revision)
    }

    /// Remove what the Hub accepted, unless it changed again meanwhile.
    pub(crate) fn acknowledge(&self, sent: &[Pending<V>]) {
        let mut state = self.lock();
        for item in sent {
            let key = item.value.key();
            if state
                .entries
                .get(key)
                .is_some_and(|entry| entry.revision == item.revision)
            {
                state.entries.remove(key);
            }
        }
    }

    /// The Hub has no route for this table: stop queueing until the daemon
    /// restarts. Returns how many pending values this drops.
    pub(crate) fn mark_unsupported(&self) -> u64 {
        let mut state = self.lock();
        self.unsupported.store(true, Ordering::Relaxed);
        let dropped = state.entries.len() as u64;
        state.entries.clear();
        state.order.clear();
        dropped
    }

    pub(crate) fn is_unsupported(&self) -> bool {
        self.unsupported.load(Ordering::Relaxed)
    }

    pub(crate) fn len(&self) -> usize {
        self.lock().entries.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queue<V>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// What tells one Hub import route from another: its caps, its body, its
/// two metrics and its log lines.
pub(crate) struct Route<V: 'static> {
    /// Values per request.
    pub(crate) max_batch: usize,
    /// The Hub's body cap on this route.
    pub(crate) max_body: usize,
    pub(crate) body: fn(&[Pending<V>]) -> serde_json::Result<Vec<u8>>,
    pub(crate) pending: fn(&MetricsState) -> &Gauge,
    pub(crate) dropped: fn(&MetricsState) -> &IntCounter,
    /// Logged when the Hub refuses a batch for good.
    pub(crate) rejected: &'static str,
    /// Logged on every retry.
    pub(crate) retrying: &'static str,
    /// Logged once, when the Hub turns out to have no such route.
    pub(crate) missing: &'static str,
}

/// The import route under the configured findings import. Config
/// validation guarantees the suffix, so this only swaps it.
pub(crate) fn import_route(findings_endpoint: &str, path: &str) -> String {
    format!(
        "{}{path}",
        findings_endpoint
            .strip_suffix("/api/import/findings")
            .unwrap_or(findings_endpoint)
    )
}

/// One route's half of the exporter task: the table, where it goes, and
/// its own failure count, so one route's backoff never slows another.
pub(crate) struct RouteExport<V: 'static> {
    route: &'static Route<V>,
    buffer: Arc<LatestTable<V>>,
    uri: http_client::Uri,
    failures: u32,
    /// When this half's next round is due, so its backoff holds whatever
    /// pace the other halves keep.
    pub(super) due: tokio::time::Instant,
}

impl<V: Keyed + Clone + 'static> RouteExport<V> {
    pub(crate) fn with_route(
        route: &'static Route<V>,
        buffer: Arc<LatestTable<V>>,
        uri: http_client::Uri,
    ) -> Self {
        Self {
            route,
            buffer,
            uri,
            failures: 0,
            due: tokio::time::Instant::now(),
        }
    }

    /// Send one batch of what is pending, then set the time of the next.
    /// False when nothing was pending, or when the Hub turned out to have
    /// no such route.
    pub(crate) async fn round(
        &mut self,
        client: &http_client::HttpClientWithBody,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
    ) -> bool {
        let mut batch = self.buffer.snapshot(self.route.max_batch);
        if batch.is_empty() {
            self.due = tokio::time::Instant::now() + flush_interval;
            return false;
        }
        let Some(body) = bounded_body(&mut batch, self.route.max_body, self.route.body) else {
            // One value alone over the cap: no batch can ever carry it.
            self.buffer.acknowledge(&batch[..1]);
            (self.route.dropped)(metrics).inc();
            self.publish(metrics);
            self.due = tokio::time::Instant::now();
            return true;
        };
        let outcome = match post_batch(client, &self.uri, api_key, body).await {
            Ok(status) if is_missing_route(status) => {
                self.unsupported(metrics);
                self.due = tokio::time::Instant::now() + flush_interval;
                return false;
            }
            Ok(status) => classify(status),
            Err(reason) => ExportOutcome::Retry(reason),
        };
        let delay = self.apply(&outcome, &batch, metrics, flush_interval);
        self.publish(metrics);
        self.due = tokio::time::Instant::now() + delay;
        true
    }

    fn apply(
        &mut self,
        outcome: &ExportOutcome,
        batch: &[Pending<V>],
        metrics: &MetricsState,
        flush_interval: Duration,
    ) -> Duration {
        match outcome {
            ExportOutcome::Accepted => self.buffer.acknowledge(batch),
            ExportOutcome::Rejected(status) => {
                self.buffer.acknowledge(batch);
                (self.route.dropped)(metrics).inc_by(batch.len() as u64);
                tracing::error!(
                    endpoint = %http_client::redact_endpoint(&self.uri),
                    status = *status,
                    dropped = batch.len(),
                    "{}",
                    self.route.rejected
                );
            }
            ExportOutcome::Busy | ExportOutcome::Retry(_) => {}
        }
        let delay = next_delay(
            outcome,
            &mut self.failures,
            flush_interval,
            self.buffer.len() > 0,
        );
        if let ExportOutcome::Retry(reason) = outcome {
            // A Hub older than the route answers 401, not 404, when its
            // sign-in is on: the retry is logged so that case is visible.
            tracing::warn!(
                endpoint = %http_client::redact_endpoint(&self.uri),
                reason = %reason,
                retry_seconds = delay.as_secs_f64(),
                "{}",
                self.route.retrying
            );
        }
        delay
    }

    fn unsupported(&self, metrics: &MetricsState) {
        let dropped = self.buffer.mark_unsupported();
        (self.route.dropped)(metrics).inc_by(dropped);
        self.publish(metrics);
        tracing::warn!(
            endpoint = %http_client::redact_endpoint(&self.uri),
            dropped,
            "{}",
            self.route.missing
        );
    }

    fn publish(&self, metrics: &MetricsState) {
        (self.route.pending)(metrics).set(self.buffer.len() as f64);
    }
}

/// A Hub that predates the route answers it as any unmapped path.
fn is_missing_route(status: hyper::StatusCode) -> bool {
    matches!(
        status,
        hyper::StatusCode::NOT_FOUND | hyper::StatusCode::METHOD_NOT_ALLOWED
    )
}
```

The struct is `Queue`, not `Entries`: clippy pedantic's `struct_field_names` refuses a field (`entries`) that starts with its struct's name.

- [ ] **Step 3: Reduce `hub_incident_export.rs` to the incident specifics**

Replace everything above `#[cfg(test)]` (lines 1-284) with:

```rust
//! The daemon's incidents on their way to the Hub, beside its
//! findings: a latest-value table per incident id, fed by the incident
//! store's change hooks and drained by the exporter task.

use std::sync::Arc;

use prometheus::{Gauge, IntCounter};
use serde::Serialize;

use super::hub_table::{Keyed, LatestTable, Pending, Route, RouteExport, import_route};
use super::incidents::Incident;
use crate::http_client;
use crate::report::metrics::MetricsState;

/// Incidents per request. One carries up to a thousand frozen findings.
pub(super) const MAX_INCIDENT_BATCH: usize = 10;
/// The Hub's cap on an incident import, the size of one of its poll pages.
pub(super) const MAX_INCIDENT_BODY_BYTES: usize = 4 * 1024 * 1024;

pub(crate) type IncidentExportBuffer = LatestTable<Incident>;
pub(crate) type PendingIncident = Pending<Incident>;
/// The exporter task's incident half.
pub(crate) type IncidentExport = RouteExport<Incident>;

impl Keyed for Incident {
    fn key(&self) -> &str {
        &self.id
    }
}

#[derive(Serialize)]
struct IncidentPayload<'a> {
    producer_version: &'static str,
    incidents: Vec<&'a Incident>,
}

fn incident_body(pending: &[PendingIncident]) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&IncidentPayload {
        producer_version: env!("CARGO_PKG_VERSION"),
        incidents: pending.iter().map(|entry| &*entry.value).collect(),
    })
}

fn incidents_pending(metrics: &MetricsState) -> &Gauge {
    &metrics.hub_export_incidents_pending
}

fn incidents_dropped(metrics: &MetricsState) -> &IntCounter {
    &metrics.hub_export_incidents_dropped_total
}

static INCIDENT_ROUTE: Route<Incident> = Route {
    max_batch: MAX_INCIDENT_BATCH,
    max_body: MAX_INCIDENT_BODY_BYTES,
    body: incident_body,
    pending: incidents_pending,
    dropped: incidents_dropped,
    rejected: "PerfSentinelHub rejected the incident batch permanently; dropping it",
    retrying: "PerfSentinelHub incident export failed; retaining the batch",
    missing: "PerfSentinelHub has no incident import route; incidents are not pushed until restart, findings are",
};

impl RouteExport<Incident> {
    pub(crate) fn new(buffer: Arc<IncidentExportBuffer>, uri: http_client::Uri) -> Self {
        Self::with_route(&INCIDENT_ROUTE, buffer, uri)
    }
}

/// At most this many incidents wait for the Hub. One can carry a thousand
/// frozen findings, so a findings-sized table of 10,000 could pin ten million
/// of them through a Hub outage.
const MAX_PENDING_INCIDENTS: usize = 1_000;

/// The incident table's bound, from the findings one.
pub(super) fn incident_capacity(max_pending: usize) -> usize {
    max_pending.min(MAX_PENDING_INCIDENTS)
}

/// The incident import beside the configured findings import.
pub(super) fn incidents_endpoint(findings_endpoint: &str) -> String {
    import_route(findings_endpoint, "/api/import/incidents")
}
```

In the test module of the same file, the parent no longer imports `Mutex` and `Duration`, so open it with:

```rust
#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
```

and rename the field in three test lines: `batch[0].incident.ended_at_ms` becomes `batch[0].value.ended_at_ms` (twice) and `pending.incident.id.clone()` becomes `pending.value.id.clone()`. In `crates/sentinel-core/src/daemon/query_api/tests.rs:1056`, `batch[0].incident.ended_at_ms` becomes `batch[0].value.ended_at_ms`.

In `crates/sentinel-core/src/daemon/mod.rs`, add `mod hub_table;` after `mod hub_incident_export;`.

- [ ] **Step 4: Run the tests to verify nothing moved**

Run: `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_ daemon::query_api::tests` then `cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings`
Expected: PASS with the Step 1 count, clippy clean.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings && cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_ daemon::query_api::tests && git add crates/sentinel-core/src/daemon/hub_table.rs crates/sentinel-core/src/daemon/hub_incident_export.rs crates/sentinel-core/src/daemon/mod.rs crates/sentinel-core/src/daemon/query_api/tests.rs && git commit -S -m "refactor(daemon): share the Hub exporter's latest-value table and route half"
```

---

### Task 2: Pair and episode identity, and the episode wire type

**Files:**
- Create: `crates/sentinel-core/src/daemon/hub_correlation_export.rs`
- Create: `crates/sentinel-core/src/daemon/hub_correlation_export/tests.rs`
- Modify: `crates/sentinel-core/src/daemon/mod.rs:17` (add `mod hub_correlation_export;` before `mod hub_export;`)

**Interfaces:**
- Consumes: `hub_table::Keyed` (Task 1), `crate::detect::correlate_cross::{CorrelationEndpoint, CrossTraceCorrelation}` (unchanged, `CorrelationEndpoint` derives `Serialize`, `FindingType::as_str` exists).
- Produces, in `daemon::hub_correlation_export`:
  - `pub(super) fn episode_gap_ms(window_ms: u64) -> u64`
  - `#[derive(Clone, Debug, PartialEq, Serialize)] pub(crate) struct Episode` with `pub(crate)` fields `id: String, pair_key: String, source: CorrelationEndpoint, target: CorrelationEndpoint, started_at_ms: u64, last_seen_ms: u64, ended_at_ms: Option<u64>, peak_confidence: f64, peak_co_occurrences: u32, last_confidence: f64, last_co_occurrences: u32, median_lag_ms: f64, sample_trace_id: Option<String>, source_sample_trace_id: Option<String>, gap_ms: u64`, and a private `fn open(correlation: &CrossTraceCorrelation, pair_key: String, started_at_ms: u64, last_seen_ms: u64, gap_ms: u64) -> Self`
  - `impl Keyed for Episode` (key = `id`)
  - `pub(crate) fn pair_key(source: &CorrelationEndpoint, target: &CorrelationEndpoint) -> String`
  - `pub(crate) fn episode_id(pair_key: &str, started_at_ms: u64) -> String`
  - test helpers, `pub(crate)` in `hub_correlation_export::tests`: `fn correlation(first_seen_ms: u64, last_seen_ms: u64, confidence: f64, co_occurrences: u32) -> CrossTraceCorrelation` (pair `order-svc` N+1 SQL to `payment-svc` pool saturation), `fn episode(id: &str) -> Episode`

- [ ] **Step 1: Write the failing tests** in `hub_correlation_export/tests.rs`

```rust
use super::*;
use crate::detect::FindingType;

const T0: u64 = 1_800_000_000_000;
const GAP: u64 = 600_000;

fn endpoint(service: &str, finding_type: FindingType, template: &str) -> CorrelationEndpoint {
    CorrelationEndpoint {
        finding_type,
        service: service.to_string(),
        template: template.to_string(),
        grouping_key: None,
        grouping_value: None,
    }
}

/// One snapshot row of the pair `order-svc` N+1 then `payment-svc` pool
/// saturation.
pub(crate) fn correlation(
    first_seen_ms: u64,
    last_seen_ms: u64,
    confidence: f64,
    co_occurrences: u32,
) -> CrossTraceCorrelation {
    CrossTraceCorrelation {
        source: endpoint(
            "order-svc",
            FindingType::NPlusOneSql,
            "SELECT * FROM orders WHERE id = ?",
        ),
        target: endpoint("payment-svc", FindingType::PoolSaturation, "pool"),
        co_occurrence_count: co_occurrences,
        source_total_occurrences: co_occurrences,
        confidence,
        median_lag_ms: 1_200.0,
        first_seen: crate::time::millis_to_iso8601(first_seen_ms),
        last_seen: crate::time::millis_to_iso8601(last_seen_ms),
        sample_trace_id: Some("target-trace".to_string()),
        source_sample_trace_id: Some("source-trace".to_string()),
    }
}

/// An open episode of that pair, under the given id.
pub(crate) fn episode(id: &str) -> Episode {
    let row = correlation(T0 - 5_000, T0, 0.8, 10);
    let mut episode = Episode::open(
        &row,
        pair_key(&row.source, &row.target),
        T0 - 5_000,
        T0,
        GAP,
    );
    episode.id = id.to_string();
    episode
}

#[test]
fn the_pair_key_and_episode_id_are_pinned() {
    let row = correlation(T0, T0, 0.8, 10);
    let key = pair_key(&row.source, &row.target);
    // Stable across releases and daemons: the Hub groups episodes on it.
    assert_eq!("0d671493b960acaebfa5688cb91fe4a9", key);
    assert_eq!("5d8c7d7f98c7e297ae27b52b61bff20d", episode_id(&key, T0));
}

#[test]
fn the_pair_key_follows_the_direction_of_the_pair() {
    let row = correlation(T0, T0, 0.8, 10);
    assert_ne!(
        pair_key(&row.source, &row.target),
        pair_key(&row.target, &row.source)
    );
}

#[test]
fn two_namespaces_never_share_a_pair_key() {
    let row = correlation(T0, T0, 0.8, 10);
    let (mut source_a, mut target_a) = (row.source.clone(), row.target.clone());
    for side in [&mut source_a, &mut target_a] {
        side.grouping_key = Some("k8s.namespace.name".to_string());
        side.grouping_value = Some("team-a".to_string());
    }
    let (mut source_b, mut target_b) = (source_a.clone(), target_a.clone());
    source_b.grouping_value = Some("team-b".to_string());
    target_b.grouping_value = Some("team-b".to_string());

    assert_ne!(
        pair_key(&source_a, &target_a),
        pair_key(&source_b, &target_b)
    );
}

#[test]
fn each_start_is_its_own_episode() {
    let key = pair_key(
        &correlation(T0, T0, 0.8, 10).source,
        &correlation(T0, T0, 0.8, 10).target,
    );
    let first = episode_id(&key, T0);
    assert_eq!(32, first.len());
    assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(first, episode_id(&key, T0 + 1));
}

#[test]
fn an_episode_serializes_to_the_wire_format() {
    let json = serde_json::to_value(episode("a")).expect("json");

    let mut keys: Vec<&str> = json
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        vec![
            "ended_at_ms",
            "gap_ms",
            "id",
            "last_co_occurrences",
            "last_confidence",
            "last_seen_ms",
            "median_lag_ms",
            "pair_key",
            "peak_co_occurrences",
            "peak_confidence",
            "sample_trace_id",
            "source",
            "source_sample_trace_id",
            "started_at_ms",
            "target",
        ],
        keys
    );
    assert!(json["ended_at_ms"].is_null());
    assert_eq!("n_plus_one_sql", json["source"]["finding_type"]);
    assert_eq!("payment-svc", json["target"]["service"]);
    assert_eq!(GAP, json["gap_ms"]);
}

#[test]
fn the_gap_is_the_window_up_to_thirty_minutes() {
    assert_eq!(600_000, episode_gap_ms(600_000));
    assert_eq!(1_800_000, episode_gap_ms(7 * 24 * 3_600_000));
}
```

In `crates/sentinel-core/src/daemon/mod.rs`, add `mod hub_correlation_export;` before `mod hub_export;`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_correlation_export`
Expected: compile error, file not found for module `hub_correlation_export`.

- [ ] **Step 3: Implement** `hub_correlation_export.rs`

```rust
//! The daemon's cross-trace correlations on their way to the Hub, as
//! episodes: each period during which a pair kept co-occurring. The daemon
//! cuts them, since only it knows whether a pair still fires.
// Wired into the exporter task by a later change.
#![cfg_attr(not(test), allow(dead_code))]

use std::fmt::Write as _;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::hub_table::Keyed;
use crate::detect::correlate_cross::{CorrelationEndpoint, CrossTraceCorrelation};

/// The longest silence an episode's gap can reach, whatever the window.
const MAX_EPISODE_GAP_MS: u64 = 30 * 60 * 1_000;

/// How long a pair's `last_seen` may stand still before its episode
/// closes: the correlator's window, at most 30 minutes.
pub(super) fn episode_gap_ms(window_ms: u64) -> u64 {
    window_ms.min(MAX_EPISODE_GAP_MS)
}

/// One period of continuous activity of a pair, as the Hub stores it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Episode {
    pub(crate) id: String,
    pub(crate) pair_key: String,
    pub(crate) source: CorrelationEndpoint,
    pub(crate) target: CorrelationEndpoint,
    pub(crate) started_at_ms: u64,
    pub(crate) last_seen_ms: u64,
    pub(crate) ended_at_ms: Option<u64>,
    pub(crate) peak_confidence: f64,
    pub(crate) peak_co_occurrences: u32,
    pub(crate) last_confidence: f64,
    pub(crate) last_co_occurrences: u32,
    pub(crate) median_lag_ms: f64,
    pub(crate) sample_trace_id: Option<String>,
    pub(crate) source_sample_trace_id: Option<String>,
    /// Lets the Hub tell an open episode from one whose daemon died
    /// before closing it.
    pub(crate) gap_ms: u64,
}

impl Keyed for Episode {
    fn key(&self) -> &str {
        &self.id
    }
}

impl Episode {
    fn open(
        correlation: &CrossTraceCorrelation,
        pair_key: String,
        started_at_ms: u64,
        last_seen_ms: u64,
        gap_ms: u64,
    ) -> Self {
        Self {
            id: episode_id(&pair_key, started_at_ms),
            pair_key,
            source: correlation.source.clone(),
            target: correlation.target.clone(),
            started_at_ms,
            last_seen_ms,
            ended_at_ms: None,
            peak_confidence: correlation.confidence,
            peak_co_occurrences: correlation.co_occurrence_count,
            last_confidence: correlation.confidence,
            last_co_occurrences: correlation.co_occurrence_count,
            median_lag_ms: correlation.median_lag_ms,
            sample_trace_id: correlation.sample_trace_id.clone(),
            source_sample_trace_id: correlation.source_sample_trace_id.clone(),
            gap_ms,
        }
    }
}

/// 32 hex characters of a SHA-256 over both endpoints, source first. Every
/// field the correlator keys a pair on goes in, `grouping_value` included,
/// so two namespaces never share a pair.
pub(crate) fn pair_key(source: &CorrelationEndpoint, target: &CorrelationEndpoint) -> String {
    let mut hasher = Sha256::new();
    for side in [source, target] {
        for field in [
            Some(side.finding_type.as_str()),
            Some(side.service.as_str()),
            Some(side.template.as_str()),
            side.grouping_key.as_deref(),
            side.grouping_value.as_deref(),
        ] {
            feed(&mut hasher, field);
        }
    }
    hex32(&hasher.finalize())
}

/// 32 hex characters of a SHA-256 over the pair and the episode's start, so
/// every copy of one episode carries the same id.
pub(crate) fn episode_id(pair_key: &str, started_at_ms: u64) -> String {
    let mut hasher = Sha256::new();
    feed(&mut hasher, Some(pair_key));
    hasher.update(started_at_ms.to_be_bytes());
    hex32(&hasher.finalize())
}

/// Length-prefixed, so no two field lists hash the same bytes.
fn feed(hasher: &mut Sha256, field: Option<&str>) {
    match field {
        Some(value) => {
            hasher.update([1_u8]);
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        None => hasher.update([0_u8]),
    }
}

fn hex32(digest: &[u8]) -> String {
    let mut out = String::with_capacity(32);
    for byte in &digest[..16] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
```

The pinned values in `the_pair_key_and_episode_id_are_pinned` were computed from this exact encoding (presence byte, big-endian `u64` length, bytes, for the ten fields in order, then the first 16 digest bytes as lowercase hex). If the test fails, the encoding drifted: fix the code, never the constants.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_correlation_export`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings && cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_correlation_export && git add crates/sentinel-core/src/daemon/hub_correlation_export.rs crates/sentinel-core/src/daemon/hub_correlation_export/tests.rs crates/sentinel-core/src/daemon/mod.rs && git commit -S -m "feat(daemon): identify correlation pairs and episodes for the Hub"
```

---

### Task 3: The episode tracker

**Files:**
- Modify: `crates/sentinel-core/src/daemon/hub_correlation_export.rs`
- Test: `crates/sentinel-core/src/daemon/hub_correlation_export/tests.rs`

**Interfaces:**
- Consumes: `Episode`, `Episode::open`, `pair_key`, `episode_gap_ms` (Task 2), `crate::time::parse_iso8601_utc_to_ms` (exists, `pub(crate)`, parses what `millis_to_iso8601` writes to the millisecond, so `time.rs` does not change).
- Produces: `pub(crate) struct EpisodeTracker` with `pub(crate) fn new(window_ms: u64) -> Self`, `pub(crate) fn observe(&mut self, snapshot: &[CrossTraceCorrelation], now_ms: u64) -> Vec<Episode>` (the copies to queue, opens and updates in snapshot order, then closes in pair order), `pub(crate) fn close_all(&mut self) -> Vec<Episode>`. `now_ms` is the event loop's clock (`event_loop::current_time_ms`), the one the correlator stamps `last_seen` with.

- [ ] **Step 1: Write the failing tests.** In `hub_correlation_export/tests.rs`, add `const WINDOW: u64 = 600_000;` between `T0` and `GAP`, then append:

```rust
#[test]
fn a_new_pair_opens_an_episode_at_the_correlator_first_seen() {
    let mut tracker = EpisodeTracker::new(WINDOW);

    let queued = tracker.observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0);

    assert_eq!(1, queued.len());
    assert_eq!(T0 - 5_000, queued[0].started_at_ms);
    assert_eq!(T0, queued[0].last_seen_ms);
    assert_eq!(None, queued[0].ended_at_ms);
    assert_eq!(episode_id(&queued[0].pair_key, T0 - 5_000), queued[0].id);
}

#[test]
fn an_advance_under_a_minute_is_held_back_and_one_past_it_is_sent() {
    let mut tracker = EpisodeTracker::new(WINDOW);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);

    assert_eq!(
        0,
        tracker
            .observe(&[correlation(T0, T0 + 10_000, 0.8, 10)], T0 + 10_000)
            .len()
    );
    let queued = tracker.observe(&[correlation(T0, T0 + 60_000, 0.8, 10)], T0 + 60_000);

    assert_eq!(1, queued.len());
    assert_eq!(T0 + 60_000, queued[0].last_seen_ms);
}

#[test]
fn a_rising_peak_is_sent_at_once() {
    let mut tracker = EpisodeTracker::new(WINDOW);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);

    let queued = tracker.observe(&[correlation(T0, T0 + 5_000, 0.9, 10)], T0 + 5_000);
    assert_eq!(1, queued.len());
    assert!((queued[0].peak_confidence - 0.9).abs() < f64::EPSILON);

    let queued = tracker.observe(&[correlation(T0, T0 + 6_000, 0.85, 12)], T0 + 6_000);
    assert_eq!(1, queued.len());
    assert_eq!(12, queued[0].peak_co_occurrences);
    assert!((queued[0].peak_confidence - 0.9).abs() < f64::EPSILON);
    assert!((queued[0].last_confidence - 0.85).abs() < f64::EPSILON);
}

#[test]
fn an_episode_closes_once_last_seen_stood_still_for_the_gap() {
    let mut tracker = EpisodeTracker::new(WINDOW);
    let still = correlation(T0, T0, 0.8, 10);
    tracker.observe(std::slice::from_ref(&still), T0);

    assert_eq!(
        0,
        tracker
            .observe(std::slice::from_ref(&still), T0 + GAP - 1)
            .len()
    );
    let closed = tracker.observe(std::slice::from_ref(&still), T0 + GAP);

    assert_eq!(1, closed.len());
    assert_eq!(Some(T0), closed[0].ended_at_ms);
}

#[test]
fn a_pair_gone_from_the_snapshot_closes_after_the_gap() {
    let mut tracker = EpisodeTracker::new(WINDOW);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);

    assert_eq!(0, tracker.observe(&[], T0 + GAP - 1).len());
    let closed = tracker.observe(&[], T0 + GAP);

    assert_eq!(1, closed.len());
    assert_eq!(Some(T0), closed[0].ended_at_ms);
}

#[test]
fn activity_after_the_gap_opens_a_new_episode_at_its_own_start() {
    let mut tracker = EpisodeTracker::new(WINDOW);
    let first = tracker.observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0);
    tracker.observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0 + GAP);

    let back = T0 + GAP + 30_000;
    let second = tracker.observe(&[correlation(T0 - 5_000, back, 0.8, 10)], back);

    assert_eq!(1, second.len());
    assert_eq!(back, second[0].started_at_ms);
    assert_eq!(first[0].pair_key, second[0].pair_key);
    assert_ne!(first[0].id, second[0].id);
}

#[test]
fn a_pair_above_the_thresholds_with_a_still_last_seen_stays_closed() {
    let week = 7 * 24 * 3_600_000;
    let gap = episode_gap_ms(week);
    let mut tracker = EpisodeTracker::new(week);
    let still = correlation(T0, T0, 0.8, 10);
    tracker.observe(std::slice::from_ref(&still), T0);

    let closed = tracker.observe(std::slice::from_ref(&still), T0 + gap);
    assert_eq!(Some(T0), closed[0].ended_at_ms);

    assert_eq!(
        0,
        tracker
            .observe(std::slice::from_ref(&still), T0 + gap + 5_000)
            .len()
    );
    assert_eq!(
        0,
        tracker
            .observe(std::slice::from_ref(&still), T0 + 2 * gap)
            .len()
    );
}

#[test]
fn a_pair_back_in_the_snapshot_does_not_start_before_its_last_episode_ended() {
    let mut tracker = EpisodeTracker::new(WINDOW);
    tracker.observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0);
    tracker.observe(&[], T0 + GAP);

    let back = T0 + GAP + 1_000;
    let reopened = tracker.observe(&[correlation(T0 - 5_000, back, 0.8, 10)], back);

    // The correlator still holds the pair's old first_seen, which predates
    // the episode already closed: the new one starts at its own activity.
    assert_eq!(back, reopened[0].started_at_ms);
}

#[test]
fn a_pair_first_met_after_it_went_quiet_opens_nothing() {
    let mut tracker = EpisodeTracker::new(WINDOW);

    assert_eq!(
        0,
        tracker
            .observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0 + GAP)
            .len()
    );
    assert_eq!(
        0,
        tracker
            .observe(&[correlation(T0 - 5_000, T0, 0.8, 10)], T0 + GAP + 5_000)
            .len()
    );
}

#[test]
fn a_closed_pair_is_forgotten_one_window_after_its_last_co_occurrence() {
    let mut tracker = EpisodeTracker::new(WINDOW);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);
    tracker.observe(&[], T0 + GAP);
    assert_eq!(1, tracker.pairs.len());

    tracker.observe(&[], T0 + WINDOW + 1);

    assert_eq!(0, tracker.pairs.len());
}

#[test]
fn shutdown_closes_open_episodes_at_their_latest_last_seen() {
    let mut tracker = EpisodeTracker::new(WINDOW);
    tracker.observe(&[correlation(T0, T0, 0.8, 10)], T0);
    // Held back by the one-minute throttle, yet the close must carry it.
    tracker.observe(&[correlation(T0, T0 + 10_000, 0.8, 10)], T0 + 10_000);

    let closed = tracker.close_all();

    assert_eq!(1, closed.len());
    assert_eq!(T0 + 10_000, closed[0].last_seen_ms);
    assert_eq!(Some(T0 + 10_000), closed[0].ended_at_ms);
    assert_eq!(0, tracker.close_all().len());
}
```

Write `assert_eq!(0, x.len())`, not `assert!(x.is_empty())`: clippy 1.99's `assert_is_empty` refuses the latter on these values. Floats compare within `f64::EPSILON`, as `correlate_cross/tests.rs` does, since `float_cmp` is pedantic.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_correlation_export`
Expected: compile error, `EpisodeTracker` not found.

- [ ] **Step 3: Implement**, in `hub_correlation_export.rs`. Extend the imports:

```rust
use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::hub_table::Keyed;
use crate::detect::correlate_cross::{CorrelationEndpoint, CrossTraceCorrelation};
use crate::time::parse_iso8601_utc_to_ms;
```

add under `MAX_EPISODE_GAP_MS`:

```rust
/// An open episode is re-sent once its `last_seen` moved this far.
const REQUEUE_AFTER_MS: u64 = 60_000;
```

and after `hex32`, before `#[cfg(test)]`:

```rust
struct OpenEpisode {
    episode: Episode,
    /// `last_seen_ms` of the copy last queued, for the one-minute throttle.
    queued_seen_ms: u64,
}

impl OpenEpisode {
    fn new(episode: Episode) -> Self {
        let queued_seen_ms = episode.last_seen_ms;
        Self {
            episode,
            queued_seen_ms,
        }
    }

    /// Fold a snapshot row in when its `last_seen` advanced. The copy to
    /// queue when a peak rose or `last_seen` moved a minute past the last
    /// copy queued, otherwise nothing: rolling drift is not sent.
    fn update(
        &mut self,
        correlation: &CrossTraceCorrelation,
        last_seen_ms: u64,
    ) -> Option<Episode> {
        let episode = &mut self.episode;
        if last_seen_ms <= episode.last_seen_ms {
            return None;
        }
        let peaked = correlation.confidence > episode.peak_confidence
            || correlation.co_occurrence_count > episode.peak_co_occurrences;
        episode.last_seen_ms = last_seen_ms;
        episode.peak_confidence = episode.peak_confidence.max(correlation.confidence);
        episode.peak_co_occurrences = episode
            .peak_co_occurrences
            .max(correlation.co_occurrence_count);
        episode.last_confidence = correlation.confidence;
        episode.last_co_occurrences = correlation.co_occurrence_count;
        episode.median_lag_ms = correlation.median_lag_ms;
        episode
            .sample_trace_id
            .clone_from(&correlation.sample_trace_id);
        episode
            .source_sample_trace_id
            .clone_from(&correlation.source_sample_trace_id);
        if !peaked && last_seen_ms - self.queued_seen_ms < REQUEUE_AFTER_MS {
            return None;
        }
        self.queued_seen_ms = last_seen_ms;
        Some(self.episode.clone())
    }
}

#[derive(Default)]
struct Track {
    open: Option<OpenEpisode>,
    /// `last_seen_ms` of the latest episode closed for this pair.
    closed_seen_ms: Option<u64>,
}

impl Track {
    fn close(&mut self) -> Option<Episode> {
        let OpenEpisode { mut episode, .. } = self.open.take()?;
        episode.ended_at_ms = Some(episode.last_seen_ms);
        self.closed_seen_ms = Some(episode.last_seen_ms);
        Some(episode)
    }
}

/// Cuts the correlator's pairs into episodes. Owned by the exporter task,
/// fed one `active_correlations()` snapshot per round.
pub(crate) struct EpisodeTracker {
    window_ms: u64,
    gap_ms: u64,
    // Ordered, so the copies of one round queue in the same order every run.
    pairs: BTreeMap<String, Track>,
}

impl EpisodeTracker {
    pub(crate) fn new(window_ms: u64) -> Self {
        Self {
            window_ms,
            gap_ms: episode_gap_ms(window_ms),
            pairs: BTreeMap::new(),
        }
    }

    /// Fold one snapshot in, close what went quiet, and return the copies
    /// to queue.
    pub(crate) fn observe(
        &mut self,
        snapshot: &[CrossTraceCorrelation],
        now_ms: u64,
    ) -> Vec<Episode> {
        let mut queue: Vec<Episode> = snapshot
            .iter()
            .filter_map(|correlation| self.step(correlation, now_ms))
            .collect();
        let gap_ms = self.gap_ms;
        queue.extend(
            self.pairs
                .values_mut()
                .filter(|track| {
                    track.open.as_ref().is_some_and(|open| {
                        now_ms.saturating_sub(open.episode.last_seen_ms) >= gap_ms
                    })
                })
                .filter_map(Track::close),
        );
        let window_ms = self.window_ms;
        // A closed pair is remembered for one window: past it the correlator
        // has dropped the pair too, so its next `first_seen` is fresh.
        self.pairs.retain(|_, track| {
            track.open.is_some()
                || track
                    .closed_seen_ms
                    .is_some_and(|seen| now_ms.saturating_sub(seen) <= window_ms)
        });
        queue
    }

    /// Close every open episode at its `last_seen`, for a graceful shutdown.
    pub(crate) fn close_all(&mut self) -> Vec<Episode> {
        self.pairs.values_mut().filter_map(Track::close).collect()
    }

    fn step(&mut self, correlation: &CrossTraceCorrelation, now_ms: u64) -> Option<Episode> {
        let last_seen_ms = parse_iso8601_utc_to_ms(&correlation.last_seen).ok()?;
        let key = pair_key(&correlation.source, &correlation.target);
        let track = self.pairs.entry(key.clone()).or_default();
        if let Some(open) = track.open.as_mut() {
            return open.update(correlation, last_seen_ms);
        }
        // Active means `last_seen` advancing: a pair quiet for the gap, or
        // not newer than the episode already closed for it, opens nothing.
        let quiet = now_ms.saturating_sub(last_seen_ms) >= self.gap_ms;
        if quiet
            || track
                .closed_seen_ms
                .is_some_and(|closed| last_seen_ms <= closed)
        {
            return None;
        }
        let started_at_ms = match track.closed_seen_ms {
            Some(_) => last_seen_ms,
            None => parse_iso8601_utc_to_ms(&correlation.first_seen).unwrap_or(last_seen_ms),
        };
        let episode = Episode::open(correlation, key, started_at_ms, last_seen_ms, self.gap_ms);
        track.open = Some(OpenEpisode::new(episode.clone()));
        Some(episode)
    }
}
```

`step` may leave an empty `Track` behind when nothing opens: the `retain` at the end of the same `observe` drops it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_correlation_export`
Expected: PASS (17 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings && cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_correlation_export && git add crates/sentinel-core/src/daemon/hub_correlation_export.rs crates/sentinel-core/src/daemon/hub_correlation_export/tests.rs && git commit -S -m "feat(daemon): cut correlation episodes from the correlator's last_seen"
```

---

### Task 4: The two correlation export metrics

**Files:**
- Modify: `crates/sentinel-core/src/report/metrics.rs` (fields after ~175, creation after ~566, registration after ~749, struct literal after ~1186)
- Test: `crates/sentinel-core/src/report/metrics/tests.rs` (append)
- Modify: `docs/METRICS.md` (after line 184) and `docs/FR/METRICS-FR.md` (after line 190)

**Interfaces:**
- Produces: `MetricsState::hub_export_correlations_pending: Gauge` and `MetricsState::hub_export_correlations_dropped_total: IntCounter`.

- [ ] **Step 1: Write the failing test**, appended to `report/metrics/tests.rs`

```rust
#[test]
fn correlation_export_series_are_registered_label_free() {
    let metrics = MetricsState::new();
    metrics.hub_export_correlations_pending.set(3.0);
    metrics.hub_export_correlations_dropped_total.inc_by(2);

    let text = metrics.render();
    assert!(text.contains("\nperf_sentinel_hub_export_correlations_pending 3\n"));
    assert!(text.contains("\nperf_sentinel_hub_export_correlations_dropped_total 2\n"));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p perf-sentinel-core --lib -- report::metrics::tests::correlation_export_series_are_registered_label_free`
Expected: compile error, no field `hub_export_correlations_pending`.

- [ ] **Step 3: Implement**, right after each of the four places that handle `hub_export_incidents_dropped_total`. Fields:

```rust
    /// Distinct correlation episodes waiting for Hub export.
    pub hub_export_correlations_pending: Gauge,
    /// Correlation episodes evicted, oversized, rejected or abandoned for a Hub without the route.
    pub hub_export_correlations_dropped_total: IntCounter,
```

Creation:

```rust
        let hub_export_correlations_pending = Gauge::new(
            "perf_sentinel_hub_export_correlations_pending",
            "Distinct correlation episodes waiting for PerfSentinelHub export",
        )
        .expect("metric creation should not fail");

        let hub_export_correlations_dropped_total = IntCounter::new(
            "perf_sentinel_hub_export_correlations_dropped_total",
            "Correlation episodes the PerfSentinelHub export dropped: evicted, oversized, rejected, or held for a Hub without the route",
        )
        .expect("metric creation should not fail");
```

Registration:

```rust
        registry
            .register(Box::new(hub_export_correlations_pending.clone()))
            .expect("registration should not fail");
        registry
            .register(Box::new(hub_export_correlations_dropped_total.clone()))
            .expect("registration should not fail");
```

Struct literal, after `hub_export_incidents_dropped_total,`:

```rust
            hub_export_correlations_pending,
            hub_export_correlations_dropped_total,
```

- [ ] **Step 4: Document the series.** In `docs/METRICS.md`, after the `perf_sentinel_hub_export_incidents_dropped_total` row, add two rows padded to the table's column widths:

```markdown
| `perf_sentinel_hub_export_correlations_pending`           | gauge     | (none)                                    | Distinct correlation episodes waiting for PerfSentinelHub export, since 0.30.0. A new copy of an episode replaces its pending one rather than increasing this gauge. |
| `perf_sentinel_hub_export_correlations_dropped_total`     | counter   | (none)                                    | Correlation episodes the export dropped, since 0.30.0: evicted at `hub_export.max_pending`, over 1 MiB alone, rejected with an unretryable 4xx, or still pending when the Hub answered 404 or 405 on its correlation route. Alert on `rate(...) > 0`. |
```

In `docs/FR/METRICS-FR.md`, after the `perf_sentinel_hub_export_incidents_dropped_total` row:

```markdown
| `perf_sentinel_hub_export_correlations_pending`           | gauge     | (aucun)                                   | Épisodes de corrélation distincts en attente d'export vers PerfSentinelHub, depuis 0.30.0. Une nouvelle copie d'un épisode remplace celle en attente au lieu d'augmenter cette gauge. |
| `perf_sentinel_hub_export_correlations_dropped_total`     | counter   | (aucun)                                   | Épisodes de corrélation que l'export a abandonnés, depuis 0.30.0 : évincés à `hub_export.max_pending`, de plus de 1 Mio à eux seuls, rejetés par un 4xx non rejouable, ou encore en attente quand le Hub a répondu 404 ou 405 sur sa route des corrélations. Alertez sur `rate(...) > 0`. |
```

- [ ] **Step 5: Run the tests and commit**

Run: `cargo test -p perf-sentinel-core --lib -- report::metrics`
Expected: PASS.

```bash
cargo fmt --all && cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings && cargo test -p perf-sentinel-core --lib -- report::metrics && git add crates/sentinel-core/src/report/metrics.rs crates/sentinel-core/src/report/metrics/tests.rs docs/METRICS.md docs/FR/METRICS-FR.md && git commit -S -m "feat(metrics): count correlation episodes waiting for and dropped by the Hub export"
```

---

### Task 5: The correlation half

**Files:**
- Modify: `crates/sentinel-core/src/daemon/hub_correlation_export.rs`
- Modify: `crates/sentinel-core/src/daemon/hub_incident_export.rs` (test helper `fake_hub_routes` gains a three-route sibling)
- Test: `crates/sentinel-core/src/daemon/hub_correlation_export/tests.rs`

**Interfaces:**
- Consumes: `LatestTable`, `Pending`, `Route`, `RouteExport::with_route`, `RouteExport::round`, `RouteExport::due`, `import_route` (Task 1), `EpisodeTracker` (Task 3), the Task 4 metrics, `super::event_loop::current_time_ms` (`pub(super)` in `daemon`).
- Produces:
  - `pub(crate) type CorrelationExportBuffer = LatestTable<Episode>`
  - `pub(super) const MAX_EPISODE_BATCH: usize = 100`, `pub(super) const MAX_EPISODE_BODY_BYTES: usize = 1024 * 1024`
  - `pub(super) fn correlations_endpoint(findings_endpoint: &str) -> String`
  - `pub(crate) struct CorrelationExport` with `pub(crate) fn new(correlator: Arc<tokio::sync::Mutex<CrossTraceCorrelator>>, window_ms: u64, buffer: Arc<CorrelationExportBuffer>, uri: http_client::Uri) -> Self`, `pub(super) fn due(&self) -> tokio::time::Instant`, `pub(crate) async fn round(&mut self, client: &http_client::HttpClientWithBody, api_key: &str, metrics: &MetricsState, flush_interval: Duration, draining: bool) -> bool` (true when a batch went out)
  - test helpers in `hub_correlation_export::tests`: `pub(crate) fn empty_correlator() -> Arc<tokio::sync::Mutex<CrossTraceCorrelator>>`, `pub(crate) async fn correlator_with_one_pair() -> Arc<tokio::sync::Mutex<CrossTraceCorrelator>>` (`svc-a` then `svc-b`, active now), `pub(crate) async fn co_occur(correlator: &tokio::sync::Mutex<CrossTraceCorrelator>, now_ms: u64)`
  - in `hub_incident_export::tests`: `pub(crate) async fn fake_hub_paths(findings: u16, incidents: u16, correlations: u16) -> (String, Seen)`

- [ ] **Step 1: A fake Hub that tells the three routes apart.** In `hub_incident_export.rs` tests, replace `fake_hub_routes` with a delegate and a three-route version:

```rust
    /// A Hub that answers the findings import with `findings` and every
    /// other path, the incident import included, with `incidents`.
    pub(crate) async fn fake_hub_routes(findings: u16, incidents: u16) -> (String, Seen) {
        fake_hub_paths(findings, incidents, incidents).await
    }

    /// A Hub that answers the findings import with `findings`, the
    /// correlation import with `correlations`, and every other path, the
    /// incident import included, with `incidents`.
    pub(crate) async fn fake_hub_paths(
        findings: u16,
        incidents: u16,
        correlations: u16,
    ) -> (String, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let store = seen.clone();
        let app = axum::Router::new().fallback(
            move |uri: axum::http::Uri, headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let store = store.clone();
                async move {
                    let key = headers
                        .get("x-api-key")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string);
                    let status = match uri.path() {
                        "/api/import/findings" => findings,
                        "/api/import/correlations" => correlations,
                        _ => incidents,
                    };
                    store
                        .lock()
                        .expect("lock")
                        .push((uri.to_string(), key, body.to_vec()));
                    axum::http::StatusCode::from_u16(status).expect("status")
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{address}"), seen)
    }
```

- [ ] **Step 2: Write the failing tests.** Replace the two `use` lines at the top of `hub_correlation_export/tests.rs` with:

```rust
use std::time::Duration;

use super::*;
use crate::daemon::hub_incident_export::tests::fake_hub;
use crate::detect::correlate_cross::CorrelationConfig;
use crate::detect::{FindingType, Severity};
```

and append:

```rust
/// A correlator that reports nothing.
pub(crate) fn empty_correlator() -> Arc<tokio::sync::Mutex<CrossTraceCorrelator>> {
    Arc::new(tokio::sync::Mutex::new(CrossTraceCorrelator::new(
        CorrelationConfig {
            enabled: true,
            ..CorrelationConfig::default()
        },
    )))
}

/// Feed `svc-a` then `svc-b` one second apart, analysed at `now_ms`.
pub(crate) async fn co_occur(correlator: &tokio::sync::Mutex<CrossTraceCorrelator>, now_ms: u64) {
    let finding = |service: &str, at_ms: u64| {
        let mut finding =
            crate::test_helpers::make_finding(FindingType::NPlusOneSql, Severity::Warning);
        finding.service = service.to_string();
        finding.first_timestamp = crate::time::millis_to_iso8601(at_ms);
        finding
    };
    let mut correlator = correlator.lock().await;
    let _ = correlator.ingest(&[finding("svc-a", now_ms)], now_ms);
    let _ = correlator.ingest(&[finding("svc-b", now_ms + 1_000)], now_ms);
}

/// A correlator reporting one pair, `svc-a` then `svc-b`, active now.
pub(crate) async fn correlator_with_one_pair() -> Arc<tokio::sync::Mutex<CrossTraceCorrelator>> {
    let correlator = Arc::new(tokio::sync::Mutex::new(CrossTraceCorrelator::new(
        CorrelationConfig {
            enabled: true,
            min_co_occurrences: 1,
            min_confidence: 0.0,
            ..CorrelationConfig::default()
        },
    )));
    co_occur(&correlator, current_time_ms()).await;
    correlator
}

fn export(
    base: &str,
    correlator: &Arc<tokio::sync::Mutex<CrossTraceCorrelator>>,
    buffer: &Arc<CorrelationExportBuffer>,
) -> CorrelationExport {
    CorrelationExport::new(
        correlator.clone(),
        WINDOW,
        buffer.clone(),
        format!("{base}/api/import/correlations?source_id=lab")
            .parse()
            .expect("uri"),
    )
}

fn open_last_seen(export: &CorrelationExport) -> Option<u64> {
    export
        .tracker
        .pairs
        .values()
        .find_map(|track| track.open.as_ref())
        .map(|open| open.episode.last_seen_ms)
}

#[test]
fn the_correlations_route_sits_beside_the_findings_one() {
    assert_eq!(
        "https://hub.example.com/api/import/correlations",
        correlations_endpoint("https://hub.example.com/api/import/findings")
    );
}

#[test]
fn a_late_acknowledgement_keeps_the_closing_copy() {
    let buffer = CorrelationExportBuffer::new(10);
    let open = episode("a");
    buffer.push(&open);
    let in_flight = buffer.snapshot(10);
    let mut closed = open.clone();
    closed.ended_at_ms = Some(closed.last_seen_ms);
    buffer.push(&closed);

    buffer.acknowledge(&in_flight);

    let batch = buffer.snapshot(10);
    assert_eq!(1, batch.len(), "the close is still to be sent");
    assert_eq!(Some(open.last_seen_ms), batch[0].value.ended_at_ms);
}

#[tokio::test]
async fn a_round_posts_the_open_episode_to_the_correlations_route() {
    let (base, seen) = fake_hub(200).await;
    let correlator = correlator_with_one_pair().await;
    let buffer = Arc::new(CorrelationExportBuffer::new(10));
    let metrics = MetricsState::new();
    let client = http_client::build_client_with_body();

    let sent = export(&base, &correlator, &buffer)
        .round(&client, &"k".repeat(32), &metrics, Duration::from_secs(5), false)
        .await;

    assert!(sent);
    assert_eq!(0, buffer.len());
    let requests = seen.lock().expect("lock");
    let (uri, key, body) = &requests[0];
    assert_eq!("/api/import/correlations?source_id=lab", uri);
    assert_eq!(Some("k".repeat(32)), *key);
    let body: serde_json::Value = serde_json::from_slice(body).expect("json");
    assert_eq!(env!("CARGO_PKG_VERSION"), body["producer_version"]);
    let episode = &body["episodes"][0];
    assert_eq!("svc-a", episode["source"]["service"]);
    assert_eq!("svc-b", episode["target"]["service"]);
    assert_eq!(32, episode["pair_key"].as_str().expect("pair key").len());
    assert!(episode["ended_at_ms"].is_null());
    assert_eq!(GAP, episode["gap_ms"]);
}

#[tokio::test]
async fn an_episode_over_the_cap_alone_is_dropped_and_counted() {
    let (base, seen) = fake_hub(200).await;
    let buffer = Arc::new(CorrelationExportBuffer::new(10));
    let mut big = episode("a");
    big.source.template = "x".repeat(MAX_EPISODE_BODY_BYTES);
    buffer.push(&big);
    let metrics = MetricsState::new();
    let client = http_client::build_client_with_body();

    export(&base, &empty_correlator(), &buffer)
        .round(&client, &"k".repeat(32), &metrics, Duration::from_secs(5), false)
        .await;

    assert_eq!(0, buffer.len());
    assert_eq!(1, metrics.hub_export_correlations_dropped_total.get());
    assert!(seen.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn a_hub_without_the_route_stops_correlation_export_for_good() {
    let (base, seen) = fake_hub(404).await;
    let correlator = correlator_with_one_pair().await;
    let buffer = Arc::new(CorrelationExportBuffer::new(10));
    buffer.push(&episode("a"));
    let metrics = MetricsState::new();
    let client = http_client::build_client_with_body();
    let mut export = export(&base, &correlator, &buffer);

    // The reading queues the live pair beside "a", then the 404 drops both.
    assert!(
        !export
            .round(&client, &"k".repeat(32), &metrics, Duration::ZERO, false)
            .await
    );
    co_occur(&correlator, current_time_ms() + 1_000).await;
    assert!(
        !export
            .round(&client, &"k".repeat(32), &metrics, Duration::ZERO, false)
            .await
    );

    assert!(buffer.is_unsupported());
    assert_eq!(0, buffer.len());
    assert_eq!(2, metrics.hub_export_correlations_dropped_total.get());
    assert!(metrics.hub_export_correlations_pending.get().abs() < f64::EPSILON);
    assert_eq!(1, seen.lock().expect("lock").len(), "one probe, then silence");
}

#[tokio::test]
async fn a_failing_route_keeps_reading_the_correlator_on_its_own_clock() {
    let (base, seen) = fake_hub(500).await;
    let correlator = correlator_with_one_pair().await;
    let buffer = Arc::new(CorrelationExportBuffer::new(10));
    let metrics = MetricsState::new();
    let client = http_client::build_client_with_body();
    let flush = Duration::from_millis(50);
    let mut export = export(&base, &correlator, &buffer);

    assert!(
        export
            .round(&client, &"k".repeat(32), &metrics, flush, false)
            .await
    );
    assert!(
        export.due() < export.send.due,
        "the send backs off, the reading does not"
    );
    let before = open_last_seen(&export).expect("an open episode");

    tokio::time::sleep_until(export.due()).await;
    co_occur(&correlator, current_time_ms() + 1_000).await;
    assert!(
        !export
            .round(&client, &"k".repeat(32), &metrics, flush, false)
            .await
    );

    assert!(open_last_seen(&export).expect("still open") > before);
    assert_eq!(
        1,
        seen.lock().expect("lock").len(),
        "no send before the backoff ends"
    );
}
```

`correlator_with_one_pair` mirrors `setup_correlator_derives_ingest_skew_from_trace_ttl` in `listeners.rs`: two findings of different services one second apart pair with thresholds at 1 and 0.0. It ingests at `current_time_ms()`, the clock the tracker reads, so the pair is recent.

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_correlation_export`
Expected: compile error, `CorrelationExport`, `CorrelationExportBuffer`, `correlations_endpoint` not found.

- [ ] **Step 4: Implement**, in `hub_correlation_export.rs`. Imports become:

```rust
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use prometheus::{Gauge, IntCounter};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::event_loop::current_time_ms;
use super::hub_table::{Keyed, LatestTable, Pending, Route, RouteExport, import_route};
use crate::detect::correlate_cross::{
    CorrelationEndpoint, CrossTraceCorrelation, CrossTraceCorrelator,
};
use crate::http_client;
use crate::report::metrics::MetricsState;
use crate::time::parse_iso8601_utc_to_ms;
```

add under `REQUEUE_AFTER_MS`:

```rust
/// Episodes per request.
pub(super) const MAX_EPISODE_BATCH: usize = 100;
/// The Hub's cap on a correlation import.
pub(super) const MAX_EPISODE_BODY_BYTES: usize = 1024 * 1024;
```

and after `impl EpisodeTracker`, before `#[cfg(test)]`:

```rust
pub(crate) type CorrelationExportBuffer = LatestTable<Episode>;

#[derive(Serialize)]
struct EpisodePayload<'a> {
    producer_version: &'static str,
    episodes: Vec<&'a Episode>,
}

fn episode_body(pending: &[Pending<Episode>]) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&EpisodePayload {
        producer_version: env!("CARGO_PKG_VERSION"),
        episodes: pending.iter().map(|entry| &*entry.value).collect(),
    })
}

fn episodes_pending(metrics: &MetricsState) -> &Gauge {
    &metrics.hub_export_correlations_pending
}

fn episodes_dropped(metrics: &MetricsState) -> &IntCounter {
    &metrics.hub_export_correlations_dropped_total
}

static CORRELATION_ROUTE: Route<Episode> = Route {
    max_batch: MAX_EPISODE_BATCH,
    max_body: MAX_EPISODE_BODY_BYTES,
    body: episode_body,
    pending: episodes_pending,
    dropped: episodes_dropped,
    rejected: "PerfSentinelHub rejected the correlation batch permanently; dropping it",
    retrying: "PerfSentinelHub correlation export failed; retaining the batch",
    missing: "PerfSentinelHub has no correlation import route; correlations are not pushed until restart, findings and incidents are",
};

/// The correlation import beside the configured findings import.
pub(super) fn correlations_endpoint(findings_endpoint: &str) -> String {
    import_route(findings_endpoint, "/api/import/correlations")
}

/// The exporter task's correlation half: it reads the correlator, cuts
/// episodes, and sends them on its own route.
pub(crate) struct CorrelationExport {
    correlator: Arc<tokio::sync::Mutex<CrossTraceCorrelator>>,
    tracker: EpisodeTracker,
    buffer: Arc<CorrelationExportBuffer>,
    send: RouteExport<Episode>,
    /// When the correlator is next read. Its own clock, so a send backoff
    /// does not blur when episodes start and end.
    observe_due: tokio::time::Instant,
    /// Set once shutdown closed every episode, so they are closed once.
    closed: bool,
}

impl CorrelationExport {
    pub(crate) fn new(
        correlator: Arc<tokio::sync::Mutex<CrossTraceCorrelator>>,
        window_ms: u64,
        buffer: Arc<CorrelationExportBuffer>,
        uri: http_client::Uri,
    ) -> Self {
        Self {
            correlator,
            tracker: EpisodeTracker::new(window_ms),
            send: RouteExport::with_route(&CORRELATION_ROUTE, buffer.clone(), uri),
            buffer,
            observe_due: tokio::time::Instant::now(),
            closed: false,
        }
    }

    /// The earlier of the next reading and the next send.
    pub(super) fn due(&self) -> tokio::time::Instant {
        self.observe_due.min(self.send.due)
    }

    /// Read the correlator when due, then send one batch when due. While
    /// draining, both happen every call, and the first one closes every
    /// open episode. True when a batch went out.
    pub(crate) async fn round(
        &mut self,
        client: &http_client::HttpClientWithBody,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
        draining: bool,
    ) -> bool {
        let now = tokio::time::Instant::now();
        if draining || self.observe_due <= now {
            self.observe(metrics, draining).await;
            self.observe_due = now + flush_interval;
        }
        if draining || self.send.due <= now {
            return self
                .send
                .round(client, api_key, metrics, flush_interval)
                .await;
        }
        false
    }

    async fn observe(&mut self, metrics: &MetricsState, draining: bool) {
        if self.closed || self.buffer.is_unsupported() {
            return;
        }
        let snapshot = self.correlator.lock().await.active_correlations();
        let mut episodes = self.tracker.observe(&snapshot, current_time_ms());
        if draining {
            episodes.extend(self.tracker.close_all());
            self.closed = true;
        }
        let evicted: u64 = episodes
            .iter()
            .map(|episode| self.buffer.push(episode))
            .sum();
        metrics
            .hub_export_correlations_dropped_total
            .inc_by(evicted);
        metrics
            .hub_export_correlations_pending
            .set(self.buffer.len() as f64);
    }
}
```

`CorrelationExport` keeps its own `Arc` of the table beside the one inside `send`, so `hub_table` needs no accessor. The `tokio::sync::Mutex` is the one `setup_correlator` builds and `/api/correlations` locks, held only for the snapshot.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_` then run it four more times
Expected: PASS every time (the reading-clock test sleeps 50 ms against a backoff of 800 ms or more, so it is not timing-sensitive).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings && cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_ && git add crates/sentinel-core/src/daemon/hub_correlation_export.rs crates/sentinel-core/src/daemon/hub_correlation_export/tests.rs crates/sentinel-core/src/daemon/hub_incident_export.rs && git commit -S -m "feat(daemon): send correlation episodes to the Hub's correlations route"
```

---

### Task 6: Push correlation episodes beside findings and incidents

**Files:**
- Modify: `crates/sentinel-core/src/daemon/hub_export.rs` (imports, `HubExporter`, `spawn`, `pending`, `shutdown`, `run_exporter`, a new `Halves`, tests)
- Modify: `crates/sentinel-core/src/daemon/hub_correlation_export.rs` (two accessors, drop the dead-code allowance)
- Modify: `crates/sentinel-core/src/daemon/mod.rs:265` and `:286` (build the correlator before the exporter and pass it)

**Interfaces:**
- Consumes: `CorrelationExport`, `CorrelationExportBuffer`, `correlations_endpoint` (Task 5), `setup_correlator` (`listeners.rs:717`, returns `None` when `[daemon.correlation]` is off).
- Produces:
  - `HubExporter::spawn(config: &DaemonHubExportConfig, metrics: Arc<MetricsState>, toml_acks: Arc<AckTomlState>, ack_store: Option<Arc<AckStore>>, push_incidents: bool, correlator: Option<Arc<tokio::sync::Mutex<CrossTraceCorrelator>>>, correlation_window_ms: u64) -> Result<Option<Self>, DaemonError>`
  - `run_exporter(..., incidents: Option<IncidentExport>, correlations: Option<CorrelationExport>)`
  - `CorrelationExport::buffer(&self) -> Arc<CorrelationExportBuffer>`, `CorrelationExport::start_at(&mut self, at: tokio::time::Instant)`

- [ ] **Step 1: Write the failing tests**, in `hub_export.rs` tests, before `fn acks(`

```rust
    fn route(base: &str, path: &str) -> http_client::Uri {
        format!("{base}{path}?source_id=lab").parse().expect("uri")
    }

    /// The exporter task with all three halves against the fake Hub at `base`.
    fn spawn_all(
        base: &str,
        findings: &Arc<HubExportBuffer>,
        incidents: &Arc<crate::daemon::hub_incident_export::IncidentExportBuffer>,
        correlations: &Arc<CorrelationExportBuffer>,
        correlator: Arc<tokio::sync::Mutex<CrossTraceCorrelator>>,
        flush_interval: Duration,
    ) -> (tokio::task::JoinHandle<()>, Arc<tokio::sync::Notify>) {
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let handle = tokio::spawn(run_exporter(
            findings.clone(),
            shutdown.clone(),
            Arc::new(MetricsState::new()),
            route(base, "/api/import/findings"),
            "k".repeat(32),
            100,
            flush_interval,
            acks(None),
            Some(IncidentExport::new(
                incidents.clone(),
                route(base, "/api/import/incidents"),
            )),
            Some(CorrelationExport::new(
                correlator,
                600_000,
                correlations.clone(),
                route(base, "/api/import/correlations"),
            )),
        ));
        (handle, shutdown)
    }

    /// A Hub without the correlation route stops correlation pushes, never
    /// findings or incidents.
    #[tokio::test]
    async fn findings_and_incidents_keep_flowing_after_the_correlation_route_answers_404() {
        use crate::daemon::hub_correlation_export::tests as episodes;
        use crate::daemon::hub_incident_export::{IncidentExportBuffer, tests};
        let (base, seen) = tests::fake_hub_paths(200, 200, 404).await;
        let findings = Arc::new(HubExportBuffer::new(100));
        let incidents = Arc::new(IncidentExportBuffer::new(10));
        let correlations = Arc::new(CorrelationExportBuffer::new(10));
        findings.push_batch(&[finding("first", "trace")], unix_time_ms());
        incidents.push(&tests::incident("a", None));
        correlations.push(&episodes::episode("a"));
        let (handle, _shutdown) = spawn_all(
            &base,
            &findings,
            &incidents,
            &correlations,
            episodes::empty_correlator(),
            Duration::from_millis(50),
        );

        wait_for(|| {
            correlations.is_unsupported()
                && count(&seen, "/api/import/findings") == 1
                && count(&seen, "/api/import/incidents") == 1
        })
        .await;
        findings.push_batch(&[finding("second", "trace")], unix_time_ms());
        incidents.push(&tests::incident("b", None));
        correlations.push(&episodes::episode("b"));
        wait_for(|| {
            count(&seen, "/api/import/findings") == 2 && count(&seen, "/api/import/incidents") == 2
        })
        .await;
        // A few flush intervals more, for a refused episode that leaked.
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        assert_eq!(1, count(&seen, "/api/import/correlations"));
        assert_eq!(2, count(&seen, "/api/import/findings"));
        assert_eq!(2, count(&seen, "/api/import/incidents"));
    }

    /// A restart must not leave the pair's episode open in the Hub.
    #[tokio::test]
    async fn a_drain_closes_the_open_episodes_and_sends_them() {
        use crate::daemon::hub_correlation_export::tests as episodes;
        use crate::daemon::hub_incident_export::{IncidentExportBuffer, tests};
        let (base, seen) = tests::fake_hub(200).await;
        let correlations = Arc::new(CorrelationExportBuffer::new(10));
        let (handle, shutdown) = spawn_all(
            &base,
            &Arc::new(HubExportBuffer::new(10)),
            &Arc::new(IncidentExportBuffer::new(10)),
            &correlations,
            episodes::correlator_with_one_pair().await,
            Duration::from_hours(1),
        );
        tokio::task::yield_now().await;
        shutdown.notify_one();

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("the drain ends once the three tables are empty")
            .expect("exporter task panicked");
        assert_eq!(0, correlations.len());
        let requests = seen.lock().expect("lock");
        let (_, _, body) = requests
            .iter()
            .find(|(uri, _, _)| uri.starts_with("/api/import/correlations"))
            .expect("the closed episode was sent");
        let body: serde_json::Value = serde_json::from_slice(body).expect("json");
        let episode = &body["episodes"][0];
        assert_eq!(episode["last_seen_ms"], episode["ended_at_ms"]);
    }

    /// `[daemon.correlation]` off: no correlator, so no table and no route.
    #[tokio::test]
    async fn the_correlation_table_exists_only_with_a_correlator() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = dir.path().join("hub-key");
        std::fs::write(&key, "k".repeat(32)).expect("key");
        let config = DaemonHubExportConfig {
            enabled: true,
            endpoint: Some("http://127.0.0.1:1/api/import/findings".to_string()),
            source_id: Some("lab".to_string()),
            api_key_file: Some(key.display().to_string()),
            ..DaemonHubExportConfig::default()
        };
        let spawn = |correlator: Option<Arc<tokio::sync::Mutex<CrossTraceCorrelator>>>| {
            HubExporter::spawn(
                &config,
                Arc::new(MetricsState::new()),
                Arc::new(crate::daemon::ack_toml_state::AckTomlState::new(
                    HashMap::new(),
                )),
                None,
                false,
                correlator,
                600_000,
            )
            .expect("spawn")
            .expect("enabled")
        };

        assert!(spawn(None).correlation_buffer.is_none());
        let with = spawn(Some(
            crate::daemon::hub_correlation_export::tests::empty_correlator(),
        ));
        assert!(with.correlation_buffer.is_some());
    }
```

Add `None` as a tenth argument to the four existing `run_exporter(...)` calls in these tests (`shutdown_stops_the_exporter_instead_of_waiting_for_the_next_flush`, `a_drain_against_an_unreachable_hub_still_yields`, `a_drain_with_only_incidents_pending_still_sends_them`, `spawn_with_incidents`).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::hub_export`
Expected: compile errors, `run_exporter` takes 9 arguments, `HubExporter::spawn` takes 5, no field `correlation_buffer`.

- [ ] **Step 3: The two accessors** in `hub_correlation_export.rs`, inside `impl CorrelationExport`, after `new`:

```rust
    pub(super) fn buffer(&self) -> Arc<CorrelationExportBuffer> {
        self.buffer.clone()
    }
```

and after `due`:

```rust
    pub(super) fn start_at(&mut self, at: tokio::time::Instant) {
        self.observe_due = at;
        self.send.due = at;
    }
```

Remove the `// Wired into the exporter task by a later change.` line and `#![cfg_attr(not(test), allow(dead_code))]` at the top of the file.

- [ ] **Step 4: Wire the third half** in `hub_export.rs`. Imports, beside their neighbours:

```rust
use super::hub_correlation_export::{
    CorrelationExport, CorrelationExportBuffer, correlations_endpoint,
};
```

```rust
use crate::detect::correlate_cross::CrossTraceCorrelator;
```

`HubExporter` gains, after `incident_buffer`:

```rust
    correlation_buffer: Option<Arc<CorrelationExportBuffer>>,
```

`spawn` gains two parameters after `push_incidents: bool,`:

```rust
        correlator: Option<Arc<tokio::sync::Mutex<CrossTraceCorrelator>>>,
        correlation_window_ms: u64,
```

after the `let incidents = ... .transpose()?;` statement:

```rust
        let correlations = correlator
            .map(|correlator| {
                let table = Arc::new(CorrelationExportBuffer::new(config.max_pending));
                source_uri(&correlations_endpoint(endpoint), source_id).map(|uri| {
                    CorrelationExport::new(correlator, correlation_window_ms, table, uri)
                })
            })
            .transpose()?;
        let correlation_buffer = correlations.as_ref().map(CorrelationExport::buffer);
```

`correlations` as the last `run_exporter` argument, and `correlation_buffer,` in the `Self { ... }` literal after `incident_buffer,`. `pending()` adds the third table:

```rust
    fn pending(&self) -> usize {
        self.buffer.len()
            + self
                .incident_buffer
                .as_ref()
                .map_or(0, |incidents| incidents.len())
            + self
                .correlation_buffer
                .as_ref()
                .map_or(0, |correlations| correlations.len())
    }
```

The drain timeout warning in `shutdown` reads `"Hub export drain timed out, the findings, incidents and correlation episodes still pending are dropped"`.

Replace `run_exporter` (from its `#[allow(clippy::too_many_arguments)]` line to the line before `/// The exporter task's findings half`) with the version below. It moves the halves into `Halves`, which keeps `run_exporter` under Sonar's 15 (a third `if let` in the loop takes it to about 18):

```rust
#[allow(clippy::too_many_arguments)] // one task, every dependency injected for the tests
async fn run_exporter(
    buffer: Arc<HubExportBuffer>,
    shutdown: Arc<tokio::sync::Notify>,
    metrics: Arc<MetricsState>,
    uri: http_client::Uri,
    api_key: String,
    batch_size: usize,
    flush_interval: Duration,
    acks: AckSources,
    incidents: Option<IncidentExport>,
    correlations: Option<CorrelationExport>,
) {
    let client = http_client::build_client_with_body();
    let start = tokio::time::Instant::now();
    let mut halves = Halves {
        findings: FindingsExport {
            buffer,
            uri,
            batch_size,
            acks,
            failures: 0,
            due: start,
        },
        incidents,
        correlations,
    };
    halves.start_at(start + flush_interval);
    // Once shutdown fires the loop stops waiting for any half's next round
    // and flushes until every table empties. It never returns on a table
    // it cannot send: `HubExporter::shutdown` owns the time budget and
    // aborts, so the retry policy stays the same on both paths.
    let mut draining = false;
    loop {
        // Each half keeps its own pace: one's backoff is not cut short by
        // another's flush, nor stretched by it.
        let wake = halves.wake();
        if draining {
            // Still yield between attempts, otherwise an unreachable Hub
            // turns the drain budget into a spin.
            let yield_at = tokio::time::Instant::now() + Duration::from_millis(250);
            tokio::time::sleep_until(wake.min(yield_at)).await;
        } else {
            tokio::select! {
                () = tokio::time::sleep_until(wake) => {}
                () = shutdown.notified() => draining = true,
            }
        }
        let sent = halves
            .round(&client, &api_key, &metrics, flush_interval, draining)
            .await;
        if draining && !sent {
            return;
        }
    }
}

/// The exporter task's three halves, findings always, incidents and
/// correlations when configured.
struct Halves {
    findings: FindingsExport,
    incidents: Option<IncidentExport>,
    correlations: Option<CorrelationExport>,
}

impl Halves {
    fn start_at(&mut self, at: tokio::time::Instant) {
        self.findings.due = at;
        if let Some(export) = self.incidents.as_mut() {
            export.due = at;
        }
        if let Some(export) = self.correlations.as_mut() {
            export.start_at(at);
        }
    }

    /// The earliest of the halves' next rounds.
    fn wake(&self) -> tokio::time::Instant {
        let mut wake = self.findings.due;
        if let Some(export) = &self.incidents {
            wake = wake.min(export.due);
        }
        if let Some(export) = &self.correlations {
            wake = wake.min(export.due());
        }
        wake
    }

    /// One round of every half that is due, of all of them while draining.
    /// True when one of them sent a batch.
    async fn round(
        &mut self,
        client: &http_client::HttpClientWithBody,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
        draining: bool,
    ) -> bool {
        let now = tokio::time::Instant::now();
        let mut sent = false;
        if draining || self.findings.due <= now {
            sent |= self
                .findings
                .round(client, api_key, metrics, flush_interval)
                .await;
        }
        if let Some(export) = self.incidents.as_mut()
            && (draining || export.due <= now)
        {
            sent |= export
                .round(client, api_key, metrics, flush_interval)
                .await;
        }
        if let Some(export) = self.correlations.as_mut() {
            sent |= export
                .round(client, api_key, metrics, flush_interval, draining)
                .await;
        }
        sent
    }
}
```

The correlation half checks its own two clocks inside `CorrelationExport::round`, so `Halves::round` calls it on every pass.

- [ ] **Step 5: Build the correlator before the exporter** in `daemon/mod.rs`. Replace the `HubExporter::spawn` call at ~265 with:

```rust
    // Built before the exporter, which reads it to push correlation episodes.
    let correlator = setup_correlator(&config);
    let hub_export = hub_export::HubExporter::spawn(
        &config.daemon.hub_export,
        metrics.clone(),
        toml_acks.clone(),
        ack_store.clone(),
        config.daemon.incidents.enabled,
        correlator.clone(),
        config.daemon.correlation.window_ms,
    )?;
```

and delete the later `let correlator = setup_correlator(&config);` line (~286, above the `green_summary_cell` comment). `spawn_listeners` and `run_event_loop` keep receiving the same `correlator`.

- [ ] **Step 6: Run the tests and every gate**

Run, in order:
- `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::` (three times, the exporter tests run real sockets)
- `cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo check --workspace --no-default-features`
- `cargo check -p perf-sentinel-core --no-default-features --all-targets`

Expected: PASS every time, clippy clean, both feature sets build.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all && cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings && cargo clippy --workspace --all-targets -- -D warnings && cargo check --workspace --no-default-features && cargo test -p perf-sentinel-core --features daemon --lib -- daemon:: && git add crates/sentinel-core/src/daemon/hub_export.rs crates/sentinel-core/src/daemon/hub_correlation_export.rs crates/sentinel-core/src/daemon/mod.rs && git commit -S -m "feat(daemon): push correlation episodes beside findings and incidents"
```

Then `graphify update .` (the graph is gitignored, nothing to commit).

---

### Task 7: Correlation episodes in the findings dashboard

**Files:**
- Modify: `examples/grafana-findings-dashboard.json` (through a throwaway script, the file round-trips byte for byte through `json.dumps(indent=2, ensure_ascii=False)`)
- Modify: `scripts/translate-dashboards-fr.py`
- Regenerate: `examples/FR/grafana-findings-dashboard-FR.json`

**Interfaces:**
- Consumes: the Hub read contract of Ruling 12, the dashboard's `DS_HUB`, `DS_PROMETHEUS`, `service` and `hub_url` variables.
- Produces: a `hub_source` variable, a `Correlation episodes` State timeline (id 102) and a `Correlated pairs` table (id 103) in the `History (Hub)` row (id 100), a `Hub incidents` annotation (off by default), dashboard `version` 15.

- [ ] **Step 1: Write the edit script** to `/tmp/correlation-dashboard-edit.py` (not committed)

```python
import io
import json
import sys

path = sys.argv[1]
d = json.loads(io.open(path, encoding="utf-8").read())

HUB = {"type": "yesoreyeram-infinity-datasource", "uid": "${DS_HUB}"}
EPISODES_URL = "/api/correlations?from=${__from}&to=${__to}&service=${service}&source_id=${hub_source}&limit=1000"
PAIR = ('$e.source.service & " " & $e.source.finding_type & " -> " & $e.target.service'
        ' & " " & $e.target.finding_type & " (" & $substring($e.pair_key, 0, 6) & ", " & $e.source_id & ")"')

d["__requires"].append({"type": "panel", "id": "state-timeline", "name": "State timeline", "version": ""})

variables = d["templating"]["list"]
at = next(i for i, v in enumerate(variables) if v["name"] == "environment") + 1
variables.insert(at, {
    "name": "hub_source",
    "label": "Hub source",
    "type": "query",
    "datasource": {"type": "prometheus", "uid": "${DS_PROMETHEUS}"},
    "definition": "label_values(perf_sentinel_hub_source_last_import_seconds, source)",
    "query": {"query": "label_values(perf_sentinel_hub_source_last_import_seconds, source)", "refId": "hub_source"},
    "refresh": 2,
    "sort": 1,
    "includeAll": True,
    "allValue": " ",
    "multi": False,
    "current": {"selected": True, "text": "All", "value": "$__all"},
    "options": [],
    "hide": 0,
    "description": "Daemon of the Correlation episodes and Correlated pairs panels and of the Hub incidents annotation, one of the Hub's source ids, listed from the source label of perf_sentinel_hub_source_last_import_seconds, so Prometheus has to scrape the Hub and only daemons that push appear. All sends a single space, which the Hub reads as every source the datasource's token reaches.",
})

d["annotations"] = {"list": [{
    "name": "Hub incidents",
    "enable": False,
    "hide": False,
    "iconColor": "red",
    "datasource": HUB,
    "target": {
        "refId": "Anno",
        "datasource": HUB,
        "type": "json",
        "source": "url",
        "parser": "backend",
        "format": "table",
        "url": "/api/incidents?service=${service}&source_id=${hub_source}&limit=200",
        "url_options": {"method": "GET"},
        "root_selector": '$count($) > 0 ? $map($, function($i) {{"time": $i.at_ms, "timeEnd": $i.ended_at_ms ? $i.ended_at_ms : $i.at_ms, "title": $i.service & " " & $i.kind, "text": $i.source_id}}) : []',
        "columns": [
            {"selector": "time", "text": "time", "type": "timestamp_epoch"},
            {"selector": "timeEnd", "text": "timeEnd", "type": "timestamp_epoch"},
            {"selector": "title", "text": "title", "type": "string"},
            {"selector": "text", "text": "text", "type": "string"},
        ],
    },
}]}

timeline = {
    "type": "state-timeline",
    "id": 102,
    "title": "Correlation episodes",
    "description": "One line per pair of findings that kept following each other, one segment per episode: a period during which the daemon saw the pair co-occur, cut once the pair stayed quiet for the episode gap (10 minutes on the default window). Read from the Hub's GET /api/correlations, so it reaches past the daemon's rolling window. An open episode runs to now.\n\nNeeds PerfSentinelHub 0.6.0 and a Perf Sentinel 0.30.0 daemon with [daemon.hub_export] and [daemon.correlation] on. An older Hub answers 404 and the panel stays empty. Turn on the Hub incidents annotation to set incidents against the episodes.",
    "datasource": HUB,
    "gridPos": {"h": 10, "w": 24, "x": 0, "y": 85},
    "targets": [{
        "refId": "A",
        "datasource": HUB,
        "type": "json",
        "source": "url",
        "parser": "backend",
        "format": "table",
        "url": EPISODES_URL,
        "url_options": {"method": "GET"},
        "root_selector": '$count($) > 0 ? $reduce($, function($rows, $e) {($p := ' + PAIR + '; $append($rows, [{"time": $e.started_at_ms, "pair": $p, "state": "active"}, {"time": $e.ended_at_ms ? $e.ended_at_ms : $millis(), "pair": $p, "state": "quiet"}]))}, []) : []',
        "columns": [
            {"selector": "time", "text": "time", "type": "timestamp_epoch"},
            {"selector": "pair", "text": "pair", "type": "string"},
            {"selector": "state", "text": "state", "type": "string"},
        ],
    }],
    "fieldConfig": {
        "defaults": {
            "displayName": "${__field.labels.pair}",
            "color": {"mode": "fixed", "fixedColor": "transparent"},
            "mappings": [{"type": "value", "options": {
                "active": {"text": "Active", "color": "orange", "index": 0},
                "quiet": {"text": "Quiet", "color": "transparent", "index": 1},
            }}],
        },
        "overrides": [],
    },
    "options": {
        "showValue": "never",
        "mergeValues": True,
        "alignValue": "left",
        "rowHeight": 0.8,
        "legend": {"showLegend": False, "displayMode": "list", "placement": "bottom"},
        "tooltip": {"mode": "single", "sort": "none"},
    },
    "transformations": [
        {"id": "sortBy", "options": {"fields": {}, "sort": [{"field": "time", "desc": False}]}},
        {"id": "partitionByValues", "options": {"fields": ["pair"], "keepFields": False, "naming": {"asLabels": True}}},
    ],
}

pairs = {
    "type": "table",
    "id": 103,
    "title": "Correlated pairs",
    "description": "One row per pair over the dashboard's time range, grouped from the same episodes: how many episodes, the highest confidence any of them reached, when the latest one started and whether one is still open. Pair opens the Hub's Correlations screen on that pair, once Hub URL is set.\n\nNeeds PerfSentinelHub 0.6.0. The read stops at 1000 episodes, so a wide range on a busy fleet can miss older pairs.",
    "datasource": HUB,
    "gridPos": {"h": 8, "w": 24, "x": 0, "y": 95},
    "targets": [{
        "refId": "A",
        "datasource": HUB,
        "type": "json",
        "source": "url",
        "parser": "backend",
        "format": "table",
        "url": EPISODES_URL,
        "url_options": {"method": "GET"},
        "root_selector": '$count($) > 0 ? $each($ {pair_key: $}, function($v, $k) {($f := $v[0]; {"pair": $f.source.service & " " & $f.source.finding_type & " -> " & $f.target.service & " " & $f.target.finding_type, "episodes": $count($v), "peak_confidence": $max($v.peak_confidence), "last_episode": $max($v.started_at_ms), "status": $count($v[$not($exists(ended_at_ms)) or ended_at_ms = null]) > 0 ? "active" : "closed", "daemon": $f.source_name ? $f.source_name : $f.source_id, "pair_key": $k})}) : []',
        "columns": [
            {"selector": "pair", "text": "Pair", "type": "string"},
            {"selector": "status", "text": "Status", "type": "string"},
            {"selector": "episodes", "text": "Episodes", "type": "number"},
            {"selector": "peak_confidence", "text": "Peak confidence", "type": "number"},
            {"selector": "last_episode", "text": "Last episode", "type": "timestamp_epoch"},
            {"selector": "daemon", "text": "Daemon", "type": "string"},
            {"selector": "pair_key", "text": "pair_key", "type": "string"},
        ],
    }],
    "fieldConfig": {
        "defaults": {"custom": {"align": "auto", "filterable": True}},
        "overrides": [
            {"matcher": {"id": "byName", "options": "Pair"}, "properties": [
                {"id": "custom.width", "value": 520},
                {"id": "links", "value": [{
                    "title": "Open this pair in the Hub",
                    "url": "${hub_url}/#/correlations?pair_key=${__data.fields.pair_key:percentencode}",
                    "targetBlank": True,
                }]},
            ]},
            {"matcher": {"id": "byName", "options": "Status"}, "properties": [
                {"id": "custom.width", "value": 110},
                {"id": "mappings", "value": [{"type": "value", "options": {
                    "active": {"text": "Active", "color": "orange", "index": 0},
                    "closed": {"text": "Closed", "color": "blue", "index": 1},
                }}]},
                {"id": "custom.cellOptions", "value": {"type": "color-text"}},
            ]},
        ],
    },
    "options": {"showHeader": True, "sortBy": [{"displayName": "Last episode", "desc": True}]},
    "transformations": [{"id": "organize", "options": {
        "excludeByName": {"pair_key": True},
        "indexByName": {"Pair": 0, "Status": 1, "Episodes": 2, "Peak confidence": 3, "Last episode": 4, "Daemon": 5, "pair_key": 6},
        "renameByName": {},
    }}],
}

row = next(p for p in d["panels"] if p.get("title") == "History (Hub)")
row["panels"].extend([timeline, pairs])
d["version"] += 1

io.open(path, "w", encoding="utf-8", newline="\n").write(json.dumps(d, indent=2, ensure_ascii=False) + "\n")
```

Design notes the script encodes, so a reviewer can check them:
- The timeline's field names `time`, `pair` and `state` are lowercase and stay English in the French file: `partitionByValues` turns `pair` into a label, and `displayName` reads `${__field.labels.pair}`. A translated column name would rename the label and blank every row name with no error.
- Each episode becomes two rows, `active` at its start and `quiet` at its end (or now), and `Quiet` maps to a transparent color, so a pair's gaps show as empty stretches.
- The JSONata never writes `${`, and its locals (`$e`, `$p`, `$f`, `$v`, `$k`, `$rows`) name no dashboard variable, so Grafana's interpolation leaves them alone. The group-by is written `$ {pair_key: $}` with a space for the same reason.
- The `pair_key` column is hidden and stays English because the Pair link reads it by name, as the Ack links read `Signature`.

- [ ] **Step 2: Run the script and check the result**

Run: `python3 /tmp/correlation-dashboard-edit.py examples/grafana-findings-dashboard.json && python3 -c "import json;d=json.load(open('examples/grafana-findings-dashboard.json'));print(d['version'],[p['title'] for p in d['panels'][-1]['panels']])"`
Expected: `15 ['Findings history', 'Correlation episodes', 'Correlated pairs']`

- [ ] **Step 3: See the generator refuse the untranslated strings**

Run: `python3 scripts/translate-dashboards-fr.py --check`
Expected: FAIL, listing `not covered by the table` for `Hub source`, the three descriptions, `Correlation episodes`, `Correlated pairs`, `Quiet`, `Closed`, `Open this pair in the Hub`, the column headers and `time`, `pair`, `state`, `timeEnd`, `title`, `text`, `pair_key`, `Daemon`.

- [ ] **Step 4: Add the translations** to `scripts/translate-dashboards-fr.py`. After the line `f("text", "Not observed", "Non observé")`:

```python

# --- 0.30.0: correlation episodes from the Hub ---
f("label", "Hub source", "Source Hub")

f("description",
  "Daemon of the Correlation episodes and Correlated pairs panels and of the Hub incidents annotation, one of the Hub's source ids, listed from the source label of perf_sentinel_hub_source_last_import_seconds, so Prometheus has to scrape the Hub and only daemons that push appear. All sends a single space, which the Hub reads as every source the datasource's token reaches.",
  "Daemon des panneaux Épisodes de corrélation et Paires corrélées et de l'annotation Incidents du Hub, parmi les identifiants de source du Hub, listés depuis le label source de perf_sentinel_hub_source_last_import_seconds : Prometheus doit donc scraper le Hub, et seuls les daemons qui poussent y figurent. All envoie une simple espace, que le Hub lit comme toutes les sources que le jeton du datasource atteint.")

f("name", "Hub incidents", "Incidents du Hub")
f("title", "Correlation episodes", "Épisodes de corrélation")

f("description",
  "One line per pair of findings that kept following each other, one segment per episode: a period during which the daemon saw the pair co-occur, cut once the pair stayed quiet for the episode gap (10 minutes on the default window). Read from the Hub's GET /api/correlations, so it reaches past the daemon's rolling window. An open episode runs to now.\n\nNeeds PerfSentinelHub 0.6.0 and a Perf Sentinel 0.30.0 daemon with [daemon.hub_export] and [daemon.correlation] on. An older Hub answers 404 and the panel stays empty. Turn on the Hub incidents annotation to set incidents against the episodes.",
  "Une ligne par paire de findings qui se suivent, un segment par épisode : une période pendant laquelle le daemon a vu la paire se produire ensemble, close une fois la paire restée calme le temps de l'écart d'épisode (10 minutes sur la fenêtre par défaut). Lu depuis GET /api/correlations du Hub, donc au-delà de la fenêtre glissante du daemon. Un épisode ouvert court jusqu'à maintenant.\n\nDemande PerfSentinelHub 0.6.0 et un daemon Perf Sentinel 0.30.0 avec [daemon.hub_export] et [daemon.correlation] actifs. Un Hub plus ancien répond 404 et le panneau reste vide. Activez l'annotation Incidents du Hub pour confronter les incidents aux épisodes.")

f("text", "Quiet", "Calme")
f("title", "Correlated pairs", "Paires corrélées")

f("description",
  "One row per pair over the dashboard's time range, grouped from the same episodes: how many episodes, the highest confidence any of them reached, when the latest one started and whether one is still open. Pair opens the Hub's Correlations screen on that pair, once Hub URL is set.\n\nNeeds PerfSentinelHub 0.6.0. The read stops at 1000 episodes, so a wide range on a busy fleet can miss older pairs.",
  "Une ligne par paire sur la période du dashboard, regroupée depuis les mêmes épisodes : combien d'épisodes, la plus haute confiance atteinte par l'un d'eux, le début du plus récent et s'il en reste un ouvert. Paire ouvre l'écran Corrélations du Hub sur cette paire, une fois URL du Hub renseignée.\n\nDemande PerfSentinelHub 0.6.0. La lecture s'arrête à 1000 épisodes, donc une large période sur un parc chargé peut manquer des paires plus anciennes.")

f("title", "Open this pair in the Hub", "Ouvrir cette paire dans le Hub")
f("text", "Closed", "Terminé")
```

In `COLUMNS`, after `("Target operation", "Opération cible"),`:

```python
    ("Pair", "Paire"),
    ("Episodes", "Épisodes"),
    ("Peak confidence", "Confiance maximale"),
    ("Last episode", "Dernier épisode"),
```

In `KEEP_EN`, after the `"All", "true", "false", "DOWN", "UP", "OK", "REJECTING",` line:

```python
    "Daemon",
    # Field names a transformation, a data link or Grafana's annotation
    # mapping reads by name: translating one breaks it with no error.
    "time", "timeEnd", "title", "text", "pair", "state", "pair_key",
```

`Active` and `Status` already have entries and are reused.

- [ ] **Step 5: Regenerate and check**

Run: `python3 scripts/translate-dashboards-fr.py && python3 scripts/translate-dashboards-fr.py --check`
Expected: `examples/FR/grafana-findings-dashboard-FR.json: 119 strings translated` then `written`, and the `--check` run exits 0 with the same count and no problem line. The French table's organize reads `Paire`, `Statut`, `Épisodes`, `Confiance maximale`, `Dernier épisode`, `Daemon`, `pair_key`, and its link URL still reads `${__data.fields.pair_key:percentencode}`.

- [ ] **Step 6: Check it on the Grafana bench against a fed Hub**

On the local Grafana bench (Grafana in Docker with the Infinity plugin, a `DS_HUB` datasource on a Hub built from its correlation branch, the Hub plan's Task that serves `GET /api/correlations`), post two episodes of one pair and one incident:

```bash
KEY=<the bench source's import key>; HUB=http://127.0.0.1:8080; NOW=$(($(date +%s)*1000))
curl -s -X POST "$HUB/api/import/correlations?source_id=bench" -H "X-API-Key: $KEY" -H 'Content-Type: application/json' -d "{\"producer_version\":\"0.30.0\",\"episodes\":[
 {\"id\":\"$(printf a%.0s {1..32})\",\"pair_key\":\"$(printf b%.0s {1..32})\",\"source\":{\"finding_type\":\"n_plus_one_sql\",\"service\":\"order-svc\",\"template\":\"SELECT 1\"},\"target\":{\"finding_type\":\"pool_saturation\",\"service\":\"payment-svc\",\"template\":\"pool\"},\"started_at_ms\":$((NOW-3600000)),\"last_seen_ms\":$((NOW-3000000)),\"ended_at_ms\":$((NOW-3000000)),\"peak_confidence\":0.9,\"peak_co_occurrences\":12,\"last_confidence\":0.8,\"last_co_occurrences\":10,\"median_lag_ms\":1200.0,\"sample_trace_id\":null,\"source_sample_trace_id\":null,\"gap_ms\":600000},
 {\"id\":\"$(printf c%.0s {1..32})\",\"pair_key\":\"$(printf b%.0s {1..32})\",\"source\":{\"finding_type\":\"n_plus_one_sql\",\"service\":\"order-svc\",\"template\":\"SELECT 1\"},\"target\":{\"finding_type\":\"pool_saturation\",\"service\":\"payment-svc\",\"template\":\"pool\"},\"started_at_ms\":$((NOW-900000)),\"last_seen_ms\":$((NOW-60000)),\"ended_at_ms\":null,\"peak_confidence\":0.95,\"peak_co_occurrences\":20,\"last_confidence\":0.9,\"last_co_occurrences\":18,\"median_lag_ms\":900.0,\"sample_trace_id\":null,\"source_sample_trace_id\":null,\"gap_ms\":600000}]}"
```

Import `examples/grafana-findings-dashboard.json`, open the History (Hub) row on the last 2 hours, and check, in light and dark:
- the timeline shows one row named `order-svc n_plus_one_sql -> payment-svc pool_saturation (bbbbbb, bench)` with two orange segments and an empty stretch between them, the second running to now.
- the table shows that pair with `Active`, 2 episodes, 0.95, the second start, and the source name, and its Pair link opens `<Hub URL>/#/correlations?pair_key=bbbb...`.
- with the `Hub incidents` toggle on, an incident posted to the Hub in that range shows as a red region on the timeline.
- with `DS_HUB` left empty, the dashboard loads with no error toast (the annotation is off).

If the rows read `state` instead of the pair, the bench's Grafana predates `partitionByValues` naming: note the Grafana version in the PR and raise `__requires` for `grafana` to the first version that renders it. Do the same check with `examples/FR/grafana-findings-dashboard-FR.json`.

- [ ] **Step 7: Commit**

```bash
python3 scripts/translate-dashboards-fr.py --check && git add examples/grafana-findings-dashboard.json examples/FR/grafana-findings-dashboard-FR.json scripts/translate-dashboards-fr.py && git commit -S -m "feat(dashboards): show the Hub's correlation episodes in the findings dashboard"
```

---

### Task 8: Configuration docs, runbook, chart comment and changelog

**Files:**
- Modify: `docs/CONFIGURATION.md` (`#### [daemon.correlation] (optional)` at ~701, `#### [daemon.hub_export] (optional)` paragraphs ending ~812) and `docs/FR/CONFIGURATION-FR.md` (~717, ~834)
- Modify: `docs/RUNBOOK.md` (~129) and `docs/FR/RUNBOOK-FR.md` (~130)
- Modify: `charts/perf-sentinel/values.yaml` (the comment above the commented `[daemon.hub_export]` block, ~452)
- Modify: `CHANGELOG.md` (`## [Unreleased]`)

**Interfaces:**
- Consumes: the behaviour and names of Tasks 1-7. No code.

- [ ] **Step 1: `[daemon.hub_export]` in CONFIGURATION.md.** After the paragraph that ends "`perf_sentinel_hub_export_incidents_dropped_total` count what waits and what is dropped." and before "On a graceful shutdown", add:

```markdown
With `[daemon.correlation]` enabled too, the daemon pushes its cross-trace correlations to
`/api/import/correlations` beside `endpoint`, with the same key and source id, from 0.30.0,
as episodes: one per period during which a pair kept co-occurring. An episode opens when a
pair's last co-occurrence (`last_seen`) is recent, follows it while it advances, and closes
once it has stood still for the episode gap, `min(window_minutes, 30 minutes)`, 10 minutes by
default. A copy is sent when an episode opens, when its peak confidence or co-occurrence count
rises, at most once a minute while it stays active, and when it closes, at most 100 episodes
per request and 1 MiB per body. A graceful shutdown closes every open episode before the
drain. The table holds the latest copy per episode, bounded by `max_pending`, and retries with
the same backoff on its own clock. An episode larger than 1 MiB alone is dropped. A Hub that
answers 404 or 405 on that route predates it: the daemon logs one warning and stops pushing
correlations until it restarts, while findings and incidents keep flowing. An older Hub with
sign-in on answers 401 instead, so the daemon keeps retrying and logs each retry: upgrade the
Hub to PerfSentinelHub 0.6.0 first. `perf_sentinel_hub_export_correlations_pending` and
`perf_sentinel_hub_export_correlations_dropped_total` count what waits and what is dropped.
```

French mirror in CONFIGURATION-FR.md, after the paragraph that ends "comptent ce qui attend et ce qui est abandonné." and before "À l'arrêt propre":

```markdown
Avec `[daemon.correlation]` actif aussi, le daemon pousse ses corrélations cross-trace vers
`/api/import/correlations` à côté d'`endpoint`, avec la même clé et le même identifiant de
source, depuis 0.30.0, sous forme d'épisodes : un par période pendant laquelle une paire a
continué de se produire ensemble. Un épisode s'ouvre quand la dernière co-occurrence d'une
paire (`last_seen`) est récente, la suit tant qu'elle avance, et se ferme une fois qu'elle est
restée immobile le temps de l'écart d'épisode, `min(window_minutes, 30 minutes)`, 10 minutes
par défaut. Une copie part quand un épisode s'ouvre, quand sa confiance ou son nombre de
co-occurrences maximal monte, au plus une fois par minute tant qu'il reste actif, et quand il
se ferme, au plus 100 épisodes par requête et 1 Mio par corps. Un arrêt propre ferme chaque
épisode ouvert avant que l'exporteur se vide. La table garde la dernière copie par épisode,
bornée par `max_pending`, et retente avec le même backoff sur sa propre horloge. Un épisode
de plus de 1 Mio à lui seul est abandonné. Un Hub qui répond 404 ou 405 sur cette route la
précède : le daemon journalise un avertissement et cesse de pousser les corrélations jusqu'à
son redémarrage, tandis que les findings et les incidents continuent. Un Hub plus ancien avec
la connexion active répond 401 à la place, donc le daemon retente et journalise chaque
tentative : mettez d'abord le Hub en PerfSentinelHub 0.6.0.
`perf_sentinel_hub_export_correlations_pending` et
`perf_sentinel_hub_export_correlations_dropped_total` comptent ce qui attend et ce qui est
abandonné.
```

- [ ] **Step 2: `[daemon.correlation]` in CONFIGURATION.md.** After "Correlations are exposed via `GET /api/correlations` (when `api_enabled = true`) and snapshotted under `correlations` in `GET /api/export/report`. The daemon's stdout stream never carries them.", add:

```markdown
With `[daemon.hub_export]` on as well, the daemon pushes each pair's active periods to
PerfSentinelHub 0.6.0 or later as episodes, from 0.30.0, so a correlation outlives the rolling
window there. An episode closes once the pair's last co-occurrence has stood still for
`min(window_minutes, 30 minutes)`. `[daemon.hub_export]` below describes what travels.
```

French mirror after "Les corrélations sont exposées via `GET /api/correlations` ... Le flux stdout du daemon ne les porte jamais.":

```markdown
Avec `[daemon.hub_export]` actif aussi, le daemon pousse les périodes actives de chaque paire
vers PerfSentinelHub 0.6.0 ou ultérieur sous forme d'épisodes, depuis 0.30.0, donc une
corrélation y survit à la fenêtre glissante. Un épisode se ferme une fois que la dernière
co-occurrence de la paire est restée immobile pendant `min(window_minutes, 30 minutes)`.
`[daemon.hub_export]` plus bas décrit ce qui part.
```

- [ ] **Step 3: RUNBOOK.md.** At the end of the paragraph that starts "**Cross-trace correlations cannot be recovered after the fact.**" (after "that output is gone for good."), append:

```markdown
The exception, from 0.30.0, is a daemon with `[daemon.hub_export]` on and a Hub on
PerfSentinelHub 0.6.0 or later: the daemon pushes each pair's active periods as episodes, and
the Hub keeps them past the window.
```

French mirror, at the end of the paragraph that starts "**Les corrélations cross-trace ne sont pas récupérables après coup.**":

```markdown
L'exception, depuis 0.30.0, est un daemon avec `[daemon.hub_export]` actif et un Hub en
PerfSentinelHub 0.6.0 ou ultérieur : le daemon pousse les périodes actives de chaque paire
sous forme d'épisodes, et le Hub les garde au-delà de la fenêtre.
```

Both paragraphs are single long lines in the source: append to the same line, separated by one space, rather than as a wrapped block.

- [ ] **Step 4: chart comment.** In `charts/perf-sentinel/values.yaml`, after the two lines that mention `/api/import/incidents (PerfSentinelHub 0.5.0+)`, add:

```yaml
    # With [daemon.correlation] on as well, it pushes correlation episodes to
    # /api/import/correlations (PerfSentinelHub 0.6.0+) from perf-sentinel 0.30.0.
```

A comment edit changes the rendered ConfigMap checksum, so `bash scripts/check-chart-version-bumped.sh main` fails from this commit until the chart is bumped. Leave it failing: the 0.30.0 release commit bumps the chart in lockstep, as 0.29.0 did, and is a separate step.

- [ ] **Step 5: CHANGELOG.md**, under `## [Unreleased]`:

```markdown
### Added

- The Hub export pushes cross-trace correlations too, as episodes. With
  `[daemon.hub_export]` and `[daemon.correlation]` both enabled, the daemon cuts each
  pair's activity into episodes, open while the pair's `last_seen` advances and closed once
  it has stood still for `min(window_minutes, 30 minutes)`, and sends them to the Hub's
  `/api/import/correlations` when they open, peak, advance by a minute and close, so a
  correlation outlives the rolling window. Batches hold at most 100 episodes and 1 MiB. A
  graceful shutdown closes the open episodes before the drain. A Hub without the route (404
  or 405) stops correlation pushes until restart, and findings and incidents keep flowing. An
  older Hub with sign-in on answers 401, which is retried and logged, so upgrade the Hub to
  PerfSentinelHub 0.6.0 first. New label-free metrics
  `perf_sentinel_hub_export_correlations_pending` and
  `perf_sentinel_hub_export_correlations_dropped_total`.
- `examples/grafana-findings-dashboard.json` gains a Correlation episodes timeline, a
  Correlated pairs table, a Hub incidents annotation (off by default) and a Hub source
  variable in its History (Hub) row, read from PerfSentinelHub 0.6.0. Its daemon floor does
  not move.
```

- [ ] **Step 6: Verify and commit**

Run: `git diff -U0 -- docs CHANGELOG.md charts | grep '^+[^+]' | grep -nE '—|;'` then `cargo test --workspace`
Expected: the grep prints nothing (no em dash or semicolon in the added prose), the tests PASS.

```bash
! git diff -U0 -- docs CHANGELOG.md charts | grep '^+[^+]' | grep -qE '—|;' && cargo test --workspace && git add docs/CONFIGURATION.md docs/FR/CONFIGURATION-FR.md docs/RUNBOOK.md docs/FR/RUNBOOK-FR.md charts/perf-sentinel/values.yaml CHANGELOG.md && git commit -S -m "docs: describe the correlation push and its episode gap"
```

---

## Self-review

**Spec coverage (Engine, Grafana, engine Documentation and Testing).**
- Pair and episode identity: Task 2 (`pair_key`, `episode_id`, pinned values, Ruling 1).
- Tracker in the exporter, correlator unchanged, reads `active_correlations()`: Tasks 3 and 5.
- Open, update, close, graceful shutdown close, memory bound: Task 3, shutdown wiring Task 6 (`a_drain_closes_the_open_episodes_and_sends_them`), Rulings 3 and 4.
- Pending table keyed by episode id with revisions, the four queue triggers, `max_pending`, eviction counted: Tasks 1, 3, 5, 6 (`a_late_acknowledgement_keeps_the_closing_copy`, throttle and peak tests).
- Third half with own backoff and pace, shared classification, jitter, busy handling and POST helper: Tasks 1, 5, 6.
- Batches of 100 and 1 MiB, oversized episode dropped and counted: Task 5.
- Body and URL, same `?source_id=` and `X-API-Key`: Task 5 (`a_round_posts_the_open_episode_to_the_correlations_route`).
- 404 or 405 stop, one `WARN`, table cleared and counted, findings and incidents still flowing: Tasks 5 and 6.
- Drain of three tables within 10 s: Task 6 (`HubExporter::pending`, `Halves`, drain test).
- Wire format with `gap_ms`: Task 2 (Ruling 2 on absent grouping fields).
- Metrics, label-free: Task 4.
- Implicit activation, no new key: Task 6 (`setup_correlator` reused, spawn test).
- Grafana State timeline, table, annotations, `service` and source variables, Hub dependency 0.6.0, daemon floor unchanged, FR regenerated, bench check: Task 7 (Rulings 9-12).
- Documentation: CONFIGURATION both sections, METRICS, values.yaml, CHANGELOG, FR mirrors in the same commits: Tasks 4 and 8 (RUNBOOK by Ruling 13).
- Testing, engine unit and integration lines: every listed case has a named test in Tasks 2, 3, 5 and 6.
- Not mapped, by design: the Hub section, the lab scenario, the Hub-side docs and the 0.30.0 release bump (separate plans and step).

**Placeholder scan.** No TBD or "similar to". Every code step shows its code. The bench step names its inputs and its pass criteria.

**Type consistency.** `CorrelationExportBuffer = LatestTable<Episode>` throughout. `CorrelationExport::new(correlator, window_ms, buffer, uri)` in Tasks 5 and 6. `round(..., draining: bool) -> bool` on the correlation half, `round(...) -> bool` without it on `RouteExport` and `FindingsExport`. `HubExporter::spawn` takes seven arguments in Task 6's tests and in `daemon/mod.rs`. `Pending<V>.value` everywhere after Task 1, `query_api/tests.rs` included. `fake_hub_paths` is defined in Task 5 and used in Task 6.

**Verification note.** Tasks 1 to 6 were applied in a scratch copy of this branch before the plan was written: `cargo test -p perf-sentinel-core --features daemon --lib -- daemon::` passed three times in a row, both clippy runs were clean, `cargo check --workspace --no-default-features` and `cargo fmt --all -- --check` passed. Task 7's script and translation entries were run against copies of the dashboards: the generator wrote the French file and `--check` passed. The Grafana bench check (Task 7 Step 6) was not run, it needs the Hub's read route.
