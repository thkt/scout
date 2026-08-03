//! Shared test scaffolding for integration test binaries under `tests/`.
//!
//! `tests/*.rs` binaries compile as separate crates from `src/`, so they
//! cannot see `src/test_support.rs`'s items even where that module marks them
//! `pub(crate)` — the `pub(crate)` visibility scopes to the `scout` crate
//! itself, not to a downstream integration-test crate that merely links
//! against it. That module also stays private on purpose (making it `pub`
//! would put a test-only server on the library's public API), so it cannot be
//! re-exported either. A helper `tests/` needs has to live here instead, even
//! where it mirrors something `src/test_support.rs` already does.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Forward proxy mock that loops `accept` instead of serving one connection,
/// so a client that re-dials (retry after failure, connection churn, ...)
/// keeps getting served instead of hitting a closed listener.
///
/// Mirrors `spawn_forward_proxy` in `src/test_support.rs` (bind loopback,
/// hand the connection to a spawned thread, write a canned response), with
/// two deviations the plan calls for: the single `listener.accept()` call
/// becomes a loop, and the fixed 200/no-delay/text-body response becomes the
/// caller-supplied `status` / `delay` / `body` below.
///
/// - `status`: the numeric status line code the response opens with (e.g.
///   `200`, `503`). The reason phrase is always written as `OK` regardless of
///   `status`; a caller asserting on the reason phrase itself needs a
///   different helper.
/// - `delay`: sleep before writing the response, so a caller can simulate a
///   slow proxy or upstream.
/// - `body`: written verbatim after a `Content-Length` header sized to it, so
///   arbitrary (including non-UTF-8) payloads round-trip byte for byte —
///   unlike `spawn_forward_proxy`'s `&str` body.
///
/// Returns `(base_url, connection_count, join_handle)`, or `None` when
/// loopback bind is unavailable, matching `spawn_forward_proxy`'s early
/// return for restricted environments. `connection_count` increments once
/// per accepted connection, so a test that drives several requests (a proxy
/// retry, several keep-alive-less calls, ...) through the returned base URL
/// can assert how many dials actually reached the proxy.
///
/// The accept loop has no exit condition other than a fatal `accept` error
/// (e.g. the OS closing the socket), so in the happy path the spawned thread
/// runs until the test process itself exits — `join_handle` is returned for
/// completeness and for a caller that wants to detect that fatal-error exit,
/// not for a caller to `.join()` and wait on, which would hang.
pub fn spawn_mock_proxy(
    status: u16,
    delay: Duration,
    body: &[u8],
) -> Option<(String, Arc<AtomicUsize>, JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let addr = listener.local_addr().ok()?;
    let connection_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connection_count);
    let body = body.to_vec();
    let handle = thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                // Fatal accept error (e.g. listener torn down): stop looping
                // rather than spin.
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            // Drain the request so the write below is the response, not
            // racing an unread request buffer, matching
            // `spawn_forward_proxy`'s rationale.
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            let mut response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            response.extend_from_slice(&body);
            let _ = stream.write_all(&response);
            // stream drops here → connection closes after the framed body,
            // which is what forces a keep-alive-unaware client to re-dial
            // for its next request and exercise the accept loop again.
        }
    });
    Some((format!("http://{addr}"), connection_count, handle))
}
