//! `tempo` subcommand: exit-code contract for local argument-validation
//! and fetch failures, which don't need a live Tempo backend to trigger.

#![cfg(feature = "tempo")]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

/// The id the stub search hands back and the stub trace carries. Hex
/// only, so the fetch's own trace-id validation accepts it.
const STUB_TRACE_ID: [u8; 16] = [0xa1; 16];
const STUB_TRACE_ID_HEX: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";

/// Append a protobuf varint.
fn push_varint(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let byte = u8::try_from(n & 0x7f).expect("seven bits fit a byte");
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Append a length-delimited protobuf field (wire type 2).
fn push_bytes_field(out: &mut Vec<u8>, field: u32, payload: &[u8]) {
    push_varint(out, (u64::from(field) << 3) | 2);
    push_varint(out, u64::try_from(payload.len()).expect("body fits u64"));
    out.extend_from_slice(payload);
}

/// Append a fixed64 protobuf field (wire type 1).
fn push_fixed64(out: &mut Vec<u8>, field: u32, value: u64) {
    push_varint(out, (u64::from(field) << 3) | 1);
    out.extend_from_slice(&value.to_le_bytes());
}

/// Append a varint protobuf field (wire type 0), which is how an enum travels.
fn push_varint_field(out: &mut Vec<u8>, field: u32, value: u64) {
    push_varint(out, u64::from(field) << 3);
    push_varint(out, value);
}

/// `SpanKind` as `opentelemetry_proto::tonic::trace::v1::span::SpanKind`
/// numbers them, checked against that enum the way every field number in this
/// encoder was checked against its message. A span carrying `db.system` is kept
/// whatever its kind, so marking these correctly changes no assertion today. It
/// stops the stub from serving UNSPECIFIED spans no backend sends, and it is
/// what a reader copying this shape needs.
const SPAN_KIND_SERVER: u64 = 2;
const SPAN_KIND_CLIENT: u64 = 3;

/// One OTLP string attribute: `KeyValue { key, AnyValue { string_value } }`.
fn otlp_attr(key: &str, value: &str) -> Vec<u8> {
    let mut any = Vec::new();
    push_bytes_field(&mut any, 1, value.as_bytes());
    let mut kv = Vec::new();
    push_bytes_field(&mut kv, 1, key.as_bytes());
    push_bytes_field(&mut kv, 2, &any);
    kv
}

/// One OTLP `Span`. Span ids are a repeated byte so a single seed names
/// each of them.
fn otlp_span(
    span_id: u8,
    parent: Option<u8>,
    name: &str,
    kind: u64,
    start_ns: u64,
    end_ns: u64,
    attrs: &[Vec<u8>],
) -> Vec<u8> {
    let mut span = Vec::new();
    push_bytes_field(&mut span, 1, &STUB_TRACE_ID);
    push_bytes_field(&mut span, 2, &[span_id; 8]);
    if let Some(parent) = parent {
        push_bytes_field(&mut span, 4, &[parent; 8]);
    }
    push_bytes_field(&mut span, 5, name.as_bytes());
    push_varint_field(&mut span, 6, kind);
    push_fixed64(&mut span, 7, start_ns);
    push_fixed64(&mut span, 8, end_ns);
    for attr in attrs {
        push_bytes_field(&mut span, 9, attr);
    }
    span
}

/// The `ExportTraceServiceRequest` the stub serves for `/api/traces/{id}`:
/// one routed root over six sibling SELECTs, the n+1 shape the jaeger
/// fixture carries. Hand-encoded because the CLI test crate has no prost
/// dependency to build it from the generated types.
fn otlp_trace_body() -> Vec<u8> {
    const ROOT_NS: u64 = 1_720_621_921_000_000_000;

    let mut scope_spans = Vec::new();
    push_bytes_field(
        &mut scope_spans,
        2,
        &otlp_span(
            0x0a,
            None,
            "OrderService::create_order",
            SPAN_KIND_SERVER,
            ROOT_NS,
            ROOT_NS + 50_000_000,
            &[
                otlp_attr("http.route", "POST /api/orders/42/submit"),
                otlp_attr("code.function", "OrderService::create_order"),
            ],
        ),
    );
    for i in 1..=6u64 {
        let start = ROOT_NS + i * 1_000_000;
        push_bytes_field(
            &mut scope_spans,
            2,
            &otlp_span(
                u8::try_from(i).expect("child index fits a byte"),
                Some(0x0a),
                "db.query",
                SPAN_KIND_CLIENT,
                start,
                start + 800_000,
                &[
                    otlp_attr(
                        "db.statement",
                        &format!("SELECT * FROM order_item WHERE order_id = {i}"),
                    ),
                    otlp_attr("db.system", "postgresql"),
                ],
            ),
        );
    }

    let mut resource = Vec::new();
    push_bytes_field(&mut resource, 1, &otlp_attr("service.name", "order-svc"));

    let mut resource_spans = Vec::new();
    push_bytes_field(&mut resource_spans, 1, &resource);
    push_bytes_field(&mut resource_spans, 2, &scope_spans);

    let mut request = Vec::new();
    push_bytes_field(&mut request, 1, &resource_spans);
    request
}

/// Serve the two hops a tempo search makes, then stop: `/api/search`
/// answers JSON trace ids, `/api/traces/{id}` answers OTLP protobuf.
/// Every response closes its connection, so one accept loop covers the
/// pair whether or not the client reuses the socket. Returns the port.
fn spawn_tempo_stub(search_body: String, trace_body: Vec<u8>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();

    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            if serve_one_hop(&mut stream, &search_body, &trace_body) {
                return;
            }
        }
    });

    port
}

/// Answer one request, and say whether the stub is done. The trace hop is
/// the last one the client makes, and an unreadable request line means the
/// peer went away, so both end the accept loop.
fn serve_one_hop(stream: &mut TcpStream, search_body: &str, trace_body: &[u8]) -> bool {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return true;
    }
    // Drain the headers, otherwise the client can see a reset before it
    // reads the response.
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
        line.clear();
    }

    let is_trace = request_line.contains("/api/traces/");
    let (body, content_type): (&[u8], &str) = if is_trace {
        (trace_body, "application/protobuf")
    } else {
        (search_body.as_bytes(), "application/json")
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
    is_trace
}

#[test]
fn cli_tempo_missing_trace_id_and_service_exits_tooling_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["tempo", "--endpoint", "http://127.0.0.1:1"])
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--trace-id or --service"),
        "stderr should name the missing flag, got: {stderr}"
    );
    // Argument validation, never a quality-gate breach. See docs/CI.md
    // "Exit codes".
    assert_eq!(
        output.status.code(),
        Some(75),
        "bad invocation must exit EXIT_TOOLING_ERROR (75), not 1"
    );
}

#[test]
fn cli_tempo_invalid_lookback_exits_tooling_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "order-svc",
            "--lookback",
            "not-a-duration",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(!output.status.success());
    assert_eq!(
        output.status.code(),
        Some(75),
        "unparsable --lookback must exit EXIT_TOOLING_ERROR (75), not 1"
    );
}

#[test]
fn cli_tempo_fetch_failure_exits_tooling_error() {
    // Port 1 is a privileged port nothing listens on. The fetch fails
    // fast with a connection error, no live Tempo backend needed.
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "order-svc",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error fetching traces from Tempo"),
        "stderr should name the fetch failure, got: {stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(75),
        "a fetch/network failure must exit EXIT_TOOLING_ERROR (75), not 1"
    );
}

#[test]
fn cli_tempo_absolute_window_conflicts_with_lookback() {
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "order-svc",
            "--lookback",
            "2h",
            "--from",
            "2026-08-20T15:00:00Z",
            "--to",
            "2026-08-20T16:00:00Z",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--lookback") && stderr.contains("cannot be used with"),
        "stderr should name the conflict, got: {stderr}"
    );
}

#[test]
fn cli_tempo_absolute_window_conflicts_with_trace_id() {
    // A trace ID resolves to exactly one trace, so a window would be read
    // and silently dropped rather than applied.
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--trace-id",
            "abc123def456",
            "--from",
            "2026-08-20T15:00:00Z",
            "--to",
            "2026-08-20T16:00:00Z",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "stderr should name the conflict, got: {stderr}"
    );
}

#[test]
fn cli_tempo_from_requires_to() {
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "order-svc",
            "--from",
            "2026-08-20T15:00:00Z",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--to"),
        "stderr should name the missing half of the pair, got: {stderr}"
    );
}

#[test]
fn cli_tempo_inverted_absolute_window_exits_tooling_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "order-svc",
            "--from",
            "2026-08-20T16:00:00Z",
            "--to",
            "2026-08-20T15:00:00Z",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--from 2026-08-20T16:00:00Z --to 2026-08-20T15:00:00Z"),
        "stderr should echo the window the operator typed, got: {stderr}"
    );
    // The window is refused before any request is issued, so this is an
    // invocation error like an unparsable --lookback, not a fetch failure.
    assert_eq!(
        output.status.code(),
        Some(75),
        "an inverted window must exit EXIT_TOOLING_ERROR (75), not 1"
    );
}

#[test]
fn cli_tempo_absolute_window_reaches_the_fetch() {
    // Nothing listens on port 1, so getting as far as a connection error
    // proves the window parsed and the request was issued.
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "order-svc",
            "--from",
            "2026-08-20T15:00:00Z",
            "--to",
            "2026-08-20T16:00:00Z",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error fetching traces from Tempo"),
        "an absolute window must reach the fetch, got: {stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(75),
        "a fetch/network failure must exit EXIT_TOOLING_ERROR (75), not 1"
    );
}

#[test]
fn cli_tempo_rejects_max_traces_past_the_shared_ceiling() {
    // Parse-time, no server involved: the bound is the client's, shared
    // with jaeger-query, and must refuse before any request is issued.
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "svc",
            "--max-traces",
            "999999",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    assert_eq!(output.status.code(), Some(2), "clap usage error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("1..=10000"),
        "the refusal names the range, got: {stderr}"
    );
}

#[test]
fn cli_tempo_rejects_zero_max_traces() {
    // The bottom of the same range: a search for zero traces is a
    // typo, refused before the subcommand runs.
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "svc",
            "--max-traces",
            "0",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    assert_eq!(output.status.code(), Some(2), "clap usage error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--max-traces") && stderr.contains("1..=10000"),
        "the refusal names the flag and the range, got: {stderr}"
    );
}

#[test]
fn cli_tempo_rejects_max_traces_one_past_the_ceiling() {
    // 10001 is the first refused value, the boundary a far-out figure
    // like 999999 never pins down.
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "svc",
            "--max-traces",
            "10001",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    assert_eq!(output.status.code(), Some(2), "clap usage error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--max-traces") && stderr.contains("1..=10000"),
        "the refusal names the flag and the range, got: {stderr}"
    );
}

#[test]
fn cli_tempo_max_traces_at_the_ceiling_reaches_the_fetch() {
    // 10000 is the last accepted value. Nothing listens on port 1, so
    // failing on the connection proves the validator let it through.
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "svc",
            "--max-traces",
            "10000",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error fetching traces from Tempo"),
        "an in-range --max-traces must reach the fetch, got: {stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(75),
        "a fetch/network failure must exit EXIT_TOOLING_ERROR (75), not a clap usage error"
    );
}

/// `tempo` against a closed port with this `--fetch-concurrency`.
fn tempo_with_fetch_concurrency(value: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            "http://127.0.0.1:1",
            "--service",
            "svc",
            "--fetch-concurrency",
            value,
        ])
        .output()
        .expect("failed to execute perf-sentinel")
}

#[test]
fn cli_tempo_rejects_fetch_concurrency_outside_its_range() {
    // The core clamps silently, so only this refusal tells the operator.
    for value in ["0", "65"] {
        let output = tempo_with_fetch_concurrency(value);

        assert_eq!(output.status.code(), Some(2), "clap usage error on {value}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--fetch-concurrency") && stderr.contains("1..=64"),
            "the refusal names the flag and the range, got: {stderr}"
        );
    }
}

#[test]
fn cli_tempo_fetch_concurrency_at_its_bounds_reaches_the_fetch() {
    // Nothing listens on port 1, so failing on the connection proves the
    // validator let the value through.
    for value in ["1", "64"] {
        let output = tempo_with_fetch_concurrency(value);

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Error fetching traces from Tempo"),
            "an in-range --fetch-concurrency must reach the fetch, got: {stderr}"
        );
        assert_eq!(output.status.code(), Some(75), "tooling error on {value}");
    }
}

/// Answer every connection on its own thread and report the most hops held
/// at once. A hop leaves the count before its response goes out, so a client
/// fetching one at a time can never read as two. Returns the port and the peak.
fn spawn_tempo_peak_stub(search_body: String, trace_body: Vec<u8>) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let peak_out = Arc::clone(&peak);

    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let (in_flight, peak) = (Arc::clone(&in_flight), Arc::clone(&peak));
            let (search_body, trace_body) = (search_body.clone(), trace_body.clone());
            thread::spawn(move || {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(300));
                in_flight.fetch_sub(1, Ordering::SeqCst);
                serve_one_hop(&mut stream, &search_body, &trace_body);
            });
        }
    });

    (port, peak_out)
}

/// Run a three-trace search with `[tempo] fetch_concurrency = 1` in the config
/// and `extra_args` on the command line, and return the peak the stub saw.
fn peak_with_config_cap_of_one(extra_args: &[&str]) -> usize {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("perf-sentinel.toml");
    std::fs::write(&config, "[tempo]\nfetch_concurrency = 1\n").expect("write config");
    let search = r#"{"traces":[{"traceID":"a1"},{"traceID":"b2"},{"traceID":"c3"}]}"#;
    let (port, peak) = spawn_tempo_peak_stub(search.to_string(), otlp_trace_body());

    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            &format!("http://127.0.0.1:{port}"),
            "--service",
            "order-svc",
            "--no-acknowledgments",
            "--format",
            "json",
            "--config",
        ])
        .arg(&config)
        .args(extra_args)
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    peak.load(Ordering::SeqCst)
}

#[test]
fn cli_tempo_reads_fetch_concurrency_from_the_config() {
    assert_eq!(peak_with_config_cap_of_one(&[]), 1);
}

#[test]
fn cli_tempo_fetch_concurrency_flag_overrides_the_config() {
    let peak = peak_with_config_cap_of_one(&["--fetch-concurrency", "3"]);
    assert!(
        peak > 1,
        "the flag must win over the config, peak was {peak}"
    );
}

#[test]
fn cli_tempo_json_carries_the_findings_spans() {
    // Mirrors the jaeger-query test of this contract, over tempo's two hops:
    // the search hands back an id and the trace fetch answers OTLP
    // protobuf. The JSON travels without its input, so it still has to
    // carry the spans of the traces its findings point at.
    let search = format!(r#"{{"traces":[{{"traceID":"{STUB_TRACE_ID_HEX}"}}]}}"#);
    let port = spawn_tempo_stub(search, otlp_trace_body());

    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--endpoint",
            &format!("http://127.0.0.1:{port}"),
            "--service",
            "order-svc",
            "--no-acknowledgments",
            "--format",
            "json",
        ])
        .output()
        .expect("failed to execute perf-sentinel");

    assert!(
        output.status.success(),
        "the stubbed two-hop fetch must analyze cleanly, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout parses as JSON");
    assert!(
        !report["findings"]
            .as_array()
            .expect("findings array")
            .is_empty(),
        "the stub trace must yield a finding for a trace to be embedded"
    );
    let embedded = report["embedded_traces"]
        .as_array()
        .expect("embedded_traces array present");
    assert_eq!(
        embedded.len(),
        1,
        "the one trace the findings point at travels with the report"
    );
    assert_eq!(embedded[0]["trace_id"], STUB_TRACE_ID_HEX);
    assert!(
        !embedded[0]["spans"]
            .as_array()
            .expect("spans array")
            .is_empty(),
        "an embedded trace without spans draws nothing"
    );
}

#[test]
fn cli_tempo_help_mentions_sort() {
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["tempo", "--help"])
        .output()
        .expect("failed to execute perf-sentinel");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--sort"), "help mentions --sort");
}

/// Serves `reads` from the search and answers the trace reads as it says: the
/// real body for an id it maps to true, a 500 otherwise.
fn tempo_report_with_reads(reads: &'static [(&'static str, bool)]) -> serde_json::Value {
    let ids: Vec<String> = reads
        .iter()
        .map(|(id, _)| format!(r#"{{"traceID":"{id}"}}"#))
        .collect();
    let search_body = format!(r#"{{"traces":[{}]}}"#, ids.join(","));
    let trace_body = otlp_trace_body();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let answers = reads
                .iter()
                .any(|(id, ok)| *ok && request_line.contains(&format!("/api/traces/{id}")));
            let (status, body): (&str, &[u8]) = if request_line.contains("/api/search") {
                ("200 OK", search_body.as_bytes())
            } else if answers {
                ("200 OK", &trace_body)
            } else {
                ("500 Internal", b"")
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
    });

    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--service",
            "stub",
            "--no-acknowledgments",
            "--format",
            "json",
            "--endpoint",
        ])
        .arg(format!("http://127.0.0.1:{port}"))
        .output()
        .expect("run the binary");
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).expect("a JSON report")
}

#[test]
fn cli_tempo_report_says_when_a_read_failed() {
    let report = tempo_report_with_reads(&[
        (STUB_TRACE_ID_HEX, true),
        ("b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2", false),
    ]);
    let partial: Vec<_> = report["warning_details"]
        .as_array()
        .expect("warning_details")
        .iter()
        .filter(|warning| warning["kind"] == "partial_fetch")
        .collect();
    assert_eq!(partial.len(), 1, "{report}");
    let message = partial[0]["message"].as_str().expect("message");
    assert!(
        message.starts_with("1 of 2 trace reads failed"),
        "{message}"
    );
}

#[test]
fn cli_tempo_report_stays_silent_when_every_read_answers() {
    let report = tempo_report_with_reads(&[(STUB_TRACE_ID_HEX, true)]);
    let details = report["warning_details"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        details
            .iter()
            .all(|warning| warning["kind"] != "partial_fetch"),
        "{report}"
    );
}

#[test]
fn cli_rejects_a_memory_limit_below_the_floor() {
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["--memory-limit-mib", "63", "tempo", "--service", "stub"])
        // `--endpoint` is required: without it clap exits 2 whatever the floor.
        .args(["--endpoint", "http://127.0.0.1:1"])
        .output()
        .expect("run the binary");
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

/// A hostname takes a resolver thread, and a released binary's allocator
/// reserves 1 GiB at startup: both have to fit under a cap well below that.
#[test]
fn cli_tempo_resolves_a_hostname_under_a_memory_limit() {
    let search = format!(r#"{{"traces":[{{"traceID":"{STUB_TRACE_ID_HEX}"}}]}}"#);
    let port = spawn_tempo_stub(search, otlp_trace_body());
    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args([
            "tempo",
            "--service",
            "stub",
            "--no-acknowledgments",
            "--format",
            "json",
        ])
        .args(["--memory-limit-mib", "512", "--endpoint"])
        .arg(format!("http://localhost:{port}"))
        .output()
        .expect("run the binary");
    assert!(output.status.success(), "{output:?}");
}

/// Eight 60 MiB bodies in flight cannot fit in 128 MiB: the process fails on
/// its own allocation, which is the point of the flag.
#[cfg(target_os = "linux")]
#[test]
fn cli_tempo_aborts_on_its_own_past_the_memory_limit() {
    const TRACES: usize = 8;
    let ids: Vec<String> = (0..TRACES)
        .map(|n| format!(r#"{{"traceID":"{n:032x}"}}"#))
        .collect();
    let search_body = format!(r#"{{"traces":[{}]}}"#, ids.join(","));
    let trace_body = Arc::new(vec![0_u8; 60 * 1024 * 1024]);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let search_body = search_body.clone();
            let trace_body = Arc::clone(&trace_body);
            thread::spawn(move || serve_one_hop(&mut stream, &search_body, &trace_body));
        }
    });

    let output = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["--memory-limit-mib", "128", "tempo", "--service", "stub"])
        .args(["--no-acknowledgments", "--format", "json", "--endpoint"])
        .arg(format!("http://127.0.0.1:{port}"))
        .output()
        .expect("run the binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("memory allocation"), "{stderr}");
}

/// A signal mid-run keeps what already answered, and the report has to say the
/// rest was never read. The stub answers one trace and holds the other open,
/// and the child's debug log says when the first one landed.
#[cfg(unix)]
#[test]
fn cli_tempo_report_says_when_a_signal_cut_the_reads() {
    const HELD: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
    let search_body =
        format!(r#"{{"traces":[{{"traceID":"{STUB_TRACE_ID_HEX}"}},{{"traceID":"{HELD}"}}]}}"#);
    let trace_body = otlp_trace_body();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let mut peek = [0_u8; 256];
            let read = stream.peek(&mut peek).unwrap_or(0);
            if String::from_utf8_lossy(&peek[..read]).contains(HELD) {
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(60));
                    drop(stream);
                });
                continue;
            }
            serve_one_hop(&mut stream, &search_body, &trace_body);
        }
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["tempo", "--service", "stub", "--no-acknowledgments"])
        .args(["--format", "json", "--endpoint"])
        .arg(format!("http://127.0.0.1:{port}"))
        .env("RUST_LOG", "sentinel_core::ingest::tempo=debug")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run the binary");
    // Held open to the end: a closed pipe would make the child's next log line
    // panic, which is not what this test is about.
    let mut stderr = BufReader::new(child.stderr.take().expect("stderr")).lines();
    let fetched = stderr
        .by_ref()
        .map_while(Result::ok)
        .any(|line| line.contains("Fetched trace"));
    assert!(fetched, "the first trace never landed");

    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("run kill");
    assert!(killed.success());
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "{output:?}");

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("a JSON report");
    let partial = report["warning_details"]
        .as_array()
        .expect("warning_details")
        .iter()
        .find(|warning| warning["kind"] == "partial_fetch")
        .expect("a partial_fetch warning");
    let message = partial["message"].as_str().expect("message");
    assert!(
        message.starts_with("A shutdown signal cancelled 1 of 2"),
        "{message}"
    );
}
