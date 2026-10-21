use super::*;
use crate::detect::{Confidence, Finding, FindingType, Pattern, Severity};
use crate::report::interpret::InterpretationLevel;
use crate::report::{Analysis, GreenSummary, PerEndpointIoOps, QualityGate, Report};
use crate::score::carbon::{CarbonEstimate, CarbonReport};
use chrono::TimeZone;
use core::assert_matches;
use std::io::Write;
use tempfile::TempDir;

fn make_finding(service: &str, ft: FindingType, template: &str) -> Finding {
    Finding {
        finding_type: ft,
        severity: Severity::Warning,
        trace_id: "abc".to_string(),
        service: service.to_string(),
        grouping: Vec::new(),
        source_endpoint: "/api/test".to_string(),
        pattern: Pattern {
            template: template.to_string(),
            occurrences: 5,
            window_ms: 100,
            distinct_params: 3,
            ..Default::default()
        },
        suggestion: String::new(),
        first_timestamp: "2026-01-01T00:00:00Z".to_string(),
        last_timestamp: "2026-01-01T00:00:10Z".to_string(),
        green_impact: None,
        confidence: Confidence::DaemonProduction,
        classification_method: None,
        code_location: None,
        instrumentation_scopes: vec![],
        suggested_fix: None,
        signature: String::new(),
    }
}

fn make_report(
    traces: usize,
    total_io: usize,
    avoidable_io: usize,
    services_io: &[(&str, &str, usize)],
    findings: Vec<Finding>,
) -> Report {
    let carbon = CarbonReport {
        total: CarbonEstimate {
            low: 0.5,
            mid: 1.0,
            high: 2.0,
            model: "io_proxy_v3".to_string(),
            methodology: "sci_numerator".to_string(),
        },
        avoidable: CarbonEstimate {
            low: 0.1,
            mid: 0.2,
            high: 0.4,
            model: "io_proxy_v3".to_string(),
            methodology: "operational_ratio".to_string(),
        },
        operational_gco2: 0.8,
        embodied_gco2: 0.2,
        transport_gco2: None,
        database_gco2: None,
        messaging_gco2: None,
        sci_per_trace: None,
        functional_unit: String::new(),
    };
    let waste_ratio = if total_io == 0 {
        0.0
    } else {
        avoidable_io as f64 / total_io as f64
    };
    let band = InterpretationLevel::for_waste_ratio(waste_ratio);
    Report {
        analysis: Analysis {
            duration_ms: 10,
            events_processed: traces,
            traces_analyzed: traces,
            ingest: None,
        },
        findings,
        green_summary: GreenSummary {
            total_io_ops: total_io,
            avoidable_io_ops: avoidable_io,
            io_waste_ratio: waste_ratio,
            io_waste_ratio_band: band,
            co2: Some(carbon),
            ..GreenSummary::disabled(0)
        },
        quality_gate: QualityGate {
            passed: true,
            rules: vec![],
        },
        per_endpoint_io_ops: services_io
            .iter()
            .map(|(svc, ep, ops)| PerEndpointIoOps {
                service: (*svc).to_string(),
                endpoint: (*ep).to_string(),
                io_ops: *ops,
            })
            .collect(),
        correlations: vec![],
        embedded_traces: vec![],
        warnings: vec![],
        warning_details: vec![],
        acknowledged_findings: vec![],
        binary_version: String::new(),
        detection_config: None,
        disclosure_waste: None,
    }
}

fn write_archive(lines: &[(DateTime<Utc>, Report)]) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("archive.ndjson");
    let mut file = File::create(&path).unwrap();
    for (ts, report) in lines {
        let envelope = serde_json::json!({ "ts": ts, "report": report });
        writeln!(file, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
    }
    (dir, path)
}

/// Same shape the daemon writer produces, built with the shared chain
/// primitives rather than a second implementation of them.
fn write_chained_archive(lines: &[(DateTime<Utc>, Report)]) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("archive.ndjson");
    let mut file = File::create(&path).unwrap();
    let mut prev = super::super::hasher::ARCHIVE_CHAIN_SEED.to_string();
    for (seq, (ts, report)) in lines.iter().enumerate() {
        let body =
            serde_json::json!({ "ts": ts, "report": report, "prev": prev, "seq": seq as u64 });
        let hash = super::super::hasher::archive_chain_hash(&body).unwrap();
        let mut line = body;
        line.as_object_mut()
            .unwrap()
            .insert("hash".to_string(), serde_json::Value::String(hash.clone()));
        writeln!(file, "{}", serde_json::to_string(&line).unwrap()).unwrap();
        prev = hash;
    }
    (dir, path)
}

/// Chained archive whose lines carry the cumulative `drops` counter,
/// the shape the daemon writes since v1.7.
fn write_chained_archive_with_drops(lines: &[(DateTime<Utc>, Report, u64)]) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = write_drops_file(dir.path(), "archive.ndjson", lines);
    (dir, path)
}

fn write_drops_file(dir: &Path, name: &str, lines: &[(DateTime<Utc>, Report, u64)]) -> PathBuf {
    let path = dir.join(name);
    let mut file = File::create(&path).unwrap();
    let mut prev = super::super::hasher::ARCHIVE_CHAIN_SEED.to_string();
    for (seq, (ts, report, drops)) in lines.iter().enumerate() {
        let body = serde_json::json!({
            "ts": ts, "report": report, "prev": prev, "seq": seq as u64, "drops": drops,
        });
        let hash = super::super::hasher::archive_chain_hash(&body).unwrap();
        let mut line = body;
        line.as_object_mut()
            .unwrap()
            .insert("hash".to_string(), serde_json::Value::String(hash.clone()));
        writeln!(file, "{}", serde_json::to_string(&line).unwrap()).unwrap();
        prev = hash;
    }
    path
}

fn q1_2026() -> Period {
    Period {
        from_date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        to_date: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
        period_type: crate::report::periodic::schema::PeriodType::CalendarQuarter,
        days_covered: 90,
    }
}

fn plain_window() -> Report {
    make_report(10, 100, 10, &[("svc-a", "/api", 100)], vec![])
}

/// The deltas of the cumulative counter are the period's losses. The
/// first carrying line only sets the baseline (the counter is
/// daemon-lifetime, its absolute value says nothing about this
/// file), and a decrease is a daemon restart: counted as a reset,
/// the delta restarts from the new value.
#[test]
fn drop_counter_deltas_become_windows_dropped() {
    let ts = |m, d| Utc.with_ymd_and_hms(2026, m, d, 0, 0, 0).unwrap();
    let windows = [
        (ts(1, 10), plain_window(), 2),
        (ts(1, 20), plain_window(), 5),
        (ts(2, 10), plain_window(), 5),
        (ts(2, 20), plain_window(), 1),
    ];
    let (_dir, path) = write_chained_archive_with_drops(&windows);
    let inputs = aggregate_from_paths(std::slice::from_ref(&path), &q1_2026(), false).unwrap();
    // Baseline 2, then +3, +0, reset (1 < 5) restarting at 1.
    assert_eq!(inputs.windows_dropped, Some(4));
    assert_eq!(inputs.drop_counter_resets, Some(1));
    assert_eq!(inputs.chain_verified, 4, "drops are covered by the hash");
}

/// The family key drives the drop baseline, and its shape is
/// coupled to `daemon::archive::rotate`'s `{stem}-%Y%m%dT%H%M%S%fZ`.
#[test]
fn archive_family_groups_rotations_and_separates_hosts() {
    let key = |p: &str| archive_family(Path::new(p));
    // A rotation and its active file share a family.
    assert_eq!(
        key("/var/log/archive-20260110T000000000000000Z.ndjson"),
        key("/var/log/archive.ndjson")
    );
    // Two hosts with the same basename do not.
    assert_ne!(
        key("/hosts/a/archive.ndjson"),
        key("/hosts/b/archive.ndjson")
    );
    // A hyphen that is not a stamp stays part of the family name.
    assert_ne!(key("/var/log/host-a.ndjson"), key("/var/log/host-b.ndjson"));
    // A rotation of a hyphenated stem still joins its own family.
    assert_eq!(
        key("/var/log/host-a-20260110T000000000000000Z.ndjson"),
        key("/var/log/host-a.ndjson")
    );
    // A bare filename has no parent and must still yield a key.
    assert_ne!(key("archive.ndjson"), "");
}

#[test]
fn rotation_stamps_are_told_apart_from_ordinary_suffixes() {
    assert!(is_rotation_stamp("20260110T000000000000000Z"));
    assert!(
        !is_rotation_stamp("20260110T000000000000000"),
        "no trailing Z"
    );
    assert!(!is_rotation_stamp("2026Z"), "no T separator");
    assert!(
        !is_rotation_stamp("2026011T000000Z"),
        "date is not 8 digits"
    );
    assert!(
        !is_rotation_stamp("2026011aT000000Z"),
        "date is not all digits"
    );
    assert!(!is_rotation_stamp("20260110TZ"), "empty time");
    assert!(
        !is_rotation_stamp("20260110T00000aZ"),
        "time is not all digits"
    );
    assert!(!is_rotation_stamp("a"), "an ordinary suffix");
}

/// The counter is daemon-lifetime and rotated files sort before the
/// active one, so the delta across a rotation boundary must be kept.
#[test]
fn drop_deltas_survive_a_rotation_boundary() {
    let ts = |m, d| Utc.with_ymd_and_hms(2026, m, d, 0, 0, 0).unwrap();
    let dir = TempDir::new().unwrap();
    // Rotation naming: the stamped file sorts before the active one.
    let rotated = write_drops_file(
        dir.path(),
        "archive-20260110T000000000000000Z.ndjson",
        &[(ts(1, 10), plain_window(), 5)],
    );
    let active = write_drops_file(
        dir.path(),
        "archive.ndjson",
        &[(ts(2, 10), plain_window(), 8)],
    );
    // Passed in reverse order: `resolve_files` sorts them, and that
    // sort makes the cross-rotation delta correct.
    let inputs = aggregate_from_paths(&[active, rotated], &q1_2026(), false).unwrap();
    assert_eq!(inputs.windows_dropped, Some(3));
    assert_eq!(inputs.drop_counter_resets, Some(0));
}

/// `disclose` takes a list of archives. Two hosts' counters are
/// independent, so a lower start in the second must not read as a
/// reset of the first.
#[test]
fn drop_counters_do_not_leak_between_archive_families() {
    let ts = |m, d| Utc.with_ymd_and_hms(2026, m, d, 0, 0, 0).unwrap();
    let dir = TempDir::new().unwrap();
    let host_a = write_drops_file(
        dir.path(),
        "host-a.ndjson",
        &[
            (ts(1, 10), plain_window(), 4),
            (ts(1, 11), plain_window(), 9),
        ],
    );
    // Sorts after host-a and starts lower: a shared baseline would
    // call this a restart and add 5 more drops.
    let host_b = write_drops_file(
        dir.path(),
        "host-b.ndjson",
        &[
            (ts(2, 10), plain_window(), 5),
            (ts(2, 11), plain_window(), 6),
        ],
    );
    let inputs = aggregate_from_paths(&[host_a, host_b], &q1_2026(), false).unwrap();
    assert_eq!(inputs.windows_dropped, Some(6), "5 within a, 1 within b");
    assert_eq!(inputs.drop_counter_resets, Some(0), "no host restarted");
}

/// An out-of-period carrying line must not turn "not measured" into
/// "measured zero" for a period whose own windows carry no counter.
#[test]
fn out_of_period_counter_lines_do_not_claim_zero_drops() {
    let outside = Utc.with_ymd_and_hms(2025, 6, 15, 0, 0, 0).unwrap();
    let inside = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let dir = TempDir::new().unwrap();
    // The period's own window predates the counter. Only an
    // out-of-period line carries one.
    let carrying = write_drops_file(
        dir.path(),
        "archive-20250615T000000000000000Z.ndjson",
        &[(outside, plain_window(), 7)],
    );
    let plain = {
        let path = dir.path().join("archive.ndjson");
        let mut file = File::create(&path).unwrap();
        let envelope = serde_json::json!({ "ts": inside, "report": plain_window() });
        writeln!(file, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
        path
    };
    let inputs = aggregate_from_paths(&[carrying, plain], &q1_2026(), false).unwrap();
    assert_eq!(inputs.windows_dropped, None);
    assert_eq!(inputs.drop_counter_resets, None);
}

/// A pre-v1.7 archive carries no counter: "not measured" must stay
/// distinguishable from "zero drops".
#[test]
fn archives_without_the_counter_yield_no_drop_figures() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let (_dir, path) = write_archive(&[(ts1, plain_window())]);
    let inputs = aggregate_from_paths(std::slice::from_ref(&path), &q1_2026(), false).unwrap();
    assert_eq!(inputs.windows_dropped, None);
    assert_eq!(inputs.drop_counter_resets, None);
}

#[test]
fn an_intact_chain_verifies_and_an_edited_window_breaks_it() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let ts3 = Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap();
    let windows = [
        (ts1, plain_window()),
        (ts2, plain_window()),
        (ts3, plain_window()),
    ];

    let (_dir, path) = write_chained_archive(&windows);
    let clean = aggregate_from_paths(std::slice::from_ref(&path), &q1_2026(), false).unwrap();
    assert_eq!(clean.chain_verified, 3);
    assert_eq!(clean.chain_breaks, 0);
    assert_eq!(clean.chain_unchained, 0);

    // Edit the middle window the way a hand-tuned archive would be.
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(ToString::to_string).collect();
    let mut middle: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    middle["report"]["green_summary"]["io_waste_ratio"] = serde_json::json!(0.01);
    lines[1] = serde_json::to_string(&middle).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    let tampered = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(tampered.chain_breaks, 1, "the edited window must show up");
    assert_eq!(
        tampered.chain_verified, 2,
        "the untouched windows stay attestable"
    );
}

#[test]
fn a_crash_truncated_fragment_is_not_a_break() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let (_dir, path) = write_chained_archive(&[(ts1, plain_window()), (ts2, plain_window())]);
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    // A power loss mid-write leaves a newline-terminated partial line.
    std::fs::write(
        &path,
        format!("{}\n{{\"ts\":\"2026-02-01T\n{}\n", lines[0], lines[1]),
    )
    .unwrap();
    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.chain_breaks, 0, "a dropped window is not tampering");
    assert_eq!(out.chain_verified, 2);
    assert_eq!(out.malformed_lines_skipped, 1);
}

#[test]
fn a_removed_window_breaks_the_chain_and_pre_chain_archives_do_not() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let ts3 = Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap();
    let (_dir, path) = write_chained_archive(&[
        (ts1, plain_window()),
        (ts2, plain_window()),
        (ts3, plain_window()),
    ]);
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    // Drop the middle line: the third no longer points at its predecessor.
    std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.chain_breaks, 1);

    // An archive written before chaining existed is not a break.
    let (_dir2, old) = write_archive(&[(ts1, plain_window()), (ts2, plain_window())]);
    let legacy = aggregate_from_paths(&[old], &q1_2026(), false).unwrap();
    assert_eq!(legacy.chain_unchained, 2);
    assert_eq!(legacy.chain_breaks, 0);
    assert_eq!(legacy.chain_verified, 0);
}

#[test]
fn a_break_revealed_just_after_the_period_still_affects_the_period() {
    let in_period = Utc.with_ymd_and_hms(2026, 3, 30, 0, 0, 0).unwrap();
    let removed = Utc.with_ymd_and_hms(2026, 3, 31, 0, 0, 0).unwrap();
    let after = Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap();
    let (_dir, path) = write_chained_archive(&[
        (in_period, plain_window()),
        (removed, plain_window()),
        (after, plain_window()),
    ]);
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.chain_breaks, 1);
    assert_eq!(out.chain_breaks_outside, 0);
}

#[test]
fn carbon_methodology_and_embodied_are_folded_from_the_windows() {
    // Methodology and embodied values are read from archived windows,
    // not from the config of whoever runs `disclose`.
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let mut with_transport = plain_window();
    if let Some(co2) = with_transport.green_summary.co2.as_mut() {
        co2.total.methodology = "sci_v1_numerator+transport".to_string();
        co2.transport_gco2 = Some(0.05);
        // The real pipeline computes total as operational + embodied +
        // transport, so a window carrying transport carries it in its
        // total too. Adding the term without the total would test an
        // arithmetic the product never produces.
        co2.total.mid += 0.05;
    }
    let (_dir, path) = write_archive(&[(ts1, plain_window()), (ts2, with_transport)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(
        out.carbon_methodologies
            .contains("sci_v1_numerator+transport"),
        "a window counting transport must be visible: {:?}",
        out.carbon_methodologies
    );
    assert_eq!(
        out.carbon_methodologies.len(),
        2,
        "a period spanning legacy and current windows shows both tags"
    );
    // 0.2 gCO2eq of M per window, two windows.
    assert!((out.embodied_gco2_total - 0.4).abs() < 1e-9);

    // The split must add up to the published total, otherwise a reader
    // cannot tell the reducible part from the rest.
    let bd = out.aggregate.carbon_breakdown.expect("breakdown present");
    let sum = bd.operational_kgco2eq.unwrap_or(0.0)
        + bd.embodied_kgco2eq.unwrap_or(0.0)
        + bd.transport_kgco2eq.unwrap_or(0.0);
    assert!(
        (sum - out.aggregate.total_carbon_kgco2eq).abs() < 1e-9,
        "operational {:?} + embodied {:?} + transport {:?} must equal total {}",
        bd.operational_kgco2eq,
        bd.embodied_kgco2eq,
        bd.transport_kgco2eq,
        out.aggregate.total_carbon_kgco2eq
    );
    assert!(
        bd.transport_kgco2eq.is_some_and(|t| t > 0.0),
        "the window counting transport must show up in the split"
    );
}

#[test]
fn stripping_the_hash_field_is_a_break_not_a_pre_chain_line() {
    // After a chained line, a deleted `hash` must not read as
    // "written before chaining existed" (the benign bucket), or an
    // editor could rewrite a window and publish breaks: 0.
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let (_dir, path) = write_chained_archive(&[(ts1, plain_window()), (ts2, plain_window())]);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    lines[1].as_object_mut().unwrap().remove("hash");
    lines[1]["report"]["green_summary"]["io_waste_ratio"] = serde_json::json!(0.01);
    let rewritten: Vec<String> = lines.iter().map(ToString::to_string).collect();
    std::fs::write(&path, rewritten.join("\n") + "\n").unwrap();

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.chain_breaks, 1, "a stripped hash after a chained line");
    assert_eq!(out.chain_unchained, 0, "and it is not filed as benign");
}

#[test]
fn a_removed_run_that_ends_inside_the_file_is_a_break() {
    // `prev` alone cannot see a removed run: the line after it points
    // at a hash that is no longer there, and `seq` jumps too.
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let ts3 = Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap();
    let (_dir, path) = write_chained_archive(&[
        (ts1, plain_window()),
        (ts2, plain_window()),
        (ts3, plain_window()),
    ]);
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.chain_breaks, 1, "the missing middle must surface");
}

#[test]
fn truncating_the_tail_is_invisible_to_the_chain_alone() {
    // Pins the documented limit rather than a capability: what is left
    // after a clean tail cut is a shorter self-consistent chain, and
    // no field inside the file can contradict it. Only an anchor kept
    // outside it can, which `integrity.cross_period_log` reserves.
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let ts3 = Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap();
    let (_dir, path) = write_chained_archive(&[
        (ts1, plain_window()),
        (ts2, plain_window()),
        (ts3, plain_window()),
    ]);
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    std::fs::write(&path, format!("{}\n", lines[0])).unwrap();

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.chain_breaks, 0, "the remaining prefix is consistent");
    assert_eq!(out.chain_verified, 1);
}

#[test]
fn one_stripped_hash_costs_one_break_not_two() {
    // The line after a damaged one chains onto a hash the walk never
    // saw. Counting that as a second break would report two edits for
    // one, and a reader cannot tell an inflated count from a real one.
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let ts3 = Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap();
    let (_dir, path) = write_chained_archive(&[
        (ts1, plain_window()),
        (ts2, plain_window()),
        (ts3, plain_window()),
    ]);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    lines[1].as_object_mut().unwrap().remove("hash");
    let rewritten: Vec<String> = lines.iter().map(ToString::to_string).collect();
    std::fs::write(&path, rewritten.join("\n") + "\n").unwrap();

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.chain_breaks, 1, "one damaged line, one break");
    assert_eq!(out.chain_verified, 2, "the third line re-anchors");
}

#[test]
fn a_break_outside_the_period_is_counted_apart() {
    // One rolling archive can cover years. A window edited in 2025
    // must not be published as a break in the 2026 Q1 disclosure, and
    // the verified count must match what the period folded.
    let old_ts = Utc.with_ymd_and_hms(2025, 6, 15, 0, 0, 0).unwrap();
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let (_dir, path) = write_chained_archive(&[
        (old_ts, plain_window()),
        (ts1, plain_window()),
        (ts2, plain_window()),
    ]);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    lines[0]["report"]["green_summary"]["io_waste_ratio"] = serde_json::json!(0.01);
    let rewritten: Vec<String> = lines.iter().map(ToString::to_string).collect();
    std::fs::write(&path, rewritten.join("\n") + "\n").unwrap();

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.chain_breaks, 0, "the period itself is intact");
    assert_eq!(out.chain_breaks_outside, 1, "the 2025 edit still surfaces");
    assert_eq!(out.chain_verified, 2, "only the period's windows count");
}

#[test]
fn the_applied_coefficients_are_published_and_a_change_shows_both() {
    // They scale every published figure and appear nowhere else, so a
    // period scored under two different coefficients must say so
    // rather than average them into one number.
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let mut first = plain_window();
    first.green_summary.scoring_config = Some(crate::score::carbon::ScoringConfig {
        embodied_per_request_gco2: Some(0.001),
        ..crate::score::carbon::ScoringConfig::default()
    });
    let mut second = plain_window();
    second.green_summary.scoring_config = Some(crate::score::carbon::ScoringConfig {
        embodied_per_request_gco2: Some(0.0001),
        ..crate::score::carbon::ScoringConfig::default()
    });
    let (_dir, path) = write_archive(&[(ts1, first), (ts2, second)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(
        out.scoring_coefficients
            .contains("embodied_gco2_per_request=0.001"),
        "{:?}",
        out.scoring_coefficients
    );
    assert!(
        out.scoring_coefficients
            .contains("embodied_gco2_per_request=0.0001"),
        "a coefficient lowered mid-period must stay visible"
    );
}

#[test]
fn transport_is_omitted_rather_than_zeroed_when_nothing_counted_it() {
    // With `include_network_transport` off, and with it on but no
    // cross-region traffic, the windows are identical. Publishing 0.0
    // would assert a measurement neither case made.
    let ts = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let (_dir, path) = write_archive(&[(ts, plain_window())]);
    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    let bd = out.aggregate.carbon_breakdown.expect("breakdown present");
    assert!(bd.transport_kgco2eq.is_none());
    assert!(
        bd.embodied_kgco2eq.unwrap_or(0.0) > 0.0,
        "the other terms still publish"
    );
}

#[test]
fn transport_bracket_needs_every_window_to_declare_the_fixed_coefficient() {
    use crate::score::carbon::{
        CarbonEstimate, CarbonReport, DEFAULT_NETWORK_ENERGY_PER_BYTE_KWH, ScoringConfig,
    };
    let estimate = || CarbonEstimate {
        low: 0.0,
        mid: 0.0,
        high: 0.0,
        model: String::new(),
        methodology: String::new(),
    };
    let with_transport = |gco2: f64| CarbonReport {
        total: estimate(),
        avoidable: estimate(),
        operational_gco2: 0.0,
        embodied_gco2: 0.0,
        transport_gco2: Some(gco2),
        database_gco2: None,
        messaging_gco2: None,
        sci_per_trace: None,
        functional_unit: String::new(),
    };
    let coefficient = |v: Option<f64>| ScoringConfig {
        network_energy_per_byte_kwh: v,
        ..ScoringConfig::default()
    };

    // A window declaring the fixed coefficient keeps the bracket.
    let mut acc = PeriodState::default();
    acc.fold_transport_coefficient(
        Some(&with_transport(4.0)),
        Some(&coefficient(Some(DEFAULT_NETWORK_ENERGY_PER_BYTE_KWH))),
    );
    assert!(!acc.transport_coefficient_uncertain);

    // A custom one disqualifies the period.
    let mut acc = PeriodState::default();
    acc.fold_transport_coefficient(Some(&with_transport(4.0)), Some(&coefficient(Some(5e-10))));
    assert!(acc.transport_coefficient_uncertain);

    // So does an absent one: pre-0.9.25 windows record no coefficient
    // and could have been scored with anything.
    let mut acc = PeriodState::default();
    acc.fold_transport_coefficient(Some(&with_transport(4.0)), Some(&coefficient(None)));
    assert!(acc.transport_coefficient_uncertain);
    let mut acc = PeriodState::default();
    acc.fold_transport_coefficient(Some(&with_transport(4.0)), None);
    assert!(acc.transport_coefficient_uncertain);

    // A window that contributed no transport says nothing either way.
    let mut acc = PeriodState::default();
    acc.fold_transport_coefficient(Some(&with_transport(0.0)), None);
    assert!(!acc.transport_coefficient_uncertain);

    let mut state = PeriodState {
        operational_gco2_total: 10.0,
        transport_gco2_total: 4.0,
        ..PeriodState::default()
    };
    let with_bracket = build_carbon_breakdown(&state).unwrap();
    assert!(with_bracket.transport_kgco2eq_low.is_some());
    state.transport_coefficient_uncertain = true;
    let without = build_carbon_breakdown(&state).unwrap();
    assert!(
        without.transport_kgco2eq.is_some(),
        "the mid still publishes"
    );
    assert!(without.transport_kgco2eq_low.is_none());
    assert!(without.transport_kgco2eq_high.is_none());
}

#[test]
fn standalone_subsystem_carbon_emits_a_breakdown() {
    for (database_gco2, messaging_gco2) in [(Some(2.0), None), (None, Some(3.0))] {
        let mut state = PeriodState::default();
        state.db_waste.energy_g = database_gco2;
        state.msg_waste.energy_g = messaging_gco2;
        let breakdown = build_carbon_breakdown(&state).expect("subsystem carbon emits a breakdown");

        // Unmeasured terms are omitted, never published as 0.0.
        assert!(breakdown.operational_kgco2eq.is_none());
        assert!(breakdown.embodied_kgco2eq.is_none());

        assert_eq!(
            breakdown.database_kgco2eq_out_of_total,
            database_gco2.map(|g| g / 1000.0)
        );
        assert_eq!(
            breakdown.messaging_kgco2eq_out_of_total,
            messaging_gco2.map(|g| g / 1000.0)
        );
    }
}

#[test]
fn temporal_coverage_counts_distinct_days() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let ts3 = Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap();
    let (_dir, path) = write_archive(&[
        (ts1, plain_window()),
        (ts2, plain_window()),
        (ts3, plain_window()),
    ]);
    let tc = aggregate_from_paths(&[path], &q1_2026(), false)
        .unwrap()
        .aggregate
        .temporal_coverage;
    assert_eq!(tc.observed_days, 3);
    assert_eq!(tc.days_in_period, 90);
    assert!((tc.temporal_coverage - 3.0 / 90.0).abs() < 1e-9);
    // The three days are a month apart, so the gap is large.
    assert!(tc.largest_gap_days > 25, "gap was {}", tc.largest_gap_days);
}

#[test]
fn temporal_coverage_dedups_same_day_windows() {
    let morning = Utc.with_ymd_and_hms(2026, 1, 10, 1, 0, 0).unwrap();
    let evening = Utc.with_ymd_and_hms(2026, 1, 10, 23, 0, 0).unwrap();
    let (_dir, path) = write_archive(&[(morning, plain_window()), (evening, plain_window())]);
    let tc = aggregate_from_paths(&[path], &q1_2026(), false)
        .unwrap()
        .aggregate
        .temporal_coverage;
    assert_eq!(tc.observed_days, 1);
}

#[test]
fn temporal_coverage_buckets_subsecond_near_midnight_by_utc_day() {
    // 23:59:59.500 on Jan 31 and 00:00:00.200 on Feb 1 are distinct days.
    let jan31 = Utc.with_ymd_and_hms(2026, 1, 31, 23, 59, 59).unwrap()
        + chrono::Duration::milliseconds(500);
    let feb1 =
        Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap() + chrono::Duration::milliseconds(200);
    let (_dir, path) = write_archive(&[(jan31, plain_window()), (feb1, plain_window())]);
    let tc = aggregate_from_paths(&[path], &q1_2026(), false)
        .unwrap()
        .aggregate
        .temporal_coverage;
    assert_eq!(tc.observed_days, 2);
}

#[test]
fn aggregator_surfaces_both_waste_tiers() {
    let ts = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    // green_summary avoidable (50) differs from the canonical tier (200),
    // so the assertions prove the disclosure_waste tiers drive the output,
    // not the operational green_summary.
    let mut report = make_report(100, 1_000, 50, &[("svc-a", "/api", 1_000)], vec![]);
    report.disclosure_waste = Some(crate::report::DisclosureWaste {
        database: None,
        messaging: None,
        canonical: crate::report::AvoidableTier {
            n_plus_one_threshold: 2,
            avoidable_io_ops: 200,
            avoidable_kwh: 0.5,
            avoidable_gco2: 300.0,
        },
        operational: crate::report::AvoidableTier {
            n_plus_one_threshold: 5,
            avoidable_io_ops: 50,
            avoidable_kwh: 0.1,
            avoidable_gco2: 80.0,
        },
    });

    let (_dir, path) = write_archive(&[(ts, report)]);
    let agg = aggregate_from_paths(&[path], &q1_2026(), false)
        .unwrap()
        .aggregate;

    assert_eq!(agg.canonical_waste.n_plus_one_threshold, 2);
    assert_eq!(agg.operational_waste.n_plus_one_threshold, 5);
    assert!((agg.canonical_waste.carbon_kgco2eq - 0.3).abs() < 1e-9);
    assert!((agg.operational_waste.carbon_kgco2eq - 0.08).abs() < 1e-9);
    assert!((agg.canonical_waste.energy_kwh - 0.5).abs() < 1e-9);
    assert!((agg.operational_waste.energy_kwh - 0.1).abs() < 1e-9);
    assert!((agg.canonical_waste.waste_ratio - 0.2).abs() < 1e-9);
    assert!((agg.operational_waste.waste_ratio - 0.05).abs() < 1e-9);
    // Flat fields alias the canonical tier.
    assert!(
        (agg.estimated_optimization_potential_kgco2eq - agg.canonical_waste.carbon_kgco2eq).abs()
            < 1e-12
    );
    assert!((agg.aggregate_waste_ratio - agg.canonical_waste.waste_ratio).abs() < 1e-12);
    // No window carried a database block: the aggregate omits it.
    assert!(agg.database_waste.is_none());
}

#[test]
fn aggregator_sums_messaging_waste_and_splits_provenance() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let tier = crate::report::AvoidableTier {
        n_plus_one_threshold: 2,
        avoidable_io_ops: 10,
        avoidable_kwh: 0.1,
        avoidable_gco2: 1.0,
    };
    let mut r1 = make_report(100, 1_000, 50, &[("svc-a", "/api", 1_000)], vec![]);
    r1.disclosure_waste = Some(crate::report::DisclosureWaste {
        canonical: tier.clone(),
        operational: tier.clone(),
        database: None,
        messaging: Some(waste_block(2.0, "broker_specpower")),
    });
    let mut r2 = make_report(100, 1_000, 50, &[("svc-a", "/api", 1_000)], vec![]);
    r2.disclosure_waste = Some(crate::report::DisclosureWaste {
        canonical: tier.clone(),
        operational: tier,
        database: None,
        messaging: Some(waste_block(1.0, "estimated")),
    });

    let (_dir, path) = write_archive(&[(ts1, r1), (ts2, r2)]);
    let agg = aggregate_from_paths(&[path], &q1_2026(), false)
        .unwrap()
        .aggregate;
    assert_eq!(
        agg.carbon_breakdown
            .as_ref()
            .and_then(|b| b.messaging_kgco2eq_out_of_total),
        Some(0.2),
        "estimated fallback carbon is already inside the service total"
    );
    let mw = agg.messaging_waste.expect("messaging block emitted");

    assert!((mw.energy_kwh - 3.0).abs() < 1e-12);
    // A declared cluster is neither measured nor estimated: it is an
    // operator statement about provisioned hardware, and publishing
    // it under `measured_*` would read as a reading of the broker.
    assert!(
        (mw.measured_energy_kwh - 0.0).abs() < 1e-12,
        "no window was measured here"
    );
    assert!((mw.declared_energy_kwh - 2.0).abs() < 1e-12);
    assert_eq!(mw.windows_with_figure, 2);
    assert_eq!(mw.measured_windows, 0);
    assert_eq!(mw.declared_windows, 1);
    assert_eq!(mw.estimated_windows, 1);
    assert_eq!(
        mw.measured_windows + mw.declared_windows + mw.estimated_windows,
        mw.windows_with_figure
    );
    assert_eq!(
        mw.models,
        ["broker_specpower".to_string(), "estimated".to_string()]
            .into_iter()
            .collect()
    );
}

#[test]
fn aggregator_sums_database_waste_across_windows() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let tier = crate::report::AvoidableTier {
        n_plus_one_threshold: 2,
        avoidable_io_ops: 10,
        avoidable_kwh: 0.1,
        avoidable_gco2: 1.0,
    };
    let mut r1 = make_report(100, 1_000, 50, &[("svc-a", "/api", 1_000)], vec![]);
    r1.disclosure_waste = Some(crate::report::DisclosureWaste {
        canonical: tier.clone(),
        operational: tier.clone(),
        database: Some(waste_block(1.0, "alumet_rapl")),
        messaging: None,
    });
    let mut r2 = make_report(100, 1_000, 50, &[("svc-a", "/api", 1_000)], vec![]);
    r2.disclosure_waste = Some(crate::report::DisclosureWaste {
        canonical: tier.clone(),
        operational: tier.clone(),
        database: Some(waste_block(0.5, "estimated")),
        messaging: None,
    });
    // Out-of-spec provenance tag: the whole block is dropped, none
    // of its figures reach the sums.
    let ts3 = Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap();
    let mut r3 = make_report(100, 1_000, 50, &[("svc-a", "/api", 1_000)], vec![]);
    r3.disclosure_waste = Some(crate::report::DisclosureWaste {
        canonical: tier.clone(),
        operational: tier,
        database: Some(waste_block(9.0, "bad tag!")),
        messaging: None,
    });

    let (_dir, path) = write_archive(&[(ts1, r1), (ts2, r2), (ts3, r3)]);
    let agg = aggregate_from_paths(&[path], &q1_2026(), false)
        .unwrap()
        .aggregate;

    assert_eq!(
        agg.carbon_breakdown
            .as_ref()
            .and_then(|b| b.database_kgco2eq_out_of_total),
        Some(0.1),
        "estimated fallback carbon is already inside the service total"
    );

    let db = agg.database_waste.expect("database aggregate");
    assert_eq!(db.windows_with_figure, 2);
    assert!((db.energy_kwh - 1.5).abs() < 1e-12);
    assert!((db.operational_waste_kwh - 0.75).abs() < 1e-12);
    // gCO2 sums are converted to kg: (50 + 25) / 1000.
    assert!((db.operational_waste_kgco2eq.unwrap() - 0.075).abs() < 1e-12);
    assert!((db.canonical_waste_kwh - 1.2).abs() < 1e-12);
    assert!((db.canonical_waste_kgco2eq.unwrap() - 0.12).abs() < 1e-12);
    let models: Vec<&str> = db.models.iter().map(String::as_str).collect();
    assert_eq!(models, vec!["alumet_rapl", "estimated"]);
    // Provenance split: one measured window, one estimated.
    assert!((db.measured_energy_kwh - 1.0).abs() < 1e-12);
    assert_eq!(db.measured_windows, 1);
    assert_eq!(db.estimated_windows, 1);
    assert_eq!(db.windows_with_carbon, 2);
    let breakdown = agg.carbon_breakdown.expect("breakdown");
    assert_eq!(
        breakdown.database_kgco2eq, None,
        "a figure without the flag stays beside the total"
    );
}

const DB_KWH: f64 = 0.4;
const DB_GCO2: f64 = 20.0;
const MSG_KWH: f64 = 0.1;
const MSG_GCO2: f64 = 5.0;

/// A window whose totals hold `app_kwh` of application energy plus an
/// in-total database figure, an in-total broker figure, or both, the way
/// the daemon archives them: the host kWh inside `energy_kwh`, the host
/// gCO2 inside `co2.total` and in its own `co2` field.
fn in_total_window(app_kwh: f64, db: bool, msg: bool) -> Report {
    let mut r = make_report(
        10,
        100,
        5,
        &[("svc-a", "/api", 60), ("svc-b", "/api", 40)],
        vec![],
    );
    let tier = crate::report::AvoidableTier::default();
    let mut waste = crate::report::DisclosureWaste {
        canonical: tier.clone(),
        operational: tier,
        database: None,
        messaging: None,
    };
    let (mut host_kwh, mut host_gco2) = (0.0, 0.0);
    if db {
        r.green_summary.database_waste = Some(
            serde_json::from_value(serde_json::json!({
                "energy_kwh": DB_KWH, "waste_kwh": 0.0, "sql_waste_ratio": 0.0,
                "model": "alumet_rapl", "in_totals": true
            }))
            .unwrap(),
        );
        r.green_summary.co2.as_mut().unwrap().database_gco2 = Some(DB_GCO2);
        waste.database = Some(crate::report::DisclosureDbWaste {
            in_totals: true,
            energy_gco2: Some(DB_GCO2),
            ..waste_block(DB_KWH, "alumet_rapl")
        });
        (host_kwh, host_gco2) = (host_kwh + DB_KWH, host_gco2 + DB_GCO2);
    }
    if msg {
        r.green_summary.messaging_waste = Some(
            serde_json::from_value(serde_json::json!({
                "energy_kwh": MSG_KWH, "waste_kwh": 0.0, "messaging_waste_ratio": 0.0,
                "model": "broker_specpower", "in_totals": true
            }))
            .unwrap(),
        );
        r.green_summary.co2.as_mut().unwrap().messaging_gco2 = Some(MSG_GCO2);
        waste.messaging = Some(crate::report::DisclosureDbWaste {
            in_totals: true,
            energy_gco2: Some(MSG_GCO2),
            ..waste_block(MSG_KWH, "broker_specpower")
        });
        (host_kwh, host_gco2) = (host_kwh + MSG_KWH, host_gco2 + MSG_GCO2);
    }
    r.green_summary.energy_kwh = app_kwh + host_kwh;
    let co2 = r.green_summary.co2.as_mut().unwrap();
    co2.total.mid += host_gco2;
    co2.total.methodology = "sci_v1_numerator+transport".to_string();
    r.disclosure_waste = Some(waste);
    r
}

fn aggregate_windows(lines: &[(DateTime<Utc>, Report)]) -> AggregateInputs {
    let (_dir, path) = write_archive(lines);
    aggregate_from_paths(&[path], &q1_2026(), false).unwrap()
}

#[test]
fn in_total_host_terms_land_inside_the_breakdown() {
    let out = aggregate_windows(&[
        (day(10, 9), in_total_window(1.0, true, false)),
        (day(11, 9), in_total_window(2.0, false, true)),
    ]);
    let agg = &out.aggregate;
    // Each host kWh counted once, inside the window energy.
    assert!((agg.total_energy_kwh - (3.0 + DB_KWH + MSG_KWH)).abs() < 1e-9);
    let b = agg.carbon_breakdown.as_ref().expect("breakdown");
    assert_eq!(b.database_kgco2eq, Some(DB_GCO2 / 1000.0));
    assert_eq!(b.messaging_kgco2eq, Some(MSG_GCO2 / 1000.0));
    assert_eq!(b.database_kgco2eq_out_of_total, None);
    assert_eq!(b.messaging_kgco2eq_out_of_total, None);
    let terms = [
        b.operational_kgco2eq,
        b.embodied_kgco2eq,
        b.transport_kgco2eq,
        b.database_kgco2eq,
        b.messaging_kgco2eq,
    ];
    let sum: f64 = terms.into_iter().flatten().sum();
    assert!((sum - agg.total_carbon_kgco2eq).abs() < 1e-9, "{sum}");
    assert!(out.energy_source_models.contains("alumet_rapl"));
    assert!(out.energy_source_models.contains("broker_specpower"));
    // In-total blocks still feed the waste blocks' provenance split.
    let db = agg.database_waste.as_ref().expect("database block");
    assert_eq!(db.measured_windows, 1);
}

#[test]
fn applications_never_receive_the_host_terms() {
    let host_kwh = DB_KWH + MSG_KWH;
    let host_kg = (DB_GCO2 + MSG_GCO2) / 1000.0;
    // Runtime path: the per-service maps already hold the application part.
    let mut runtime = in_total_window(1.0, true, true);
    let gs = &mut runtime.green_summary;
    gs.per_service_energy_kwh = [("svc-a".to_string(), 0.6), ("svc-b".to_string(), 0.4)].into();
    gs.per_service_carbon_kgco2eq =
        [("svc-a".to_string(), 0.0006), ("svc-b".to_string(), 0.0004)].into();
    // Unattributed path: no I/O per service and no runtime maps.
    let mut unattributed = in_total_window(1.0, true, true);
    unattributed.per_endpoint_io_ops.clear();
    let proxy_share = in_total_window(1.0, true, true);
    for window in [runtime, proxy_share, unattributed] {
        let out = aggregate_windows(&[(day(10, 9), window)]);
        let apps_kwh: f64 = out.per_service.values().map(|s| s.energy_kwh).sum();
        assert!(
            (apps_kwh + host_kwh - out.aggregate.total_energy_kwh).abs() < 1e-9,
            "{apps_kwh} + {host_kwh} vs {}",
            out.aggregate.total_energy_kwh
        );
        if out.runtime_windows == 0 {
            let apps_kg: f64 = out.per_service.values().map(|s| s.carbon_kgco2eq).sum();
            assert!((apps_kg + host_kg - out.aggregate.total_carbon_kgco2eq).abs() < 1e-9);
        }
    }
}

#[test]
fn an_application_part_without_energy_falls_back_to_the_proxy_and_keeps_the_host() {
    let out = aggregate_windows(&[(day(10, 9), in_total_window(0.0, true, false))]);
    let proxy = 100.0 * ENERGY_PER_IO_OP_KWH;
    assert!((out.aggregate.total_energy_kwh - (proxy + DB_KWH)).abs() < 1e-12);
    let apps_kwh: f64 = out.per_service.values().map(|s| s.energy_kwh).sum();
    assert!((apps_kwh - proxy).abs() < 1e-12);
}

#[test]
fn a_rollup_state_without_the_host_terms_reads_and_merges_them() {
    let state = folded(&[(day(10, 9), in_total_window(1.0, true, true))]);
    let mut json = serde_json::to_value(&state).unwrap();
    let map = json.as_object_mut().unwrap();
    assert!(map.remove("database_gco2_total").is_some());
    assert!(map.remove("messaging_gco2_total").is_some());
    let mut older: PeriodState = serde_json::from_value(json).unwrap();
    assert_eq!(older.database_gco2_total, 0.0);
    assert_eq!(older.messaging_gco2_total, 0.0);
    older.merge(state);
    older.merge(folded(&[(day(11, 9), in_total_window(1.0, true, true))]));
    assert!((older.database_gco2_total - 2.0 * DB_GCO2).abs() < 1e-12);
    assert!((older.messaging_gco2_total - 2.0 * MSG_GCO2).abs() < 1e-12);
}

#[test]
fn aggregator_folds_three_windows() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let ts3 = Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap();

    let r1 = make_report(
        100,
        1_000,
        100,
        &[("svc-a", "/api", 600), ("svc-b", "/api", 400)],
        vec![make_finding("svc-a", FindingType::NPlusOneSql, "SELECT *")],
    );
    let r2 = make_report(
        200,
        2_000,
        200,
        &[("svc-a", "/api", 1_200), ("svc-b", "/api", 800)],
        vec![
            make_finding("svc-a", FindingType::NPlusOneSql, "SELECT *"),
            make_finding("svc-b", FindingType::RedundantHttp, "GET /x"),
        ],
    );
    let r3 = make_report(150, 1_500, 150, &[("svc-a", "/other", 1_500)], vec![]);

    let (_dir, path) = write_archive(&[(ts1, r1), (ts2, r2), (ts3, r3)]);
    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();

    assert_eq!(out.windows_aggregated, 3);
    assert_eq!(out.aggregate.total_requests, 100 + 200 + 150);
    assert!(out.aggregate.total_energy_kwh > 0.0);
    // These windows are legacy (no disclosure_waste), so the avoidable
    // figures land only in the operational tier. The canonical tier stays
    // the all-zero default (omitted on the wire, not "100% efficient")
    // rather than being fed legacy data, and the flat aliases stay zero.
    assert!(out.aggregate.operational_waste.waste_ratio > 0.0);
    assert!(out.aggregate.operational_waste.efficiency_score < 100.0);
    assert_eq!(out.aggregate.canonical_waste, WasteTier::default());
    assert!(out.aggregate.aggregate_waste_ratio.abs() < 1e-12);
    assert_eq!(out.aggregate.anti_patterns_detected_count, 3);

    let svc_a = out.per_service.get("svc-a").expect("svc-a missing");
    let svc_b = out.per_service.get("svc-b").expect("svc-b missing");
    assert_eq!(
        svc_a
            .anti_patterns
            .get("n_plus_one_sql")
            .unwrap()
            .occurrences,
        2
    );
    assert_eq!(
        svc_b
            .anti_patterns
            .get("redundant_http")
            .unwrap()
            .occurrences,
        1
    );
    // svc-a saw two endpoints across the windows.
    assert!(svc_a.endpoints_seen.len() >= 2);
}

#[test]
fn aggregate_request_total_does_not_sum_rounded_service_shares() {
    let ts = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let report = make_report(
        1,
        2,
        0,
        &[("svc-a", "/api", 1), ("svc-b", "/api", 1)],
        vec![],
    );
    let (_dir, path) = write_archive(&[(ts, report)]);
    let aggregate = aggregate_from_paths(&[path], &q1_2026(), false)
        .unwrap()
        .aggregate;
    assert_eq!(aggregate.total_requests, 1);
}

#[test]
fn archive_time_range_reports_min_and_max() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 3, 20, 12, 0, 0).unwrap();
    let ts3 = Utc.with_ymd_and_hms(2026, 2, 10, 0, 0, 0).unwrap();
    let r = make_report(10, 100, 5, &[("svc", "/", 100)], vec![]);
    let (_dir, path) = write_archive(&[(ts1, r.clone()), (ts2, r.clone()), (ts3, r)]);

    let range = archive_time_range(&[path])
        .unwrap()
        .expect("non-empty archive");
    assert_eq!(range.0, ts1);
    assert_eq!(range.1, ts2);
}

#[test]
fn archive_time_range_empty_for_no_paths() {
    assert_eq!(archive_time_range(&[]).unwrap(), None);
}

#[test]
fn archive_time_range_skips_malformed_lines() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("archive.ndjson");
    let mut file = File::create(&path).unwrap();
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let r = make_report(10, 100, 0, &[("svc", "/", 100)], vec![]);
    let envelope = serde_json::json!({ "ts": ts, "report": r });
    writeln!(file, "{{ not json").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
    drop(file);

    let range = archive_time_range(&[path])
        .unwrap()
        .expect("one valid window");
    assert_eq!(range, (ts, ts));
}

#[test]
fn aggregator_filters_outside_period() {
    let in_p = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let before = Utc.with_ymd_and_hms(2025, 12, 31, 0, 0, 0).unwrap();
    let after = Utc.with_ymd_and_hms(2026, 4, 1, 12, 0, 0).unwrap();

    let r = make_report(50, 100, 5, &[("svc", "/", 100)], vec![]);
    let (_dir, path) = write_archive(&[(before, r.clone()), (in_p, r.clone()), (after, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.windows_aggregated, 1);
}

#[test]
fn aggregator_skips_malformed_lines() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("archive.ndjson");
    let mut file = File::create(&path).unwrap();
    let r = make_report(10, 100, 0, &[("svc", "/", 100)], vec![]);
    let envelope = serde_json::json!({
        "ts": Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap(),
        "report": r,
    });
    writeln!(file, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
    writeln!(file, "{{ not json").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.windows_aggregated, 2);
    assert_eq!(out.malformed_lines_skipped, 1);
}

#[test]
fn aggregator_errors_when_no_windows_in_period() {
    let outside = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let r = make_report(10, 100, 0, &[("svc", "/", 100)], vec![]);
    let (_dir, path) = write_archive(&[(outside, r)]);

    let err = aggregate_from_paths(&[path], &q1_2026(), false).unwrap_err();
    assert_matches!(err, AggregationError::NoWindowsInPeriod);
}

#[test]
fn aggregator_strict_attribution_errors_on_empty_io() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let r = make_report(10, 100, 0, &[], vec![]);
    let (_dir, path) = write_archive(&[(ts, r)]);

    let err = aggregate_from_paths(&[path], &q1_2026(), true).unwrap_err();
    assert_matches!(err, AggregationError::UnattributedWindow { .. });
}

#[test]
fn aggregator_falls_back_to_unattributed_when_lax() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    // A split finding folds whole into the bucket, no per-service row.
    let mut finding = make_finding("svc-a", FindingType::NPlusOneSql, "SELECT *");
    finding.pattern.occurrences_by_service =
        BTreeMap::from([("svc-a".to_string(), 2), ("svc-b".to_string(), 3)]);
    let r = make_report(20, 100, 5, &[], vec![finding]);
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    let ap = &out.per_service[UNATTRIBUTED_SERVICE].anti_patterns["n_plus_one_sql"];
    assert_eq!((ap.occurrences, ap.avoidable_io_ops), (1, 4));
    assert!(!out.per_service.contains_key("svc-b"));
}

/// A finding whose spans came from two services counts once, on its
/// owner, while its avoidable ops are split: the other service gets
/// its share and its timestamps, and no occurrence, so the global
/// count is unchanged.
#[test]
fn route_findings_splits_avoidable_across_services() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let mut finding = make_finding("svc-a", FindingType::NPlusOneSql, "SELECT *");
    assert_eq!(finding.pattern.occurrences, 5);
    finding.pattern.occurrences_by_service =
        BTreeMap::from([("svc-a".to_string(), 2), ("svc-b".to_string(), 3)]);
    let r = make_report(
        10,
        100,
        4,
        &[("svc-a", "/", 50), ("svc-b", "/", 50)],
        vec![finding],
    );
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    let row = |service: &str| {
        let ap = &out.per_service[service].anti_patterns["n_plus_one_sql"];
        (ap.occurrences, ap.avoidable_io_ops)
    };
    assert_eq!(row("svc-a"), (1, 1));
    assert_eq!(row("svc-b"), (0, 3));
    assert_eq!(out.aggregate.anti_patterns_detected_count, 1);
    let key = ("svc-b".to_string(), "n_plus_one_sql".to_string());
    assert_eq!(*out.first_seen.get(&key).unwrap(), ts);
}

#[test]
fn aggregator_resolves_directory_of_ndjson() {
    let dir = TempDir::new().unwrap();
    let p1 = dir.path().join("a.ndjson");
    let p2 = dir.path().join("b.ndjson");
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let r = make_report(10, 100, 0, &[("svc", "/", 100)], vec![]);
    for p in [&p1, &p2] {
        let mut f = File::create(p).unwrap();
        let env = serde_json::json!({ "ts": ts, "report": r });
        writeln!(f, "{}", serde_json::to_string(&env).unwrap()).unwrap();
    }

    let out = aggregate_from_paths(&[dir.path().to_path_buf()], &q1_2026(), false).unwrap();
    assert_eq!(out.windows_aggregated, 2);
    assert_eq!(out.source_files.len(), 2);
}

#[test]
fn aggregator_tracks_first_and_last_seen() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 3, 25, 0, 0, 0).unwrap();
    let r1 = make_report(
        10,
        100,
        10,
        &[("svc", "/", 100)],
        vec![make_finding("svc", FindingType::NPlusOneSql, "SELECT *")],
    );
    let r2 = r1.clone();
    let (_dir, path) = write_archive(&[(ts1, r1), (ts2, r2)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    let key = ("svc".to_string(), "n_plus_one_sql".to_string());
    assert_eq!(*out.first_seen.get(&key).unwrap(), ts1);
    assert_eq!(*out.last_seen.get(&key).unwrap(), ts2);
}

fn make_runtime_report(
    services: &[(&str, &str, usize)],
    per_service_carbon: &[(&str, f64)],
    per_service_energy: &[(&str, f64)],
    per_service_region: &[(&str, &str)],
    energy_kwh: f64,
    energy_model: &str,
) -> Report {
    let mut r = make_report(10, 100, 5, services, vec![]);
    r.green_summary.energy_kwh = energy_kwh;
    r.green_summary.energy_model = energy_model.to_string();
    r.green_summary.per_service_carbon_kgco2eq = per_service_carbon
        .iter()
        .map(|(s, v)| ((*s).to_string(), *v))
        .collect();
    r.green_summary.per_service_energy_kwh = per_service_energy
        .iter()
        .map(|(s, v)| ((*s).to_string(), *v))
        .collect();
    r.green_summary.per_service_region = per_service_region
        .iter()
        .map(|(s, r)| ((*s).to_string(), (*r).to_string()))
        .collect();
    r
}

#[test]
fn aggregator_uses_runtime_attribution_when_present() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let r = make_runtime_report(
        &[("svc-low", "/api", 100), ("svc-high", "/api", 100)],
        &[("svc-low", 0.005), ("svc-high", 0.500)],
        &[("svc-low", 0.001), ("svc-high", 0.001)],
        &[("svc-low", "eu-west-3"), ("svc-high", "pl")],
        0.002,
        "scaphandre_rapl",
    );
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.runtime_windows, 1);
    assert_eq!(out.fallback_windows, 0);
    assert!(
        (out.aggregate.total_energy_kwh - 0.002).abs() < 1e-12,
        "runtime energy must replace the proxy"
    );
    assert!((out.aggregate.period_coverage - 1.0).abs() < f64::EPSILON);
    assert_eq!(out.aggregate.runtime_windows_count, 1);
    assert_eq!(out.aggregate.fallback_windows_count, 0);
    let low = out.per_service.get("svc-low").expect("svc-low");
    let high = out.per_service.get("svc-high").expect("svc-high");
    assert!((low.carbon_kgco2eq - 0.005).abs() < 1e-12);
    assert!((high.carbon_kgco2eq - 0.500).abs() < 1e-12);
    assert!(out.energy_source_models.contains("scaphandre_rapl"));
}

#[test]
fn aggregator_falls_back_to_proxy_for_legacy_archives() {
    // make_report leaves the per-service maps empty and energy_kwh
    // at zero, mirroring an archive without runtime energy attribution.
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let r = make_report(10, 100, 5, &[("svc", "/", 100)], vec![]);
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.runtime_windows, 0);
    assert_eq!(out.fallback_windows, 1);
    assert!(out.energy_source_models.is_empty());
    // Proxy energy = 100 ops * 1e-7 kWh.
    assert!((out.aggregate.total_energy_kwh - 100.0 * 1e-7).abs() < 1e-12);
    assert!(out.aggregate.period_coverage.abs() < f64::EPSILON);
    assert_eq!(out.aggregate.runtime_windows_count, 0);
    assert_eq!(out.aggregate.fallback_windows_count, 1);
}

#[test]
fn aggregator_mixed_archive_per_window_strategy() {
    let ts_legacy = Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
    let ts_runtime = Utc.with_ymd_and_hms(2026, 2, 10, 0, 0, 0).unwrap();
    let legacy = make_report(10, 100, 5, &[("svc-a", "/", 100)], vec![]);
    let runtime = make_runtime_report(
        &[("svc-b", "/", 50)],
        &[("svc-b", 0.020)],
        &[("svc-b", 0.0005)],
        &[("svc-b", "eu-west-3")],
        0.0005,
        "cloud_specpower+cal",
    );
    let (_dir, path) = write_archive(&[(ts_legacy, legacy), (ts_runtime, runtime)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.runtime_windows, 1);
    assert_eq!(out.fallback_windows, 1);
    // `+cal` suffix is stripped in the collected set.
    assert!(out.energy_source_models.contains("cloud_specpower"));
    assert!(!out.energy_source_models.iter().any(|m| m.ends_with("+cal")));
    assert!((out.aggregate.period_coverage - 0.5).abs() < f64::EPSILON);
    assert_eq!(out.aggregate.runtime_windows_count, 1);
    assert_eq!(out.aggregate.fallback_windows_count, 1);
    // Invariant: coverage × total ≈ runtime count.
    let total = out.aggregate.runtime_windows_count + out.aggregate.fallback_windows_count;
    let derived = out.aggregate.period_coverage * total as f64;
    assert!(
        (derived - out.aggregate.runtime_windows_count as f64).abs() < f64::EPSILON,
        "period_coverage × total = {derived} should match runtime count {}",
        out.aggregate.runtime_windows_count
    );
}

#[test]
fn aggregator_clamps_negative_energy_and_carbon_from_tampered_archive() {
    // JSON allows negative numbers. A tampered archive could carry
    // them to skew the period downward. Without the clamp, per-service
    // sums would go negative and propagate to `total_energy_kwh`.
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let r = make_runtime_report(
        &[("svc-a", "/", 100)],
        &[("svc-a", -1.0e10), ("svc-b", -0.5)],
        &[("svc-a", -1.0), ("svc-b", -2.0)],
        &[("svc-a", "eu-west-3"), ("svc-b", "pl")],
        -1.0e6,
        "scaphandre_rapl",
    );
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    // Per-service clamp exercised here: every negative input maps to 0.
    let svc_a = out.per_service.get("svc-a").expect("svc-a");
    assert!((svc_a.carbon_kgco2eq - 0.0).abs() < f64::EPSILON);
    assert!((svc_a.energy_kwh - 0.0).abs() < f64::EPSILON);
    let svc_b = out.per_service.get("svc-b").expect("svc-b");
    assert!((svc_b.carbon_kgco2eq - 0.0).abs() < f64::EPSILON);
    assert!((svc_b.energy_kwh - 0.0).abs() < f64::EPSILON);
    // Negative `energy_kwh` was rejected by the `> 0.0` check, so the
    // proxy fallback ran: 100 ops × 1e-7 kWh = 1e-5.
    assert!((out.aggregate.total_energy_kwh - 100.0 * 1e-7).abs() < 1e-12);
}

#[test]
fn aggregator_caps_per_service_cardinality() {
    // A tampered archive carrying MAX_SERVICES + N distinct service
    // strings must not balloon `per_service`. Overflow services are
    // silently dropped, existing services keep accumulating.
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let overflow = 32_usize;
    let services_raw: Vec<(String, f64, f64, String)> = (0..(MAX_SERVICES + overflow))
        .map(|i| {
            (
                format!("svc-{i:05}"),
                0.001,
                0.0001,
                "eu-west-3".to_string(),
            )
        })
        .collect();
    let services: Vec<(&str, &str, usize)> = services_raw
        .iter()
        .map(|(s, _, _, _)| (s.as_str(), "/", 1))
        .collect();
    let carbon: Vec<(&str, f64)> = services_raw
        .iter()
        .map(|(s, c, _, _)| (s.as_str(), *c))
        .collect();
    let energy: Vec<(&str, f64)> = services_raw
        .iter()
        .map(|(s, _, e, _)| (s.as_str(), *e))
        .collect();
    let regions: Vec<(&str, &str)> = services_raw
        .iter()
        .map(|(s, _, _, r)| (s.as_str(), r.as_str()))
        .collect();
    let r = make_runtime_report(
        &services,
        &carbon,
        &energy,
        &regions,
        0.0001,
        "scaphandre_rapl",
    );
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(out.per_service.len() <= MAX_SERVICES);
    assert_eq!(out.windows_aggregated, 1);
}

#[test]
fn aggregator_rejects_oversize_energy_model_strings() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let oversize = "x".repeat(1024);
    let r = make_runtime_report(
        &[("svc", "/", 10)],
        &[("svc", 0.001)],
        &[("svc", 0.0001)],
        &[("svc", "eu-west-3")],
        0.0001,
        &oversize,
    );
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(
        out.energy_source_models.is_empty(),
        "oversize energy_model strings must not enter the set"
    );
}

#[test]
fn aggregator_caps_distinct_energy_models() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let mut reports = Vec::new();
    for i in 0..(MAX_ENERGY_MODELS + 20) {
        let model = format!("model_{i:04}");
        let r = make_runtime_report(
            &[("svc", "/", 10)],
            &[("svc", 0.001)],
            &[("svc", 0.0001)],
            &[("svc", "eu-west-3")],
            0.0001,
            &model,
        );
        let offset = i64::try_from(i).expect("test bound");
        reports.push((ts + chrono::Duration::seconds(offset), r));
    }
    let (_dir, path) = write_archive(&reports);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    // Fed 84 distinct models, cap is 64. Set must saturate at the cap.
    assert_eq!(out.energy_source_models.len(), MAX_ENERGY_MODELS);
}

#[test]
fn aggregator_collects_single_binary_version() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let mut r = make_report(10, 100, 5, &[("svc", "/", 100)], vec![]);
    r.binary_version = "0.6.2".to_string();
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.aggregate.binary_versions.len(), 1);
    assert!(out.aggregate.binary_versions.contains("0.6.2"));
}

#[test]
fn aggregator_collects_distinct_binary_versions_in_mixed_archive() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 10, 0, 0, 0).unwrap();
    let mut r1 = make_report(10, 100, 5, &[("svc-a", "/", 100)], vec![]);
    r1.binary_version = "0.6.2".to_string();
    let mut r2 = make_report(10, 100, 5, &[("svc-b", "/", 50)], vec![]);
    r2.binary_version = "0.6.3".to_string();
    let (_dir, path) = write_archive(&[(ts1, r1), (ts2, r2)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.aggregate.binary_versions.len(), 2);
    assert!(out.aggregate.binary_versions.contains("0.6.2"));
    assert!(out.aggregate.binary_versions.contains("0.6.3"));
}

#[test]
fn aggregator_skips_empty_binary_version_from_legacy_archive() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let r = make_report(10, 100, 5, &[("svc", "/", 100)], vec![]);
    // make_report leaves binary_version as String::new()
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(out.aggregate.binary_versions.is_empty());
}

#[test]
fn aggregator_rejects_oversize_binary_version_strings() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let mut r = make_report(10, 100, 5, &[("svc", "/", 100)], vec![]);
    r.binary_version = "x".repeat(MAX_BINARY_VERSION_LEN + 1);
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(out.aggregate.binary_versions.is_empty());
}

#[test]
fn aggregator_detects_calibration_when_cal_suffix_present() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let r = make_runtime_report(
        &[("svc", "/", 10)],
        &[("svc", 0.001)],
        &[("svc", 0.0001)],
        &[("svc", "eu-west-3")],
        0.0001,
        "io_proxy_v3+cal",
    );
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(out.calibration_applied);
    // Bare model is collected without the +cal suffix.
    assert!(out.energy_source_models.contains("io_proxy_v3"));
}

#[test]
fn aggregator_sets_calibration_from_the_flag_behind_a_measured_tag() {
    // A real-time or measured window tag drops `+cal`, so only
    // `energy_calibrated` says the modeled services were calibrated.
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    for window_tag in ["electricity_maps_api", "scaphandre_rapl"] {
        let mut r = make_runtime_report(
            &[("svc", "/", 10)],
            &[("svc", 0.001)],
            &[("svc", 0.0001)],
            &[("svc", "eu-west-3")],
            0.0001,
            window_tag,
        );
        r.green_summary.energy_calibrated = true;
        let (_dir, path) = write_archive(&[(ts, r)]);
        let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
        assert!(out.calibration_applied, "{window_tag}");
        assert!(out.energy_source_models.contains(window_tag));
    }
}

#[test]
fn aggregator_does_not_set_calibration_when_no_cal_suffix() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let r = make_runtime_report(
        &[("svc", "/", 10)],
        &[("svc", 0.001)],
        &[("svc", 0.0001)],
        &[("svc", "eu-west-3")],
        0.0001,
        "scaphandre_rapl",
    );
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(!out.calibration_applied);
}

#[test]
fn aggregator_collects_per_service_energy_models_single_window() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let mut r = make_runtime_report(
        &[("svc-a", "/", 10), ("svc-b", "/", 10)],
        &[("svc-a", 0.001), ("svc-b", 0.001)],
        &[("svc-a", 0.0001), ("svc-b", 0.0001)],
        &[("svc-a", "eu-west-3"), ("svc-b", "eu-west-3")],
        0.0002,
        "scaphandre_rapl",
    );
    r.green_summary
        .per_service_energy_model
        .insert("svc-a".to_string(), "scaphandre_rapl".to_string());
    r.green_summary
        .per_service_energy_model
        .insert("svc-b".to_string(), "io_proxy_v3".to_string());
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    let map = &out.aggregate.per_service_energy_models;
    assert_eq!(map.len(), 2);
    assert!(map.get("svc-a").unwrap().contains("scaphandre_rapl"));
    assert!(map.get("svc-b").unwrap().contains("io_proxy_v3"));
}

#[test]
fn aggregator_merges_per_service_energy_models_across_windows() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 10, 0, 0, 0).unwrap();
    let mut r1 = make_runtime_report(
        &[("svc", "/", 10)],
        &[("svc", 0.001)],
        &[("svc", 0.0001)],
        &[("svc", "eu-west-3")],
        0.0001,
        "io_proxy_v3",
    );
    r1.green_summary
        .per_service_energy_model
        .insert("svc".to_string(), "io_proxy_v3".to_string());
    let mut r2 = make_runtime_report(
        &[("svc", "/", 10)],
        &[("svc", 0.001)],
        &[("svc", 0.0001)],
        &[("svc", "eu-west-3")],
        0.0001,
        "scaphandre_rapl",
    );
    r2.green_summary
        .per_service_energy_model
        .insert("svc".to_string(), "scaphandre_rapl".to_string());
    let (_dir, path) = write_archive(&[(ts1, r1), (ts2, r2)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    let set = out.aggregate.per_service_energy_models.get("svc").unwrap();
    assert_eq!(set.len(), 2);
    assert!(set.contains("io_proxy_v3"));
    assert!(set.contains("scaphandre_rapl"));
}

#[test]
fn aggregator_strips_cal_suffix_from_per_service_energy_models() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let mut r = make_runtime_report(
        &[("svc", "/", 10)],
        &[("svc", 0.001)],
        &[("svc", 0.0001)],
        &[("svc", "eu-west-3")],
        0.0001,
        "io_proxy_v3+cal",
    );
    r.green_summary
        .per_service_energy_model
        .insert("svc".to_string(), "io_proxy_v3+cal".to_string());
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    let set = out.aggregate.per_service_energy_models.get("svc").unwrap();
    assert!(set.contains("io_proxy_v3"));
    assert!(!set.iter().any(|m| m.ends_with("+cal")));
}

#[test]
fn aggregator_per_service_measured_ratio_means_across_windows() {
    // Three windows with the same service at ratios 0.5, 0.8, 0.3.
    // Period-level mean: (0.5 + 0.8 + 0.3) / 3 = 0.533...
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 10, 0, 0, 0).unwrap();
    let ts3 = Utc.with_ymd_and_hms(2026, 3, 10, 0, 0, 0).unwrap();
    let make = |ratio: f64| {
        let mut r = make_runtime_report(
            &[("svc", "/", 10)],
            &[("svc", 0.001)],
            &[("svc", 0.0001)],
            &[("svc", "eu-west-3")],
            0.0001,
            "scaphandre_rapl",
        );
        r.green_summary
            .per_service_measured_ratio
            .insert("svc".to_string(), ratio);
        r
    };
    let (_dir, path) = write_archive(&[(ts1, make(0.5)), (ts2, make(0.8)), (ts3, make(0.3))]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    let mean = out
        .aggregate
        .per_service_measured_ratio
        .get("svc")
        .copied()
        .expect("ratio entry");
    let expected = (0.5 + 0.8 + 0.3) / 3.0;
    assert!(
        (mean - expected).abs() < 1e-9,
        "expected mean {expected}, got {mean}"
    );
}

#[test]
fn aggregator_per_service_measured_ratio_clamps_out_of_range_symmetrically() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let mut r = make_runtime_report(
        &[("svc", "/", 10)],
        &[("svc", 0.001)],
        &[("svc", 0.0001)],
        &[("svc", "eu-west-3")],
        0.0001,
        "scaphandre_rapl",
    );
    // Negative maps to 0.0 (sanitize_f64), overshoot to 1.0 (.min(1.0)).
    // Symmetric: both produce a mean entry instead of dropping.
    r.green_summary
        .per_service_measured_ratio
        .insert("svc-neg".to_string(), -0.5);
    r.green_summary
        .per_service_measured_ratio
        .insert("svc-over".to_string(), 1.5);
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(
        out.aggregate.per_service_measured_ratio.get("svc-neg"),
        Some(&0.0)
    );
    assert_eq!(
        out.aggregate.per_service_measured_ratio.get("svc-over"),
        Some(&1.0)
    );
}

#[test]
fn aggregator_per_service_energy_models_empty_for_legacy_archive() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    // make_report leaves the per-service map empty.
    let r = make_report(10, 100, 5, &[("svc", "/", 100)], vec![]);
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(out.aggregate.per_service_energy_models.is_empty());
}

#[test]
fn aggregator_calibration_sticky_when_only_one_window_has_cal() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 10, 0, 0, 0).unwrap();
    let r1 = make_runtime_report(
        &[("svc", "/", 10)],
        &[("svc", 0.001)],
        &[("svc", 0.0001)],
        &[("svc", "eu-west-3")],
        0.0001,
        "io_proxy_v3",
    );
    let r2 = make_runtime_report(
        &[("svc", "/", 10)],
        &[("svc", 0.001)],
        &[("svc", 0.0001)],
        &[("svc", "eu-west-3")],
        0.0001,
        "io_proxy_v3+cal",
    );
    let (_dir, path) = write_archive(&[(ts1, r1), (ts2, r2)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(out.calibration_applied);
}

#[test]
fn aggregator_rejects_invalid_binary_version_pattern() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let mut r = make_report(10, 100, 5, &[("svc", "/", 100)], vec![]);
    // Control char + arbitrary UTF-8: must be rejected by the
    // boundary check, no entry in the period-level set.
    r.binary_version = "0.6.2\u{0001}\u{00e9}".to_string();
    let (_dir, path) = write_archive(&[(ts, r)]);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert!(out.aggregate.binary_versions.is_empty());
}

/// A mixed period is the unguarded case: `fold_tier` takes the max of
/// the thresholds, so official validation passes while the canonical
/// tier under-reports by the legacy windows' share.
#[test]
fn legacy_windows_are_counted_not_silently_folded() {
    let ts1 = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let tier = crate::report::AvoidableTier {
        n_plus_one_threshold: crate::detect::n_plus_one::DISCLOSURE_N_PLUS_ONE_THRESHOLD,
        avoidable_io_ops: 10,
        avoidable_kwh: 0.1,
        avoidable_gco2: 1.0,
    };
    let mut canonical = make_report(100, 1_000, 50, &[("svc-a", "/api", 1_000)], vec![]);
    canonical.disclosure_waste = Some(crate::report::DisclosureWaste {
        canonical: tier.clone(),
        operational: tier,
        database: None,
        messaging: None,
    });
    // Second window keeps `disclosure_waste: None`, the legacy shape.
    let legacy = make_report(100, 1_000, 50, &[("svc-a", "/api", 1_000)], vec![]);

    let (_dir, path) = write_archive(&[(ts1, canonical), (ts2, legacy)]);
    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();

    assert_eq!(out.windows_aggregated, 2);
    assert_eq!(
        out.legacy_waste_windows, 1,
        "the window without a canonical figure must be counted"
    );
}

#[test]
fn aggregator_caps_distinct_binary_versions() {
    let ts = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let mut reports = Vec::new();
    for i in 0..(MAX_BINARY_VERSIONS + 5) {
        let mut r = make_report(10, 100, 5, &[("svc", "/", 100)], vec![]);
        r.binary_version = format!("0.6.{i}");
        let offset = i64::try_from(i).expect("test bound");
        reports.push((ts + chrono::Duration::seconds(offset), r));
    }
    let (_dir, path) = write_archive(&reports);

    let out = aggregate_from_paths(&[path], &q1_2026(), false).unwrap();
    assert_eq!(out.aggregate.binary_versions.len(), MAX_BINARY_VERSIONS);
}

/// A database or broker waste block whose figures scale with `energy`.
fn waste_block(energy: f64, model: &str) -> crate::report::DisclosureDbWaste {
    crate::report::DisclosureDbWaste {
        energy_kwh: energy,
        model: model.to_string(),
        operational_waste_kwh: energy * 0.5,
        operational_waste_gco2: Some(energy * 50.0),
        canonical_waste_kwh: energy * 0.8,
        canonical_waste_gco2: Some(energy * 80.0),
        energy_gco2: Some(energy * 100.0),
        ..Default::default()
    }
}

#[test]
fn an_archive_starting_before_the_period_reaches_its_start() {
    // A quiet first week: no window inside the period before mid-January,
    // but the archive holds December lines, so nothing was pruned.
    let (_dir, path) = write_archive(&[
        (
            Utc.with_ymd_and_hms(2025, 12, 20, 9, 0, 0).unwrap(),
            plain_window(),
        ),
        (
            Utc.with_ymd_and_hms(2026, 1, 15, 9, 0, 0).unwrap(),
            plain_window(),
        ),
    ]);
    let period = q1_2026();
    let inputs = aggregate_from_paths(&[path], &period, false).unwrap();
    assert_eq!(
        inputs.earliest_archived_window,
        Some(Utc.with_ymd_and_hms(2025, 12, 20, 9, 0, 0).unwrap())
    );
    assert!(archive_reaches_period_start(
        inputs.earliest_archived_window,
        &period
    ));
}

#[test]
fn a_pruned_archive_does_not_reach_the_period_start() {
    // What rotation leaves after pruning a busy quarter: only its last weeks.
    let (_dir, path) = write_archive(&[
        (
            Utc.with_ymd_and_hms(2026, 3, 10, 9, 0, 0).unwrap(),
            plain_window(),
        ),
        (
            Utc.with_ymd_and_hms(2026, 3, 30, 9, 0, 0).unwrap(),
            plain_window(),
        ),
    ]);
    let period = q1_2026();
    let inputs = aggregate_from_paths(&[path], &period, false).unwrap();
    assert!(!archive_reaches_period_start(
        inputs.earliest_archived_window,
        &period
    ));
}

#[test]
fn a_window_on_the_first_day_reaches_the_period_start() {
    let period = q1_2026();
    let first_day = Utc.with_ymd_and_hms(2026, 1, 1, 23, 59, 0).unwrap();
    assert!(archive_reaches_period_start(Some(first_day), &period));
    let second_day = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
    assert!(!archive_reaches_period_start(Some(second_day), &period));
    assert!(!archive_reaches_period_start(None, &period));
}

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
    let mut merged = folded(std::slice::from_ref(&a));
    merged.merge(folded(std::slice::from_ref(&b)));
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
    bucket.anti_patterns.insert(
        "not_a_pattern".to_string(),
        AntiPatternAccumulator::default(),
    );
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
    let mut with_waste = make_report(
        40,
        400,
        30,
        &[("svc-a", "/api", 300), ("svc-b", "/b", 100)],
        vec![],
    );
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
            assert_eq!(
                x.keys().collect::<Vec<_>>(),
                y.keys().collect::<Vec<_>>(),
                "{at}"
            );
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
    assert_eq!(
        windows.earliest_archived_window,
        rollups.earliest_archived_window
    );
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
fn a_rollup_covers_the_windows_it_holds_and_no_more() {
    let pool = window_pool();
    let dir = TempDir::new().unwrap();
    let rolled = vec![(day(10, 9), pool[0].clone()), (day(11, 9), pool[1].clone())];
    // The window archive beside the rollups also holds a window at 15:00
    // on day 11 the rollup never folded, as after a crash's last snapshot,
    // and day 12, which has no rollup yet.
    let mut windows: Vec<_> = rolled.iter().map(|(ts, r)| (*ts, r.clone(), 0)).collect();
    windows.push((day(11, 15), pool[2].clone(), 0));
    windows.push((day(12, 9), pool[3].clone(), 0));
    let mut paths = vec![write_drops_file(dir.path(), "archive.ndjson", &windows)];
    paths.extend(write_rollups(dir.path(), "archive", &rolled));
    let mixed = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();

    let only = TempDir::new().unwrap();
    let alone = write_drops_file(only.path(), "archive.ndjson", &windows);
    let expected = aggregate_from_paths(&[alone], &q1_2026(), false).unwrap();
    assert_eq!(mixed.windows_aggregated, 4);
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
    let kept: Vec<&str> = text
        .lines()
        .enumerate()
        .filter(|(i, _)| *i != 1)
        .map(|(_, l)| l)
        .collect();
    std::fs::write(&paths[0], kept.join("\n") + "\n").unwrap();
    let agg = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();
    assert_eq!(agg.chain_breaks, 1);
    assert_eq!(agg.windows_aggregated, 3);
}

#[test]
fn the_rollup_chain_runs_across_monthly_files() {
    let pool = window_pool();
    let dir = TempDir::new().unwrap();
    let lines = vec![
        (day(30, 9), pool[0].clone()),
        (day(31, 9), pool[0].clone()),
        (
            Utc.with_ymd_and_hms(2026, 2, 1, 9, 0, 0).unwrap(),
            pool[0].clone(),
        ),
    ];
    let paths = write_rollups(dir.path(), "archive", &lines);
    assert_eq!(paths.len(), 2);
    let agg = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();
    assert_eq!((agg.chain_breaks, agg.chain_verified), (0, 3));
}

#[test]
fn strict_attribution_refuses_a_rollup_holding_an_unattributed_window() {
    let dir = TempDir::new().unwrap();
    let paths = write_rollups(
        dir.path(),
        "archive",
        &[(day(10, 9), make_report(5, 50, 0, &[], vec![]))],
    );
    let err = aggregate_from_paths(&paths, &q1_2026(), true).unwrap_err();
    assert_matches!(err, AggregationError::UnattributedWindow { .. });
}

#[test]
fn a_rollup_cannot_claim_days_other_than_its_own() {
    let dir = TempDir::new().unwrap();
    let mut acc = DayAccumulator::default();
    acc.observe(day(10, 9), &plain_window());
    let mut closed = acc.take().unwrap();
    closed
        .state
        .insert_observed_day_for_test(day(20, 0).date_naive());
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

#[test]
fn one_hosts_rollup_does_not_silence_another_hosts_windows() {
    let pool = window_pool();
    let host_a = TempDir::new().unwrap();
    let host_b = TempDir::new().unwrap();
    let mut paths = write_rollups(host_a.path(), "archive", &[(day(10, 9), pool[0].clone())]);
    paths.push(write_drops_file(
        host_b.path(),
        "archive.ndjson",
        &[(day(10, 10), pool[0].clone(), 0)],
    ));
    let agg = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();
    assert_eq!(agg.windows_aggregated, 2);
}

#[test]
fn windows_outside_the_rollups_span_still_count_on_its_day() {
    // Upgrade day: the old binary archived 08:00, the new one rolls up
    // 12:00 and 13:00, which the window lines also hold.
    let pool = window_pool();
    let dir = TempDir::new().unwrap();
    let windows = [(day(10, 8), 0), (day(10, 12), 1), (day(10, 13), 3)]
        .map(|(ts, i)| (ts, pool[i].clone(), 0));
    let mut paths = vec![write_drops_file(dir.path(), "archive.ndjson", &windows)];
    paths.extend(write_rollups(
        dir.path(),
        "archive",
        &[
            (day(10, 12), pool[1].clone()),
            (day(10, 13), pool[3].clone()),
        ],
    ));
    let agg = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();
    assert_eq!(agg.windows_aggregated, 3);
}

#[test]
fn removing_the_periods_last_rollup_day_is_an_in_period_break() {
    let pool = window_pool();
    let dir = TempDir::new().unwrap();
    let lines: Vec<_> = (10..14).map(|d| (day(d, 9), pool[0].clone())).collect();
    let paths = write_rollups(dir.path(), "archive", &lines);
    let text = std::fs::read_to_string(&paths[0]).unwrap();
    let kept: Vec<&str> = text
        .lines()
        .enumerate()
        .filter(|(i, _)| *i != 2)
        .map(|(_, l)| l)
        .collect();
    std::fs::write(&paths[0], kept.join("\n") + "\n").unwrap();
    let period = Period {
        from_date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        to_date: NaiveDate::from_ymd_opt(2026, 1, 12).unwrap(),
        period_type: crate::report::periodic::schema::PeriodType::Custom,
        days_covered: 12,
    };
    let agg = aggregate_from_paths(&paths, &period, false).unwrap();
    assert_eq!((agg.chain_breaks, agg.chain_breaks_outside), (1, 0));
}

/// One window on each of two days, in one chained archive file.
fn two_chained_rollups(dir: &Path) -> Vec<PathBuf> {
    let pool = window_pool();
    write_rollups(
        dir,
        "archive",
        &[(day(10, 9), pool[0].clone()), (day(11, 9), pool[0].clone())],
    )
}

#[test]
fn a_rollup_file_with_a_crash_fragment_first_is_still_a_rollup_file() {
    let dir = TempDir::new().unwrap();
    let paths = two_chained_rollups(dir.path());
    let text = std::fs::read_to_string(&paths[0]).unwrap();
    std::fs::write(&paths[0], format!("{{\"format\":\"perf-sent\n{text}")).unwrap();
    let agg = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();
    assert_eq!(agg.windows_aggregated, 2);
    assert_eq!(agg.malformed_lines_skipped, 1);
}

#[test]
fn a_rollup_line_stripped_of_its_hash_is_a_break_even_first() {
    let dir = TempDir::new().unwrap();
    let paths = two_chained_rollups(dir.path());
    let text = std::fs::read_to_string(&paths[0]).unwrap();
    let mut lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    lines[0].as_object_mut().unwrap().remove("hash");
    let rewritten: Vec<String> = lines.iter().map(ToString::to_string).collect();
    std::fs::write(&paths[0], rewritten.join("\n") + "\n").unwrap();
    let agg = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();
    assert_eq!(agg.chain_breaks, 1);
}

#[test]
fn a_hashless_line_ahead_of_a_chained_archive_does_not_reach_back_for_it() {
    // One line without a hash, dated before the period, put in front of a
    // chained archive that only starts in March: it must not stand in for
    // the pruned windows, nor vanish from the integrity counters' view.
    let (_dir, path) = write_chained_archive(&[(
        Utc.with_ymd_and_hms(2026, 3, 10, 9, 0, 0).unwrap(),
        plain_window(),
    )]);
    let chained = std::fs::read_to_string(&path).unwrap();
    let forged = serde_json::json!({
        "ts": Utc.with_ymd_and_hms(2025, 12, 1, 0, 0, 0).unwrap(),
        "report": plain_window(),
    });
    std::fs::write(&path, format!("{forged}\n{chained}")).unwrap();
    let period = q1_2026();
    let inputs = aggregate_from_paths(&[path], &period, false).unwrap();
    assert_eq!(
        inputs.earliest_archived_window,
        Some(Utc.with_ymd_and_hms(2026, 3, 10, 9, 0, 0).unwrap())
    );
    assert!(!archive_reaches_period_start(
        inputs.earliest_archived_window,
        &period
    ));
}

#[test]
fn a_hashless_rollup_line_ahead_of_the_chain_does_not_reach_back_for_it() {
    let pool = window_pool();
    let dir = TempDir::new().unwrap();
    let paths = write_rollups(dir.path(), "archive", &[(day(20, 9), pool[0].clone())]);
    let mut acc = DayAccumulator::default();
    acc.observe(
        Utc.with_ymd_and_hms(2025, 12, 1, 9, 0, 0).unwrap(),
        &pool[0],
    );
    let mut forged: serde_json::Value =
        serde_json::from_str(&rollup::serialize_rollup(&acc.take().unwrap(), "x", 0, 0).unwrap())
            .unwrap();
    forged.as_object_mut().unwrap().remove("hash");
    let chained = std::fs::read_to_string(&paths[0]).unwrap();
    std::fs::write(&paths[0], format!("{forged}\n{chained}")).unwrap();
    let agg = aggregate_from_paths(&paths, &q1_2026(), false).unwrap();
    assert_eq!(agg.earliest_archived_window, Some(day(20, 9)));
}
