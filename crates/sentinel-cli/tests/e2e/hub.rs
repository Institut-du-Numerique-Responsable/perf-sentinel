//! `perf-sentinel login`, `logout` and `hub` against a scripted mock Hub.
//!
//! The mock answers one scripted response per connection on
//! `127.0.0.1:0` and records each request (line, headers, body, time).
//! Every run points the token file at a fresh temporary directory, so the
//! developer's own sign-in is never read or written.

#![cfg(feature = "daemon")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

struct Seen {
    line: String,
    headers: Vec<(String, String)>,
    body: String,
    at: Instant,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Serve `script` in order, one response per connection, then stop.
fn spawn_hub(script: Vec<(u16, String)>) -> (String, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for (status, body) in script {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let mut headers = Vec::new();
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                if let Some((n, v)) = h.split_once(':') {
                    headers.push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
                }
            }
            let len = headers
                .iter()
                .find(|(n, _)| n == "content-length")
                .and_then(|(_, v)| v.parse::<usize>().ok())
                .unwrap_or(0);
            let mut buf = vec![0u8; len];
            let _ = reader.read_exact(&mut buf);
            let _ = tx.send(Seen {
                line: line.trim_end().to_string(),
                headers,
                body: String::from_utf8_lossy(&buf).into_owned(),
                at: Instant::now(),
            });
            let location = if (300..400).contains(&status) {
                "Location: https://sso.example.com/authorize\r\n"
            } else {
                ""
            };
            let response = format!(
                "HTTP/1.1 {status} X\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (base, rx)
}

/// The next request the mock saw. Fails instead of hanging when the CLI
/// never made it.
fn next(rx: &mpsc::Receiver<Seen>) -> Seen {
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the CLI never sent the expected request")
}

/// A port nothing listens on.
fn dead_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

fn run(config_home: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"));
    cmd.args(args)
        .stdin(Stdio::null())
        .env("XDG_CONFIG_HOME", config_home)
        .env("APPDATA", config_home)
        .env_remove("PERF_SENTINEL_HUB_URL")
        .env_remove("PERF_SENTINEL_HUB_TOKEN");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn perf-sentinel")
}

fn token_path(config_home: &Path) -> PathBuf {
    config_home.join("perf-sentinel").join("hub-token.json")
}

/// A token file as `login` leaves it, owner-only.
fn store_token(config_home: &Path, hub: &str, token: &str, expires_at_ms: i64) {
    let path = token_path(config_home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(r#"{{"hub_url":"{hub}","token":"{token}","expires_at_ms":{expires_at_ms}}}"#),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

const FAR_FUTURE_MS: i64 = 4_102_444_800_000;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture")
}

const DEVICE: &str = r#"{"device_code":"dc-123","user_code":"BCDF-GHJK","verification_uri":"https://elsewhere.example/device","expires_in":600,"interval":0}"#;

fn oauth_error(code: &str) -> (u16, String) {
    (400, format!(r#"{{"error":"{code}"}}"#))
}

#[test]
fn login_runs_the_device_flow_and_stores_an_owner_only_token() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![
        (200, DEVICE.replace(r#""interval":0"#, r#""interval":1"#)),
        oauth_error("authorization_pending"),
        oauth_error("slow_down"),
        (
            200,
            r#"{"access_token":"pshub_s3cret","token_type":"Bearer","expires_in":3600}"#
                .to_string(),
        ),
    ]);
    let out = run(home.path(), &["login", "--hub", &base], &[]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "stdout={stdout} stderr={stderr}");
    // The address the user types is the Hub's own, never the one the answer names.
    assert!(stdout.contains(&format!("Open {base}/device")), "{stdout}");
    assert!(stdout.contains("BCDF-GHJK"), "{stdout}");
    assert!(!stdout.contains("elsewhere.example"), "{stdout}");
    assert!(
        stdout.contains(&format!("Signed in to {base} until")),
        "{stdout}"
    );
    assert!(!stdout.contains("pshub_s3cret") && !stderr.contains("pshub_s3cret"));

    let seen: Vec<Seen> = rx.try_iter().collect();
    assert_eq!(seen.len(), 4);
    assert_eq!(seen[0].line, "POST /auth/device HTTP/1.1");
    assert_eq!(
        seen[0].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(seen[0].body, "client_id=perf-sentinel");
    for poll in &seen[1..] {
        assert_eq!(poll.line, "POST /auth/device/token HTTP/1.1");
        assert!(poll.body.contains("device_code=dc-123"), "{}", poll.body);
        assert!(
            poll.body.contains("client_id=perf-sentinel"),
            "{}",
            poll.body
        );
        assert!(
            poll.body
                .contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"),
            "{}",
            poll.body
        );
        assert!(poll.header("authorization").is_none());
    }
    // The server's interval of 1 s holds after authorization_pending...
    let pending = seen[2].at - seen[1].at;
    assert!(
        pending >= Duration::from_secs(1) && pending < Duration::from_secs(5),
        "{pending:?}"
    );
    // ...and slow_down adds 5 s to it.
    assert!(seen[3].at - seen[2].at >= Duration::from_secs(6));

    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(token_path(home.path())).unwrap()).unwrap();
    assert_eq!(stored["hub_url"], base.as_str());
    assert_eq!(stored["token"], "pshub_s3cret");
    assert!(stored["expires_at_ms"].as_i64().unwrap() > 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&token_path(home.path())), 0o600);
        assert_eq!(mode(token_path(home.path()).parent().unwrap()), 0o700);
    }
}

#[test]
fn login_stops_on_a_denied_or_expired_code() {
    for (code, sentence) in [
        ("access_denied", "denied"),
        ("expired_token", "expired"),
        ("invalid_grant", "no longer knows"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let (base, _rx) = spawn_hub(vec![(200, DEVICE.to_string()), oauth_error(code)]);
        let out = run(home.path(), &["login", "--hub", &base], &[]);
        assert_eq!(out.status.code(), Some(1), "{code}");
        assert!(
            text(&out.stderr).contains(sentence),
            "{code}: {}",
            text(&out.stderr)
        );
        assert!(!token_path(home.path()).exists(), "{code}");
    }
}

#[test]
fn login_gives_up_when_the_code_expires_on_the_client() {
    let home = tempfile::tempdir().unwrap();
    // The Hub never decides; once the script ends, polls fail to connect.
    let device = DEVICE.replace(r#""expires_in":600"#, r#""expires_in":1"#);
    let mut script = vec![(200, device)];
    script.extend((0..3).map(|_| oauth_error("authorization_pending")));
    let (base, _rx) = spawn_hub(script);
    let started = Instant::now();
    let out = run(home.path(), &["login", "--hub", &base], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("expired"),
        "{}",
        text(&out.stderr)
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!token_path(home.path()).exists());
}

#[test]
fn login_reports_a_hub_without_terminal_sign_in() {
    for status in [302, 401, 404] {
        let home = tempfile::tempdir().unwrap();
        let (base, _rx) = spawn_hub(vec![(status, String::new())]);
        let out = run(home.path(), &["login", "--hub", &base], &[]);
        assert_eq!(out.status.code(), Some(1), "{status}");
        assert!(
            text(&out.stderr).contains("This Hub does not offer terminal sign-in"),
            "{status}: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn login_refuses_plain_http_to_another_host() {
    let home = tempfile::tempdir().unwrap();
    let out = run(
        home.path(),
        &["login", "--hub", "http://hub.example.com"],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("https"), "{}", text(&out.stderr));
}

#[test]
fn hub_findings_without_a_credential_sends_no_authorization() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![(200, fixture("hub_findings.json"))]);
    let out = run(
        home.path(),
        &["hub", "--hub", &base, "findings", "--status", "active"],
        &[],
    );
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let seen = next(&rx);
    assert_eq!(
        seen.line,
        "GET /api/findings?limit=50&status=active HTTP/1.1"
    );
    assert!(seen.header("authorization").is_none());
    // The Hub envelope renders through the daemon renderer.
    assert!(stdout.contains("order-svc"), "{stdout}");
    assert!(stdout.contains("GET /orders"), "{stdout}");
}

#[test]
fn hub_findings_relays_the_window_paging_and_signature() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![(200, fixture("hub_findings.json"))]);
    let out = run(
        home.path(),
        &[
            "hub",
            "--hub",
            &base,
            "findings",
            "--from",
            "2026-10-21T00:00:00Z",
            "--to",
            "1793145600000",
            "--offset",
            "100",
            "--include-acked",
            "false",
            "--signature",
            "n_plus_one_sql:svc:GET /x:abc",
        ],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        next(&rx).line,
        "GET /api/findings?limit=50&from=1792540800000&to=1793145600000&offset=100&include_acked=false&signature=n_plus_one_sql%3Asvc%3AGET%20%2Fx%3Aabc HTTP/1.1"
    );
}

#[test]
fn hub_findings_refuses_from_after_to() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![(200, fixture("hub_findings.json"))]);
    let out = run(
        home.path(),
        &[
            "hub",
            "--hub",
            &base,
            "findings",
            "--from",
            "2026-10-28T00:00:00Z",
            "--to",
            "2026-10-21T00:00:00Z",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("--from") && stderr.contains("--to"),
        "{stderr}"
    );
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn hub_findings_sends_the_stored_token_and_prints_json_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let body = fixture("hub_findings.json");
    let (base, rx) = spawn_hub(vec![(200, body.clone())]);
    store_token(home.path(), &base, "pshub_stored", FAR_FUTURE_MS);
    let out = run(home.path(), &["hub", "findings", "--format", "json"], &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).trim_end(), body.trim_end());
    let seen = next(&rx);
    assert_eq!(seen.header("authorization"), Some("Bearer pshub_stored"));
}

#[test]
fn hub_env_token_wins_and_needs_a_hub_url() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![(200, "[]".to_string())]);
    store_token(
        home.path(),
        "https://other.example",
        "pshub_file",
        FAR_FUTURE_MS,
    );
    let out = run(
        home.path(),
        &["hub", "findings"],
        &[
            ("PERF_SENTINEL_HUB_TOKEN", "machine-token"),
            ("PERF_SENTINEL_HUB_URL", &base),
        ],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        next(&rx).header("authorization"),
        Some("Bearer machine-token")
    );

    let out = run(
        home.path(),
        &["hub", "findings"],
        &[("PERF_SENTINEL_HUB_TOKEN", "machine-token")],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("--hub"), "{}", text(&out.stderr));
}

#[test]
fn hub_refuses_a_token_over_plain_http_to_another_host() {
    let home = tempfile::tempdir().unwrap();
    let out = run(
        home.path(),
        &["hub", "--hub", "http://hub.example.com", "findings"],
        &[("PERF_SENTINEL_HUB_TOKEN", "pshub_leak")],
    );
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("https"), "{stderr}");
    assert!(!stderr.contains("pshub_leak"), "{stderr}");
}

#[test]
fn hub_refuses_a_foreign_hub_with_the_file_token() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![(200, "[]".to_string())]);
    store_token(
        home.path(),
        "https://hub.example.com",
        "pshub_x",
        FAR_FUTURE_MS,
    );
    let out = run(home.path(), &["hub", "--hub", &base, "findings"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("The stored token was issued by https://hub.example.com"),
        "{}",
        text(&out.stderr)
    );
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn hub_refuses_an_expired_token_before_any_request() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![(200, "[]".to_string())]);
    store_token(home.path(), &base, "pshub_old", 1_000);
    let out = run(home.path(), &["hub", "findings"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("perf-sentinel login --hub"));
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn hub_explains_a_401_and_a_redirect() {
    let home = tempfile::tempdir().unwrap();
    let (base, _rx) = spawn_hub(vec![(401, String::new()), (302, String::new())]);
    let out = run(home.path(), &["hub", "--hub", &base, "findings"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains(&format!("run `perf-sentinel login --hub {base}`")),
        "{}",
        text(&out.stderr)
    );
    let out = run(home.path(), &["hub", "--hub", &base, "findings"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("The Hub asked for a browser sign-in"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn hub_incidents_render_the_listing_and_one_incident() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![
        (200, fixture("hub_incidents.json")),
        (200, fixture("hub_incident.json")),
    ]);
    let out = run(
        home.path(),
        &["hub", "--hub", &base, "incidents", "--source-id", "prod-eu"],
        &[],
    );
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        next(&rx).line,
        "GET /api/incidents?offset=0&limit=50&source_id=prod-eu HTTP/1.1"
    );
    assert!(stdout.contains("shop/order-svc"), "{stdout}");
    assert!(stdout.contains("1 finding"), "{stdout}");
    assert!(stdout.contains("Findings not listed"), "{stdout}");

    let out = run(
        home.path(),
        &[
            "hub",
            "--hub",
            &base,
            "incidents",
            "0123456789abcdef0123456789abcdef",
        ],
        &[],
    );
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        next(&rx).line,
        "GET /api/incidents/0123456789abcdef0123456789abcdef HTTP/1.1"
    );
    assert!(stdout.contains("GET /orders"), "{stdout}");
}

#[test]
fn hub_ack_and_unack_post_to_the_source() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![
        (204, String::new()),
        (
            403,
            r#"{"detail":"You may not acknowledge findings at this source."}"#.to_string(),
        ),
    ]);
    let out = run(
        home.path(),
        &[
            "hub",
            "--hub",
            &base,
            "ack",
            "--source-id",
            "prod-eu",
            "-s",
            "sig:1",
            "-r",
            "deferred",
        ],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let seen = next(&rx);
    assert_eq!(seen.line, "POST /api/sources/prod-eu/acks HTTP/1.1");
    assert_eq!(seen.header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"signature":"sig:1","reason":"deferred"})
    );

    let out = run(
        home.path(),
        &[
            "hub",
            "--hub",
            &base,
            "unack",
            "--source-id",
            "prod-eu",
            "-s",
            "sig:1",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        next(&rx).line,
        "POST /api/sources/prod-eu/acks/revoke HTTP/1.1"
    );
    assert!(
        text(&out.stderr).contains("You may not acknowledge findings at this source."),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn hub_analyze_waits_for_the_run_and_writes_the_report() {
    let home = tempfile::tempdir().unwrap();
    let report = home.path().join("run.html");
    let (base, rx) = spawn_hub(vec![
        (202, r#"{"id":"r1","status":"pending"}"#.to_string()),
        (
            200,
            r#"{"id":"r1","status":"succeeded","error_code":null}"#.to_string(),
        ),
        (200, "<html>report</html>".to_string()),
    ]);
    let out = run(
        home.path(),
        &[
            "hub",
            "--hub",
            &base,
            "analyze",
            "--source-id",
            "tempo",
            "--service",
            "order-svc",
            "--lookback",
            "1h",
            "-o",
            report.to_str().unwrap(),
        ],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let submit = next(&rx);
    assert_eq!(submit.line, "POST /api/analyses HTTP/1.1");
    let body: serde_json::Value = serde_json::from_str(&submit.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"source_id":"tempo","request":{"service":"order-svc","lookback":"1h"}})
    );
    assert_eq!(next(&rx).line, "GET /api/analyses/r1 HTTP/1.1");
    assert_eq!(next(&rx).line, "GET /reports/r1.html HTTP/1.1");
    assert_eq!(
        std::fs::read_to_string(&report).unwrap(),
        "<html>report</html>"
    );
}

#[test]
fn hub_analyze_refuses_a_run_id_unfit_for_a_path() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![
        (202, r#"{"id":"../evil","status":"pending"}"#.to_string()),
        (200, r#"{"id":"../evil","status":"succeeded"}"#.to_string()),
    ]);
    let out = run(
        home.path(),
        &["hub", "--hub", &base, "analyze", "--source-id", "tempo"],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("malformed run id"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(next(&rx).line, "POST /api/analyses HTTP/1.1");
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn hub_json_output_refuses_a_body_that_is_not_json() {
    let home = tempfile::tempdir().unwrap();
    let (base, _rx) = spawn_hub(vec![(200, "\u{1b}]0;owned\u{7}".to_string())]);
    let out = run(
        home.path(),
        &["hub", "--hub", &base, "findings", "--format", "json"],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(
        text(&out.stderr).contains("GET /api/findings: malformed response."),
        "{}",
        text(&out.stderr)
    );
    assert!(!out.stderr.contains(&0x1b), "{}", text(&out.stderr));
}

#[test]
fn hub_ignores_a_relative_xdg_config_home() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![(200, "[]".to_string())]);
    let config = home.path().join(".config");
    store_token(&config, &base, "pshub_home", FAR_FUTURE_MS);
    let out = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["hub", "findings"])
        .current_dir(home.path())
        .stdin(Stdio::null())
        .env("XDG_CONFIG_HOME", "cfg")
        .env("HOME", home.path())
        .env_remove("PERF_SENTINEL_HUB_URL")
        .env_remove("PERF_SENTINEL_HUB_TOKEN")
        .output()
        .expect("spawn perf-sentinel");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(next(&rx).header("authorization"), Some("Bearer pshub_home"));
}

#[test]
fn hub_analyze_exits_1_with_the_run_error_code() {
    let home = tempfile::tempdir().unwrap();
    let (base, _rx) = spawn_hub(vec![
        (202, r#"{"id":"r2","status":"pending"}"#.to_string()),
        (
            200,
            r#"{"id":"r2","status":"failed","error_code":"source_unreachable"}"#.to_string(),
        ),
    ]);
    let out = run(
        home.path(),
        &["hub", "--hub", &base, "analyze", "--source-id", "prod-eu"],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("source_unreachable"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn logout_revokes_and_deletes_the_token_file() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![(200, String::new())]);
    store_token(home.path(), &base, "pshub_bye", FAR_FUTURE_MS);
    let out = run(home.path(), &["logout"], &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let seen = next(&rx);
    assert_eq!(seen.line, "POST /auth/revoke HTTP/1.1");
    assert_eq!(seen.body, "token=pshub_bye");
    assert!(!token_path(home.path()).exists());
}

#[test]
fn logout_deletes_the_file_when_the_hub_is_unreachable() {
    let home = tempfile::tempdir().unwrap();
    store_token(home.path(), &dead_url(), "pshub_bye", FAR_FUTURE_MS);
    let out = run(home.path(), &["logout"], &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("stays valid until"),
        "{}",
        text(&out.stderr)
    );
    assert!(!text(&out.stderr).contains("pshub_bye"));
    assert!(!token_path(home.path()).exists());
}

#[cfg(unix)]
#[test]
fn logout_revokes_a_token_file_open_to_other_users() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![(200, String::new())]);
    store_token(home.path(), &base, "pshub_open", FAR_FUTURE_MS);
    let path = token_path(home.path());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = run(home.path(), &["logout"], &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let seen = next(&rx);
    assert_eq!(seen.line, "POST /auth/revoke HTTP/1.1");
    assert_eq!(seen.body, "token=pshub_open");
    assert!(
        text(&out.stderr).contains("mode 644"),
        "{}",
        text(&out.stderr)
    );
    assert!(!text(&out.stderr).contains("pshub_open"));
    assert!(!path.exists());

    // `hub` still refuses such a file, and points at logout.
    store_token(home.path(), &base, "pshub_open", FAR_FUTURE_MS);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = run(home.path(), &["hub", "findings"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("perf-sentinel logout"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn logout_deletes_an_unparseable_token_file_unrevoked() {
    let home = tempfile::tempdir().unwrap();
    let (base, rx) = spawn_hub(vec![(200, String::new())]);
    let path = token_path(home.path());
    store_token(home.path(), &base, "pshub_bad", FAR_FUTURE_MS);
    // No expiry: the file no longer parses.
    std::fs::write(
        &path,
        format!(r#"{{"hub_url":"{base}","token":"pshub_bad"}}"#),
    )
    .unwrap();
    let out = run(home.path(), &["logout"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("without being revoked"), "{stderr}");
    assert!(!stderr.contains("pshub_bad"), "{stderr}");
    assert!(!path.exists());
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn hub_help_tells_its_actions_apart() {
    let out = Command::new(env!("CARGO_BIN_EXE_perf-sentinel"))
        .args(["hub", "--help"])
        .output()
        .expect("spawn perf-sentinel");
    let stdout = text(&out.stdout);
    for action in ["findings", "incidents", "ack", "unack", "analyze"] {
        let line = stdout
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{action} ")))
            .unwrap_or_else(|| panic!("{action} missing: {stdout}"));
        assert!(line.contains("Through the Hub:"), "{line}");
    }
}
