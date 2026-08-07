//! Shared test scaffolding, and the test-id convention every test module follows.
//!
//! # Test ids
//!
//! A test carries `[T-<PREFIX><NNN>]` as the first thing in its doc comment. DRs
//! cite those ids to name the test that pins a decision, so an id has to resolve
//! to exactly one test:
//!
//! - The prefix names the subject under test (`FS` = fetch/ssrf, `SK` = slack,
//!   `TOK` = token_source, ...), so one prefix covers several files when they
//!   test the same thing — `SK` spans `slack/` and the `fetch <slack-url>` tests
//!   in `tools/`. What a prefix must not do is cover two unrelated subjects:
//!   `R` once meant both retry and the stdin resolver, which left the id
//!   ambiguous even where the numbers differed.
//! - Numbers are unique within their prefix, not per file.
//!
//! Cite another test **without** brackets: `Companion to T-TS020`. Brackets mark a
//! definition, so a bracketed citation is indistinguishable from a second
//! definition — by grep, and by a reader scanning for where an id lives.

use std::env;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use reqwest::Client;
use reqwest::redirect::Policy;
use wiremock::{MockServer, ResponseTemplate};

/// Build a reqwest `Client` with redirects disabled. No connect or read
/// timeouts are set; wrap calls in `tokio::time::timeout` if a bounded test
/// is needed.
pub(crate) fn no_redirect_client() -> Client {
    Client::builder().redirect(Policy::none()).build().unwrap()
}

/// Produce a real "connection refused" `reqwest::Error` deterministically:
/// reserve a loopback port, then drop the listener so the port closes
/// synchronously (no async shutdown race, unlike `MockServer`), and GET it.
///
/// Returns `None` when loopback bind is unavailable so callers can
/// early-return in restricted environments, matching `try_spawn_mock_server`.
pub(crate) async fn connection_refused_error(test_name: &str) -> Option<reqwest::Error> {
    let listener = bind_loopback(test_name)?;
    let addr = listener.local_addr().expect("local_addr");
    drop(listener);

    Some(
        Client::new()
            .get(format!("http://{addr}/should-refuse"))
            .send()
            .await
            .expect_err("request to dead port should fail"),
    )
}

/// Mount a `users.info` responder that resolves every lookup to a name.
///
/// The body is the one shape `UserBody` / `UserDetail` accept, so it lives in
/// one place: a change to that deserializer has to change this fixture, and
/// twelve copies of it would each have to be found.
pub(crate) async fn mount_users_info_resolving(server: &MockServer) {
    mount_get(
        server,
        "/users.info",
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "user": {"real_name": "Someone"}
        })),
    )
    .await;
}

/// Covers the plain `method(GET) + path + respond_with + mount` shape only.
/// A mock that also matches on query params or asserts a call count encodes
/// that condition as part of what the test verifies, so those stay
/// hand-written at the call site.
pub(crate) async fn mount_get(server: &MockServer, path: &str, template: ResponseTemplate) {
    use wiremock::Mock;
    use wiremock::matchers::{method, path as path_matcher};

    Mock::given(method("GET"))
        .and(path_matcher(path))
        .respond_with(template)
        .mount(server)
        .await;
}

/// The single bind-failure decision every loopback-binding helper routes
/// through, so a restricted environment produces one uniform outcome across
/// the suite: skip (`None` + warn), or a panic when `SCOUT_NETWORK_TESTS`
/// asserts the network must exist.
fn guard_loopback_bind(
    test_name: &str,
    bind_result: io::Result<TcpListener>,
    force: bool,
) -> Option<TcpListener> {
    match bind_result {
        Ok(listener) => Some(listener),
        Err(e) => {
            if force {
                panic!(
                    "[network-guard] {test_name}: bind failed and SCOUT_NETWORK_TESTS is set: {e}"
                );
            }
            tracing::warn!("[network-guard] {test_name}: loopback bind unavailable, early return");
            None
        }
    }
}

/// Bind a loopback listener under the shared guard policy.
fn bind_loopback(test_name: &str) -> Option<TcpListener> {
    let force = env::var("SCOUT_NETWORK_TESTS").is_ok();
    guard_loopback_bind(test_name, TcpListener::bind("127.0.0.1:0"), force)
}

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
    let listener = guard_loopback_bind(test_name, bind_result, force)?;
    Some(MockServer::builder().listener(listener).start().await)
}

/// The one primitive every one-shot test server routes through.
///
/// `respond` runs once per accepted connection, so `Fn` rather than
/// `FnOnce`/`FnMut`. Its failure does not cut the accept loop short: a
/// caller asserting on the counter (a retry-budget test) needs every one of
/// `accept_count` connections accepted and counted even when a mid-loop
/// write fails, so the loop runs to completion and the *first* error
/// surfaces afterwards.
pub(crate) fn spawn_accept_loop<F>(
    test_name: &str,
    accept_count: usize,
    respond: F,
) -> Option<(String, Arc<AtomicUsize>, JoinHandle<io::Result<()>>)>
where
    F: Fn(&mut TcpStream) -> io::Result<()> + Send + 'static,
{
    let listener = bind_loopback(test_name)?;
    let addr = listener.local_addr().ok()?;
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);
    let handle = thread::spawn(move || -> io::Result<()> {
        let mut first_err = None;
        for _ in 0..accept_count {
            let (mut stream, _) = listener.accept()?;
            counter_clone.fetch_add(1, Ordering::SeqCst);
            // Drain the request before replying so reqwest observes
            // whatever `respond` writes as the response, not racing an
            // unread request buffer.
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            if let Err(e) = respond(&mut stream) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    });
    Some((format!("http://{addr}"), counter, handle))
}

/// Joins a `spawn_accept_loop` server thread and asserts neither the thread
/// panicked nor `respond` returned an `Err`. Discarding the join result
/// instead would leave an accept or write failure entirely silent, which is
/// what this replaces.
pub(crate) fn join_server_thread(handle: JoinHandle<io::Result<()>>) {
    join_server_thread_with_deadline(handle, Duration::from_secs(5));
}

/// Testable core: inject a deadline to control how long a join waits on a
/// server thread that never finishes.
///
/// Polls `JoinHandle::is_finished` instead of blocking on `join` directly, so
/// a thread stuck in `listener.accept()` fails with a diagnosis instead of
/// hanging the test. Once the deadline elapses the handle is dropped rather
/// than joined: joining a still-running thread blocks again, which is
/// exactly the hang this guards against.
pub(crate) fn join_server_thread_with_deadline(
    handle: JoinHandle<io::Result<()>>,
    deadline: Duration,
) {
    let started = Instant::now();
    while !handle.is_finished() {
        if started.elapsed() >= deadline {
            drop(handle);
            panic!(
                "server thread did not finish within {deadline:?}; likely cause: accept_count exceeds the number of client connections, so the thread blocks in listener.accept()"
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
    handle
        .join()
        .expect("server thread should not panic")
        .expect("server thread should not fail while writing the response");
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
/// passing a larger value blocks the spawned thread on `listener.accept()`.
/// `join_server_thread` no longer hangs on that: it panics naming
/// `accept_count` once its deadline elapses (`join_server_thread_with_deadline`).
pub fn spawn_mid_stream_drop_server(
    accept_count: usize,
) -> Option<(String, Arc<AtomicUsize>, JoinHandle<io::Result<()>>)> {
    spawn_accept_loop("spawn_mid_stream_drop_server", accept_count, |stream| {
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nhello")
    })
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
pub fn spawn_close_delimited_body_server(
    body_size: usize,
) -> Option<(String, JoinHandle<io::Result<()>>)> {
    let (addr, _counter, handle) =
        spawn_accept_loop("spawn_close_delimited_body_server", 1, move |stream| {
            stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")?;
            stream.write_all(&vec![b'x'; body_size])
        })?;
    Some((addr, handle))
}

/// One-shot server that declares `Content-Length: declared_len` in the
/// response head and then closes the connection without writing a single
/// body byte. Mirrors `spawn_close_delimited_body_server`'s shape (bind,
/// accept once, drain the request, write the response, drop the stream) but
/// controls the header instead of the framing.
///
/// Proves `read_body_capped`'s pre-check rejects an oversized declared
/// length before it reads any body byte: since zero body bytes are ever
/// written, an implementation that tried to read past the pre-check would
/// see the connection close before satisfying `declared_len`, which reqwest
/// surfaces as a decode/network error — not `too_large`. Observing
/// `too_large` therefore is itself the proof that the body was never read
/// (issue #219 / TC-006).
///
/// Returns `None` when loopback bind is unavailable so callers can
/// early-return in restricted environments, matching
/// `spawn_close_delimited_body_server`.
pub fn spawn_declared_length_no_body_server(
    declared_len: usize,
) -> Option<(String, JoinHandle<io::Result<()>>)> {
    let (addr, _counter, handle) =
        spawn_accept_loop("spawn_declared_length_no_body_server", 1, move |stream| {
            stream.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {declared_len}\r\n\r\n").as_bytes(),
            )
        })?;
    Some((addr, handle))
}

/// One-shot forward proxy: binds loopback, accepts exactly one connection,
/// drains the absolute-form request line reqwest sends an HTTP proxy
/// (`GET http://example.com/... HTTP/1.1`), and replies with a canned
/// `200 OK` HTML body regardless of the requested target. Mirrors
/// `spawn_close_delimited_body_server`; used to prove `fetch_page` in Proxied
/// egress mode routes through the proxy without consulting scout's DNS
/// resolver. `body` is returned verbatim, Content-Length framed.
///
/// Returns `None` when loopback bind is unavailable so callers can early-return
/// in restricted environments, matching `spawn_close_delimited_body_server`.
pub fn spawn_forward_proxy(body: &str) -> Option<(String, JoinHandle<io::Result<()>>)> {
    let body = body.to_owned();
    let (addr, _counter, handle) = spawn_accept_loop("spawn_forward_proxy", 1, move |stream| {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes())
    })?;
    Some((addr, handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::panic::{AssertUnwindSafe, catch_unwind};
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

    /// [T-SUP001] respond 閉包が Err を返すとサーバ thread の join 結果が Err になる
    #[test]
    fn respond_err_makes_thread_join_result_err() {
        let Some((addr, _counter, handle)) = spawn_accept_loop(
            "respond_err_makes_thread_join_result_err",
            1,
            |_stream: &mut TcpStream| -> io::Result<()> { Err(io::Error::other("respond failed")) },
        ) else {
            return; // bind unavailable — can't verify happy path
        };

        let host = addr
            .strip_prefix("http://")
            .expect("spawn_accept_loop should return an http:// URL");
        let _ = TcpStream::connect(host);

        let result = handle.join().expect("server thread should not panic");
        assert!(
            result.is_err(),
            "respond closure's io failure should surface via the join result"
        );
    }

    /// [T-SUP002] respond 閉包が Err を返した接続の後も accept ループは続き接続カウンタは accept_count に達する
    #[test]
    fn accept_loop_continues_past_respond_err_until_accept_count() {
        let accept_count = 3;
        let Some((addr, counter, handle)) = spawn_accept_loop(
            "accept_loop_continues_past_respond_err_until_accept_count",
            accept_count,
            |_stream: &mut TcpStream| -> io::Result<()> { Err(io::Error::other("respond failed")) },
        ) else {
            return; // bind unavailable — can't verify happy path
        };

        let host = addr
            .strip_prefix("http://")
            .expect("spawn_accept_loop should return an http:// URL");
        for _ in 0..accept_count {
            let _ = TcpStream::connect(host);
        }

        let _ = handle.join().expect("server thread should not panic");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            accept_count,
            "accept loop should keep accepting connections after a respond error"
        );
    }

    /// [T-SUP003] accept_count がクライアントの接続数を超えたサーバ thread は deadline 経過で accept_count を名指す panic になる
    #[test]
    #[should_panic(expected = "accept_count")]
    fn accept_count_exceeding_client_connections_panics_naming_accept_count_after_deadline() {
        let Some((_addr, _counter, handle)) = spawn_accept_loop(
            "accept_count_exceeding_client_connections_panics_naming_accept_count_after_deadline",
            1,
            |_stream: &mut TcpStream| -> io::Result<()> { Ok(()) },
        ) else {
            return; // bind unavailable — can't verify happy path
        };
        // No client ever connects, so the thread blocks forever in
        // listener.accept() — the deadline is the only way out.

        join_server_thread_with_deadline(handle, Duration::from_millis(200));
    }

    /// [T-SUP004] 完了済みのサーバ thread は deadline を待たずに join が返る
    #[test]
    fn finished_server_thread_returns_before_deadline_elapses() {
        let Some((addr, _counter, handle)) = spawn_accept_loop(
            "finished_server_thread_returns_before_deadline_elapses",
            1,
            |_stream: &mut TcpStream| -> io::Result<()> { Ok(()) },
        ) else {
            return; // bind unavailable — can't verify happy path
        };

        let host = addr
            .strip_prefix("http://")
            .expect("spawn_accept_loop should return an http:// URL");
        let _ = TcpStream::connect(host);

        let deadline = Duration::from_secs(5);
        let started = Instant::now();
        join_server_thread_with_deadline(handle, deadline);
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(500),
            "a finished server thread should join immediately, not wait out the {deadline:?} deadline; took {elapsed:?}"
        );
    }

    /// [T-SUP005] respond が Err を返したサーバ thread は deadline 前に既存のメッセージで panic する
    #[test]
    fn respond_err_panics_with_existing_message_before_deadline() {
        let Some((addr, _counter, handle)) = spawn_accept_loop(
            "respond_err_panics_with_existing_message_before_deadline",
            1,
            |_stream: &mut TcpStream| -> io::Result<()> { Err(io::Error::other("respond failed")) },
        ) else {
            return; // bind unavailable — can't verify happy path
        };

        let host = addr
            .strip_prefix("http://")
            .expect("spawn_accept_loop should return an http:// URL");
        let _ = TcpStream::connect(host);

        let deadline = Duration::from_secs(5);
        let started = Instant::now();
        let result = catch_unwind(AssertUnwindSafe(|| {
            join_server_thread_with_deadline(handle, deadline);
        }));
        let elapsed = started.elapsed();

        let panic_payload = result.expect_err("respond Err should panic, not return Ok");
        let message = panic_payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                panic_payload
                    .downcast_ref::<&str>()
                    .map(ToString::to_string)
            })
            .unwrap_or_default();
        assert!(
            message.contains("server thread should not fail while writing the response"),
            "panic message should keep the existing respond-Err diagnostic, got: {message}"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "respond-Err panic should fire immediately, not wait out the {deadline:?} deadline; took {elapsed:?}"
        );
    }
}
