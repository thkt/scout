use std::env;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};

use reqwest::Client;
use reqwest::redirect::Policy;
use wiremock::MockServer;

/// Build a reqwest `Client` with redirects disabled. No connect or read
/// timeouts are set; wrap calls in `tokio::time::timeout` if a bounded test
/// is needed.
pub(crate) fn no_redirect_client() -> Client {
    Client::builder().redirect(Policy::none()).build().unwrap()
}

static NETWORK_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Spawn a wiremock server, returning `None` if loopback bind is unavailable.
pub async fn try_spawn_mock_server(test_name: &str) -> Option<MockServer> {
    let force = env::var("SCOUT_NETWORK_TESTS").is_ok();
    try_spawn_with_bind(test_name, TcpListener::bind("127.0.0.1:0"), force).await
}

/// Testable core: inject bind result and force flag to control skip-vs-panic.
pub async fn try_spawn_with_bind(
    test_name: &str,
    bind_result: io::Result<TcpListener>,
    force: bool,
) -> Option<MockServer> {
    match bind_result {
        Ok(listener) => Some(MockServer::builder().listener(listener).start().await),
        Err(e) => {
            if force {
                panic!(
                    "[network-guard] {test_name}: bind failed and SCOUT_NETWORK_TESTS is set: {e}"
                );
            }
            let count = NETWORK_SKIP_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(
                "[network-guard] {test_name}: loopback bind unavailable, early return ({count} skipped)"
            );
            None
        }
    }
}

/// One-shot server that accepts up to `accept_count` connections and replies
/// with an HTTP/1.1 response declaring `Content-Length: 1000` but writing
/// only `hello` before dropping the socket. reqwest surfaces the resulting
/// mid-stream close as `is_decode() == true` with an `io::Error` of kind
/// `UnexpectedEof` in the source chain (issue #113).
///
/// Returns `None` when loopback bind is unavailable so callers can early-return
/// in restricted environments, matching the `try_spawn_mock_server` pattern.
/// The returned `AtomicUsize` counts how many connections were accepted so
/// callers can confirm the retry loop kicked in.
///
/// `accept_count` must equal the number of connections the client will make;
/// passing a larger value blocks the spawned thread on `listener.accept()`
/// and makes `handle.join()` hang.
pub fn spawn_mid_stream_drop_server(
    accept_count: usize,
) -> Option<(String, Arc<AtomicUsize>, JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let addr = listener.local_addr().ok()?;
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);
    let handle = thread::spawn(move || {
        for _ in 0..accept_count {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            counter_clone.fetch_add(1, Ordering::SeqCst);
            // Drain the request before replying so reqwest observes the
            // close as a mid-stream body drop on `json().await`, not as a
            // write error during `send().await`.
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nhello");
        }
    });
    Some((format!("http://{addr}"), counter, handle))
}

/// One-shot server that accepts one connection and replies with a
/// close-delimited HTTP/1.1 response: no `Content-Length`, no
/// `Transfer-Encoding`, `Connection: close`, then `body_size` body bytes
/// before dropping the socket (EOF delimits the body). reqwest sees
/// `content_length() == None`, so `read_body_capped`'s pre-check goes inert
/// and the chunk loop becomes the live cap guard — the path a compressed or
/// Content-Length-absent upstream drives (issue #219).
///
/// Returns `None` when loopback bind is unavailable so callers can
/// early-return in restricted environments, matching
/// `spawn_mid_stream_drop_server`.
pub fn spawn_close_delimited_body_server(body_size: usize) -> Option<(String, JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let addr = listener.local_addr().ok()?;
    let handle = thread::spawn(move || {
        // Single-shot: the test makes exactly one connection, so a failed
        // accept is a test-environment fault — panic loudly rather than
        // hang the joining test on a silent return.
        let (mut stream, _) = listener.accept().expect("accept loopback connection");
        // Drain the request so the write below is the response, not racing
        // an unread request buffer.
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
        let _ = stream.write_all(&vec![b'x'; body_size]);
        // stream drops here → socket close is the body's EOF delimiter.
    });
    Some((format!("http://{addr}"), handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use tracing_test::traced_test;

    #[tokio::test]
    async fn try_spawn_mock_server_returns_some_in_normal_env() {
        let Some(server) = try_spawn_mock_server("normal_env").await else {
            return; // bind unavailable — can't verify happy path
        };

        let uri = server.uri();
        assert!(
            uri.starts_with("http://127.0.0.1:"),
            "MockServer URI should be on loopback: {uri}"
        );
    }

    #[traced_test]
    #[tokio::test]
    async fn bind_failure_without_force_returns_none_and_warns() {
        let bind_err: io::Result<TcpListener> = Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mock bind failure",
        ));

        let result = try_spawn_with_bind("permission_denied", bind_err, false).await;

        assert!(
            result.is_none(),
            "try_spawn_with_bind should return None on bind failure"
        );
        assert!(logs_contain("permission_denied"));
    }

    #[tokio::test]
    #[should_panic(expected = "forced_panic")]
    async fn bind_failure_with_force_panics() {
        let bind_err: io::Result<TcpListener> = Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mock bind failure",
        ));

        let _result = try_spawn_with_bind("forced_panic", bind_err, true).await;
    }
}
