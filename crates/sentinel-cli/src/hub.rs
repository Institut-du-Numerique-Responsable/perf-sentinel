//! `perf-sentinel login`, `logout` and `hub`: a terminal signed in to a
//! Perf Sentinel Hub through its device flow (RFC 8628), and the Hub API
//! called with that token under the same rules as the browser.
//!
//! The token lives in one owner-only file in the user's configuration
//! directory, for one Hub at a time. It never reaches argv, a log line or
//! an error message, and it only travels over https, or plain http to the
//! loopback interface. No redirect is followed.

#![cfg(feature = "daemon")]

use std::fs::OpenOptions;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue};
use sentinel_core::http_client::{FetchError, HttpClientWithBody, MAX_BODY_BYTES};
use serde::{Deserialize, Serialize};

use crate::QueryOutputFormat;

const ENV_HUB_TOKEN: &str = "PERF_SENTINEL_HUB_TOKEN";
const CLIENT_ID: &str = "perf-sentinel";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const USER_AGENT: &str = "perf-sentinel-hub";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const REPORT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_REPORT_BYTES: usize = 64 * 1024 * 1024;
const MAX_TOKEN_FILE_BYTES: u64 = 64 * 1024;
const RUN_POLL: Duration = Duration::from_secs(2);
const SLOW_DOWN: Duration = Duration::from_secs(5);
const DEFAULT_INTERVAL_SECS: u64 = 5;
const RETRY_FLOOR: Duration = Duration::from_secs(1);
const MAX_CODE_LIFETIME_SECS: u64 = 3600;
const FORM: &str = "application/x-www-form-urlencoded";
const JSON: &str = "application/json";

const NO_TERMINAL_SIGN_IN: &str = "This Hub does not offer terminal sign-in (access control is off, \
     TrustedProxyNetworks is unset, or the Hub predates it).";
const BROWSER_SIGN_IN: &str = "The Hub asked for a browser sign-in: this route needs `perf-sentinel \
     login`, or access control is off on this Hub and the route needs a session.";
const CODE_EXPIRED: &str =
    "The code expired before anyone approved it: run `perf-sentinel login` again.";

/// Actions of `perf-sentinel hub`. Each help line starts with "Through
/// the Hub:" so they read apart from `analyze`, `ack` and `query`.
#[derive(clap::Subcommand)]
pub(crate) enum HubAction {
    /// Through the Hub: list findings, with the Hub's status and sources.
    Findings {
        /// Only this service (exact match).
        #[arg(long)]
        service: Option<String>,
        /// Only this finding type (e.g. `n_plus_one_sql`).
        #[arg(long, value_name = "TYPE")]
        finding_type: Option<String>,
        /// Only this severity (critical, warning, info).
        #[arg(long)]
        severity: Option<String>,
        /// Only this status (active, `likely_resolved`, `not_observed`).
        #[arg(long)]
        status: Option<String>,
        /// Only what this Hub source reported.
        #[arg(long)]
        source_id: Option<String>,
        /// Only what the sources of this environment reported.
        #[arg(long)]
        environment: Option<String>,
        /// Only findings seen since this time (RFC 3339 UTC such as
        /// `2026-10-21T00:00:00Z`, or epoch milliseconds).
        #[arg(long, value_name = "TIME", value_parser = parse_hub_time)]
        from: Option<u64>,
        /// Only findings seen up to this time (same formats as `--from`).
        #[arg(long, value_name = "TIME", value_parser = parse_hub_time)]
        to: Option<u64>,
        /// Skip this many findings.
        #[arg(long)]
        offset: Option<usize>,
        /// Whether acknowledged findings are listed (true or false). The
        /// Hub's default applies when omitted.
        #[arg(long, value_name = "BOOL")]
        include_acked: Option<bool>,
        /// Only this finding signature.
        #[arg(long)]
        signature: Option<String>,
        /// Maximum number of results.
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output format: text (default) or json, the Hub's answer unchanged.
        #[arg(long, value_enum, default_value = "text")]
        format: QueryOutputFormat,
    },
    /// Through the Hub: list the incidents the daemons recorded, or show one.
    Incidents {
        /// Show this incident whole, its frozen findings included.
        id: Option<String>,
        /// Only this service (exact match).
        #[arg(long)]
        service: Option<String>,
        /// Only this namespace (exact match).
        #[arg(long)]
        namespace: Option<String>,
        /// Only this Hub source.
        #[arg(long)]
        source_id: Option<String>,
        /// Only the sources of this environment.
        #[arg(long)]
        environment: Option<String>,
        /// Skip this many incidents.
        #[arg(long, default_value = "0")]
        offset: usize,
        /// Maximum number of incidents.
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output format: text (default) or json, the Hub's answer unchanged.
        #[arg(long, value_enum, default_value = "text")]
        format: QueryOutputFormat,
    },
    /// Through the Hub: acknowledge a finding at the daemon behind a source.
    Ack {
        /// The Hub source whose daemon holds the finding.
        #[arg(long)]
        source_id: String,
        /// Signature of the finding. Read from stdin when omitted.
        #[arg(short, long)]
        signature: Option<String>,
        /// Why the finding is acknowledged.
        #[arg(short, long)]
        reason: String,
        /// Expiration: ISO 8601 datetime or relative duration ("7d", "24h").
        /// Omit for a permanent acknowledgment. The Hub names you itself.
        #[arg(long, value_name = "ISO8601_OR_DURATION")]
        expires: Option<String>,
    },
    /// Through the Hub: revoke an acknowledgment at the daemon behind a source.
    Unack {
        /// The Hub source whose daemon holds the acknowledgment.
        #[arg(long)]
        source_id: String,
        /// Signature of the finding. Read from stdin when omitted.
        #[arg(short, long)]
        signature: Option<String>,
    },
    /// Through the Hub: run an analysis on a source and save its HTML report.
    Analyze {
        /// The Hub source to analyze.
        #[arg(long)]
        source_id: String,
        /// Service to search traces for (trace backends).
        #[arg(long)]
        service: Option<String>,
        /// Relative window (e.g. `1h`), trace backends.
        #[arg(long)]
        lookback: Option<String>,
        #[command(flatten)]
        window: crate::cli::AbsoluteWindow,
        /// Analyze this one trace (trace backends).
        #[arg(long)]
        trace_id: Option<String>,
        /// Maximum number of traces the run reads.
        #[arg(long)]
        max_traces: Option<u32>,
        /// Where to write the report. Defaults to `hub-report-<run id>.html`.
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
}

/// The token file: one Hub at a time.
#[derive(Debug, Serialize, Deserialize)]
struct StoredToken {
    hub_url: String,
    token: String,
    expires_at_ms: i64,
}

/// Where a `hub` call goes, and with which bearer if any.
#[derive(Debug)]
struct Target {
    base: String,
    token: Option<String>,
}

#[derive(Deserialize)]
struct DeviceStart {
    device_code: String,
    user_code: String,
    expires_in: u64,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct DeviceToken {
    access_token: String,
    expires_in: i64,
}

#[derive(Deserialize)]
struct RunState {
    #[serde(default)]
    id: String,
    status: String,
    #[serde(default)]
    error_code: Option<String>,
}

/// `perf-sentinel login --hub <URL>`.
pub(crate) async fn cmd_login(hub: &str) -> i32 {
    exit_code(login(hub).await)
}

/// `perf-sentinel logout`.
pub(crate) async fn cmd_logout() -> i32 {
    exit_code(logout().await)
}

/// `perf-sentinel hub [--hub <URL>] <action>`.
pub(crate) async fn cmd_hub(hub: Option<&str>, action: HubAction) -> i32 {
    exit_code(run_action(hub, action).await)
}

fn exit_code(result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}", sentinel_core::text_safety::sanitize_for_terminal(&e));
            1
        }
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn local_time(ms: i64) -> String {
    crate::query::fmt_local_time(u64::try_from(ms).unwrap_or(0))
}

// --- URL and credential ------------------------------------------------

/// The normalized Hub URL. One that carries a token must be https, or
/// plain http to the loopback interface.
fn hub_base(url: &str, carries_token: bool) -> Result<String, String> {
    let base = crate::ack::validate_url(url).map_err(|e| e.replacen("daemon URL", "Hub URL", 1))?;
    if carries_token && !base.starts_with("https://") && !is_loopback(&base) {
        return Err(format!(
            "Refusing to send a Hub token to {base}: use https, plain http is accepted only \
             for localhost, 127.0.0.1 and ::1."
        ));
    }
    Ok(base)
}

fn is_loopback(base: &str) -> bool {
    base.parse::<sentinel_core::http_client::Uri>()
        .is_ok_and(|uri| matches!(uri.host(), Some("localhost" | "127.0.0.1" | "[::1]")))
}

/// `PERF_SENTINEL_HUB_TOKEN` wins, then the token file, and with neither
/// the call goes out with no `Authorization` and the Hub decides.
fn resolve_target(hub: Option<&str>) -> Result<Target, String> {
    if let Some(token) = std::env::var(ENV_HUB_TOKEN)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        let hub = hub.ok_or_else(|| {
            format!("{ENV_HUB_TOKEN} needs a Hub URL: pass --hub or set PERF_SENTINEL_HUB_URL.")
        })?;
        return Ok(Target {
            base: hub_base(hub, true)?,
            token: Some(token),
        });
    }
    if let Some(stored) = read_token(&token_path()?)? {
        return from_stored(hub, stored, now_ms());
    }
    let hub = hub.ok_or(
        "No Hub: pass --hub, set PERF_SENTINEL_HUB_URL, or run `perf-sentinel login --hub <URL>`.",
    )?;
    Ok(Target {
        base: hub_base(hub, false)?,
        token: None,
    })
}

/// The stored token, for its own Hub only and only before it expires.
fn from_stored(hub: Option<&str>, stored: StoredToken, now_ms: i64) -> Result<Target, String> {
    let base = hub_base(&stored.hub_url, true)?;
    if let Some(hub) = hub
        && hub_base(hub, false)? != base
    {
        return Err(format!(
            "The stored token was issued by {base}. Run `perf-sentinel login --hub {hub}` \
             to sign in there, or set {ENV_HUB_TOKEN}."
        ));
    }
    if stored.expires_at_ms <= now_ms {
        return Err(format!(
            "The token for {base} expired at {} (local time): run `perf-sentinel login --hub {base}`.",
            local_time(stored.expires_at_ms)
        ));
    }
    Ok(Target {
        base,
        token: Some(stored.token),
    })
}

// --- Token file --------------------------------------------------------

/// `$XDG_CONFIG_HOME/perf-sentinel/hub-token.json`, else under
/// `$HOME/.config`, and `%APPDATA%\perf-sentinel\hub-token.json` on Windows.
fn token_path() -> Result<PathBuf, String> {
    let set = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    #[cfg(windows)]
    let root = set("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let root = set("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        // The XDG spec ignores a relative value: it would land in the working directory.
        .filter(|p| p.is_absolute())
        .or_else(|| set("HOME").map(|h| PathBuf::from(h).join(".config")));
    root.map(|r| r.join("perf-sentinel").join("hub-token.json"))
        .ok_or_else(|| "Cannot locate the configuration directory for the Hub token.".to_string())
}

/// `None` when there is no file. A symlink, a file others can read, or one
/// that does not parse is refused. Errors never quote the content.
fn read_token(path: &Path) -> Result<Option<StoredToken>, String> {
    match load_token(path)? {
        Some((_, Some(mode))) => Err(format!(
            "The Hub token file {} is open to other users (mode {mode:o}): run `perf-sentinel \
             logout` to revoke and delete it, then `perf-sentinel login` again.",
            path.display()
        )),
        Some((stored, None)) => Ok(Some(stored)),
        None => Ok(None),
    }
}

/// The token file and, when others can read it, its mode. A symlink or a
/// file that does not parse is refused.
fn load_token(path: &Path) -> Result<Option<(StoredToken, Option<u32>)>, String> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let file = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "Cannot read the Hub token file {} ({e}): it must be a regular file, not a link.",
                path.display()
            ));
        }
    };
    #[cfg(unix)]
    let open_mode = {
        use std::os::unix::fs::MetadataExt as _;
        let mode = file.metadata().map_err(|e| e.to_string())?.mode();
        (mode & 0o077 != 0).then_some(mode & 0o777)
    };
    #[cfg(not(unix))]
    let open_mode = None;
    let mut raw = String::new();
    file.take(MAX_TOKEN_FILE_BYTES)
        .read_to_string(&mut raw)
        .map_err(|e| format!("Cannot read the Hub token file {} ({e}).", path.display()))?;
    serde_json::from_str(&raw)
        .map(|stored| Some((stored, open_mode)))
        .map_err(|_| {
            format!(
                "The Hub token file {} is not valid: run `perf-sentinel login` again.",
                path.display()
            )
        })
}

/// Written owner-only beside the target, then renamed over it.
fn write_token(path: &Path, stored: &StoredToken) -> Result<(), String> {
    let fail =
        |e: std::io::Error| format!("Cannot write the Hub token file {}: {e}", path.display());
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(fail)?;
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let json = serde_json::to_vec(stored).map_err(|e| e.to_string())?;
    let mut file = opts.open(&tmp).map_err(fail)?;
    file.write_all(&json).map_err(fail)?;
    file.sync_all().map_err(fail)?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(fail)
}

// --- HTTP --------------------------------------------------------------

/// Percent-encode everything outside the RFC 3986 unreserved set: a form
/// value, a query value or a path segment.
fn encode(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// One request to the Hub. The bearer goes out marked sensitive.
async fn send(
    client: &HttpClientWithBody,
    method: hyper::Method,
    url: &str,
    token: Option<&str>,
    body: Option<(&'static str, Vec<u8>)>,
    max_bytes: usize,
    timeout: Duration,
) -> Result<(hyper::StatusCode, bytes::Bytes), FetchError> {
    let uri: sentinel_core::http_client::Uri = url
        .parse()
        .map_err(|_| FetchError::BodyRead(format!("invalid URL `{url}`")))?;
    let mut headers: Vec<(HeaderName, HeaderValue)> = Vec::new();
    if let Some(token) = token {
        let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| FetchError::RequestBuild(e.into()))?;
        value.set_sensitive(true);
        headers.push((AUTHORIZATION, value));
    }
    let payload = match body {
        Some((content_type, bytes)) => {
            headers.push((CONTENT_TYPE, HeaderValue::from_static(content_type)));
            bytes::Bytes::from(bytes)
        }
        None => bytes::Bytes::new(),
    };
    sentinel_core::http_client::send(
        client, method, &uri, USER_AGENT, timeout, &headers, payload, max_bytes,
    )
    .await
}

/// A form POST to one of the anonymous OAuth routes.
async fn post_form(
    client: &HttpClientWithBody,
    base: &str,
    path: &str,
    fields: &[(&str, &str)],
) -> Result<(hyper::StatusCode, bytes::Bytes), FetchError> {
    let body = fields
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    send(
        client,
        hyper::Method::POST,
        &format!("{base}{path}"),
        None,
        Some((FORM, body.into_bytes())),
        MAX_BODY_BYTES,
        REQUEST_TIMEOUT,
    )
    .await
}

/// A Hub API call: the body of a 2xx, the rule's sentence otherwise.
async fn api(
    target: &Target,
    method: hyper::Method,
    path: &str,
    json: Option<Vec<u8>>,
    max_bytes: usize,
    timeout: Duration,
) -> Result<bytes::Bytes, String> {
    let client = sentinel_core::http_client::build_client_with_body();
    let route = format!("{method} {}", path.split('?').next().unwrap_or(path));
    let (status, body) = send(
        &client,
        method,
        &format!("{}{path}", target.base),
        target.token.as_deref(),
        json.map(|j| (JSON, j)),
        max_bytes,
        timeout,
    )
    .await
    .map_err(|e| unreachable(&target.base, &e))?;
    match status.as_u16() {
        200..=299 => Ok(body),
        300..=399 => Err(BROWSER_SIGN_IN.to_string()),
        401 => Err(format!(
            "Not signed in to {base}, or the token expired or was revoked: run \
             `perf-sentinel login --hub {base}`.",
            base = target.base
        )),
        code => Err(refusal(&route, code, &body)),
    }
}

fn unreachable(base: &str, e: &FetchError) -> String {
    match e {
        FetchError::RequestBuild(_) => {
            "The Hub token cannot travel in an HTTP header: check its value.".to_string()
        }
        e => format!("Cannot reach the Hub at {base}: {e}."),
    }
}

/// The Hub's `detail` when it gave one.
fn refusal(route: &str, code: u16, body: &[u8]) -> String {
    let detail = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("detail")?.as_str().map(str::to_string));
    match (detail, code) {
        (Some(detail), _) => format!("{route} refused ({code}): {detail}"),
        (None, 503) => format!("{route}: Hub busy, retry."),
        (None, _) => format!("{route} failed: HTTP {code}."),
    }
}

// --- login / logout ----------------------------------------------------

async fn login(hub: &str) -> Result<(), String> {
    let base = hub_base(hub, true)?;
    let client = sentinel_core::http_client::build_client_with_body();
    let device = start_device(&client, &base).await?;
    // The Hub's own address, never `verification_uri`: the code is typed, not followed.
    println!(
        "Open {base}/device in a browser and enter the code {} (expires in {} minutes).",
        sentinel_core::text_safety::sanitize_for_terminal(&device.user_code),
        device.expires_in.div_ceil(60)
    );
    let granted = poll_device(&client, &base, &device).await?;
    let token = granted.access_token;
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("The Hub sent a malformed token.".to_string());
    }
    let expires_at_ms = now_ms().saturating_add(granted.expires_in.saturating_mul(1000));
    write_token(
        &token_path()?,
        &StoredToken {
            hub_url: base.clone(),
            token,
            expires_at_ms,
        },
    )?;
    println!(
        "Signed in to {base} until {} (local time).",
        local_time(expires_at_ms)
    );
    Ok(())
}

async fn start_device(client: &HttpClientWithBody, base: &str) -> Result<DeviceStart, String> {
    let (status, body) = post_form(client, base, "/auth/device", &[("client_id", CLIENT_ID)])
        .await
        .map_err(|e| unreachable(base, &e))?;
    let code = status.as_u16();
    if status.is_redirection() || code == 401 || code == 404 {
        return Err(NO_TERMINAL_SIGN_IN.to_string());
    }
    if code != 200 {
        return Err(format!(
            "The Hub refused the sign-in: {}.",
            oauth_error(&body).unwrap_or_else(|| format!("HTTP {code}"))
        ));
    }
    serde_json::from_slice(&body).map_err(|_| "POST /auth/device: malformed response.".to_string())
}

/// Polls every `interval` until the code is decided or expires. A network
/// error or a 5xx is retried: the Hub keeps the code across both.
async fn poll_device(
    client: &HttpClientWithBody,
    base: &str,
    device: &DeviceStart,
) -> Result<DeviceToken, String> {
    // The Hub's numbers are untrusted: clamped so the deadline cannot overflow.
    let deadline =
        Instant::now() + Duration::from_secs(device.expires_in.min(MAX_CODE_LIFETIME_SECS));
    let mut interval = Duration::from_secs(device.interval.unwrap_or(DEFAULT_INTERVAL_SECS));
    let mut wait = interval;
    loop {
        tokio::time::sleep(wait.min(deadline.saturating_duration_since(Instant::now()))).await;
        if Instant::now() >= deadline {
            return Err(CODE_EXPIRED.to_string());
        }
        // A failed poll is retried no faster than once a second, whatever the interval.
        wait = interval.max(RETRY_FLOOR);
        let fields = [
            ("grant_type", DEVICE_GRANT),
            ("device_code", device.device_code.as_str()),
            ("client_id", CLIENT_ID),
        ];
        let Ok((status, body)) = post_form(client, base, "/auth/device/token", &fields).await
        else {
            continue;
        };
        if status.is_success() {
            return serde_json::from_slice(&body)
                .map_err(|_| "POST /auth/device/token: malformed response.".to_string());
        }
        if status.is_server_error() {
            continue;
        }
        let error = oauth_error(&body).unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
        interval = next_interval(&error, interval)?;
        wait = interval;
    }
}

/// RFC 8628 section 3.5: what a refused poll means for the next one.
fn next_interval(error: &str, interval: Duration) -> Result<Duration, String> {
    match error {
        "authorization_pending" => Ok(interval),
        "slow_down" => Ok(interval.saturating_add(SLOW_DOWN)),
        "access_denied" => Err("The sign-in was denied in the browser.".to_string()),
        "expired_token" => Err(CODE_EXPIRED.to_string()),
        "invalid_grant" => Err(
            "The Hub no longer knows this sign-in (it restarted, or the code was used): run \
             `perf-sentinel login` again."
                .to_string(),
        ),
        other => Err(format!("The Hub refused the sign-in: {other}.")),
    }
}

fn oauth_error(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_string)
}

async fn logout() -> Result<(), String> {
    let path = token_path()?;
    // A file others can read is still revoked: its token is the one most at risk.
    let (stored, open_mode) = match load_token(&path) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => {
            println!("Not signed in to any Hub.");
            return Ok(());
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(format!(
                "{e} It was deleted without being revoked: a token in it stays valid on the \
                 Hub until it expires."
            ));
        }
    };
    if let Some(mode) = open_mode {
        eprintln!(
            "Warning: the Hub token file {} was open to other users (mode {mode:o}): \
             another local account may have copied the token before this revocation.",
            path.display()
        );
    }
    let revoked = revoke(&stored).await;
    tokio::fs::remove_file(&path)
        .await
        .map_err(|e| format!("Cannot delete the Hub token file {}: {e}", path.display()))?;
    match revoked {
        Ok(base) => println!("Signed out of {base}."),
        Err(why) => eprintln!(
            "Warning: {}: the token stays valid until {} (local time).",
            sentinel_core::text_safety::sanitize_for_terminal(&why),
            local_time(stored.expires_at_ms)
        ),
    }
    Ok(())
}

/// RFC 7009 by possession: the Hub answers 200 whether or not it knew it.
async fn revoke(stored: &StoredToken) -> Result<String, String> {
    let base = hub_base(&stored.hub_url, true)?;
    let client = sentinel_core::http_client::build_client_with_body();
    let (status, _) = post_form(&client, &base, "/auth/revoke", &[("token", &stored.token)])
        .await
        .map_err(|e| format!("cannot reach {base} ({e})"))?;
    if status.is_success() {
        Ok(base)
    } else {
        Err(format!(
            "{base} answered HTTP {} to the revocation",
            status.as_u16()
        ))
    }
}

// --- hub actions -------------------------------------------------------

async fn run_action(hub: Option<&str>, action: HubAction) -> Result<(), String> {
    let target = resolve_target(hub)?;
    match action {
        HubAction::Findings {
            service,
            finding_type,
            severity,
            status,
            source_id,
            environment,
            from,
            to,
            offset,
            include_acked,
            signature,
            limit,
            format,
        } => {
            check_window(from, to)?;
            let path = with_query(
                "/api/findings",
                &[
                    ("limit", Some(limit.to_string())),
                    ("service", service),
                    ("finding_type", finding_type),
                    ("severity", severity),
                    ("status", status),
                    ("source_id", source_id),
                    ("environment", environment),
                    ("from", from.map(|v| v.to_string())),
                    ("to", to.map(|v| v.to_string())),
                    ("offset", offset.map(|v| v.to_string())),
                    ("include_acked", include_acked.map(|v| v.to_string())),
                    ("signature", signature),
                ],
            );
            let body = get(&target, &path).await?;
            match format {
                QueryOutputFormat::Json => print_unchanged("GET /api/findings", &body)?,
                QueryOutputFormat::Text => crate::query::render_findings_response(
                    &body,
                    QueryOutputFormat::Text,
                    &target.base,
                    None,
                ),
            }
            Ok(())
        }
        HubAction::Incidents {
            id,
            service,
            namespace,
            source_id,
            environment,
            offset,
            limit,
            format,
        } => {
            let path = id.as_deref().map_or_else(
                || {
                    with_query(
                        "/api/incidents",
                        &[
                            ("offset", Some(offset.to_string())),
                            ("limit", Some(limit.to_string())),
                            ("service", service),
                            ("namespace", namespace),
                            ("source_id", source_id),
                            ("environment", environment),
                        ],
                    )
                },
                |id| format!("/api/incidents/{}", encode(id)),
            );
            let body = get(&target, &path).await?;
            match (format, id.is_some()) {
                (QueryOutputFormat::Json, _) => print_unchanged("GET /api/incidents", &body)?,
                // One incident is an object: the renderer reads a list.
                (QueryOutputFormat::Text, true) => {
                    let listed = [b"[".as_slice(), &body, b"]"].concat();
                    crate::query::render_incidents_response(
                        &listed,
                        QueryOutputFormat::Text,
                        &target.base,
                    );
                }
                (QueryOutputFormat::Text, false) => crate::query::render_incidents_response(
                    &body,
                    QueryOutputFormat::Text,
                    &target.base,
                ),
            }
            Ok(())
        }
        HubAction::Ack {
            source_id,
            signature,
            reason,
            expires,
        } => {
            ack(
                &target,
                &source_id,
                signature,
                Some(reason),
                expires.as_deref(),
            )
            .await
        }
        HubAction::Unack {
            source_id,
            signature,
        } => ack(&target, &source_id, signature, None, None).await,
        HubAction::Analyze {
            source_id,
            service,
            lookback,
            window,
            trace_id,
            max_traces,
            output,
        } => {
            let mut request = serde_json::Map::new();
            let mut put = |k: &str, v: Option<serde_json::Value>| {
                if let Some(v) = v {
                    request.insert(k.to_string(), v);
                }
            };
            put("service", service.map(Into::into));
            put("lookback", lookback.map(Into::into));
            put("trace_id", trace_id.map(Into::into));
            put("max_traces", max_traces.map(Into::into));
            put("from_ms", window.from.as_deref().map(epoch_ms).transpose()?);
            put("to_ms", window.to.as_deref().map(epoch_ms).transpose()?);
            let body = serde_json::json!({"source_id": source_id, "request": request});
            analyze(&target, &body, output).await
        }
    }
}

fn epoch_ms(iso: &str) -> Result<serde_json::Value, String> {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|t| t.timestamp_millis().into())
        .map_err(|e| format!("Invalid --from/--to `{iso}`: {e}"))
}

/// A `--from`/`--to` value as epoch milliseconds: RFC 3339 or the number itself.
fn parse_hub_time(s: &str) -> Result<u64, String> {
    if let Ok(ms) = s.parse::<u64>() {
        return Ok(ms);
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .map_err(|e| format!("`{s}` is neither RFC 3339 UTC nor epoch milliseconds ({e})"))
        .and_then(|t| {
            u64::try_from(t.timestamp_millis()).map_err(|_| format!("`{s}` is before 1970"))
        })
}

/// The Hub rejects an inverted window: say so before any request.
fn check_window(from: Option<u64>, to: Option<u64>) -> Result<(), String> {
    match (from, to) {
        (Some(from), Some(to)) if from > to => {
            Err("--from must not be later than --to.".to_string())
        }
        _ => Ok(()),
    }
}

fn with_query(path: &str, params: &[(&str, Option<String>)]) -> String {
    let query: Vec<String> = params
        .iter()
        .filter_map(|(k, v)| v.as_deref().map(|v| format!("{k}={}", encode(v))))
        .collect();
    format!("{path}?{}", query.join("&"))
}

async fn get(target: &Target, path: &str) -> Result<bytes::Bytes, String> {
    api(
        target,
        hyper::Method::GET,
        path,
        None,
        MAX_BODY_BYTES,
        REQUEST_TIMEOUT,
    )
    .await
}

/// The Hub's JSON byte for byte, once it parses: valid JSON holds no raw
/// control character, so nothing reaches the terminal unescaped.
fn print_unchanged(route: &str, body: &[u8]) -> Result<(), String> {
    serde_json::from_slice::<serde::de::IgnoredAny>(body)
        .map_err(|_| format!("{route}: malformed response."))?;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(body);
    if !body.ends_with(b"\n") {
        let _ = out.write_all(b"\n");
    }
    Ok(())
}

/// An ack when `reason` is set, a revoke otherwise. The Hub names the
/// caller itself, so no `by` is sent.
async fn ack(
    target: &Target,
    source_id: &str,
    signature: Option<String>,
    reason: Option<String>,
    expires: Option<&str>,
) -> Result<(), String> {
    let signature = crate::ack::resolve_signature(signature)?;
    let mut body = serde_json::json!({ "signature": signature });
    let revoke = reason.is_none();
    if let Some(reason) = reason {
        body["reason"] = reason.into();
    }
    if let Some(expires) = expires {
        let at = crate::ack::parse_expires(expires)
            .map_err(|e| format!("Invalid --expires value, {e}"))?;
        body["expires_at"] = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true).into();
    }
    let path = format!(
        "/api/sources/{}/acks{}",
        encode(source_id),
        if revoke { "/revoke" } else { "" }
    );
    api(
        target,
        hyper::Method::POST,
        &path,
        Some(body.to_string().into_bytes()),
        MAX_BODY_BYTES,
        REQUEST_TIMEOUT,
    )
    .await?;
    let done = if revoke {
        "Revoked the ack of"
    } else {
        "Acknowledged"
    };
    println!(
        "{done} {} at {} through {}.",
        sentinel_core::text_safety::sanitize_for_terminal(&signature),
        sentinel_core::text_safety::sanitize_for_terminal(source_id),
        target.base
    );
    Ok(())
}

/// Submits the run, waits for a terminal state, then saves the report.
async fn analyze(
    target: &Target,
    body: &serde_json::Value,
    output: Option<PathBuf>,
) -> Result<(), String> {
    let submitted = api(
        target,
        hyper::Method::POST,
        "/api/analyses",
        Some(body.to_string().into_bytes()),
        MAX_BODY_BYTES,
        REQUEST_TIMEOUT,
    )
    .await?;
    let run: RunState = serde_json::from_slice(&submitted)
        .map_err(|_| "POST /api/analyses: malformed response.".to_string())?;
    // The id names a path and a file: a Hub's ids are hex.
    let id = run.id;
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("POST /api/analyses: malformed run id.".to_string());
    }
    eprintln!("Run {id} queued on {}.", target.base);
    loop {
        let state: RunState =
            serde_json::from_slice(&get(target, &format!("/api/analyses/{id}")).await?)
                .map_err(|_| "GET /api/analyses: malformed response.".to_string())?;
        match state.status.as_str() {
            "succeeded" => break,
            "failed" | "interrupted" | "expired" => {
                return Err(format!(
                    "Run {id} {}: {}.",
                    state.status,
                    state.error_code.as_deref().unwrap_or("no error code")
                ));
            }
            _ => tokio::time::sleep(RUN_POLL).await,
        }
    }
    let report = api(
        target,
        hyper::Method::GET,
        &format!("/reports/{id}.html"),
        None,
        MAX_REPORT_BYTES,
        REPORT_TIMEOUT,
    )
    .await?;
    let path = output.unwrap_or_else(|| PathBuf::from(format!("hub-report-{id}.html")));
    tokio::fs::write(&path, &report)
        .await
        .map_err(|e| format!("Cannot write the report to {}: {e}", path.display()))?;
    println!("Report of run {id} written to {}.", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(hub: &str, expires_at_ms: i64) -> StoredToken {
        StoredToken {
            hub_url: hub.to_string(),
            token: "pshub_t".to_string(),
            expires_at_ms,
        }
    }

    #[test]
    fn a_token_travels_over_https_or_to_loopback_only() {
        assert!(hub_base("https://hub.example.com/", true).is_ok());
        for local in [
            "http://localhost:5080",
            "http://127.0.0.1",
            "http://[::1]:80",
        ] {
            assert!(hub_base(local, true).is_ok(), "{local}");
        }
        let err = hub_base("http://hub.example.com", true).unwrap_err();
        assert!(err.contains("https"), "{err}");
        assert!(hub_base("http://hub.example.com", false).is_ok());
        assert!(hub_base("http://127.0.0.1.example.com", true).is_err());
        assert!(
            hub_base("ftp://hub", false)
                .unwrap_err()
                .contains("Invalid Hub URL")
        );
    }

    #[test]
    fn the_stored_token_serves_its_own_hub_until_it_expires() {
        let t = from_stored(
            Some("https://hub.example.com/"),
            stored("https://hub.example.com", 2_000),
            1_000,
        )
        .unwrap();
        assert_eq!(t.base, "https://hub.example.com");
        assert_eq!(t.token.as_deref(), Some("pshub_t"));

        let err = from_stored(
            Some("https://other.example"),
            stored("https://hub.example.com", 2_000),
            1_000,
        )
        .unwrap_err();
        assert!(
            err.starts_with("The stored token was issued by https://hub.example.com."),
            "{err}"
        );

        let err = from_stored(None, stored("https://hub.example.com", 1_000), 1_000).unwrap_err();
        assert!(err.contains("expired"), "{err}");
        assert!(!err.contains("pshub_t"), "{err}");
    }

    #[test]
    fn a_refused_poll_says_what_comes_next() {
        let five = Duration::from_secs(5);
        assert_eq!(next_interval("authorization_pending", five), Ok(five));
        assert_eq!(
            next_interval("slow_down", five),
            Ok(Duration::from_secs(10))
        );
        // A Hub's huge interval saturates instead of panicking.
        assert_eq!(next_interval("slow_down", Duration::MAX), Ok(Duration::MAX));
        assert!(
            next_interval("access_denied", five)
                .unwrap_err()
                .contains("denied")
        );
        assert!(
            next_interval("expired_token", five)
                .unwrap_err()
                .contains("expired")
        );
        assert!(next_interval("invalid_grant", five).is_err());
        assert!(
            next_interval("invalid_client", five)
                .unwrap_err()
                .contains("invalid_client")
        );
    }

    #[test]
    fn encode_leaves_only_unreserved_bytes() {
        assert_eq!(encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode("x&y=z/ :"), "x%26y%3Dz%2F%20%3A");
    }

    #[test]
    fn token_file_round_trips_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perf-sentinel").join("hub-token.json");
        write_token(&path, &stored("https://hub.example.com", 5)).unwrap();
        // A stale temporary file from a crashed write does not block the next.
        std::fs::write(path.with_extension("json.tmp"), "stale").unwrap();
        write_token(&path, &stored("https://hub.example.com", 6)).unwrap();
        let read = read_token(&path).unwrap().unwrap();
        assert_eq!(read.expires_at_ms, 6);
        assert_eq!(read.token, "pshub_t");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
        assert!(
            read_token(&dir.path().join("absent.json"))
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn token_file_open_to_others_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hub-token.json");
        write_token(&path, &stored("https://hub.example.com", 5)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = read_token(&path).unwrap_err();
        assert!(err.contains("mode 644"), "{err}");
        assert!(!err.contains("pshub_t"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn token_file_behind_a_symlink_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        write_token(&real, &stored("https://hub.example.com", 5)).unwrap();
        let link = dir.path().join("hub-token.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = read_token(&link).unwrap_err();
        assert!(err.contains("not a link"), "{err}");
    }

    #[test]
    fn a_malformed_token_file_is_refused_without_quoting_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hub-token.json");
        write_token(&path, &stored("https://hub.example.com", 5)).unwrap();
        std::fs::write(
            &path,
            r#"{"hub_url":"x","token":"pshub_secret","expires_at_ms":"soon"}"#,
        )
        .unwrap();
        let err = read_token(&path).unwrap_err();
        assert!(err.contains("not valid"), "{err}");
        assert!(
            !err.contains("pshub_secret") && !err.contains("soon"),
            "{err}"
        );
    }

    #[test]
    fn refusal_prefers_the_hub_detail() {
        assert_eq!(
            refusal(
                "POST /api/analyses",
                403,
                br#"{"detail":"You may not start this analysis."}"#
            ),
            "POST /api/analyses refused (403): You may not start this analysis."
        );
        assert_eq!(
            refusal("POST /api/analyses", 503, b""),
            "POST /api/analyses: Hub busy, retry."
        );
        assert_eq!(
            refusal("GET /api/findings", 500, b"oops"),
            "GET /api/findings failed: HTTP 500."
        );
    }

    #[test]
    fn hub_time_is_epoch_ms_or_rfc3339() {
        assert_eq!(parse_hub_time("1793145600000"), Ok(1_793_145_600_000));
        assert_eq!(
            parse_hub_time("2026-10-21T00:00:00Z"),
            Ok(1_792_540_800_000)
        );
        assert_eq!(
            parse_hub_time("2026-10-21T02:00:00+02:00"),
            Ok(1_792_540_800_000)
        );
        assert!(
            parse_hub_time("1969-12-31T23:59:59Z")
                .unwrap_err()
                .contains("1970")
        );
        assert!(parse_hub_time("yesterday").is_err());
        assert!(check_window(Some(2), Some(1)).is_err());
        assert!(check_window(Some(1), Some(1)).is_ok());
        assert!(check_window(Some(2), None).is_ok());
    }
}
