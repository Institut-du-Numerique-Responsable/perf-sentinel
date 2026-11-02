//! `Host` allow-list for the daemon HTTP listener, against DNS rebinding.
//!
//! A web page whose name an attacker rebinds to 127.0.0.1 reaches the
//! loopback API as a same-origin caller, but its requests still carry the
//! attacker's name in `Host` (HTTP/1) or `:authority` (HTTP/2). Refusing
//! names the daemon was not told about closes that path.

use std::net::IpAddr;
use std::sync::Arc;

use axum::http::{StatusCode, header::HOST};
use axum::response::{IntoResponse, Response};

use crate::config::DaemonConfig;

/// Host names the listener answers, lowercase, without port or trailing dot.
struct AllowedHosts(Vec<String>);

impl AllowedHosts {
    /// `None` when nothing is enforced: a non-loopback bind with no
    /// `[daemon] allowed_hosts`, where cluster names vary too much to guess.
    fn from_config(daemon: &DaemonConfig) -> Option<Self> {
        let loopback = daemon
            .listen_addr
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
        if !loopback && daemon.allowed_hosts.is_empty() {
            return None;
        }
        let mut names: Vec<String> = daemon.allowed_hosts.iter().map(|h| host_of(h)).collect();
        names.push("localhost".to_string());
        Some(Self(names))
    }

    fn allows(&self, authority: &str) -> bool {
        let host = host_of(authority);
        // An IP literal cannot be a rebound name: a browser sending it
        // connected to that address itself. This also keeps kubelet probes,
        // which send the pod IP, working.
        is_ip_literal(&host) || self.0.contains(&host)
    }
}

/// The host part of an authority: no userinfo, port or trailing dot,
/// lowercase. IPv6 keeps its brackets.
fn host_of(authority: &str) -> String {
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if authority.starts_with('[') {
        authority
            .find(']')
            .map_or(authority, |end| &authority[..=end])
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

fn is_ip_literal(host: &str) -> bool {
    match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(v6) => v6.parse::<std::net::Ipv6Addr>().is_ok(),
        None => host.parse::<std::net::Ipv4Addr>().is_ok(),
    }
}

/// Wrap `router` in the `Host` check when the config calls for one.
pub(super) fn guard(router: axum::Router, daemon: &DaemonConfig) -> axum::Router {
    match AllowedHosts::from_config(daemon) {
        Some(allowed) => router.layer(axum::middleware::from_fn_with_state(
            Arc::new(allowed),
            check_host,
        )),
        None => router,
    }
}

/// Answer `421 Misdirected Request` to a request naming a host outside the
/// list. Both the HTTP/2 `:authority` (carried in the URI) and the `Host`
/// header are checked when present. A request with neither (HTTP/1.0)
/// passes: browsers always send one, so it cannot come from a rebound page.
async fn check_host(
    axum::extract::State(allowed): axum::extract::State<Arc<AllowedHosts>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let authority_ok = req
        .uri()
        .authority()
        .is_none_or(|a| allowed.allows(a.as_str()));
    let host_ok = req
        .headers()
        .get_all(HOST)
        .iter()
        .all(|v| v.to_str().is_ok_and(|h| allowed.allows(h)));
    if authority_ok && host_ok {
        next.run(req).await
    } else {
        (
            StatusCode::MISDIRECTED_REQUEST,
            axum::Json(serde_json::json!({
                "error": "unknown Host, add it to [daemon] allowed_hosts"
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn app(listen_addr: &str, allowed_hosts: &[&str]) -> axum::Router {
        let daemon = DaemonConfig {
            listen_addr: listen_addr.to_string(),
            allowed_hosts: allowed_hosts.iter().map(ToString::to_string).collect(),
            ..DaemonConfig::default()
        };
        let router = axum::Router::new()
            .route("/api/findings", axum::routing::get(|| async { "[]" }))
            .route(
                "/api/findings/{sig}/ack",
                axum::routing::post(|| async { StatusCode::CREATED }),
            );
        guard(router, &daemon)
    }

    async fn status(app: &axum::Router, method: &str, uri: &str, host: Option<&str>) -> StatusCode {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(host) = host {
            req = req.header(HOST, host);
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn loopback_bind_refuses_a_rebound_name() {
        let app = app("127.0.0.1", &[]);
        for (method, uri) in [("GET", "/api/findings"), ("POST", "/api/findings/abc/ack")] {
            assert_eq!(
                status(&app, method, uri, Some("rebind.evil.test:4318")).await,
                StatusCode::MISDIRECTED_REQUEST,
                "{method} {uri}"
            );
        }
        // HTTP/2 carries the name in :authority, which lands in the URI.
        assert_eq!(
            status(
                &app,
                "GET",
                "http://rebind.evil.test:4318/api/findings",
                None
            )
            .await,
            StatusCode::MISDIRECTED_REQUEST
        );
    }

    #[tokio::test]
    async fn loopback_bind_accepts_loopback_names_and_declared_ones() {
        let app = app("127.0.0.1", &["Perf-Sentinel.local"]);
        for host in [
            "127.0.0.1:4318",
            "localhost:4318",
            "LOCALHOST.",
            "[::1]:4318",
            "perf-sentinel.local:4318",
            "10.0.0.7:4318",
        ] {
            assert_eq!(
                status(&app, "GET", "/api/findings", Some(host)).await,
                StatusCode::OK,
                "{host}"
            );
        }
        assert_eq!(
            status(&app, "GET", "http://localhost:4318/api/findings", None).await,
            StatusCode::OK
        );
        // HTTP/1.0 without Host cannot come from a browser.
        assert_eq!(
            status(&app, "GET", "/api/findings", None).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn non_loopback_bind_checks_only_with_a_declared_list() {
        let open = app("0.0.0.0", &[]);
        assert_eq!(
            status(&open, "GET", "/api/findings", Some("anything.example")).await,
            StatusCode::OK
        );
        let listed = app("0.0.0.0", &["perf-sentinel.monitoring.svc"]);
        assert_eq!(
            status(&listed, "GET", "/api/findings", Some("anything.example")).await,
            StatusCode::MISDIRECTED_REQUEST
        );
        for host in ["perf-sentinel.monitoring.svc:4318", "10.42.0.12:4318"] {
            assert_eq!(
                status(&listed, "GET", "/api/findings", Some(host)).await,
                StatusCode::OK,
                "{host}"
            );
        }
    }

    #[test]
    fn host_of_strips_port_userinfo_and_trailing_dot() {
        assert_eq!(host_of("Example.COM.:80"), "example.com");
        assert_eq!(host_of("user@localhost:1"), "localhost");
        assert_eq!(host_of("[::1]:4318"), "[::1]");
        assert!(is_ip_literal("[::1]"));
        assert!(!is_ip_literal("[evil]"));
        assert!(!is_ip_literal("127.0.0.1.evil.test"));
    }
}
