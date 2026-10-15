//! The daemon's incidents on their way to the Hub, beside its
//! findings: a latest-value table per incident id, fed by the incident
//! store's change hooks and drained by the exporter task.

// Wired into the exporter and the query API by the next commits.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::hub_export::evict_by_order;
use super::incidents::Incident;

#[derive(Clone)]
pub(crate) struct PendingIncident {
    pub(crate) incident: Incident,
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

    /// Up to `limit` pending incidents, least recently changed first.
    pub(crate) fn snapshot(&self, limit: usize) -> Vec<PendingIncident> {
        let state = self.lock();
        state
            .order
            .iter()
            .filter_map(|(id, revision)| {
                state
                    .entries
                    .get(id)
                    .filter(|entry| entry.revision == *revision)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    /// Remove what the Hub accepted, unless it changed again meanwhile.
    pub(crate) fn acknowledge(&self, sent: &[PendingIncident]) {
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

    /// The Hub has no incident route: stop queueing until the daemon
    /// restarts. Returns how many pending incidents this drops.
    pub(crate) fn mark_unsupported(&self) -> u64 {
        self.unsupported.store(true, Ordering::Relaxed);
        let mut state = self.lock();
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

    fn lock(&self) -> std::sync::MutexGuard<'_, PendingIncidents> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn incident(id: &str, ended_at_ms: Option<u64>) -> Incident {
        let mut value = serde_json::json!({
            "id": id,
            "service": "checkout",
            "kind": "oom_kill",
            "at_ms": 1_800_000_000_000_u64,
            "window_from_ms": 1_799_999_700_000_u64,
            "window_to_ms": 1_800_000_060_000_u64,
            "findings": []
        });
        if let Some(ended) = ended_at_ms {
            value["ended_at_ms"] = serde_json::json!(ended);
        }
        serde_json::from_value(value).expect("a valid incident")
    }

    fn ids(batch: Vec<PendingIncident>) -> Vec<String> {
        batch
            .into_iter()
            .map(|pending| pending.incident.id)
            .collect()
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

        assert_eq!(
            vec!["a".to_string(), "c".to_string()],
            ids(buffer.snapshot(10))
        );
    }

    #[test]
    fn snapshot_follows_change_order_and_honours_the_limit() {
        let buffer = IncidentExportBuffer::new(10);
        for id in ["a", "b", "c"] {
            buffer.push(&incident(id, None));
        }
        assert_eq!(
            vec!["a".to_string(), "b".to_string()],
            ids(buffer.snapshot(2))
        );
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
