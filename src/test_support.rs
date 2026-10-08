//! Shared test scaffolding and test-id conventions.
//!
//! Each test doc starts with `[T-<PREFIX><NNN>]`. Prefixes identify subjects;
//! numbers are unique within each prefix across files. DRs cite these IDs.
//! Cite other tests without brackets: brackets define an ID.
//!
//! Test docs name actual detection conditions and necessary fixture premises,
//! without repeating implementation rationale.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use reqwest::Client;
use reqwest::redirect::Policy;
use wiremock::{MockServer, ResponseTemplate};

/// Redirects are disabled; callers must bound requests that need a timeout.
pub(crate) fn no_redirect_client() -> Client {
    Client::builder().redirect(Policy::none()).build().unwrap()
}

/// Reserve and synchronously close a loopback port before requesting it.
/// Returns None if the bind guard permits skipping; the closed port is not reserved.
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

/// Mounts the minimal users.info response accepted by UserBody/UserDetail.
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

/// GET/path responder; keep query matching and call-count assertions at call sites.
pub(crate) async fn mount_get(server: &MockServer, path: &str, template: ResponseTemplate) {
    use wiremock::Mock;
    use wiremock::matchers::{method, path as path_matcher};

    Mock::given(method("GET"))
        .and(path_matcher(path))
        .respond_with(template)
        .mount(server)
        .await;
}

/// Bind failure skips locally but panics when SCOUT_NETWORK_TESTS is set.
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

fn bind_loopback(test_name: &str) -> Option<TcpListener> {
    let force = env::var("SCOUT_NETWORK_TESTS").is_ok();
    guard_loopback_bind(test_name, TcpListener::bind("127.0.0.1:0"), force)
}

pub(crate) async fn try_spawn_mock_server(test_name: &str) -> Option<MockServer> {
    let force = env::var("SCOUT_NETWORK_TESTS").is_ok();
    try_spawn_with_bind(test_name, TcpListener::bind("127.0.0.1:0"), force).await
}

async fn try_spawn_with_bind(
    test_name: &str,
    bind_result: io::Result<TcpListener>,
    force: bool,
) -> Option<MockServer> {
    let listener = guard_loopback_bind(test_name, bind_result, force)?;
    Some(MockServer::builder().listener(listener).start().await)
}

/// Accepts and counts every requested connection even if responses fail,
/// then returns the first response error.
fn spawn_accept_loop<F>(
    test_name: &str,
    accept_count: usize,
    respond: F,
) -> Option<(String, Arc<AtomicUsize>, JoinHandle<io::Result<()>>)>
where
    F: Fn(&mut TcpStream) -> io::Result<()> + Send + 'static,
{
    let listener = bind_loopback(test_name)?;
    // Address lookup failure must not skip a scenario after a successful bind.
    let addr = listener
        .local_addr()
        .expect("a bound listener reports its address");
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);
    let handle = thread::spawn(move || -> io::Result<()> {
        let mut first_err = None;
        for _ in 0..accept_count {
            let (mut stream, _) = listener.accept()?;
            counter_clone.fetch_add(1, Ordering::SeqCst);
            // Read once to wait for request data before replying; unread bytes may remain.
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

/// Joins within 5s and surfaces thread/response failures before nextest
/// reaches its 120s slow timeout.
pub(crate) fn join_server_thread(handle: JoinHandle<io::Result<()>>) {
    join_server_thread_with_deadline(handle, Duration::from_secs(5));
}

/// Drops an unfinished handle at the deadline; joining it would block again.
fn join_server_thread_with_deadline(handle: JoinHandle<io::Result<()>>, deadline: Duration) {
    let started = Instant::now();
    while !handle.is_finished() {
        if started.elapsed() >= deadline {
            drop(handle);
            panic!(
                "server thread did not finish within {deadline:?}; likely cause: \
                 accept_count exceeds the number of client connections, so the \
                 thread blocks in listener.accept()"
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
    handle
        .join()
        .expect("server thread should not panic")
        .expect("server thread should not fail while writing the response");
}

/// Sends only `hello` despite Content-Length: 1000, producing a truncated body.
/// `accept_count` must match client connections or accept blocks until the
/// bounded join fails. Bind failure uses the SCOUT_NETWORK_TESTS guard.
pub(crate) fn spawn_mid_stream_drop_server(
    accept_count: usize,
) -> Option<(String, Arc<AtomicUsize>, JoinHandle<io::Result<()>>)> {
    spawn_accept_loop("spawn_mid_stream_drop_server", accept_count, |stream| {
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nhello")
    })
}

/// Sends a close-delimited body without Content-Length or Transfer-Encoding,
/// exercising the streaming body cap. Bind failure uses the network guard.
pub(crate) fn spawn_close_delimited_body_server(
    body_size: usize,
) -> Option<(String, JoinHandle<io::Result<()>>)> {
    let (addr, _counter, handle) =
        spawn_accept_loop("spawn_close_delimited_body_server", 1, move |stream| {
            stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")?;
            stream.write_all(&vec![b'x'; body_size])
        })?;
    Some((addr, handle))
}

/// Declares `declared_len` then closes without body bytes, distinguishing
/// oversized-header rejection from a body decode error. Uses the network guard.
pub(crate) fn spawn_declared_length_no_body_server(
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

/// Returns the supplied HTML with Content-Length for one forward-proxy request.
/// Bind failure uses the SCOUT_NETWORK_TESTS guard.
pub(crate) fn spawn_forward_proxy(body: &str) -> Option<(String, JoinHandle<io::Result<()>>)> {
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

struct ScannedToken {
    file: PathBuf,
    token: String,
}

/// Preserve cited legacy T-201 IDs (DR-0021/DR-0012). This closed allowlist
/// does not admit new digit-leading IDs.
const DIGIT_LEADING_ALLOWLIST: &[&str] = &[
    "201-1", "201-2", "201-3", "201-4", "201-5", "201-6", "201-8", "201-9", "201-10", "201-11",
    "201-12", "201-13", "201-14", "201-15", "201-16",
];

fn scan_test_id_violations() -> Vec<String> {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut occurrences = Vec::new();
    for dir in ["src", "tests"] {
        collect_occurrences(
            &crate_root.join(dir),
            extract_bracketed_test_ids,
            &mut occurrences,
        );
    }
    find_test_id_violations(&occurrences)
}

/// Scans src/tests only; requirement citations remain allowed in docs (DR-0013).
fn scan_requirement_code_violations() -> Vec<String> {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut occurrences = Vec::new();
    for dir in ["src", "tests"] {
        collect_occurrences(
            &crate_root.join(dir),
            extract_requirement_codes,
            &mut occurrences,
        );
    }
    // Exempt only this exact file, which supplies requirement-code fixtures.
    // A suffix match would also exempt unrelated nested test_support.rs files.
    let this_file = crate_root.join("src").join("test_support.rs");
    occurrences.retain(|o| o.file != this_file);
    find_requirement_code_violations(&occurrences)
}

fn find_test_id_violations(occurrences: &[ScannedToken]) -> Vec<String> {
    let mut violations = Vec::new();
    let mut first_seen: HashMap<&str, &Path> = HashMap::new();

    for occurrence in occurrences {
        let id = occurrence.token.as_str();
        let file = occurrence.file.display();

        if id.starts_with(|c: char| c.is_ascii_digit()) && !DIGIT_LEADING_ALLOWLIST.contains(&id) {
            violations.push(format!("{file}: test id [T-{id}] starts with a digit"));
        }

        match first_seen.get(id) {
            Some(first_file) => violations.push(format!(
                "{file}: duplicate test id [T-{id}], already defined in {}",
                first_file.display()
            )),
            None => {
                first_seen.insert(id, &occurrence.file);
            }
        }
    }

    violations
}

fn find_requirement_code_violations(occurrences: &[ScannedToken]) -> Vec<String> {
    occurrences
        .iter()
        .map(|occurrence| {
            format!(
                "{}: requirement code `{}` should not appear in src/tests; cite it from docs/ instead (see ADR-0013)",
                occurrence.file.display(),
                occurrence.token
            )
        })
        .collect()
}

fn collect_occurrences(
    dir: &Path,
    extract: fn(&str) -> Vec<String>,
    occurrences: &mut Vec<ScannedToken>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_occurrences(&path, extract, occurrences);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            for token in extract(&contents) {
                occurrences.push(ScannedToken {
                    file: path.clone(),
                    token,
                });
            }
        }
    }
}

/// Extracts bracketed IDs containing ASCII letters, digits or hyphens.
/// The convention placeholder is not an ID.
fn extract_bracketed_test_ids(contents: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut offset = 0;

    while let Some(rel_open) = contents[offset..].find("[T-") {
        let after_prefix = &contents[offset + rel_open + "[T-".len()..];
        let id_len = after_prefix
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(after_prefix.len());
        if id_len > 0 && after_prefix[id_len..].starts_with(']') {
            ids.push(after_prefix[..id_len].to_owned());
        }
        offset += rel_open + "[T-".len();
    }

    ids
}

/// Match bare FR/BR/NFR prefixes with exactly three ASCII digits, with no
/// preceding ASCII letter or following digit; backticks are not required.
fn extract_requirement_codes(contents: &str) -> Vec<String> {
    const PREFIXES: [&str; 3] = ["NFR-", "FR-", "BR-"];
    let mut codes = Vec::new();

    for prefix in PREFIXES {
        let mut offset = 0;
        while let Some(rel) = contents[offset..].find(prefix) {
            let start = offset + rel;
            let after = &contents[start + prefix.len()..];
            let digits_then_boundary = after.len() >= 3
                && after.as_bytes()[..3].iter().all(u8::is_ascii_digit)
                && after[3..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_ascii_digit());
            // Reject letter-preceded hits, including FR inside NFR.
            let standalone = contents[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphabetic());
            if digits_then_boundary && standalone {
                codes.push(format!("{prefix}{}", &after[..3]));
            }
            offset = start + prefix.len();
        }
    }

    codes
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
            return;
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

    /// [T-SUP001] When the respond closure returns Err, the server thread's join result is Err
    #[test]
    fn respond_err_makes_thread_join_result_err() {
        let Some((addr, _counter, handle)) = spawn_accept_loop(
            "respond_err_makes_thread_join_result_err",
            1,
            |_stream: &mut TcpStream| -> io::Result<()> { Err(io::Error::other("respond failed")) },
        ) else {
            return;
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

    /// [T-SUP002] The accept loop continues past a connection whose respond closure
    /// returned Err, and the counter reaches accept_count
    #[test]
    fn accept_loop_continues_past_respond_err_until_accept_count() {
        let accept_count = 3;
        let Some((addr, counter, handle)) = spawn_accept_loop(
            "accept_loop_continues_past_respond_err_until_accept_count",
            accept_count,
            |_stream: &mut TcpStream| -> io::Result<()> { Err(io::Error::other("respond failed")) },
        ) else {
            return;
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

    /// [T-SUP003] Too few client connections cause a bounded join panic naming accept_count.
    /// The blocked thread is detached and may be reported as leaky by nextest.
    #[test]
    #[should_panic(expected = "accept_count")]
    fn accept_count_exceeding_client_connections_panics_naming_accept_count_after_deadline() {
        let Some((addr, _counter, handle)) = spawn_accept_loop(
            "accept_count_exceeding_client_connections_panics_naming_accept_count_after_deadline",
            2,
            |_stream: &mut TcpStream| -> io::Result<()> { Ok(()) },
        ) else {
            return;
        };

        let host = addr
            .strip_prefix("http://")
            .expect("spawn_accept_loop should return an http:// URL");
        let _ = TcpStream::connect(host);

        join_server_thread_with_deadline(handle, Duration::from_millis(200));
    }

    /// [T-SUP004] A successful thread joins within 500ms against a 5s deadline.
    /// Direct thread creation avoids an unrelated loopback dependency.
    #[test]
    fn finished_server_thread_returns_before_deadline_elapses() {
        let handle = thread::spawn(|| -> io::Result<()> { Ok(()) });

        let deadline = Duration::from_secs(5);
        let started = Instant::now();
        join_server_thread_with_deadline(handle, deadline);

        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "a finished server thread should join immediately, not wait out the {deadline:?} deadline; took {elapsed:?}"
        );
    }

    /// [T-SUP005] Thread error retains its diagnostic and panics within 500ms.
    #[test]
    fn server_thread_err_panics_with_existing_message_before_deadline() {
        let handle =
            thread::spawn(|| -> io::Result<()> { Err(io::Error::other("respond failed")) });

        let deadline = Duration::from_secs(5);
        let started = Instant::now();
        let result = catch_unwind(AssertUnwindSafe(|| {
            join_server_thread_with_deadline(handle, deadline);
        }));
        let elapsed = started.elapsed();

        let panic_payload = result.expect_err("respond Err should panic, not return Ok");
        // Result::expect produces a formatted String panic payload.
        let message = panic_payload
            .downcast_ref::<String>()
            .expect("expect's panic payload is a formatted String");
        assert!(
            message.contains("server thread should not fail while writing the response"),
            "panic message should keep the existing respond-Err diagnostic, got: {message}"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "respond-Err panic should fire immediately, not wait out the {deadline:?} deadline; took {elapsed:?}"
        );
    }

    /// [T-SUP006] Input carrying an ID that starts with a digit is reported as a violation
    #[test]
    fn digit_leading_id_is_reported_as_violation() {
        let occurrences = vec![ScannedToken {
            file: PathBuf::from("fake/digit_leading_tests.rs"),
            token: "042ABC".to_owned(),
        }];

        let violations = find_test_id_violations(&occurrences);

        assert!(
            violations
                .iter()
                .any(|v| v.contains("fake/digit_leading_tests.rs") && v.contains("042ABC")),
            "digit-leading id should be reported by file and id, got: {violations:?}"
        );
    }

    /// [T-SUP007] The allow-listed 201-8 ID is accepted.
    #[test]
    fn t201_family_id_is_not_reported_as_violation() {
        let occurrences = vec![ScannedToken {
            file: PathBuf::from("src/fetch/cdp/launch/cdp_launch_tests.rs"),
            token: "201-8".to_owned(),
        }];

        let violations = find_test_id_violations(&occurrences);

        assert!(
            violations.is_empty(),
            "T-201-8 is allow-listed and should not be reported, got: {violations:?}"
        );
    }

    /// [T-SUP008] Input where the same ID appears twice is reported as a duplicate
    #[test]
    fn duplicate_id_across_files_is_reported_as_violation() {
        let occurrences = vec![
            ScannedToken {
                file: PathBuf::from("fake/a_tests.rs"),
                token: "FS022".to_owned(),
            },
            ScannedToken {
                file: PathBuf::from("fake/b_tests.rs"),
                token: "FS022".to_owned(),
            },
        ];

        let violations = find_test_id_violations(&occurrences);

        assert!(
            violations
                .iter()
                .any(|v| v.contains("FS022") && v.to_lowercase().contains("duplicate")),
            "duplicate id across files should be reported, got: {violations:?}"
        );
    }

    /// [T-SUP010] Duplicate IDs within one file are reported.
    #[test]
    fn duplicate_id_within_one_file_is_reported_as_violation() {
        let occurrences = vec![
            ScannedToken {
                file: PathBuf::from("fake/a_tests.rs"),
                token: "SLC016".to_owned(),
            },
            ScannedToken {
                file: PathBuf::from("fake/a_tests.rs"),
                token: "SLC016".to_owned(),
            },
        ];

        let violations = find_test_id_violations(&occurrences);

        assert!(
            violations
                .iter()
                .any(|v| v.contains("SLC016") && v.to_lowercase().contains("duplicate")),
            "a duplicate inside one file should be reported, got: {violations:?}"
        );
    }

    /// [T-SUP011] The allow-listed 201-1 ID is accepted.
    #[test]
    fn t201_1_added_to_allowlist_is_not_reported_as_violation() {
        let occurrences = vec![ScannedToken {
            file: PathBuf::from("src/fetch/cdp/proxy/proxy_tests.rs"),
            token: "201-1".to_owned(),
        }];

        let violations = find_test_id_violations(&occurrences);

        assert!(
            violations.is_empty(),
            "T-201-1 is allow-listed and should not be reported, got: {violations:?}"
        );
    }

    /// [T-SUP012] 201-17, absent from the allowlist, is reported as a violation
    #[test]
    fn t201_17_absent_from_allowlist_is_reported_as_violation() {
        let occurrences = vec![ScannedToken {
            file: PathBuf::from("src/fetch/cdp/proxy/proxy_tests.rs"),
            token: "201-17".to_owned(),
        }];

        let violations = find_test_id_violations(&occurrences);

        assert!(
            violations
                .iter()
                .any(|v| v.contains("201-17") && v.contains("starts with a digit")),
            "T-201-17 is not allow-listed and should be reported, got: {violations:?}"
        );
    }

    /// [T-SUP009] Scanning the real `src/` and `tests/` finds no test-id violations
    #[test]
    fn scanning_src_and_tests_finds_no_violations() {
        let violations = scan_test_id_violations();

        assert!(
            violations.is_empty(),
            "src/ and tests/ should carry no test-id violations, got: {violations:?}"
        );
    }

    /// [T-SUP013] Real extraction respects prefix/digit boundaries and reports file + code.
    #[test]
    fn requirement_code_boundaries_and_diagnostics() {
        for (input, expected) in [
            (
                "// FR-018, BR-001; NFR-123",
                vec!["BR-001", "FR-018", "NFR-123"],
            ),
            ("FR-018", vec!["FR-018"]),
            ("BR-001", vec!["BR-001"]),
            ("NFR-123", vec!["NFR-123"]),
            ("// XFR-018 XBR-001 XNFR-123", vec![]),
            ("// FR-01 BR-00 NFR-12", vec![]),
            ("// FR-0180 BR-0012 NFR-1234", vec![]),
            ("// no requirement code", vec![]),
            ("", vec![]),
        ] {
            let mut codes = extract_requirement_codes(input);
            codes.sort();
            assert_eq!(codes, expected, "extraction boundary for {input:?}");
            let occurrences: Vec<_> = codes
                .into_iter()
                .map(|token| ScannedToken {
                    file: PathBuf::from("fake/req_code_tests.rs"),
                    token,
                })
                .collect();
            let violations = find_requirement_code_violations(&occurrences);
            assert_eq!(violations.len(), expected.len(), "{input:?}");
            for (violation, code) in violations.iter().zip(expected) {
                assert!(
                    violation.contains("fake/req_code_tests.rs") && violation.contains(code),
                    "violation must identify file and code: {violation}"
                );
            }
        }
    }

    /// [T-SUP016] Scanning the real `src/` and `tests/` finds no requirement-code violations
    #[test]
    fn scanning_src_and_tests_finds_no_requirement_code_violations() {
        let violations = scan_requirement_code_violations();

        assert!(
            violations.is_empty(),
            "src/ and tests/ should carry no requirement-code violations, got: {violations:?}"
        );
    }
}

#[cfg(feature = "js-rendering")]
pub(crate) mod browser_fixture;

#[cfg(feature = "js-rendering")]
#[path = "test_support/browser_fixture_tests.rs"]
mod browser_fixture_tests;
