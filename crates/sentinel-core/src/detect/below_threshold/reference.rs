//! The separate below-threshold walk the detectors replaced, kept as the
//! test oracle: its traversal and its "below" decisions pin, pass for
//! pass, the counts the threshold detectors make during detection.

use std::collections::HashMap;
use std::sync::Arc;

use crate::correlate::Trace;
use crate::detect::{DetectConfig, FindingType, TraceIndices, pool_saturation, serialized};
use crate::event::EventType;
use crate::normalize::NormalizedEvent;

use super::{BelowPass, WatchIndex};

/// Count the below-threshold passes of every watched signature in the
/// batch. Keys are the watched signatures, values their passes; a
/// signature with no pass is absent.
pub(super) fn count_below_threshold_reference(
    traces: &[Trace],
    config: &DetectConfig,
    watch: &WatchIndex,
) -> HashMap<Arc<str>, u64> {
    if watch.is_empty() {
        return HashMap::new();
    }
    let mut pass = BelowPass::new(watch);
    for trace in traces {
        walk_trace(&mut pass, trace, config);
    }
    pass.into_counts()
}

/// Run every pass kind that has a watched signature over one trace.
fn walk_trace<'t>(pass: &mut BelowPass<'_, 't>, trace: &'t Trace, config: &DetectConfig) {
    if pass.watches_slow() {
        walk_slow(pass, trace, config.slow_threshold_ms);
    }
    if pass.watches(&FindingType::ChattyService) {
        walk_chatty(pass, trace, config.chatty_service_min_calls);
    }
    if pass.watches(&FindingType::PoolSaturation) {
        walk_pool(pass, trace, config.pool_saturation_concurrent_threshold);
    }
    let fanout = pass.watches(&FindingType::ExcessiveFanout);
    let serialized = pass.watches(&FindingType::SerializedCalls);
    if fanout || serialized {
        let indices = TraceIndices::build(trace);
        if fanout {
            walk_fanout(pass, trace, &indices, config.max_fanout);
        }
        if serialized {
            walk_serialized(pass, trace, &indices, config.serialized_min_sequential);
        }
    }
}

fn walk_slow<'t>(pass: &mut BelowPass<'_, 't>, trace: &'t Trace, threshold_ms: u64) {
    let threshold_us = threshold_ms.saturating_mul(1000);
    for span in &trace.spans {
        if span.event.duration_us <= threshold_us {
            pass.slow_span(span);
        }
    }
}

fn walk_chatty<'t>(pass: &mut BelowPass<'_, 't>, trace: &'t Trace, min_calls: u32) {
    // First span and call count per grouping, as `detect_chatty` groups.
    let mut groups: HashMap<Option<(&str, &str)>, (&'t NormalizedEvent, usize)> = HashMap::new();
    for span in &trace.spans {
        if span.event.event_type == EventType::HttpOut {
            groups
                .entry(span.event.grouping_identity())
                .or_insert((span, 0))
                .1 += 1;
        }
    }
    for (first, calls) in groups.into_values() {
        if calls <= min_calls as usize {
            pass.chatty_group(first);
        }
    }
}

fn walk_pool<'t>(pass: &mut BelowPass<'_, 't>, trace: &'t Trace, threshold: u32) {
    for indices in pool_saturation::group_sql_indices_by_service(trace).into_values() {
        let below = indices.len() < threshold as usize
            || pool_saturation::compute_peak_concurrency(trace, &indices) < threshold;
        if below {
            pass.pool_group(&trace.spans[indices[0]]);
        }
    }
}

fn walk_fanout<'t>(
    pass: &mut BelowPass<'_, 't>,
    trace: &'t Trace,
    indices: &TraceIndices<'t>,
    max_fanout: u32,
) {
    for (parent_id, children) in indices.parents_with_children(1) {
        if children.len() <= max_fanout as usize {
            let parent = indices.span_index.get(parent_id).map(|&i| &trace.spans[i]);
            pass.fanout_parent(trace, parent, children);
        }
    }
}

fn walk_serialized<'t>(
    pass: &mut BelowPass<'_, 't>,
    trace: &'t Trace,
    indices: &TraceIndices<'t>,
    min_sequential: u32,
) {
    let min = min_sequential as usize;
    for (parent_id, children) in indices.parents_with_children(1) {
        let Some(&parent) = indices.span_index.get(parent_id) else {
            continue;
        };
        if children.len() < min || serialized::longest_chain_len(trace, children) < min {
            pass.serialized_parent(&trace.spans[parent]);
        }
    }
}
