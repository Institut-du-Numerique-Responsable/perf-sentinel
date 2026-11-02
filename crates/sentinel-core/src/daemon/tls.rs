//! TLS helpers for the daemon: stream wrapping, accept-loop incoming
//! stream, PEM loading, and an HTTPS serve loop for axum routers.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tokio::time::Duration;

use super::{DaemonError, TlsConfigError};

/// Maximum time allowed for a TLS handshake to complete. Connections that
/// do not finish the handshake within this window are dropped, preventing
/// slowloris-style resource exhaustion.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on concurrent in-flight TLS handshakes and live HTTPS connections.
/// Bounds fds, rustls buffers and task slots against handshake floods
/// where many peers complete TCP then stall on `ClientHello`. Same
/// budget as the Unix JSON socket listener.
const TLS_MAX_INFLIGHT: usize = 128;

/// A stream that is either a plain TCP connection or a TLS-wrapped one.
/// Implements `AsyncRead + AsyncWrite` so tonic and hyper can use it
/// transparently without knowing whether TLS is active.
pub(super) enum MaybeTlsStream {
    Plain(tokio::net::TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>),
}

impl AsyncRead for MaybeTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            Self::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// tonic requires streams to implement `Connected` for remote addr info.
impl tonic::transport::server::Connected for MaybeTlsStream {
    type ConnectInfo = std::net::SocketAddr;

    fn connect_info(&self) -> Self::ConnectInfo {
        match self {
            Self::Plain(s) => s.peer_addr().unwrap_or_else(|_| ([0, 0, 0, 0], 0).into()),
            Self::Tls(s) => s
                .get_ref()
                .0
                .peer_addr()
                .unwrap_or_else(|_| ([0, 0, 0, 0], 0).into()),
        }
    }
}

/// Create an async stream of connections (plain or TLS) from a TCP listener.
/// When `tls_acceptor` is `Some`, each accepted TCP connection is upgraded
/// to TLS before being yielded. Failed TLS handshakes are silently dropped.
///
/// Internally spawns a task that feeds a bounded channel. The returned
/// `ReceiverStream` is consumed by tonic's `serve_with_incoming`.
pub(super) fn tls_tcp_incoming(
    listener: tokio::net::TcpListener,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
) -> tokio_stream::wrappers::ReceiverStream<Result<MaybeTlsStream, std::io::Error>> {
    let (tx, rx) = mpsc::channel(128);
    // Cap concurrent in-flight handshakes. Prevents a flood of peers
    // that complete TCP then stall on ClientHello from pinning
    // TLS_MAX_INFLIGHT tasks plus fds plus rustls buffers for
    // TLS_HANDSHAKE_TIMEOUT each.
    let semaphore = Arc::new(tokio::sync::Semaphore::new(TLS_MAX_INFLIGHT));

    tokio::spawn(async move {
        loop {
            let (tcp, addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::debug!("TCP accept error: {e}");
                    continue;
                }
            };
            // Plain-TCP path stays inline: no handshake, no DoS surface.
            let Some(acceptor) = tls_acceptor.clone() else {
                if tx.send(Ok(MaybeTlsStream::Plain(tcp))).await.is_err() {
                    break; // receiver dropped
                }
                continue;
            };
            // TLS path: run each handshake in its own task so a single
            // stalled peer does not block accept(). A semaphore permit
            // bounds the total in-flight count.
            let Ok(permit) = semaphore.clone().acquire_owned().await else {
                break; // semaphore closed
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let stream =
                    match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                        Ok(Ok(tls)) => MaybeTlsStream::Tls(Box::new(tls)),
                        Ok(Err(e)) => {
                            tracing::debug!("TLS handshake failed from {addr}: {e}");
                            drop(permit);
                            return;
                        }
                        Err(_) => {
                            tracing::debug!("TLS handshake timed out from {addr}");
                            drop(permit);
                            return;
                        }
                    };
                let _ = tx.send(Ok(stream)).await;
                drop(permit);
            });
        }
    });

    tokio_stream::wrappers::ReceiverStream::new(rx)
}

/// Read TLS certificate and key from disk. Returns raw PEM bytes.
/// Never logs the key content.
pub(super) fn load_tls_pem(
    cert_path: &str,
    key_path: &str,
) -> Result<(Vec<u8>, Vec<u8>), DaemonError> {
    let cert = std::fs::read(cert_path).map_err(|source| {
        DaemonError::TlsConfig(TlsConfigError::ReadCert {
            path: cert_path.to_string(),
            source,
        })
    })?;
    let key = std::fs::read(key_path).map_err(|source| {
        DaemonError::TlsConfig(TlsConfigError::ReadKey {
            path: key_path.to_string(),
            source,
        })
    })?;
    Ok((cert, key))
}

/// Build a `tokio_rustls::TlsAcceptor` from PEM cert chain + key.
/// Used for the HTTP/OTLP listener. gRPC uses tonic's native TLS.
pub(super) fn build_tls_acceptor(
    cert_pem: &[u8],
    key_pem: &[u8],
) -> Result<tokio_rustls::TlsAcceptor, DaemonError> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};

    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| DaemonError::TlsConfig(TlsConfigError::ParseCerts(e)))?;
    let key = PrivateKeyDer::from_pem_slice(key_pem)
        .map_err(|e| DaemonError::TlsConfig(TlsConfigError::ParseKey(e)))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| DaemonError::TlsConfig(TlsConfigError::ServerConfig(e)))?;

    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// Deadlines that bound how long one HTTP(S) connection can hold its
/// listener slot.
///
/// hyper drops its own 30 s header-read default when no timer is set, and
/// the protocol sniffing of the auto builder has no deadline at all, so a
/// peer that connects and sends nothing would keep its slot forever.
#[derive(Clone, Copy, Debug)]
pub(super) struct ConnLimits {
    /// Live connections served at once. The accept loop waits for a free
    /// slot past it.
    pub max_connections: usize,
    /// Time allowed for the first request to arrive, and for each later
    /// HTTP/1 request header to be read.
    pub header_read_timeout: Duration,
    /// Total lifetime of a connection. Past it, the connection is asked to
    /// close after its in-flight request, then dropped after
    /// `CONN_SHUTDOWN_GRACE`, with any response body the peer never read.
    pub max_lifetime: Duration,
}

impl ConnLimits {
    pub(super) const DEFAULT: Self = Self {
        max_connections: TLS_MAX_INFLIGHT,
        header_read_timeout: Duration::from_secs(30),
        max_lifetime: Duration::from_mins(2),
    };
}

/// HTTP/2 keep-alive ping cadence and ack deadline: a peer that stops
/// answering pings is dropped instead of holding its slot.
const H2_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const H2_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Time a connection past its lifetime gets to finish its in-flight
/// request before it is dropped.
const CONN_SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Serve an axum `Router` over TLS using a manual accept loop.
///
/// Each accepted TCP connection is upgraded to TLS via the acceptor,
/// then served with hyper. Failed TLS handshakes are logged at debug
/// level and silently dropped (not fatal to the server).
pub(super) async fn serve_https(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    tls_acceptor: tokio_rustls::TlsAcceptor,
    limits: ConnLimits,
) {
    // Bound concurrent handshakes + live HTTPS connections. Without
    // this cap, a handshake-stall flood pins TLS_HANDSHAKE_TIMEOUT
    // worth of tasks/fds per attacker. Same budget as the Unix JSON
    // socket listener.
    let semaphore = Arc::new(tokio::sync::Semaphore::new(limits.max_connections));

    loop {
        let (tcp_stream, remote_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::debug!("TCP accept error: {e}");
                continue;
            }
        };

        let Ok(permit) = semaphore.clone().acquire_owned().await else {
            break; // semaphore closed
        };
        let acceptor = tls_acceptor.clone();
        let app = app.clone();

        tokio::spawn(async move {
            let tls_stream = match tokio::time::timeout(
                TLS_HANDSHAKE_TIMEOUT,
                acceptor.accept(tcp_stream),
            )
            .await
            {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    tracing::debug!("TLS handshake failed from {remote_addr}: {e}");
                    drop(permit);
                    return;
                }
                Err(_) => {
                    tracing::debug!("TLS handshake timed out from {remote_addr}");
                    drop(permit);
                    return;
                }
            };
            serve_connection(tls_stream, app, limits, remote_addr).await;
            drop(permit);
        });
    }
}

/// Serve one accepted connection with hyper under `limits`.
async fn serve_connection<I>(
    io: I,
    app: axum::Router,
    limits: ConnLimits,
    remote_addr: std::net::SocketAddr,
) where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    use tower::ServiceExt;

    // Signalled on every request, so the first-request deadline below
    // knows the peer spoke. `notify_one` stores a permit, so no race.
    let request_seen = Arc::new(tokio::sync::Notify::new());
    let seen = Arc::clone(&request_seen);
    // Bridge axum (tower) router to hyper service: convert
    // Incoming body to axum::body::Body, then oneshot the router.
    let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        seen.notify_one();
        let app = app.clone();
        async move {
            let (parts, body) = req.into_parts();
            let req = hyper::Request::from_parts(parts, axum::body::Body::new(body));
            Ok::<_, std::convert::Infallible>(
                app.oneshot(req).await.unwrap_or_else(|err| match err {}),
            )
        }
    });

    // auto::Builder negotiates HTTP/1.1 and HTTP/2, matching the behavior
    // of axum::serve. OTLP clients commonly use HTTP/2 when TLS is active.
    let mut builder =
        hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
    builder
        .http1()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(limits.header_read_timeout);
    builder
        .http2()
        .timer(hyper_util::rt::TokioTimer::new())
        .keep_alive_interval(H2_KEEPALIVE_INTERVAL)
        .keep_alive_timeout(H2_KEEPALIVE_TIMEOUT);
    let conn = builder.serve_connection(hyper_util::rt::TokioIo::new(io), service);
    let mut conn = std::pin::pin!(conn);
    let deadline = tokio::time::Instant::now() + limits.max_lifetime;

    // The protocol sniffing before the first request has no timer of its
    // own: a silent peer is dropped here.
    tokio::select! {
        res = conn.as_mut() => {
            log_conn_result(res, remote_addr);
            return;
        }
        () = request_seen.notified() => {}
        () = tokio::time::sleep(limits.header_read_timeout) => {
            tracing::debug!("HTTP connection from {remote_addr} sent no request in time");
            return;
        }
    }
    if let Ok(res) = tokio::time::timeout_at(deadline, conn.as_mut()).await {
        log_conn_result(res, remote_addr);
    } else {
        tracing::debug!("HTTP connection from {remote_addr} reached its lifetime");
        conn.as_mut().graceful_shutdown();
        let _ = tokio::time::timeout(CONN_SHUTDOWN_GRACE, conn).await;
    }
}

fn log_conn_result(
    res: Result<(), Box<dyn std::error::Error + Send + Sync>>,
    remote_addr: std::net::SocketAddr,
) {
    if let Err(e) = res {
        tracing::debug!("HTTP connection error from {remote_addr}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_tls_pem_returns_read_cert_error_for_missing_file() {
        let err = load_tls_pem("/nonexistent/cert.pem", "/nonexistent/key.pem").unwrap_err();
        match err {
            DaemonError::TlsConfig(TlsConfigError::ReadCert { path, .. }) => {
                assert_eq!(path, "/nonexistent/cert.pem");
            }
            other => panic!("expected ReadCert error, got: {other:?}"),
        }
    }

    #[test]
    fn load_tls_pem_returns_read_key_error_when_cert_exists_but_key_missing() {
        // Create a temp cert so the first read succeeds.
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("cert.pem");
        std::fs::write(&cert_path, b"dummy").unwrap();
        let err = load_tls_pem(cert_path.to_str().unwrap(), "/nonexistent/key.pem").unwrap_err();
        match err {
            DaemonError::TlsConfig(TlsConfigError::ReadKey { path, .. }) => {
                assert_eq!(path, "/nonexistent/key.pem");
            }
            other => panic!("expected ReadKey error, got: {other:?}"),
        }
    }

    #[test]
    fn build_tls_acceptor_rejects_invalid_cert_pem() {
        let bad_cert = b"not a pem certificate";
        let bad_key = b"not a pem key";
        // TlsAcceptor does not implement Debug, so we can't `.unwrap_err()`.
        // Match on the Result directly.
        match build_tls_acceptor(bad_cert, bad_key) {
            Ok(_) => panic!("expected build_tls_acceptor to reject invalid PEM"),
            Err(DaemonError::TlsConfig(
                TlsConfigError::ParseCerts(_) | TlsConfigError::ParseKey(_),
            )) => {}
            Err(other) => panic!("expected ParseCerts or ParseKey, got: {other:?}"),
        }
    }

    /// A self-signed `localhost` acceptor and a client connector trusting it.
    fn test_tls_pair() -> (tokio_rustls::TlsAcceptor, tokio_rustls::TlsConnector) {
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let server = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.der().clone()).unwrap();
        let client = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        (acceptor, tokio_rustls::TlsConnector::from(Arc::new(client)))
    }

    async fn tls_connect(
        connector: &tokio_rustls::TlsConnector,
        addr: std::net::SocketAddr,
    ) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        connector.connect(name, tcp).await.unwrap()
    }

    async fn https_get_health(
        connector: &tokio_rustls::TlsConnector,
        addr: std::net::SocketAddr,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut tls = tls_connect(connector, addr).await;
        tls.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut out = Vec::new();
        let _ = tls.read_to_end(&mut out).await;
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn silent_https_peers_do_not_lock_out_the_listener() {
        let (acceptor, connector) = test_tls_pair();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/health", axum::routing::get(|| async { "ok" }));
        let limits = ConnLimits {
            max_connections: 2,
            header_read_timeout: Duration::from_millis(300),
            max_lifetime: Duration::from_mins(1),
        };
        tokio::spawn(serve_https(listener, app, acceptor, limits));

        // Every slot taken by peers that finish the handshake, then send nothing.
        let mut silent = Vec::new();
        for _ in 0..limits.max_connections {
            silent.push(tls_connect(&connector, addr).await);
        }
        let response =
            tokio::time::timeout(Duration::from_secs(5), https_get_health(&connector, addr))
                .await
                .expect("a silent peer must lose its slot after the header-read timeout");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        drop(silent);
    }

    #[test]
    fn tls_config_error_display_contains_source_context() {
        let err = DaemonError::TlsConfig(TlsConfigError::ReadCert {
            path: "/etc/foo.pem".to_string(),
            source: std::io::Error::other("permission denied"),
        });
        let msg = format!("{err}");
        assert!(msg.contains("TLS"));
        assert!(msg.contains("/etc/foo.pem"));
    }
}
