//! `verify-card --online`: the two places a domain publishes its card keys
//! (contract C3), read through the shared HTTP client. The callers check
//! the schemes, so tests can serve plain `http://127.0.0.1`.

use std::time::Duration;

use super::binding::{Lookup, parse_doh_answer, parse_key_list};
use super::payload::is_public_host_name;
use crate::http_client::{FetchError, HttpClient, Uri, fetch_get_with_headers};
use crate::ingest::auth_header::AuthHeader;

/// One deadline for each lookup, headers and body.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest answer read: a key list holds a few keys.
const MAX_ANSWER_BYTES: usize = 64 * 1024;
const USER_AGENT: &str = concat!("perf-sentinel/", env!("CARGO_PKG_VERSION"));

/// The TXT records of `_perf-sentinel.<domain>`, asked of the
/// DNS-over-HTTPS JSON service at `doh`. A domain that is not a public host
/// name is `Failed` before any request.
pub async fn lookup_txt(client: &HttpClient, doh: &Uri, domain: &str) -> Lookup {
    if !is_public_host_name(domain) {
        return Lookup::Failed;
    }
    let name = format!("_perf-sentinel.{domain}");
    let separator = if doh.query().is_some() { '&' } else { '?' };
    let query = format!("{doh}{separator}name={name}&type=TXT");
    let (Ok(uri), Ok(accept)) = (
        query.parse::<Uri>(),
        AuthHeader::parse("Accept: application/dns-json"),
    ) else {
        return Lookup::Failed;
    };
    match fetch_get_with_headers(
        client,
        &uri,
        USER_AGENT,
        LOOKUP_TIMEOUT,
        &[accept],
        MAX_ANSWER_BYTES,
    )
    .await
    {
        Ok(body) => parse_doh_answer(&body, &name),
        Err(_) => Lookup::Failed,
    }
}

/// The key list at `url`, the domain's `/.well-known/perf-sentinel-key`:
/// a 404 is `Absent`, anything but a 200 holding a valid list `Failed`.
pub async fn lookup_well_known(client: &HttpClient, url: &Uri) -> Lookup {
    match fetch_get_with_headers(
        client,
        url,
        USER_AGENT,
        LOOKUP_TIMEOUT,
        &[],
        MAX_ANSWER_BYTES,
    )
    .await
    {
        Ok(body) => parse_key_list(&body).map_or(Lookup::Failed, Lookup::Found),
        Err(FetchError::HttpStatus(404)) => Lookup::Absent,
        // The client follows no redirect: a 3xx lands here.
        Err(_) => Lookup::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_client::build_client;
    use crate::report::card::binding::PublishedKey;
    use crate::time::parse_iso_date_to_epoch_day;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-shot server answering `response`: its base URL and the
    /// request head it received, lowercased.
    async fn serve_once(response: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let _ = socket.write_all(&response).await;
            String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase()
        });
        (format!("http://{addr}"), server)
    }

    fn response(status: &str, extra: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    #[tokio::test]
    async fn doh_lookup_sends_the_json_accept_header_and_reads_the_answer() {
        let body = br#"{"Status":0,"Answer":[{"name":"_perf-sentinel.acme.fr","type":16,"TTL":300,"data":"\"v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; from=2026-10-30\""}]}"#;
        let (base, server) = serve_once(response("200 OK", "", body)).await;
        let doh: Uri = format!("{base}/dns-query").parse().unwrap();
        let lookup = lookup_txt(&build_client(), &doh, "acme.fr").await;
        assert_eq!(
            lookup,
            Lookup::Found(vec![PublishedKey {
                fingerprint: "ps1:AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
                public_key: None,
                from: parse_iso_date_to_epoch_day("2026-10-30").unwrap(),
                to: None,
                revoked: None,
            }])
        );
        let head = server.await.unwrap();
        assert!(
            head.starts_with("get /dns-query?name=_perf-sentinel.acme.fr&type=txt http/1.1\r\n"),
            "{head}"
        );
        assert!(
            head.contains("\r\naccept: application/dns-json\r\n"),
            "{head}"
        );
    }

    #[tokio::test]
    async fn lookup_txt_refuses_a_domain_that_is_not_a_host_name() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let doh: Uri = format!("http://{}/dns-query", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        for bad in [
            "acme.fr&name=_perf-sentinel.evil.com",
            "acme.fr/x",
            "Acme.fr",
            "localhost",
        ] {
            assert_eq!(
                lookup_txt(&build_client(), &doh, bad).await,
                Lookup::Failed,
                "{bad}"
            );
        }
        // A connection made and closed would still wait in the backlog.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn doh_answer_for_another_name_is_a_failure() {
        let key = r#""data":"\"v=ps1; k=ps1:AAAAAAAAAAAAAAAAAAAAAA; from=2026-10-30\"""#;
        for body in [
            format!(
                r#"{{"Status":0,"Question":[{{"name":"_perf-sentinel.evil.com","type":16}}],"Answer":[{{"name":"_perf-sentinel.evil.com","type":16,"TTL":300,{key}}}]}}"#
            ),
            format!(
                r#"{{"Status":0,"Question":[{{"name":"_perf-sentinel.acme.fr","type":16}}],"Answer":[{{"name":"_perf-sentinel.evil.com","type":16,"TTL":300,{key}}}]}}"#
            ),
            String::from(
                r#"{"Status":3,"Question":[{"name":"_perf-sentinel.evil.com.","type":16}]}"#,
            ),
        ] {
            let (base, server) = serve_once(response("200 OK", "", body.as_bytes())).await;
            let doh: Uri = format!("{base}/dns-query").parse().unwrap();
            assert_eq!(
                lookup_txt(&build_client(), &doh, "acme.fr").await,
                Lookup::Failed,
                "{body}"
            );
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn doh_lookup_of_an_error_status_is_a_failure() {
        let (base, server) = serve_once(response("500 Internal Server Error", "", b"")).await;
        let doh: Uri = format!("{base}/dns-query").parse().unwrap();
        assert_eq!(
            lookup_txt(&build_client(), &doh, "acme.fr").await,
            Lookup::Failed
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn well_known_200_reads_the_key_list() {
        let body =
            br#"{"keys":[{"fingerprint":"ps1:AAAAAAAAAAAAAAAAAAAAAA","from":"2026-10-30"}]}"#;
        let (base, server) = serve_once(response("200 OK", "", body)).await;
        let url: Uri = format!("{base}/.well-known/perf-sentinel-key")
            .parse()
            .unwrap();
        let lookup = lookup_well_known(&build_client(), &url).await;
        assert!(
            matches!(&lookup, Lookup::Found(keys) if keys.len() == 1),
            "{lookup:?}"
        );
        let head = server.await.unwrap();
        assert!(
            head.starts_with("get /.well-known/perf-sentinel-key http/1.1\r\n"),
            "{head}"
        );
    }

    #[tokio::test]
    async fn well_known_404_is_absent() {
        let (base, server) = serve_once(response("404 Not Found", "", b"")).await;
        let url: Uri = format!("{base}/.well-known/perf-sentinel-key")
            .parse()
            .unwrap();
        assert_eq!(
            lookup_well_known(&build_client(), &url).await,
            Lookup::Absent
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn well_known_redirect_is_a_failure() {
        let (base, server) = serve_once(response(
            "301 Moved Permanently",
            "Location: http://127.0.0.1:1/\r\n",
            b"",
        ))
        .await;
        let url: Uri = format!("{base}/.well-known/perf-sentinel-key")
            .parse()
            .unwrap();
        assert_eq!(
            lookup_well_known(&build_client(), &url).await,
            Lookup::Failed
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_oversized_well_known_is_a_failure() {
        let mut body = br#"{"keys":[]}"#.to_vec();
        body.resize(MAX_ANSWER_BYTES + 1, b' ');
        let (base, server) = serve_once(response("200 OK", "", &body)).await;
        let url: Uri = format!("{base}/.well-known/perf-sentinel-key")
            .parse()
            .unwrap();
        assert_eq!(
            lookup_well_known(&build_client(), &url).await,
            Lookup::Failed
        );
        server.await.unwrap();

        // The same list within the cap is read, so the size alone fails it.
        let (base, server) = serve_once(response("200 OK", "", br#"{"keys":[]}"#)).await;
        let url: Uri = format!("{base}/.well-known/perf-sentinel-key")
            .parse()
            .unwrap();
        assert_eq!(
            lookup_well_known(&build_client(), &url).await,
            Lookup::Found(vec![])
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_malformed_well_known_is_a_failure() {
        let (base, server) = serve_once(response("200 OK", "", b"<html>")).await;
        let url: Uri = format!("{base}/.well-known/perf-sentinel-key")
            .parse()
            .unwrap();
        assert_eq!(
            lookup_well_known(&build_client(), &url).await,
            Lookup::Failed
        );
        server.await.unwrap();
    }
}
