//! Unix JSON socket listener. Reads newline-delimited `SpanEvent` arrays.
//!
//! Each NDJSON line is parsed through [`crate::ingest::json::JsonIngest`]
//! and the resulting `Vec<SpanEvent>` is forwarded to the daemon event loop.

#![cfg(unix)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;

/// Bind the JSON socket at `path`, owner-only.
///
/// `Ok(None)` when the parent directory does not exist, the expected case
/// in minimal container images. Every other failure is returned so the
/// daemon refuses to start: producers would otherwise write to a path
/// whoever squatted it controls.
///
/// # Errors
///
/// The path holds an entry this user did not create, or the bind or the
/// `chmod` failed.
pub(super) fn bind_json_socket(path: &str) -> std::io::Result<Option<tokio::net::UnixListener>> {
    use std::os::unix::fs::PermissionsExt as _;
    clear_stale_socket(std::path::Path::new(path))?;
    let listener = match tokio::net::UnixListener::bind(path) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(
                "JSON socket listener disabled: parent directory of \
                 {path} does not exist. Expected in minimal container \
                 images (FROM scratch, distroless static); mount a \
                 writable directory and set `[daemon] json_socket` to \
                 a path inside it to enable local NDJSON ingestion."
            );
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    // Setting the umask before bind() needs unsafe code, so the socket is
    // tightened right after: other local users must not inject events.
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    tracing::info!("JSON socket listening on {path}");
    Ok(Some(listener))
}

/// Remove a socket an earlier run of this user left at `path`, and refuse
/// anything else: a symlink (whose unlink could be redirected), a regular
/// file, or another user's socket squatting a shared directory such as
/// `/tmp`.
fn clear_stale_socket(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if !meta.file_type().is_socket() || meta.uid() != current_euid()? {
        return Err(std::io::Error::other(
            "the path holds an entry this user did not create (a symlink, a \
             file or another user's socket), remove it after checking it is safe",
        ));
    }
    std::fs::remove_file(path)
}

/// The effective uid, read without unsafe code: a socket the process just
/// created is owned by it, and `fstat` on it reports that owner.
fn current_euid() -> std::io::Result<u32> {
    use std::os::unix::fs::MetadataExt as _;
    let (socket, _peer) = std::os::unix::net::UnixStream::pair()?;
    let file = std::fs::File::from(std::os::fd::OwnedFd::from(socket));
    Ok(file.metadata()?.uid())
}

/// Serve the JSON socket bound by [`bind_json_socket`].
///
/// Reads newline-delimited JSON (NDJSON): each line is a JSON array of `SpanEvent`s.
pub(super) async fn run_json_socket(
    listener: tokio::net::UnixListener,
    tx: mpsc::Sender<super::IngestBatch>,
    max_payload_size: usize,
    grouping_attributes: Vec<Arc<str>>,
    over_memory: Arc<AtomicBool>,
) {
    // Interned once: the accept loop hands every connection a pointer bump
    // instead of re-allocating the key list per connection.
    let grouping_attributes: Arc<[Arc<str>]> = grouping_attributes.into();

    // Limit concurrent connections to prevent local DoS via connection flooding
    let semaphore = Arc::new(tokio::sync::Semaphore::new(128));

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let tx = tx.clone();
                let over_memory = over_memory.clone();
                let grouping_attributes = Arc::clone(&grouping_attributes);
                let Ok(permit) = semaphore.clone().acquire_owned().await else {
                    break; // semaphore closed
                };
                tokio::spawn(async move {
                    handle_json_connection(
                        stream,
                        tx,
                        max_payload_size,
                        grouping_attributes,
                        over_memory,
                    )
                    .await;
                    drop(permit);
                });
            }
            Err(e) => {
                tracing::error!("Unix socket accept error: {e}");
            }
        }
    }
}

/// Process a single JSON socket connection: read NDJSON lines and forward events.
async fn handle_json_connection(
    stream: tokio::net::UnixStream,
    tx: mpsc::Sender<super::IngestBatch>,
    max_payload_size: usize,
    grouping_attributes: Arc<[Arc<str>]>,
    over_memory: Arc<AtomicBool>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    const CONNECTION_LIMIT_FACTOR: u64 = 16;
    let limited = stream.take(max_payload_size as u64 * CONNECTION_LIMIT_FACTOR);
    let reader = tokio::io::BufReader::new(limited);
    let mut lines = reader.lines();
    let ingest = crate::ingest::json::JsonIngest::new(max_payload_size)
        .with_grouping_attributes(grouping_attributes.to_vec());
    let mut memory_drop_warned = false;
    while let Ok(Some(line)) = lines.next_line().await {
        // Memory-pressure admission control: this door feeds the same
        // ingest channel as the OTLP listeners and must honor the same
        // guard, else local NDJSON keeps growing RSS while the OTLP
        // doors are closed. Dropped before parsing. Warned once per
        // connection so a long-lived local producer is not spammed.
        if over_memory.load(Ordering::Relaxed) {
            if !memory_drop_warned {
                tracing::warn!("JSON socket: dropping batches while the memory guard is tripped");
                memory_drop_warned = true;
            }
            continue;
        }
        if line.len() > max_payload_size {
            tracing::warn!("JSON socket: line exceeds max payload size, skipping");
            continue;
        }
        match crate::ingest::IngestSource::ingest(&ingest, line.as_bytes()) {
            Ok(events) if !events.is_empty() => {
                if tx.send(events.into()).await.is_err() {
                    tracing::warn!("JSON socket: event channel closed");
                    break;
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("JSON socket: failed to parse line: {e}");
            }
        }
    }
}

/// Build a unique Unix-socket path inside a fresh `tempfile::TempDir`
/// rooted at `/tmp/`, not `std::env::temp_dir()`.
///
/// `tempfile::tempdir()` (no arg) is avoided because on macOS
/// `std::env::temp_dir()` resolves to `/var/folders/<hash>/T/...`,
/// which easily exceeds the Unix-socket `SUN_LEN` limit (104 bytes
/// on macOS, 108 on Linux). A `tempfile::TempDir` rooted at `/tmp`
/// gives us:
///
/// - **Collision-free by construction** (random 6-char suffix from
///   `tempfile`, not a timestamp-based pseudo-unique name).
/// - **Symlink-TOCTOU safe**: the directory is created with
///   `mkdir(..., 0o700)` atomically, so a local attacker cannot
///   substitute a symlink between path generation and socket bind.
/// - **Auto-cleanup on drop**: the `TempDir` owner (the test body)
///   removes the directory when it goes out of scope, including the
///   socket file and the parent dir.
///
/// The returned `TempDir` must be kept alive for the duration of the
/// test, because the returned path borrows from it.
#[cfg(test)]
pub(super) fn unique_socket_dir_and_path(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("psd-{name}-"))
        .tempdir_in("/tmp")
        .expect("mkdtemp in /tmp should succeed");
    let path = dir.path().join("daemon.sock");
    (dir, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Duration;

    #[test]
    fn current_euid_is_the_owner_of_a_file_this_process_creates() {
        use std::os::unix::fs::MetadataExt as _;
        let file = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            current_euid().unwrap(),
            file.as_file().metadata().unwrap().uid()
        );
    }

    #[test]
    fn a_file_or_a_symlink_at_the_socket_path_is_refused_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain");
        std::fs::write(&file, b"keep").unwrap();
        assert!(clear_stale_socket(&file).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"keep");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(clear_stale_socket(&link).is_err());
        assert!(link.symlink_metadata().is_ok());
        assert!(clear_stale_socket(&dir.path().join("absent")).is_ok());
    }

    #[tokio::test]
    async fn bind_is_owner_only_replaces_its_own_stale_socket_and_skips_a_missing_dir() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let path = path.to_str().unwrap();
        drop(bind_json_socket(path).unwrap().expect("bound"));
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(
            bind_json_socket(path).unwrap().is_some(),
            "own stale socket replaced"
        );
        let missing = dir.path().join("missing").join("s.sock");
        assert!(
            bind_json_socket(missing.to_str().unwrap())
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_file_at_the_socket_path_stops_daemon_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("squatted");
        std::fs::write(&path, b"x").unwrap();
        assert!(bind_json_socket(path.to_str().unwrap()).is_err());
    }

    /// Spawn the connection handler on the server half, with no grouping
    /// attributes and the memory guard clear. `max_payload` is the only
    /// knob these tests vary.
    fn spawn_handler(
        server: tokio::net::UnixStream,
        tx: mpsc::Sender<super::super::IngestBatch>,
        max_payload: usize,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            handle_json_connection(
                server,
                tx,
                max_payload,
                Arc::from(vec![]),
                Arc::new(AtomicBool::new(false)),
            )
            .await;
        })
    }

    #[tokio::test]
    async fn handle_json_connection_happy_path_forwards_events() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixStream;

        let (client, server) = UnixStream::pair().expect("UnixStream::pair should succeed");
        let (tx, mut rx) = mpsc::channel::<super::super::IngestBatch>(16);

        let handle = spawn_handler(server, tx, 1024 * 1024);

        // Write one NDJSON line with a minimal valid SpanEvent array,
        // then close the client half so the server sees EOF and returns.
        let line = r#"[{"timestamp":"2025-07-10T14:32:01.123Z","trace_id":"t1","span_id":"s1","service":"svc","type":"sql","operation":"SELECT","target":"SELECT 1","duration_us":100,"source":{"endpoint":"GET /test","method":"m"}}]"#;
        let mut client = client;
        client.write_all(line.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.shutdown().await.unwrap();

        // The handler should send the decoded events through the channel.
        let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("should receive events within 2s")
            .expect("channel still open");
        assert_eq!(received.events.len(), 1);
        assert_eq!(received.events[0].trace_id, "t1");

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn handle_json_connection_drops_batches_while_memory_guard_is_tripped() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixStream;

        let (client, server) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::channel::<super::super::IngestBatch>(16);

        // Guard tripped: this door must honor it like the OTLP listeners,
        // else local NDJSON keeps growing RSS while the OTLP doors close.
        let over_memory = Arc::new(AtomicBool::new(true));
        let handle = tokio::spawn(async move {
            handle_json_connection(server, tx, 1024 * 1024, Arc::from(vec![]), over_memory).await;
        });

        let line = r#"[{"timestamp":"2025-07-10T14:32:01.123Z","trace_id":"t1","span_id":"s1","service":"svc","type":"sql","operation":"SELECT","target":"SELECT 1","duration_us":100,"source":{"endpoint":"GET /test","method":"m"}}]"#;
        let mut client = client;
        client.write_all(line.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.shutdown().await.unwrap();

        handle.await.unwrap();
        // The handler exited (EOF) without forwarding anything.
        assert!(
            rx.try_recv().is_err(),
            "batches must be dropped while the guard is tripped"
        );
    }

    #[tokio::test]
    async fn handle_json_connection_skips_oversize_line() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixStream;

        let (client, server) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::channel::<super::super::IngestBatch>(16);

        // Small max_payload so the line is over the limit.
        let handle = spawn_handler(server, tx, 32);

        let mut client = client;
        // This line is > 32 bytes, triggers the "line exceeds max payload size" branch.
        let oversize_line = r#"[{"timestamp":"2025-07-10T14:32:01.123Z","trace_id":"t1","span_id":"s1","service":"svc","type":"sql","operation":"SELECT","target":"x","duration_us":1,"source":{"endpoint":"/","method":"m"}}]"#;
        client.write_all(oversize_line.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.shutdown().await.unwrap();

        let recv = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
        assert!(
            recv.is_err() || recv.unwrap().is_none(),
            "oversize line must be dropped, channel should not receive anything"
        );
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn handle_json_connection_skips_malformed_line() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixStream;

        let (client, server) = UnixStream::pair().unwrap();
        let (tx, mut rx) = mpsc::channel::<super::super::IngestBatch>(16);

        let handle = spawn_handler(server, tx, 1024 * 1024);

        let mut client = client;
        // Malformed: hits the Err(e) branch in the match.
        client.write_all(b"not json at all\n").await.unwrap();
        client.shutdown().await.unwrap();

        let recv = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
        assert!(
            recv.is_err() || recv.unwrap().is_none(),
            "malformed line must be dropped"
        );
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn run_json_socket_accepts_connection_and_forwards_events() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixStream;

        // Keep `_dir` alive until the end of the test. Dropping it removes
        // the socket and the parent tempdir. `path` is a PathBuf owned by us.
        let (_dir, path) = unique_socket_dir_and_path("accept");
        let (tx, mut rx) = mpsc::channel::<super::super::IngestBatch>(16);
        let listener = bind_json_socket(&path.to_string_lossy())
            .unwrap()
            .expect("bound");
        let server = tokio::spawn(async move {
            run_json_socket(
                listener,
                tx,
                1024 * 1024,
                Vec::new(),
                Arc::new(AtomicBool::new(false)),
            )
            .await;
        });

        // Give the listener a brief moment to bind.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect as a client, write one NDJSON line, close.
        let mut client = UnixStream::connect(&path).await.expect("connect to socket");
        let line = r#"[{"timestamp":"2025-07-10T14:32:01.123Z","trace_id":"t-sock","span_id":"s1","service":"svc","type":"sql","operation":"SELECT","target":"SELECT 1","duration_us":100,"source":{"endpoint":"GET /test","method":"m"}}]"#;
        client.write_all(line.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.shutdown().await.unwrap();

        let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("should receive events within 2s")
            .expect("channel still open");
        assert_eq!(received.events.len(), 1);
        assert_eq!(received.events[0].trace_id, "t-sock");

        server.abort();
        let _ = server.await;
        // _dir drops here, removing the socket and parent tempdir.
    }

    #[tokio::test]
    async fn bind_json_socket_disables_the_listener_when_the_dir_is_missing() {
        // A FROM scratch image has no /tmp: the daemon starts without the door.
        let path = "/nonexistent-directory-for-test/perf-sentinel.sock";
        assert!(bind_json_socket(path).unwrap().is_none());
    }

    #[tokio::test]
    async fn bind_json_socket_refuses_to_clobber_symlink() {
        // Symlink-TOCTOU regression guard: a symlink at `path` pointing at a
        // victim file must stop startup and leave the victim untouched.
        use std::os::unix::fs::symlink;

        let (dir, sock_path) = unique_socket_dir_and_path("symlink-guard");
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "important").unwrap();
        symlink(&victim, &sock_path).expect("symlink creation");

        assert!(bind_json_socket(&sock_path.to_string_lossy()).is_err());
        let content = std::fs::read_to_string(&victim)
            .expect("victim file must still exist after symlink refusal");
        assert_eq!(content, "important");
        assert!(sock_path.symlink_metadata().is_ok(), "symlink kept");
    }
}
