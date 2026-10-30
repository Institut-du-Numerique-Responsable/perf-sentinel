//! Below-threshold passes: runs of watched threshold-type signatures that
//! stayed at or under their threshold.
//!
//! The daemon's detection ledger counts these per day, next to the
//! detections, so "this signature ran and stayed under its threshold" can
//! be told from "this signature did not run". A pass never creates a
//! finding. It is counted during detection, inside the threshold detector
//! that measures it, when the daemon holds a watch list
//! ([`run_full_detection_with_below`](super::run_full_detection_with_below)).
//! Per kind, one pass is:
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

use crate::acknowledgments::{push_signature_head, push_signature_pair, template_digest};
use crate::correlate::Trace;
use crate::normalize::NormalizedEvent;

use super::{FindingType, fanout};

/// One watched signature: its template digest and the full text.
type Entry = ([u8; 16], Arc<str>);
/// Watched entries sharing one head.
type Entries = [Entry];

/// Watched signatures, indexed by head (the signature up to its last `:`),
/// so a pass builds the head once and hashes a template only on a hit.
#[derive(Debug, Default)]
pub struct WatchIndex {
    by_head: HashMap<Box<str>, Vec<Entry>>,
    /// Every head without its kind prefix, so a trace none of whose spans
    /// carries a watched service and endpoint is skipped whole.
    pairs: HashSet<Box<str>>,
    /// One [`kind_bit`] per watched kind.
    kinds: u8,
}

/// One bit per thresholded kind, 0 for the others.
const fn kind_bit(kind: &FindingType) -> u8 {
    match kind {
        FindingType::SlowSql => 1,
        FindingType::SlowHttp => 1 << 1,
        FindingType::SlowMessaging => 1 << 2,
        FindingType::ExcessiveFanout => 1 << 3,
        FindingType::ChattyService => 1 << 4,
        FindingType::PoolSaturation => 1 << 5,
        FindingType::SerializedCalls => 1 << 6,
        _ => 0,
    }
}

/// The thresholded kind's slot in [`BelowPass`]'s per-kind cache.
const fn kind_slot(kind: &FindingType) -> usize {
    kind_bit(kind).trailing_zeros() as usize
}

const SLOW_BITS: u8 = kind_bit(&FindingType::SlowSql)
    | kind_bit(&FindingType::SlowHttp)
    | kind_bit(&FindingType::SlowMessaging);

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
            let (_, pair) = head.split_once(':').unwrap_or_default();
            index.pairs.insert(pair.into());
            index.kinds |= kind_bit(&kind);
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
        self.kinds & kind_bit(kind) != 0
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

/// One kind's last looked-up service and endpoint, and what they matched.
type LastHead<'w, 't> = (&'t str, &'t str, Option<&'w Entries>);

/// One batch's matching state: the reused head buffer, the per-batch
/// digest memo and the counts. The threshold detectors feed it through
/// one entry method per kind, each called on a run at or under its
/// threshold with the span that carries the finding's identity.
pub(crate) struct BelowPass<'w, 't> {
    watch: &'w WatchIndex,
    head: String,
    /// One slot per thresholded kind ([`kind_slot`]), so interleaved SQL
    /// and HTTP spans do not evict each other's lookup.
    last: [Option<LastHead<'w, 't>>; 7],
    digests: HashMap<&'t str, [u8; 16]>,
    counts: HashMap<Arc<str>, u64>,
}

impl<'w, 't> BelowPass<'w, 't> {
    pub(crate) fn new(watch: &'w WatchIndex) -> Self {
        Self {
            watch,
            head: String::new(),
            last: [None; 7],
            digests: HashMap::new(),
            counts: HashMap::new(),
        }
    }

    /// The passes per watched signature, with no zero entry.
    pub(crate) fn into_counts(self) -> HashMap<Arc<str>, u64> {
        self.counts
    }

    pub(crate) fn watches(&self, kind: &FindingType) -> bool {
        self.watch.watches(kind)
    }

    /// Whether any of the three slow kinds is watched.
    pub(crate) fn watches_slow(&self) -> bool {
        self.watch.kinds & SLOW_BITS != 0
    }

    /// Whether any span of `trace` carries a watched service and endpoint,
    /// checked once per run of spans sharing them. Every pass identity is
    /// one of the trace's spans, so a trace that touches none counts none.
    pub(crate) fn touches(&mut self, trace: &Trace) -> bool {
        let mut last: Option<(&str, &str)> = None;
        for span in &trace.spans {
            let pair = (&*span.event.service, span.event.source.endpoint.as_str());
            if last == Some(pair) {
                continue;
            }
            last = Some(pair);
            self.head.clear();
            push_signature_pair(&mut self.head, pair.0, pair.1);
            if self.watch.pairs.contains(self.head.as_str()) {
                return true;
            }
        }
        false
    }

    /// The watched entries under this head, the lookup cached per kind
    /// while service and endpoint repeat (spans of one trace mostly share
    /// them).
    fn entries(
        &mut self,
        kind: &FindingType,
        service: &'t str,
        endpoint: &'t str,
    ) -> Option<&'w Entries> {
        let slot = kind_slot(kind);
        if let Some((s, e, hit)) = self.last[slot]
            && s == service
            && e == endpoint
        {
            return hit;
        }
        self.head.clear();
        push_signature_head(&mut self.head, kind, service, endpoint);
        let watch = self.watch;
        let hit = watch.by_head.get(self.head.as_str()).map(Vec::as_slice);
        self.last[slot] = Some((service, endpoint, hit));
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

    /// One span at or under the slow threshold.
    pub(crate) fn slow_span(&mut self, span: &'t NormalizedEvent) {
        let kind = FindingType::from_event_type_slow(&span.event.event_type);
        if !self.watches(&kind) {
            return;
        }
        if let Some(entries) = self.entries_of(&kind, span) {
            self.add(entries, &span.template, 1);
        }
    }

    /// One grouping's outbound calls at or under `chatty_service_min_calls`,
    /// identified by its first call.
    pub(crate) fn chatty_group(&mut self, first: &'t NormalizedEvent) {
        if let Some(entries) = self.entries_of(&FindingType::ChattyService, first) {
            self.add(entries, &first.event.source.endpoint, 1);
        }
    }

    /// One service grouping whose SQL peak stays under the threshold,
    /// identified by its first span (the group key's service is its own).
    pub(crate) fn pool_group(&mut self, first: &'t NormalizedEvent) {
        if let Some(entries) = self.entries_of(&FindingType::PoolSaturation, first) {
            self.add(entries, &first.event.service, 1);
        }
    }

    /// One parent with at most `max_fanout` children. Without the parent
    /// span the first child owns it and the dominant child template names it.
    pub(crate) fn fanout_parent(
        &mut self,
        trace: &'t Trace,
        parent: Option<&'t NormalizedEvent>,
        children: &[usize],
    ) {
        let owner = parent.unwrap_or(&trace.spans[children[0]]);
        let Some(entries) = self.entries_of(&FindingType::ExcessiveFanout, owner) else {
            return;
        };
        let template = parent.map_or_else(
            || fanout::dominant_template(trace, children),
            |span| &span.template,
        );
        self.add(entries, template, 1);
    }

    /// One parent in the trace whose longest chain is under the minimum.
    pub(crate) fn serialized_parent(&mut self, parent: &'t NormalizedEvent) {
        if let Some(entries) = self.entries_of(&FindingType::SerializedCalls, parent) {
            self.add(entries, &parent.event.source.endpoint, 1);
        }
    }
}

#[cfg(test)]
mod reference;
#[cfg(test)]
mod tests;
