//! Below-threshold passes: runs of watched threshold-type signatures that
//! stayed at or under their threshold.
//!
//! The daemon's detection ledger counts these per day, next to the
//! detections, so "this signature ran and stayed under its threshold" can
//! be told from "this signature did not run". A pass never creates a
//! finding. Per kind, one pass is:
//!
//! - slow: one span with `duration_us <= slow_threshold_ms * 1000`;
//! - chatty: one trace grouping whose outbound HTTP count is at most
//!   `chatty_service_min_calls`;
//! - pool saturation: one service grouping whose SQL peak concurrency stays
//!   under `pool_saturation_concurrent_threshold`;
//! - fanout: one parent with at most `max_fanout` children;
//! - serialized: one parent in the trace whose longest sequential chain is
//!   under `serialized_min_sequential`. A parent outside the trace is
//!   skipped, so such passes are missed, never invented.
//!
//! A pass counts only when it carries the exact signature of a watched
//! entry: same kind, service, endpoint and template, built the way
//! [`compute_signature`](crate::acknowledgments::compute_signature) builds
//! it for the finding the detector would emit.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::acknowledgments::{push_signature_head, template_digest};
use crate::correlate::Trace;
use crate::event::EventType;
use crate::normalize::NormalizedEvent;

use super::{DetectConfig, FindingType, TraceIndices, fanout, pool_saturation, serialized};

/// One watched signature: its template digest and the full text.
type Entry = ([u8; 16], Arc<str>);
/// Watched entries sharing one head.
type Entries = [Entry];

/// Watched signatures, indexed by head (the signature up to its last `:`),
/// so a pass builds the head once and hashes a template only on a hit.
#[derive(Debug, Default)]
pub struct WatchIndex {
    by_head: HashMap<Box<str>, Vec<Entry>>,
    kinds: HashSet<FindingType>,
}

impl WatchIndex {
    /// Keep each signature whose kind is thresholded and whose last part
    /// is 32 lowercase hex, once, up to `cap` entries. The head is never
    /// split into service and endpoint, so a `:` inside either is harmless.
    pub fn from_signatures<'a>(signatures: impl IntoIterator<Item = &'a str>, cap: usize) -> Self {
        let mut index = Self::default();
        let mut kept = 0;
        for signature in signatures {
            if kept == cap {
                break;
            }
            let Some((kind, head, digest)) = parse_signature(signature) else {
                continue;
            };
            let entries = index.by_head.entry(head.into()).or_default();
            if entries.iter().any(|(d, _)| *d == digest) {
                continue;
            }
            entries.push((digest, Arc::from(signature)));
            index.kinds.insert(kind);
            kept += 1;
        }
        index
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_head.values().map(Vec::len).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_head.is_empty()
    }

    #[must_use]
    pub fn watches(&self, kind: &FindingType) -> bool {
        self.kinds.contains(kind)
    }
}

fn parse_signature(signature: &str) -> Option<(FindingType, &str, [u8; 16])> {
    let (kind, _) = signature.split_once(':')?;
    let kind = FindingType::from_kind_str(kind).filter(FindingType::is_thresholded)?;
    let (head, hex) = signature.rsplit_once(':')?;
    Some((kind, head, decode_digest(hex)?))
}

fn decode_digest(hex: &str) -> Option<[u8; 16]> {
    if hex.len() != 32 {
        return None;
    }
    let (pairs, _) = hex.as_bytes().as_chunks::<2>();
    let mut digest = [0u8; 16];
    for (slot, &[high, low]) in digest.iter_mut().zip(pairs) {
        *slot = (nibble(high)? << 4) | nibble(low)?;
    }
    Some(digest)
}

const fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Count the below-threshold passes of every watched signature in the
/// batch. Keys are the watched signatures, values their passes; a
/// signature with no pass is absent.
#[must_use]
pub fn count_below_threshold(
    traces: &[Trace],
    config: &DetectConfig,
    watch: &WatchIndex,
) -> HashMap<Arc<str>, u64> {
    if watch.is_empty() {
        return HashMap::new();
    }
    let mut pass = Pass::new(watch);
    for trace in traces {
        pass.trace(trace, config);
    }
    pass.counts
}

/// The last head looked up, and what it matched.
type LastHead<'w, 't> = (FindingType, &'t str, &'t str, Option<&'w Entries>);

/// One batch's matching state: the reused head buffer, the per-batch
/// digest memo and the counts.
struct Pass<'w, 't> {
    watch: &'w WatchIndex,
    head: String,
    last: Option<LastHead<'w, 't>>,
    digests: HashMap<&'t str, [u8; 16]>,
    counts: HashMap<Arc<str>, u64>,
}

impl<'w, 't> Pass<'w, 't> {
    fn new(watch: &'w WatchIndex) -> Self {
        Self {
            watch,
            head: String::new(),
            last: None,
            digests: HashMap::new(),
            counts: HashMap::new(),
        }
    }

    /// The watched entries under this head, the lookup cached while the
    /// triple repeats (spans of one trace mostly share service and endpoint).
    fn entries(
        &mut self,
        kind: &FindingType,
        service: &'t str,
        endpoint: &'t str,
    ) -> Option<&'w Entries> {
        if let Some((k, s, e, hit)) = &self.last
            && k == kind
            && *s == service
            && *e == endpoint
        {
            return *hit;
        }
        self.head.clear();
        push_signature_head(&mut self.head, kind, service, endpoint);
        let watch = self.watch;
        let hit = watch.by_head.get(self.head.as_str()).map(Vec::as_slice);
        self.last = Some((kind.clone(), service, endpoint, hit));
        hit
    }

    /// Count `n` passes for the entry of `template`, if one is watched.
    fn add(&mut self, entries: &'w Entries, template: &'t str, n: u64) {
        let digest = *self
            .digests
            .entry(template)
            .or_insert_with(|| template_digest(template));
        if let Some((_, signature)) = entries.iter().find(|(d, _)| *d == digest) {
            *self.counts.entry(Arc::clone(signature)).or_default() += n;
        }
    }

    fn entries_of(&mut self, kind: &FindingType, span: &'t NormalizedEvent) -> Option<&'w Entries> {
        self.entries(kind, &span.event.service, &span.event.source.endpoint)
    }

    /// Run every pass kind that has a watched signature over one trace.
    fn trace(&mut self, trace: &'t Trace, config: &DetectConfig) {
        let watch = self.watch;
        let slow = [
            FindingType::SlowSql,
            FindingType::SlowHttp,
            FindingType::SlowMessaging,
        ]
        .iter()
        .any(|kind| watch.watches(kind));
        if slow {
            self.slow(trace, config.slow_threshold_ms);
        }
        if watch.watches(&FindingType::ChattyService) {
            self.chatty(trace, config.chatty_service_min_calls);
        }
        if watch.watches(&FindingType::PoolSaturation) {
            self.pool(trace, config.pool_saturation_concurrent_threshold);
        }
        let fanout = watch.watches(&FindingType::ExcessiveFanout);
        let serialized = watch.watches(&FindingType::SerializedCalls);
        if fanout || serialized {
            let indices = TraceIndices::build(trace);
            if fanout {
                self.fanout(trace, &indices, config.max_fanout);
            }
            if serialized {
                self.serialized(trace, &indices, config.serialized_min_sequential);
            }
        }
    }

    fn slow(&mut self, trace: &'t Trace, threshold_ms: u64) {
        let threshold_us = threshold_ms.saturating_mul(1000);
        for span in &trace.spans {
            let kind = FindingType::from_event_type_slow(&span.event.event_type);
            if span.event.duration_us > threshold_us || !self.watch.watches(&kind) {
                continue;
            }
            if let Some(entries) = self.entries_of(&kind, span) {
                self.add(entries, &span.template, 1);
            }
        }
    }

    fn chatty(&mut self, trace: &'t Trace, min_calls: u32) {
        // First span and call count per grouping, as `detect_chatty` groups.
        let mut groups: HashMap<Option<(&str, &str)>, (&'t NormalizedEvent, usize)> =
            HashMap::new();
        for span in &trace.spans {
            if span.event.event_type == EventType::HttpOut {
                groups
                    .entry(span.event.grouping_identity())
                    .or_insert((span, 0))
                    .1 += 1;
            }
        }
        for (first, calls) in groups.into_values() {
            if calls > min_calls as usize {
                continue;
            }
            if let Some(entries) = self.entries_of(&FindingType::ChattyService, first) {
                self.add(entries, &first.event.source.endpoint, 1);
            }
        }
    }

    fn pool(&mut self, trace: &'t Trace, threshold: u32) {
        for ((service, _), indices) in pool_saturation::group_sql_indices_by_service(trace) {
            let endpoint = trace.spans[indices[0]].event.source.endpoint.as_str();
            let Some(entries) = self.entries(&FindingType::PoolSaturation, service, endpoint)
            else {
                continue;
            };
            let below = indices.len() < threshold as usize
                || pool_saturation::compute_peak_concurrency(trace, &indices) < threshold;
            if below {
                self.add(entries, service, 1);
            }
        }
    }

    fn fanout(&mut self, trace: &'t Trace, indices: &TraceIndices<'t>, max_fanout: u32) {
        for (parent_id, children) in indices.parents_with_children(1) {
            if children.len() > max_fanout as usize {
                continue;
            }
            let parent = indices.span_index.get(parent_id).map(|&i| &trace.spans[i]);
            let owner = parent.unwrap_or(&trace.spans[children[0]]);
            let Some(entries) = self.entries_of(&FindingType::ExcessiveFanout, owner) else {
                continue;
            };
            let template = parent.map_or_else(
                || fanout::dominant_template(trace, children),
                |span| &span.template,
            );
            self.add(entries, template, 1);
        }
    }

    fn serialized(&mut self, trace: &'t Trace, indices: &TraceIndices<'t>, min_sequential: u32) {
        let min = min_sequential as usize;
        for (parent_id, children) in indices.parents_with_children(1) {
            let Some(&parent) = indices.span_index.get(parent_id) else {
                continue;
            };
            let parent = &trace.spans[parent];
            let Some(entries) = self.entries_of(&FindingType::SerializedCalls, parent) else {
                continue;
            };
            if children.len() < min || serialized::longest_chain_len(trace, children) < min {
                self.add(entries, &parent.event.source.endpoint, 1);
            }
        }
    }
}

#[cfg(test)]
mod tests;
