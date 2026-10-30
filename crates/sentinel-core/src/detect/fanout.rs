//! Fanout detection: identifies parent spans generating excessive child spans.

use crate::correlate::Trace;
use crate::detect::{Confidence, Finding, FindingType, Pattern, Severity, TraceIndices};

/// Detect excessive fanout within a trace.
///
/// A parent span with more than `max_fanout` children is flagged.
/// Severity is `Warning` if > `max_fanout`, `Critical` if > 3x `max_fanout`.
///
/// The shared [`TraceIndices`] are built once per trace in `detect()`
/// and reused here and in `detect_serialized`, halving the per-trace
/// HashMap-build cost for traces that trigger both.
#[must_use]
pub fn detect_fanout(trace: &Trace, indices: &TraceIndices<'_>, max_fanout: u32) -> Vec<Finding> {
    fanout_impl(trace, indices, max_fanout, false)
        .into_iter()
        .map(|(finding, _)| finding)
        .collect()
}

/// Same detection, also returning the ids of the children that make up the
/// fanout. The HTML sink highlights them, since the finding's template
/// names the parent when it is in the trace and would otherwise light the
/// container alone.
#[must_use]
pub(crate) fn detect_fanout_with_spans<'a>(
    trace: &'a Trace,
    indices: &TraceIndices<'_>,
    max_fanout: u32,
) -> Vec<(Finding, Vec<&'a str>)> {
    fanout_impl(trace, indices, max_fanout, true)
}

/// `collect_spans` stays off on the detection path: a fanout finding
/// exists because the child count is large, so building that id list per
/// finding only to drop it would allocate most on the widest traces.
fn fanout_impl<'a>(
    trace: &'a Trace,
    indices: &TraceIndices<'_>,
    max_fanout: u32,
    collect_spans: bool,
) -> Vec<(Finding, Vec<&'a str>)> {
    let span_index = &indices.span_index;

    let mut findings = Vec::new();

    for (parent_id, child_indices) in indices.parents_with_children(max_fanout as usize + 1) {
        let count = child_indices.len();

        let severity = if count > (max_fanout as usize) * 3 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        // Find the parent span for context (service, endpoint)
        let parent_span = span_index.get(parent_id).map(|&i| &trace.spans[i]);

        let service: String = parent_span.map_or_else(
            || trace.spans[child_indices[0]].event.service.to_string(),
            |s| s.event.service.to_string(),
        );

        let endpoint = parent_span.map_or_else(
            || trace.spans[child_indices[0]].event.source.endpoint.clone(),
            |s| s.event.source.endpoint.clone(),
        );
        let first_child = &trace.spans[child_indices[0]].event;
        let namespace_event = parent_span.map_or(first_child, |span| &span.event);

        // Compute window from children timestamps in one pass (no intermediate Vec)
        let (window_ms, first_ts, last_ts) =
            crate::detect::n_plus_one::compute_window_and_bounds_iter(
                child_indices
                    .iter()
                    .map(|&i| trace.spans[i].event.timestamp.as_str()),
            );
        let first_ts = first_ts.to_string();
        let last_ts = last_ts.to_string();

        // Parent template, else the dominant child template: a span id would make
        // every trace's signature unique, and a parent outside the trace is the
        // usual case when the server span is not a captured event.
        let template = parent_span.map_or_else(
            || dominant_template(trace, child_indices).to_string(),
            |s| s.template.to_string(),
        );

        let child_span_ids = crate::detect::member_span_ids(trace, child_indices, collect_spans);
        findings.push((
            Finding {
                finding_type: FindingType::ExcessiveFanout,
                severity,
                trace_id: trace.trace_id.clone(),
                service,
                grouping: namespace_event.grouping.clone(),
                source_endpoint: endpoint,
                pattern: Pattern {
                    template,
                    occurrences: count,
                    window_ms,
                    distinct_params: count,
                    ..Default::default()
                },
                suggestion: format!(
                    "Parent span has {count} children (threshold: {max_fanout}). \
                 Consider batching child operations to reduce fanout."
                ),
                first_timestamp: first_ts,
                last_timestamp: last_ts,
                green_impact: None,
                confidence: Confidence::default(),
                classification_method: None,
                // Representative call: the first child.
                code_location: first_child.code_location(),
                instrumentation_scopes: first_child.scope_names(),
                suggested_fix: None,
                signature: String::new(),
            },
            child_span_ids,
        ));
    }

    findings
}

/// The most frequent template among the children, ties going to the smallest,
/// so the same fanout yields the same template in every trace.
pub(crate) fn dominant_template<'t>(trace: &'t Trace, child_indices: &[usize]) -> &'t str {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for &i in child_indices {
        *counts.entry(&trace.spans[i].template).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map_or("", |(template, _)| template)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{make_sql_event, make_trace};

    fn make_events_with_parent(
        trace_id: &str,
        parent_id: &str,
        count: usize,
    ) -> Vec<crate::event::SpanEvent> {
        let mut events = Vec::new();
        let mut root = make_sql_event(trace_id, parent_id, "SELECT 1", "2025-07-10T14:32:01.000Z");
        root.parent_span_id = None;
        events.push(root);

        for i in 0..count {
            let mut child = make_sql_event(
                trace_id,
                &format!("child-{i}"),
                &format!("SELECT * FROM t WHERE id = {i}"),
                &format!("2025-07-10T14:32:01.{:03}Z", (i + 1) * 10),
            );
            child.parent_span_id = Some(parent_id.to_string());
            events.push(child);
        }
        events
    }

    #[test]
    fn detects_excessive_fanout() {
        let events = make_events_with_parent("trace-1", "root", 25);
        let trace = make_trace(events);
        let findings = detect_fanout(&trace, &TraceIndices::build(&trace), 20);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].finding_type, FindingType::ExcessiveFanout);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert_eq!(findings[0].pattern.occurrences, 25);
    }

    #[test]
    fn carries_first_child_scopes_and_code_location() {
        let mut events = make_events_with_parent("trace-1", "root", 25);
        events[0].instrumentation_scopes =
            vec![std::sync::Arc::from("io.opentelemetry.tomcat-10.0")];
        events[1].instrumentation_scopes = vec![std::sync::Arc::from("io.opentelemetry.jdbc")];
        events[1].code_namespace = Some(std::sync::Arc::from("com.example.OrderRepository"));
        let trace = make_trace(events);
        let mut findings = detect_fanout(&trace, &TraceIndices::build(&trace), 20);
        crate::test_helpers::assert_java_representative_call(
            &mut findings,
            "io.opentelemetry.jdbc",
            "com.example.OrderRepository",
        );
    }

    #[test]
    fn critical_at_3x_threshold() {
        let events = make_events_with_parent("trace-1", "root", 65);
        let trace = make_trace(events);
        let findings = detect_fanout(&trace, &TraceIndices::build(&trace), 20);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Critical);
    }

    #[test]
    fn no_finding_below_threshold() {
        let events = make_events_with_parent("trace-1", "root", 10);
        let trace = make_trace(events);
        let findings = detect_fanout(&trace, &TraceIndices::build(&trace), 20);

        assert_eq!(findings, []);
    }

    #[test]
    fn no_finding_at_threshold() {
        let events = make_events_with_parent("trace-1", "root", 20);
        let trace = make_trace(events);
        let findings = detect_fanout(&trace, &TraceIndices::build(&trace), 20);

        assert_eq!(findings, []);
    }

    #[test]
    fn parent_not_in_trace_uses_child_metadata() {
        // Parent ID references a span that doesn't exist in the trace
        let mut events = Vec::new();
        for i in 0..25 {
            let mut child = make_sql_event(
                "trace-1",
                &format!("child-{i}"),
                &format!("SELECT * FROM t WHERE id = {i}"),
                &format!("2025-07-10T14:32:01.{:03}Z", (i + 1) * 10),
            );
            child.parent_span_id = Some("nonexistent-parent".to_string());
            events.push(child);
        }
        let trace = make_trace(events);
        let findings = detect_fanout(&trace, &TraceIndices::build(&trace), 20);

        assert_eq!(findings.len(), 1);
        // Service and endpoint should come from the first child span
        assert_eq!(findings[0].service, "order-svc");
    }

    #[test]
    fn parent_not_in_trace_keeps_signature_stable_across_traces() {
        let orphan = |trace_id: &str, parent_id: &str| {
            let mut events = Vec::new();
            for i in 0..25 {
                let sql = if i < 3 {
                    format!("SELECT * FROM audit WHERE id = {i}")
                } else {
                    format!("SELECT * FROM t WHERE id = {i}")
                };
                let mut child = make_sql_event(
                    trace_id,
                    &format!("{trace_id}-child-{i}"),
                    &sql,
                    &format!("2025-07-10T14:32:01.{:03}Z", (i + 1) * 10),
                );
                child.parent_span_id = Some(parent_id.to_string());
                events.push(child);
            }
            let trace = make_trace(events);
            let mut findings = detect_fanout(&trace, &TraceIndices::build(&trace), 20);
            crate::acknowledgments::enrich_with_signatures(&mut findings);
            findings.remove(0)
        };
        let a = orphan("trace-1", "bccb4bfc238eae4a");
        let b = orphan("trace-2", "b78d2dd43064b89c");

        assert_eq!(a.pattern.template, "SELECT * FROM t WHERE id = ?");
        assert_eq!(a.signature, b.signature);
    }

    #[test]
    fn dominant_template_breaks_ties_on_the_smallest() {
        let events: Vec<_> = ["SELECT b", "SELECT a", "SELECT b", "SELECT a"]
            .iter()
            .enumerate()
            .map(|(i, sql)| {
                make_sql_event(
                    "trace-1",
                    &format!("s-{i}"),
                    sql,
                    "2025-07-10T14:32:01.000Z",
                )
            })
            .collect();
        let trace = make_trace(events);

        assert_eq!(dominant_template(&trace, &[0, 1, 2, 3]), "SELECT a");
        assert_eq!(dominant_template(&trace, &[3, 2, 1, 0]), "SELECT a");
        assert_eq!(dominant_template(&trace, &[0, 1, 2]), "SELECT b");
    }

    #[test]
    fn no_finding_without_parent_ids() {
        // Events without parent_span_id set
        let events: Vec<_> = (1..=10)
            .map(|i| {
                make_sql_event(
                    "trace-1",
                    &format!("span-{i}"),
                    &format!("SELECT * FROM t WHERE id = {i}"),
                    &format!("2025-07-10T14:32:01.{:03}Z", i * 50),
                )
            })
            .collect();
        let trace = make_trace(events);
        let findings = detect_fanout(&trace, &TraceIndices::build(&trace), 5);

        assert_eq!(findings, []);
    }
}
