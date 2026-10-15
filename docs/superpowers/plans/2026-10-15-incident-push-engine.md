# Incident push, engine side, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A perf-sentinel daemon with `[daemon.hub_export]` and `[daemon.incidents]` both enabled pushes every incident it records, closes or completes to PerfSentinelHub's `POST /api/import/incidents`.

**Architecture:** The incident store's `*_then` hooks, which already feed the NDJSON archive under the ring's write lock, also hand each changed incident to a latest-value table keyed by incident id (`IncidentExportBuffer`). The existing exporter task gains an incident round beside its findings round: batches of at most 10 incidents and 4 MiB, the same response classification and backoff, and a permanent stop on 404 or 405 (a Hub that predates the route). Two label-free metrics count what waits and what is dropped.

**Tech Stack:** Rust 2024 (MSRV 1.99.0), tokio, hyper via `crate::http_client`, serde_json, prometheus, axum (test fake Hub).

**Spec:** `docs/superpowers/specs/2026-10-15-incident-push-to-hub-design.md`. The Hub side has its own plan, `docs/superpowers/plans/2026-10-15-incident-push-hub.md`, which ships first.

**Repository:** `/Users/robintrassard/RustroverProjects/perf-sentinel`, branch `feat/incident-push` (holds the spec commit). Task 7 runs in `~/IdeaProjects/perf-sentinel-simulation-lab`, committed straight to its `main`.

## Global Constraints

- Activation: incidents are pushed exactly when `config.daemon.hub_export.enabled && config.daemon.incidents.enabled`. No new configuration key.
- URL: the configured `endpoint` with its `/api/import/findings` suffix replaced by `/api/import/incidents`, plus `?source_id=<source_id>`. Same `X-API-Key`.
- Body: `{"producer_version": "<CARGO_PKG_VERSION>", "incidents": [<Incident>...]}`, each incident serialized by its existing `Serialize` impl (the `GET /api/incidents` shape).
- Batch: at most 10 incidents (`MAX_INCIDENT_BATCH`) and 4 MiB (`MAX_INCIDENT_BODY_BYTES = 4 * 1024 * 1024`). One incident alone over 4 MiB is dropped and counted.
- Table: latest value per incident id, a revision per change, bounded by `hub_export.max_pending`, oldest evicted first and counted. No hourly re-send.
- Responses: 2xx acknowledges; 404 or 405 stops incident export until restart (one `WARN`, table cleared and counted); other permanent 4xx drop and count; 503 is busy; anything else retries with the existing `retry_delay` backoff.
- Shutdown: the existing 10 s drain covers both tables.
- Metrics, both label-free, new series only: `perf_sentinel_hub_export_incidents_pending` (gauge) and `perf_sentinel_hub_export_incidents_dropped_total` (counter). `perf_sentinel_hub_export_pending` and `perf_sentinel_hub_export_dropped_total` keep their meaning and labels.
- Freezing, the ring, the archive, the query API and `[daemon.incidents]` defaults do not change.
- `sentinel-core` stays `#![forbid(unsafe_code)]`. Clippy pedantic clean: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings`. `cargo fmt --all` before every commit. Sonar cognitive complexity at most 15 per function.
- English only in code and docs; every `docs/X.md` change ships with `docs/FR/X-FR.md` in the same commit. No em dashes, no semicolons in prose, straight quotes.
- Commits: Conventional Commits, subject line only, signed (`-S`), no Co-Authored-By. The pre-commit hook runs clippy with `--features daemon`.

## Review Focus

- An incident closed while its creation batch is in flight: the late acknowledgement of the first revision must not remove the closed revision, which must still be sent. Pinned in Task 2.
- A Hub that answers 404 on incidents must keep receiving findings, with no retry storm on incidents and exactly one `WARN`. Pinned in Task 4.
- A daemon with `[daemon.hub_export]` on and `[daemon.incidents]` off must create no incident table, send nothing to the incidents route, and keep the incident gauge at 0. Pinned in Task 4.
- An alert repost (same id, `record` returns false) must not queue a duplicate send, since the ring did not change. Pinned in Task 5.
- A drain at shutdown with only incidents pending (no findings) must still send them before the task ends. Pinned in Task 4.

---

### Task 1: Factor the exporter's eviction and body-bounding helpers

Pure refactor so the incident table and payload reuse them instead of duplicating 40 lines (Sonar CPD).

**Files:**
- Modify: `crates/sentinel-core/src/daemon/hub_export.rs` (`evict_overflow` at ~351, `bounded_payload` at ~575)

**Interfaces:**
- Produces, in `hub_export.rs`:
  - `pub(super) fn evict_by_order<V>(entries: &mut HashMap<String, V>, order: &mut VecDeque<(String, u64)>, max: usize, revision: impl Fn(&V) -> u64) -> u64`
  - `pub(super) fn bounded_body<T>(batch: &mut Vec<T>, cap: usize, serialize: impl Fn(&[T]) -> serde_json::Result<Vec<u8>>) -> Option<Vec<u8>>`
  - `pub(super) fn retry_delay`, `pub(super) fn jitter_seed`, `pub(super) fn classify`, `pub(super) enum ExportOutcome` (visibility raised from private).

- [ ] **Step 1: Run the existing exporter tests as the baseline**

Run: `cargo test -p perf-sentinel-core --features daemon -- daemon::hub_export`
Expected: PASS (13 tests).

- [ ] **Step 2: Extract `evict_by_order`**

Replace the body of `evict_overflow` with a call, and add the helper above it:

```rust
fn evict_overflow(state: &mut PendingState, max_pending: usize) -> u64 {
    evict_by_order(&mut state.entries, &mut state.order, max_pending, |entry| {
        entry.revision
    })
}

/// Drop the least recently queued entries until `entries` fits `max`, then
/// compact `order` once stale revisions make it twice the cap. An `order`
/// item whose revision no longer matches its entry is a superseded queueing
/// and is skipped, not counted. Returns how many entries were dropped.
pub(super) fn evict_by_order<V>(
    entries: &mut HashMap<String, V>,
    order: &mut VecDeque<(String, u64)>,
    max: usize,
    revision: impl Fn(&V) -> u64,
) -> u64 {
    let mut dropped = 0;
    while entries.len() > max {
        let Some((key, queued)) = order.pop_front() else {
            break;
        };
        if entries.get(&key).is_some_and(|entry| revision(entry) == queued) {
            entries.remove(&key);
            dropped += 1;
        }
    }
    if order.len() > max.saturating_mul(2) {
        order.retain(|(key, queued)| {
            entries.get(key).is_some_and(|entry| revision(entry) == *queued)
        });
    }
    dropped
}
```

- [ ] **Step 3: Extract `bounded_body`**

Replace `bounded_payload`'s loop with a call to a generic helper, keeping its doc comment on the helper:

```rust
fn bounded_payload(batch: &mut Vec<FindingResponse>) -> Option<Vec<u8>> {
    bounded_body(batch, MAX_EXPORT_BODY_BYTES, |findings| {
        serde_json::to_vec(&ExportPayload {
            producer_version: env!("CARGO_PKG_VERSION"),
            findings,
        })
    })
}

/// Serialize the largest prefix of `batch` that fits `cap`, popping from the
/// end so the caller can keep its acknowledgment aligned. (Keep the existing
/// paragraph on proportional shrinking here.)
pub(super) fn bounded_body<T>(
    batch: &mut Vec<T>,
    cap: usize,
    serialize: impl Fn(&[T]) -> serde_json::Result<Vec<u8>>,
) -> Option<Vec<u8>> {
    while !batch.is_empty() {
        let over_by = match serialize(batch) {
            Ok(body) if body.len() <= cap => return Some(body),
            Err(_) => batch.len(),
            Ok(body) => body.len(),
        };
        if batch.len() == 1 {
            return None;
        }
        let keep = batch
            .len()
            .saturating_mul(cap)
            .checked_div(over_by)
            .unwrap_or(0)
            .min(batch.len() - 1)
            .max(1);
        batch.truncate(keep);
    }
    None
}
```

Raise to `pub(super)`: `enum ExportOutcome`, `fn classify`, `fn retry_delay`, `fn jitter_seed`.

- [ ] **Step 4: Run the tests to verify nothing moved**

Run: `cargo test -p perf-sentinel-core --features daemon -- daemon::hub_export` and `cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings`
Expected: PASS, clippy clean.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/daemon/hub_export.rs
git commit -S -m "refactor(daemon): share the Hub exporter's eviction and body bounding"
```

---

### Task 2: The pending incidents table

**Files:**
- Create: `crates/sentinel-core/src/daemon/hub_incident_export.rs`
- Modify: `crates/sentinel-core/src/daemon/mod.rs` (add `mod hub_incident_export;` beside `mod hub_export;`)

**Interfaces:**
- Consumes: `evict_by_order` (Task 1), `super::incidents::Incident` (`Clone`, `Serialize`, `Deserialize`, field `id: String`).
- Produces:
  - `pub(crate) struct IncidentExportBuffer` with `pub(crate) fn new(max_pending: usize) -> Self`, `pub(crate) fn push(&self, incident: &Incident) -> u64` (returns how many were evicted), `pub(crate) fn len(&self) -> usize`, `pub(super) fn snapshot(&self, limit: usize) -> Vec<PendingIncident>`, `pub(super) fn acknowledge(&self, sent: &[PendingIncident])`, `pub(super) fn mark_unsupported(&self) -> u64` (returns how many pending were cleared), `pub(super) fn is_unsupported(&self) -> bool`.
  - `#[derive(Clone)] pub(super) struct PendingIncident { pub(super) incident: Incident, revision: u64 }`.

- [ ] **Step 1: Write the failing tests** (module skeleton with the tests at the bottom of the new file)

```rust
//! The daemon's incidents on their way to PerfSentinelHub, beside its
//! findings: a latest-value table per incident id, fed by the incident
//! store's change hooks and drained by the exporter task.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::hub_export::evict_by_order;
use super::incidents::Incident;

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn incident(id: &str, ended_at_ms: Option<u64>) -> Incident {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "service": "checkout",
            "kind": "oom_kill",
            "at_ms": 1_800_000_000_000_u64,
            "ended_at_ms": ended_at_ms,
            "window_from_ms": 1_799_999_700_000_u64,
            "window_to_ms": 1_800_000_060_000_u64,
            "findings": []
        }))
        .expect("a valid incident")
    }

    #[test]
    fn a_change_replaces_the_pending_copy_of_the_same_incident() {
        let buffer = IncidentExportBuffer::new(10);
        buffer.push(&incident("a", None));
        buffer.push(&incident("a", Some(1_800_000_045_000)));

        let batch = buffer.snapshot(10);
        assert_eq!(1, batch.len());
        assert_eq!(Some(1_800_000_045_000), batch[0].incident.ended_at_ms);
    }

    #[test]
    fn a_late_acknowledgement_keeps_the_newer_revision() {
        let buffer = IncidentExportBuffer::new(10);
        buffer.push(&incident("a", None));
        let in_flight = buffer.snapshot(10);
        buffer.push(&incident("a", Some(1_800_000_045_000)));

        buffer.acknowledge(&in_flight);

        let batch = buffer.snapshot(10);
        assert_eq!(1, batch.len(), "the close is still to be sent");
        assert_eq!(Some(1_800_000_045_000), batch[0].incident.ended_at_ms);
    }

    #[test]
    fn an_acknowledged_incident_leaves_the_table() {
        let buffer = IncidentExportBuffer::new(10);
        buffer.push(&incident("a", None));
        buffer.acknowledge(&buffer.snapshot(10));
        assert_eq!(0, buffer.len());
    }

    #[test]
    fn the_table_evicts_the_least_recently_changed_incident_past_its_cap() {
        let buffer = IncidentExportBuffer::new(2);
        assert_eq!(0, buffer.push(&incident("a", None)));
        assert_eq!(0, buffer.push(&incident("b", None)));
        buffer.push(&incident("a", Some(1)));
        assert_eq!(1, buffer.push(&incident("c", None)));

        let ids: Vec<String> = buffer.snapshot(10).into_iter().map(|p| p.incident.id).collect();
        assert_eq!(vec!["a".to_string(), "c".to_string()], ids);
    }

    #[test]
    fn snapshot_follows_change_order_and_honours_the_limit() {
        let buffer = IncidentExportBuffer::new(10);
        for id in ["a", "b", "c"] {
            buffer.push(&incident(id, None));
        }
        let ids: Vec<String> = buffer.snapshot(2).into_iter().map(|p| p.incident.id).collect();
        assert_eq!(vec!["a".to_string(), "b".to_string()], ids);
    }

    #[test]
    fn an_unsupported_hub_clears_the_table_and_refuses_later_changes() {
        let buffer = IncidentExportBuffer::new(10);
        buffer.push(&incident("a", None));
        buffer.push(&incident("b", None));

        assert_eq!(2, buffer.mark_unsupported());
        assert!(buffer.is_unsupported());
        assert_eq!(0, buffer.push(&incident("c", None)));
        assert_eq!(0, buffer.len());
    }
}
```

Add `mod hub_incident_export;` in `crates/sentinel-core/src/daemon/mod.rs` next to `mod hub_export;`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p perf-sentinel-core --features daemon -- daemon::hub_incident_export`
Expected: compile error, `IncidentExportBuffer` not found.

- [ ] **Step 3: Implement** (between the `use` lines and the tests)

```rust
#[derive(Clone)]
pub(super) struct PendingIncident {
    pub(super) incident: Incident,
    revision: u64,
}

#[derive(Default)]
struct PendingIncidents {
    entries: HashMap<String, PendingIncident>,
    order: VecDeque<(String, u64)>,
    next_revision: u64,
}

pub(crate) struct IncidentExportBuffer {
    inner: Mutex<PendingIncidents>,
    max_pending: usize,
    // Set once a Hub answered that it has no incident route. Until restart,
    // queueing would only grow a table nothing can drain.
    unsupported: AtomicBool,
}

impl IncidentExportBuffer {
    pub(crate) fn new(max_pending: usize) -> Self {
        Self {
            inner: Mutex::new(PendingIncidents::default()),
            max_pending,
            unsupported: AtomicBool::new(false),
        }
    }

    /// Queue the incident's current state, replacing an older one still
    /// pending. Every change is a new revision, so the acknowledgment of a
    /// batch already in flight cannot remove it. Returns how many incidents
    /// were evicted to stay within `max_pending`.
    pub(crate) fn push(&self, incident: &Incident) -> u64 {
        if self.is_unsupported() {
            return 0;
        }
        let mut state = self.lock();
        state.next_revision = state.next_revision.wrapping_add(1);
        let revision = state.next_revision;
        state.entries.insert(
            incident.id.clone(),
            PendingIncident {
                incident: incident.clone(),
                revision,
            },
        );
        state.order.push_back((incident.id.clone(), revision));
        let PendingIncidents { entries, order, .. } = &mut *state;
        evict_by_order(entries, order, self.max_pending, |entry| entry.revision)
    }

    pub(super) fn snapshot(&self, limit: usize) -> Vec<PendingIncident> {
        let state = self.lock();
        state
            .order
            .iter()
            .filter_map(|(id, revision)| {
                state.entries.get(id).filter(|entry| entry.revision == *revision)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    pub(super) fn acknowledge(&self, sent: &[PendingIncident]) {
        let mut state = self.lock();
        for item in sent {
            if state
                .entries
                .get(&item.incident.id)
                .is_some_and(|entry| entry.revision == item.revision)
            {
                state.entries.remove(&item.incident.id);
            }
        }
    }

    /// The Hub has no incident route: stop queueing until the daemon restarts.
    /// Returns how many pending incidents this drops.
    pub(super) fn mark_unsupported(&self) -> u64 {
        self.unsupported.store(true, Ordering::Relaxed);
        let mut state = self.lock();
        let dropped = state.entries.len() as u64;
        state.entries.clear();
        state.order.clear();
        dropped
    }

    pub(super) fn is_unsupported(&self) -> bool {
        self.unsupported.load(Ordering::Relaxed)
    }

    pub(crate) fn len(&self) -> usize {
        self.lock().entries.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PendingIncidents> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
```

If `Incident` has no `Deserialize` reachable from the test (it derives it, `incidents.rs:92`), keep the `serde_json::from_value` helper. If `ended_at_ms: null` is refused by the derive, build the JSON without the key when `None`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p perf-sentinel-core --features daemon -- daemon::hub_incident_export`
Expected: PASS (6 tests). Clippy may flag dead code until Task 4 uses `snapshot`, `acknowledge` and `mark_unsupported` outside tests: add `#[cfg_attr(not(test), allow(dead_code))]` on those three methods in this task and remove it in Task 4.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/daemon/hub_incident_export.rs crates/sentinel-core/src/daemon/mod.rs
git commit -S -m "feat(daemon): queue incident changes for the Hub in a latest-value table"
```

---

### Task 3: The two incident export metrics

**Files:**
- Modify: `crates/sentinel-core/src/report/metrics.rs` (fields at ~169, creation at ~539, registration at ~723, struct literal at ~1161)
- Modify: `docs/METRICS.md` (~181) and `docs/FR/METRICS-FR.md`

**Interfaces:**
- Produces: `MetricsState::hub_export_incidents_pending: Gauge` and `MetricsState::hub_export_incidents_dropped_total: IntCounter`.

- [ ] **Step 1: Write the failing test** (in the metrics test module, `crates/sentinel-core/src/report/metrics/tests.rs`)

```rust
#[test]
fn incident_export_series_are_registered_label_free() {
    let metrics = MetricsState::new();
    metrics.hub_export_incidents_pending.set(3.0);
    metrics.hub_export_incidents_dropped_total.inc_by(2);

    let text = metrics.render();
    assert!(text.contains("perf_sentinel_hub_export_incidents_pending 3"));
    assert!(text.contains("perf_sentinel_hub_export_incidents_dropped_total 2"));
}
```

Use whatever the neighbouring tests call to render the registry (`render`, `encode`, or `gather` plus `TextEncoder`); match the test that checks `perf_sentinel_hub_export_pending`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p perf-sentinel-core -- report::metrics::tests::incident_export_series_are_registered_label_free`
Expected: compile error, unknown field.

- [ ] **Step 3: Implement**, mirroring `hub_export_pending` and `hub_export_dropped_total` at each of the four places:

```rust
    /// Distinct incidents waiting for Hub export.
    pub hub_export_incidents_pending: Gauge,
    /// Incidents evicted, oversized, rejected or abandoned for a Hub without the route.
    pub hub_export_incidents_dropped_total: IntCounter,
```

```rust
        let hub_export_incidents_pending = Gauge::new(
            "perf_sentinel_hub_export_incidents_pending",
            "Distinct incidents waiting for PerfSentinelHub export",
        )
        .expect("metric creation should not fail");

        let hub_export_incidents_dropped_total = IntCounter::new(
            "perf_sentinel_hub_export_incidents_dropped_total",
            "Incidents the PerfSentinelHub export dropped: evicted, oversized, rejected, or held for a Hub without the route",
        )
        .expect("metric creation should not fail");
```

plus the two `registry.register(Box::new(....clone()))` calls and the two fields in the struct literal.

- [ ] **Step 4: Document the series.** In `docs/METRICS.md`, after the two `perf_sentinel_hub_export_*` rows, add rows in the table's own format:

```markdown
| `perf_sentinel_hub_export_incidents_pending` | gauge | Distinct incidents waiting for PerfSentinelHub export. Since 0.29.0 |
| `perf_sentinel_hub_export_incidents_dropped_total` | counter | Incidents the export dropped: evicted past `max_pending`, over 4 MiB alone, rejected, or held when the Hub has no incident route. Since 0.29.0 |
```

and in `docs/FR/METRICS-FR.md`:

```markdown
| `perf_sentinel_hub_export_incidents_pending` | gauge | Incidents distincts en attente d'export vers PerfSentinelHub. Depuis 0.29.0 |
| `perf_sentinel_hub_export_incidents_dropped_total` | counter | Incidents que l'export a abandonnés : évincés au-delà de `max_pending`, de plus de 4 Mio à eux seuls, refusés, ou en attente quand le Hub n'a pas la route des incidents. Depuis 0.29.0 |
```

- [ ] **Step 5: Run the tests and commit**

Run: `cargo test -p perf-sentinel-core -- report::metrics`
Expected: PASS.

```bash
cargo fmt --all
git add crates/sentinel-core/src/report/metrics.rs crates/sentinel-core/src/report/metrics/tests.rs docs/METRICS.md docs/FR/METRICS-FR.md
git commit -S -m "feat(metrics): count incidents waiting for and dropped by the Hub export"
```

---

### Task 4: The incident round in the exporter

**Files:**
- Modify: `crates/sentinel-core/src/daemon/hub_incident_export.rs` (payload and round)
- Modify: `crates/sentinel-core/src/daemon/hub_export.rs` (`HubExporter`, `spawn`, `shutdown`, `run_exporter`, existing tests' `run_exporter` calls)
- Modify: `crates/sentinel-core/src/daemon/mod.rs:264` (the `HubExporter::spawn` call)

**Interfaces:**
- Consumes: Task 1 helpers, Task 2 buffer, Task 3 metrics.
- Produces:
  - `pub(super) struct IncidentExport` with `pub(super) fn new(buffer: Arc<IncidentExportBuffer>, uri: http_client::Uri) -> Self` and `pub(super) async fn round(&mut self, client: &http_client::HttpClientWithBody, api_key: &str, metrics: &MetricsState) -> Option<Duration>`: `None` when nothing was pending, else the delay before the next round.
  - `pub(super) fn incidents_endpoint(findings_endpoint: &str) -> String`.
  - `HubExporter::spawn(config, metrics, toml_acks, ack_store, push_incidents: bool)` and `pub(super) fn incident_buffer(&self) -> Option<Arc<IncidentExportBuffer>>`.
  - `run_exporter(..., acks: AckSources, incidents: Option<IncidentExport>)`.

- [ ] **Step 1: Write the failing tests** (in `hub_incident_export.rs` tests, with a fake Hub)

```rust
    use std::sync::Arc;
    use std::time::Duration;

    use crate::http_client;
    use crate::report::metrics::MetricsState;

    /// A Hub that records each request and answers `status`.
    async fn fake_hub(status: u16) -> (String, Arc<Mutex<Vec<(String, Option<String>, Vec<u8>)>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let store = seen.clone();
        let app = axum::Router::new().fallback(
            move |uri: axum::http::Uri, headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let store = store.clone();
                async move {
                    let key = headers
                        .get("x-api-key")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    store
                        .lock()
                        .expect("lock")
                        .push((uri.to_string(), key, body.to_vec()));
                    axum::http::StatusCode::from_u16(status).expect("status")
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{address}"), seen)
    }

    fn export(base: &str, buffer: &Arc<IncidentExportBuffer>) -> IncidentExport {
        IncidentExport::new(
            buffer.clone(),
            format!("{base}/api/import/incidents?source_id=lab")
                .parse()
                .expect("uri"),
        )
    }

    #[test]
    fn the_incidents_route_sits_beside_the_findings_one() {
        assert_eq!(
            "https://hub.example.com/api/import/incidents",
            incidents_endpoint("https://hub.example.com/api/import/findings")
        );
    }

    #[tokio::test]
    async fn a_round_posts_the_pending_incidents_and_clears_them() {
        let (base, seen) = fake_hub(200).await;
        let buffer = Arc::new(IncidentExportBuffer::new(10));
        buffer.push(&incident("a", None));
        let metrics = MetricsState::new();
        let client = http_client::build_client_with_body();

        let next = export(&base, &buffer).round(&client, &"k".repeat(32), &metrics).await;

        assert!(next.is_some());
        assert_eq!(0, buffer.len());
        let requests = seen.lock().expect("lock");
        let (uri, key, body) = &requests[0];
        assert_eq!("/api/import/incidents?source_id=lab", uri);
        assert_eq!(Some("k".repeat(32)), *key);
        let body: serde_json::Value = serde_json::from_slice(body).expect("json");
        assert_eq!(env!("CARGO_PKG_VERSION"), body["producer_version"]);
        assert_eq!("a", body["incidents"][0]["id"]);
    }

    #[tokio::test]
    async fn an_empty_table_sends_nothing() {
        let (base, seen) = fake_hub(200).await;
        let buffer = Arc::new(IncidentExportBuffer::new(10));
        let client = http_client::build_client_with_body();

        let next = export(&base, &buffer)
            .round(&client, &"k".repeat(32), &MetricsState::new())
            .await;

        assert!(next.is_none());
        assert!(seen.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_hub_without_the_route_stops_incident_export_for_good() {
        let (base, seen) = fake_hub(404).await;
        let buffer = Arc::new(IncidentExportBuffer::new(10));
        buffer.push(&incident("a", None));
        buffer.push(&incident("b", None));
        let metrics = MetricsState::new();
        let client = http_client::build_client_with_body();
        let mut export = export(&base, &buffer);

        assert!(export.round(&client, &"k".repeat(32), &metrics).await.is_none());
        buffer.push(&incident("c", None));
        assert!(export.round(&client, &"k".repeat(32), &metrics).await.is_none());

        assert!(buffer.is_unsupported());
        assert_eq!(2, metrics.hub_export_incidents_dropped_total.get());
        assert_eq!(1, seen.lock().expect("lock").len(), "one probe, then silence");
    }

    #[tokio::test]
    async fn an_incident_over_the_cap_alone_is_dropped_and_counted() {
        let (base, seen) = fake_hub(200).await;
        let buffer = Arc::new(IncidentExportBuffer::new(10));
        let mut big = incident("a", None);
        big.detail = Some("x".repeat(MAX_INCIDENT_BODY_BYTES));
        buffer.push(&big);
        let metrics = MetricsState::new();
        let client = http_client::build_client_with_body();

        export(&base, &buffer).round(&client, &"k".repeat(32), &metrics).await;

        assert_eq!(0, buffer.len());
        assert_eq!(1, metrics.hub_export_incidents_dropped_total.get());
        assert!(seen.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_busy_hub_keeps_the_batch() {
        let (base, _seen) = fake_hub(503).await;
        let buffer = Arc::new(IncidentExportBuffer::new(10));
        buffer.push(&incident("a", None));
        let client = http_client::build_client_with_body();

        let next = export(&base, &buffer)
            .round(&client, &"k".repeat(32), &MetricsState::new())
            .await;

        assert!(next.is_some());
        assert_eq!(1, buffer.len());
    }
```

In `hub_export.rs` tests, add the drain and "incidents off" cases:

```rust
    #[tokio::test]
    async fn a_drain_with_only_incidents_pending_still_sends_them() {
        let (base, seen) = crate::daemon::hub_incident_export::tests::fake_hub(200).await;
        let incidents = Arc::new(crate::daemon::hub_incident_export::IncidentExportBuffer::new(10));
        incidents.push(&crate::daemon::hub_incident_export::tests::incident("a", None));
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let handle = tokio::spawn(run_exporter(
            Arc::new(HubExportBuffer::new(10)),
            shutdown.clone(),
            Arc::new(MetricsState::new()),
            format!("{base}/api/import/findings?source_id=lab").parse().expect("uri"),
            "k".repeat(32),
            100,
            Duration::from_hours(1),
            acks(None),
            Some(crate::daemon::hub_incident_export::IncidentExport::new(
                incidents.clone(),
                format!("{base}/api/import/incidents?source_id=lab").parse().expect("uri"),
            )),
        ));
        tokio::task::yield_now().await;
        shutdown.notify_one();

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("the drain ends once both tables are empty")
            .expect("exporter task panicked");
        assert_eq!(0, incidents.len());
        assert_eq!(1, seen.lock().expect("lock").len());
    }
```

Make `fake_hub` and `incident` `pub(crate)` inside a `pub(crate) mod tests` (`#[cfg(test)]`) so `hub_export.rs` tests can reuse them.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p perf-sentinel-core --features daemon -- daemon::hub_incident_export daemon::hub_export`
Expected: compile errors for `IncidentExport`, `incidents_endpoint`, the new `run_exporter` parameter.

- [ ] **Step 3: Payload, endpoint and round** in `hub_incident_export.rs`

```rust
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde::Serialize;

use super::hub_export::{ExportOutcome, bounded_body, classify, jitter_seed, retry_delay};
use crate::http_client;
use crate::report::metrics::MetricsState;

/// Incidents per request. One carries up to a thousand frozen findings.
pub(super) const MAX_INCIDENT_BATCH: usize = 10;
/// The Hub's cap on an incident import, the size of one of its poll pages.
pub(super) const MAX_INCIDENT_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize)]
struct IncidentPayload<'a> {
    producer_version: &'static str,
    incidents: Vec<&'a Incident>,
}

/// The incident import beside the configured findings import. Config
/// validation guarantees the suffix, so this only swaps it.
pub(super) fn incidents_endpoint(findings_endpoint: &str) -> String {
    format!(
        "{}/api/import/incidents",
        findings_endpoint
            .strip_suffix("/api/import/findings")
            .unwrap_or(findings_endpoint)
    )
}

pub(super) struct IncidentExport {
    buffer: Arc<IncidentExportBuffer>,
    uri: http_client::Uri,
    failures: u32,
}

impl IncidentExport {
    pub(super) fn new(buffer: Arc<IncidentExportBuffer>, uri: http_client::Uri) -> Self {
        Self { buffer, uri, failures: 0 }
    }

    /// Send one batch of what is pending. `None` when nothing was, otherwise
    /// the delay before the next round, by the findings export's own rules.
    pub(super) async fn round(
        &mut self,
        client: &http_client::HttpClientWithBody,
        api_key: &str,
        metrics: &MetricsState,
    ) -> Option<Duration> {
        let mut batch = self.buffer.snapshot(MAX_INCIDENT_BATCH);
        if batch.is_empty() {
            return None;
        }
        let Some(body) = bounded_body(&mut batch, MAX_INCIDENT_BODY_BYTES, |pending| {
            serde_json::to_vec(&IncidentPayload {
                producer_version: env!("CARGO_PKG_VERSION"),
                incidents: pending.iter().map(|p| &p.incident).collect(),
            })
        }) else {
            // One incident alone over the cap: the poll can still copy it.
            self.buffer.acknowledge(&batch[..1]);
            metrics.hub_export_incidents_dropped_total.inc();
            self.publish(metrics);
            return Some(Duration::ZERO);
        };
        let outcome = match http_client::fetch_with_body(
            client,
            hyper::Method::POST,
            &self.uri,
            "perf-sentinel-hub-export",
            Duration::from_secs(10),
            Some(api_key),
            Bytes::from(body),
        )
        .await
        {
            Ok((status, _)) if is_missing_route(status) => return self.unsupported(metrics),
            Ok((status, _)) => classify(status),
            Err(error) => ExportOutcome::Retry(error.to_string()),
        };
        let next = self.apply(&outcome, &batch, metrics);
        self.publish(metrics);
        Some(next)
    }

    fn apply(
        &mut self,
        outcome: &ExportOutcome,
        batch: &[PendingIncident],
        metrics: &MetricsState,
    ) -> Duration {
        match outcome {
            ExportOutcome::Accepted => {
                self.buffer.acknowledge(batch);
                self.failures = 0;
                Duration::ZERO
            }
            ExportOutcome::Rejected(status) => {
                self.buffer.acknowledge(batch);
                metrics
                    .hub_export_incidents_dropped_total
                    .inc_by(batch.len() as u64);
                self.failures = 0;
                tracing::error!(
                    endpoint = %http_client::redact_endpoint(&self.uri),
                    status = *status,
                    dropped = batch.len(),
                    "PerfSentinelHub rejected the incident batch permanently; dropping it"
                );
                Duration::ZERO
            }
            ExportOutcome::Busy | ExportOutcome::Retry(_) => {
                self.failures = self.failures.saturating_add(1);
                retry_delay(self.failures, jitter_seed())
            }
        }
    }

    fn unsupported(&self, metrics: &MetricsState) -> Option<Duration> {
        let dropped = self.buffer.mark_unsupported();
        metrics.hub_export_incidents_dropped_total.inc_by(dropped);
        self.publish(metrics);
        tracing::warn!(
            endpoint = %http_client::redact_endpoint(&self.uri),
            dropped,
            "PerfSentinelHub has no incident import route; incidents are not pushed until restart, findings are"
        );
        None
    }

    fn publish(&self, metrics: &MetricsState) {
        metrics
            .hub_export_incidents_pending
            .set(self.buffer.len() as f64);
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

Remove the `#[cfg_attr(not(test), allow(dead_code))]` added in Task 2. Accepted batches return `Duration::ZERO`: the exporter loop takes the smaller of both rounds' delays, so an emptied table simply yields `None` on the next round.

- [ ] **Step 4: Wire the round into the exporter task** in `hub_export.rs`

Extract the current findings work of `run_exporter` (from `let mut batch = buffer.snapshot(batch_size);` to the `apply_outcome` call) into a `FindingsExport` struct method so the loop reads:

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
    mut incidents: Option<super::hub_incident_export::IncidentExport>,
) {
    let client = http_client::build_client_with_body();
    let mut findings = FindingsExport { buffer, uri, batch_size, acks, failures: 0 };
    let mut delay = flush_interval;
    let mut draining = false;
    loop {
        if draining {
            tokio::time::sleep(delay.min(Duration::from_millis(250))).await;
        } else {
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = shutdown.notified() => draining = true,
            }
        }
        let findings_next = findings.round(&client, &api_key, &metrics, flush_interval).await;
        let incidents_next = match incidents.as_mut() {
            Some(export) => export.round(&client, &api_key, &metrics).await,
            None => None,
        };
        match findings_next.into_iter().chain(incidents_next).min() {
            Some(next) => delay = next,
            None if draining => return,
            None => delay = flush_interval,
        }
    }
}

struct FindingsExport {
    buffer: Arc<HubExportBuffer>,
    uri: http_client::Uri,
    batch_size: usize,
    acks: AckSources,
    failures: u32,
}

impl FindingsExport {
    /// One findings batch. `None` when nothing was pending.
    async fn round(
        &mut self,
        client: &http_client::HttpClientWithBody,
        api_key: &str,
        metrics: &MetricsState,
        flush_interval: Duration,
    ) -> Option<Duration> {
        let mut batch = self.buffer.snapshot(self.batch_size);
        if batch.is_empty() {
            return None;
        }
        let mut annotated = annotate(&batch, &self.acks).await;
        let Some(body) = bounded_payload(&mut annotated) else {
            self.buffer.acknowledge(&batch[..1], unix_time_ms());
            metrics.hub_export_dropped_total.inc();
            metrics.hub_export_pending.set(self.buffer.len() as f64);
            return Some(Duration::ZERO);
        };
        batch.truncate(annotated.len());
        let outcome = match http_client::fetch_with_body(
            client,
            hyper::Method::POST,
            &self.uri,
            "perf-sentinel-hub-export",
            Duration::from_secs(10),
            Some(api_key),
            Bytes::from(body),
        )
        .await
        {
            Ok((status, _)) => classify(status),
            Err(error) => ExportOutcome::Retry(error.to_string()),
        };
        Some(apply_outcome(
            &outcome,
            &batch,
            &self.buffer,
            metrics,
            &self.uri,
            flush_interval,
            &mut self.failures,
        ))
    }
}
```

Keep the comments that explained `draining`, the yield and the truncate on their new lines. The old "batch empty and draining" exit is now the `None if draining` arm.

`HubExporter` gains `incident_buffer: Option<Arc<IncidentExportBuffer>>`, `spawn` a `push_incidents: bool` parameter:

```rust
        let incident_buffer = push_incidents
            .then(|| Arc::new(super::hub_incident_export::IncidentExportBuffer::new(config.max_pending)));
        let incidents = match &incident_buffer {
            Some(buffer) => {
                let uri = format!(
                    "{}?source_id={}",
                    super::hub_incident_export::incidents_endpoint(
                        config.endpoint.as_deref().expect("config validation")
                    ),
                    config.source_id.as_deref().expect("config validation"),
                )
                .parse::<http_client::Uri>()
                .map_err(|_| super::DaemonError::HubExportEndpoint)?;
                Some(super::hub_incident_export::IncidentExport::new(buffer.clone(), uri))
            }
            None => None,
        };
```

passed as the last `run_exporter` argument, with an accessor:

```rust
    pub(super) fn incident_buffer(&self) -> Option<Arc<super::hub_incident_export::IncidentExportBuffer>> {
        self.incident_buffer.clone()
    }
```

`shutdown` counts both tables in its `pending` figure and its timeout warning (`self.buffer.len() + self.incident_buffer.as_ref().map_or(0, |b| b.len())`).

Add `None` as the last argument to the two existing `run_exporter` calls in the tests (`shutdown_stops_the_exporter...`, `a_drain_against_an_unreachable_hub...`).

In `daemon/mod.rs:264`:

```rust
    let hub_export = hub_export::HubExporter::spawn(
        &config.daemon.hub_export,
        metrics.clone(),
        toml_acks.clone(),
        ack_store.clone(),
        config.daemon.incidents.enabled,
    )?;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p perf-sentinel-core --features daemon -- daemon::hub_incident_export daemon::hub_export` then `cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings`
Expected: PASS, clippy clean. Check `run_exporter`, `FindingsExport::round` and `IncidentExport::round` stay at cognitive complexity 15 or less; split `round` at the `fetch_with_body` call into a `send(&self, ...) -> ExportOutcome` helper if not.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/daemon/hub_export.rs crates/sentinel-core/src/daemon/hub_incident_export.rs crates/sentinel-core/src/daemon/mod.rs
git commit -S -m "feat(daemon): push queued incidents to the Hub beside the findings"
```

---

### Task 5: Feed incident changes to the exporter

**Files:**
- Modify: `crates/sentinel-core/src/daemon/query_api/mod.rs` (`QueryApiState` at ~102, `archive_record` at ~1389, its three callers at ~1466, ~1517, ~1533)
- Modify: `crates/sentinel-core/src/daemon/listeners.rs` (`spawn_listeners` at ~63 and `build_http_router` at ~397: one more parameter each, passed into `QueryApiState`)
- Modify: `crates/sentinel-core/src/daemon/mod.rs` (~294, pass `hub_export.as_ref().and_then(HubExporter::incident_buffer)`)
- Test: `crates/sentinel-core/src/daemon/query_api/tests.rs` (state builders at ~15 and ~2829)

**Interfaces:**
- Consumes: `IncidentExportBuffer::push`, `len` (Task 2), the Task 3 metrics.
- Produces: `QueryApiState::incident_export: Option<Arc<IncidentExportBuffer>>`.

- [ ] **Step 1: Write the failing tests** in `query_api/tests.rs`, next to the existing incident intake tests (reuse their helper that posts an Alertmanager envelope and their state builder; set `incident_export: Some(buffer.clone())` on it)

```rust
#[tokio::test]
async fn a_recorded_incident_is_queued_for_the_hub() {
    let buffer = Arc::new(crate::daemon::hub_incident_export::IncidentExportBuffer::new(10));
    let state = state_with_incident_export(buffer.clone());

    post_alert(&state, firing_alert("checkout", "oom_kill")).await;

    assert_eq!(1, buffer.len());
    assert_eq!(1.0, state.metrics.hub_export_incidents_pending.get());
}

#[tokio::test]
async fn a_reposted_alert_queues_nothing_new() {
    let buffer = Arc::new(crate::daemon::hub_incident_export::IncidentExportBuffer::new(10));
    let state = state_with_incident_export(buffer.clone());
    post_alert(&state, firing_alert("checkout", "oom_kill")).await;
    let first = buffer.snapshot(10);
    buffer.acknowledge(&first);

    post_alert(&state, firing_alert("checkout", "oom_kill")).await;

    assert_eq!(0, buffer.len(), "the ring did not change, so nothing is re-sent");
}

#[tokio::test]
async fn a_resolved_alert_queues_the_closed_incident() {
    let buffer = Arc::new(crate::daemon::hub_incident_export::IncidentExportBuffer::new(10));
    let state = state_with_incident_export(buffer.clone());
    post_alert(&state, firing_alert("checkout", "oom_kill")).await;
    buffer.acknowledge(&buffer.snapshot(10));

    post_alert(&state, resolved_alert("checkout", "oom_kill")).await;

    let batch = buffer.snapshot(10);
    assert_eq!(1, batch.len());
    assert!(batch[0].incident.ended_at_ms.is_some());
}
```

`state_with_incident_export`, `post_alert`, `firing_alert` and `resolved_alert` are thin wrappers over what the existing incident tests already build (the state literal at line ~15 with `incident_export` set, the handler call and the envelope JSON the close test uses). Write them in the test module, reusing the existing envelope builder rather than a second copy of the JSON. `snapshot` and `acknowledge` are `pub(super)` in `daemon`, so they are reachable from `daemon::query_api::tests` only if raised to `pub(crate)`: raise them.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p perf-sentinel-core --features daemon -- daemon::query_api::tests::a_recorded_incident_is_queued_for_the_hub`
Expected: compile error, no field `incident_export`.

- [ ] **Step 3: Implement**

`QueryApiState` gains, after `incident_archive`:

```rust
    /// The Hub exporter's incident table, with `[daemon.hub_export]` and
    /// `[daemon.incidents]` both on. Fed beside the archive, under the same
    /// lock, so the Hub sees one id's changes in the ring's order.
    pub incident_export: Option<Arc<super::hub_incident_export::IncidentExportBuffer>>,
```

Rename `archive_record` to `on_incident_change` and extend it:

```rust
/// Hand one changed record to the archive writer and the Hub exporter, when
/// there are any. Called through the store's `*_then` methods, under the
/// ring's write lock, so both see one id's changes in the order the ring
/// applied them. Neither waits: `try_send` never blocks and the exporter's
/// table is an in-memory map.
fn on_incident_change(state: &QueryApiState, incident: &super::incidents::Incident) {
    if let Some(export) = &state.incident_export {
        let dropped = export.push(incident);
        state.metrics.hub_export_incidents_dropped_total.inc_by(dropped);
        state
            .metrics
            .hub_export_incidents_pending
            .set(export.len() as f64);
    }
    let Some(tx) = &state.incident_archive else {
        return;
    };
    match serde_json::to_vec(incident) {
        Ok(line) => super::incidents::try_send(tx, line, &state.metrics),
        Err(error) => tracing::warn!(%error, "Incident could not be serialized for the archive"),
    }
}
```

Update its three callers (`merge_then`, `record_then`, `close_then`). Add the `incident_export` parameter to `spawn_listeners` and `build_http_router` beside `incident_archive`, set it on `QueryApiState`, pass it from `daemon/mod.rs`, and set `incident_export: None` in the two test state literals (and `self.incident_export.clone()` in the clone at ~2829).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p perf-sentinel-core --features daemon -- daemon` then `cargo check --workspace --no-default-features` and `cargo clippy -p perf-sentinel-core --features daemon --all-targets -- -D warnings`
Expected: PASS, both feature sets build, clippy clean.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sentinel-core/src/daemon/query_api/mod.rs crates/sentinel-core/src/daemon/query_api/tests.rs crates/sentinel-core/src/daemon/listeners.rs crates/sentinel-core/src/daemon/mod.rs crates/sentinel-core/src/daemon/hub_incident_export.rs
git commit -S -m "feat(daemon): hand every incident change to the Hub export"
```

---

### Task 6: Configuration docs, runbook, chart comment and changelog

**Files:**
- Modify: `docs/CONFIGURATION.md` (`#### [daemon.hub_export] (optional)` at ~763) and `docs/FR/CONFIGURATION-FR.md` (~780)
- Modify: `docs/RUNBOOK.md` (incidents part at ~243) and `docs/FR/RUNBOOK-FR.md`
- Modify: `docs/HELM-DEPLOYMENT.md` (`#### History and acks through the Hub (since 0.24.0)` at ~1275) and `docs/FR/HELM-DEPLOYMENT-FR.md`
- Modify: `docs/design/06-INGESTION-AND-DAEMON.md` (end of the daemon section)
- Modify: `charts/perf-sentinel/values.yaml` (the commented `[daemon.hub_export]` block at ~446)
- Modify: `CHANGELOG.md` (`## [Unreleased]`)

- [ ] **Step 1: CONFIGURATION.md**, after the paragraph that starts "The pending structure is a latest-value table", add:

```markdown
With `[daemon.incidents]` enabled too, the daemon pushes its incidents to
`/api/import/incidents` beside `endpoint`, with the same key and source id, from 0.29.0.
An incident is sent when it is recorded, when its alert resolves and when the settle pass
adds frozen findings to it, at most 10 per request and 4 MiB per body. Its table follows
the findings one: the latest state per incident id, bounded by `max_pending`, retried
with the same backoff, drained at shutdown. An incident larger than 4 MiB alone is
dropped, and the Hub's hourly poll can still copy it. A Hub that answers 404 or 405 on
that route predates it: the daemon logs one warning and stops pushing incidents until it
restarts, while findings keep flowing. `perf_sentinel_hub_export_incidents_pending` and
`perf_sentinel_hub_export_incidents_dropped_total` count what waits and what is dropped.
```

French mirror, same place:

```markdown
Avec `[daemon.incidents]` actif aussi, le daemon pousse ses incidents vers
`/api/import/incidents` à côté d'`endpoint`, avec la même clé et le même identifiant de
source, depuis 0.29.0. Un incident part quand il est enregistré, quand son alerte se
résout et quand la passe de consolidation lui ajoute des findings gelés, au plus 10 par
requête et 4 Mio par corps. Sa table suit celle des findings : le dernier état par
identifiant d'incident, bornée par `max_pending`, retentée avec le même backoff, vidée à
l'arrêt. Un incident de plus de 4 Mio à lui seul est abandonné, et le poll horaire du Hub
peut encore le copier. Un Hub qui répond 404 ou 405 sur cette route la précède : le daemon
journalise un avertissement et cesse de pousser les incidents jusqu'à son redémarrage,
tandis que les findings continuent. `perf_sentinel_hub_export_incidents_pending` et
`perf_sentinel_hub_export_incidents_dropped_total` comptent ce qui attend et ce qui est
abandonné.
```

- [ ] **Step 2: RUNBOOK.md**, in the incidents part, add:

```markdown
With the Hub export on, the Hub keeps every incident as it is recorded, so the ring no
longer has to hold a poll interval's worth. A smaller `[daemon.incidents] max_retained`
then saves memory: one incident can carry a thousand frozen findings, so the default 200
can pin 200,000 of them in the worst case. Keep it large enough for `query monitor` and
for the settle pass, which completes an incident about three trace TTLs after it.
```

French mirror:

```markdown
Avec l'export vers le Hub actif, le Hub garde chaque incident dès son enregistrement,
donc l'anneau n'a plus à couvrir l'intervalle d'un poll. Un `[daemon.incidents]
max_retained` plus petit économise alors de la mémoire : un incident peut porter mille
findings gelés, donc les 200 par défaut peuvent en retenir 200 000 dans le pire cas.
Gardez-le assez grand pour `query monitor` et pour la passe de consolidation, qui complète
un incident environ trois TTL de trace après lui.
```

- [ ] **Step 3: HELM-DEPLOYMENT.md**, in `#### History and acks through the Hub (since 0.24.0)`, add one sentence after the `[daemon.hub_export]` example:

```markdown
From 0.29.0 the same block also pushes incidents when `[daemon.incidents]` is enabled, to
the Hub's `/api/import/incidents` (PerfSentinelHub 0.5.0 and later), with no extra value.
```

French mirror:

```markdown
Depuis 0.29.0, le même bloc pousse aussi les incidents quand `[daemon.incidents]` est
actif, vers `/api/import/incidents` du Hub (PerfSentinelHub 0.5.0 et suivants), sans
valeur supplémentaire.
```

- [ ] **Step 4: design note.** At the end of the daemon section of `docs/design/06-INGESTION-AND-DAEMON.md`:

```markdown
The Hub export carries incidents as well as findings. The incident store's change hooks,
which order the archive's lines under the ring's write lock, also hand each changed
incident to a latest-value table keyed by id, and the exporter task drains it in its own
round after the findings one. Freezing stays in the daemon: the window's findings are only
known there, at the moment of the incident.
```

- [ ] **Step 5: chart comment.** In `charts/perf-sentinel/values.yaml`, in the comment above the commented `[daemon.hub_export]` example, add the line:

```yaml
    # With [daemon.incidents] on as well, the same block pushes incidents to
    # /api/import/incidents (PerfSentinelHub 0.5.0+) from perf-sentinel 0.29.0.
```

A comment edit changes the rendered ConfigMap checksum, so `scripts/check-chart-version-bumped.sh` will ask for a chart bump: leave the bump to the 0.29.0 release commit, which moves the chart in lockstep.

- [ ] **Step 6: CHANGELOG.md** under `## [Unreleased]` (`### Added`):

```markdown
- The Hub export pushes incidents too. With `[daemon.hub_export]` and `[daemon.incidents]`
  both enabled, each incident goes to the Hub's `/api/import/incidents` when it is
  recorded, closed or completed by the settle pass, so one the ring evicts between two Hub
  polls is no longer lost. Batches hold at most 10 incidents and 4 MiB. A Hub without the
  route (404 or 405) stops incident pushes until restart, and findings keep flowing. New
  label-free metrics `perf_sentinel_hub_export_incidents_pending` and
  `perf_sentinel_hub_export_incidents_dropped_total`.
```

- [ ] **Step 7: Verify and commit**

Run: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

```bash
git add docs/CONFIGURATION.md docs/FR/CONFIGURATION-FR.md docs/RUNBOOK.md docs/FR/RUNBOOK-FR.md docs/HELM-DEPLOYMENT.md docs/FR/HELM-DEPLOYMENT-FR.md docs/design/06-INGESTION-AND-DAEMON.md charts/perf-sentinel/values.yaml CHANGELOG.md
git commit -S -m "docs: describe the incident push and the ring it lets shrink"
```

---

### Task 7: Lab scenario `hub-incidents-push`

Runs in `~/IdeaProjects/perf-sentinel-simulation-lab` against images built from both branches (`make seed-daemon-local PERF_SENTINEL_REPO_PATH=...` from this branch, `make seed-hub-local` from the Hub's `feat/incident-import`). Commit straight to the lab's `main`.

**Files:**
- Create: `scenarios/hub-incidents-push/verify.sh`, `scenarios/hub-incidents-push/README.md`
- Modify: `Makefile` (a `verify-hub-incidents-push` target beside `verify-hub-incidents-mirror`)

- [ ] **Step 1: Build the scenario from `hub-incidents-mirror`'s isolated leg.** Start `verify.sh` from `scenarios/hub-incidents-mirror/verify.sh`: keep its header style, helpers (`step`, `ok`, `fail`, `record`, `alert_body`, `daemon_api`, `hub_api`, `row_of`, `now_ms`, `jqp`), `cleanup` trap, report writer and the isolated namespace setup of its step 6 (`ISO_NS`, the isolated daemon and Hub manifests, `iso_forward`). Change the isolated pair so:
  - the daemon's `config.toml` has `[daemon.incidents] max_retained = 2` and a `[daemon.hub_export]` block (`enabled = true`, `endpoint = "http://<iso hub service>:8080/api/import/findings"`, `source_id` = the Hub source id, `api_key_file` mounted from a Secret holding a 32-character key, `flush_interval_secs = 1`);
  - the Hub's source carries the same key as `importSecretName`/`importSecretKey` (env `Hub__Sources__0__ImportApiKey`) and `Hub__PollInterval=24:00:00`, so only a push can bring the incidents in during the run;
  - nothing in the scenario calls `hub_refresh`.

- [ ] **Step 2: Write the legs**

```bash
step "1. Five incidents through a ring that keeps two"
for i in 1 2 3 4 5; do
  STARTS="$(python3 -c "import datetime as d; print((d.datetime.now(d.timezone.utc) - d.timedelta(minutes=$i)).strftime('%Y-%m-%dT%H:%M:%SZ'))")"
  CODE="$(daemon_api POST /api/incidents "$(alert_body "incpush-svc-${i}" oom_kill incpush "${STARTS}" "push ${i}")" "${WRITE_KEY}")"
  [ "${CODE}" = "200" ] || die "the daemon refused alert ${i} (HTTP ${CODE})"
done
sleep 5
daemon_api GET "/api/incidents?findings=false" "" "${READ_KEY}" >/dev/null
RING="$(jqp 'len(d)' < "${TMP_DIR}/body")"
[ "${RING}" = "2" ] && ok "the daemon's ring holds 2" || fail "the ring holds ${RING}, expected 2"
hub_api GET "/api/incidents?source_id=${ISO_SOURCE}&limit=100" >/dev/null
HUB="$(jqp 'len([r for r in d if r["service"].startswith("incpush-svc-")])' < "${TMP_DIR}/body")"
if [ "${HUB}" = "5" ]; then
  ok "the Hub holds all 5 without a poll"; record push-survives-eviction PASS "5/5"
else
  fail "the Hub holds ${HUB} of 5"; record push-survives-eviction FAIL "${HUB}/5"
fi

step "2. A resolved alert carries its end to the Hub"
RESOLVED="$(alert_body incpush-svc-5 oom_kill incpush "${STARTS}" "push 5" | python3 -c "
import json,sys,datetime as d
b=json.load(sys.stdin); a=b['alerts'][0]
a['status']='resolved'; a['endsAt']=d.datetime.now(d.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
print(json.dumps(b))")"
daemon_api POST /api/incidents "${RESOLVED}" "${WRITE_KEY}" >/dev/null
sleep 5
hub_api GET "/api/incidents?source_id=${ISO_SOURCE}&service=incpush-svc-5" >/dev/null
ENDED="$(jqp 'd[0].get("ended_at_ms") is not None' < "${TMP_DIR}/body")"
if [ "${ENDED}" = "True" ]; then
  ok "the Hub carries the end"; record push-carries-close PASS "ended_at_ms set"
else
  fail "the Hub has no end"; record push-carries-close FAIL "ended_at_ms missing"
fi

step "3. Nothing was dropped"
DROPPED="$(curl -s "http://127.0.0.1:${ISO_DAEMON_PORT}/metrics" | awk '/^perf_sentinel_hub_export_incidents_dropped_total /{print $2}')"
[ "${DROPPED}" = "0" ] && record no-drops PASS "0" || record no-drops FAIL "${DROPPED}"
```

Leg 2 reuses `STARTS` from the last loop iteration (`i = 5`), which is the alert it resolves: keep the loop's last `STARTS` value or recompute it for minute 5. `ISO_SOURCE` and `ISO_DAEMON_PORT` are the isolated pair's source id and forwarded daemon port, as named in the step 6 code you copied.

- [ ] **Step 3: README and Makefile.** `README.md` states the claim (an incident evicted from the ring before any poll still reaches the Hub, and so does its end), the setup (isolated pair, `max_retained = 2`, 24 h poll, no refresh) and the three verdicts. Makefile:

```make
verify-hub-incidents-push: ## daemon pushes incidents: five through a ring of two reach the Hub without a poll, and a resolve carries its end
	./scenarios/hub-incidents-push/verify.sh
```

- [ ] **Step 4: Run it**

Run: `make verify-hub-incidents-push`
Expected: three PASS verdicts in `/tmp/scenario-hub-incidents-push-report.md`. Then `make verify-hub-incidents-mirror` to confirm the poll path is unchanged.

- [ ] **Step 5: Commit on the lab's main**

```bash
git add scenarios/hub-incidents-push Makefile
git commit -S -m "feat(hub-incidents-push): incidents pushed past an evicting ring reach the Hub"
```

Never commit the local image pins `seed-*-local` leave in `manifests/`.
