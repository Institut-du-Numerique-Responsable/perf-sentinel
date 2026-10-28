//! What the Hub exporter's incident and correlation halves share: a
//! latest-value table keyed by id, and the half that drains one table to
//! its own import route.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prometheus::{Gauge, IntCounter};

use super::hub_export::{
    ExportOutcome, HubClient, bounded_body, classify, evict_by_order, next_delay, post_batch,
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
        client: &HubClient,
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
pub(crate) fn is_missing_route(status: hyper::StatusCode) -> bool {
    matches!(
        status,
        hyper::StatusCode::NOT_FOUND | hyper::StatusCode::METHOD_NOT_ALLOWED
    )
}
