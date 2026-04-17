use std::env;
use std::io;
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};

use wiremock::MockServer;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use tracing_test::traced_test;

    #[tokio::test]
    async fn t001_try_spawn_mock_server_returns_some_in_normal_env() {
        let Some(server) = try_spawn_mock_server("t001_normal_env").await else {
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
    async fn t004_bind_failure_without_force_returns_none_and_warns() {
        let bind_err: io::Result<TcpListener> = Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mock bind failure",
        ));

        let result = try_spawn_with_bind("t004_permission_denied", bind_err, false).await;

        assert!(
            result.is_none(),
            "try_spawn_with_bind should return None on bind failure"
        );
        assert!(logs_contain("t004_permission_denied"));
    }

    #[tokio::test]
    #[should_panic(expected = "t005_forced_panic")]
    async fn t005_bind_failure_with_force_panics() {
        let bind_err: io::Result<TcpListener> = Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mock bind failure",
        ));

        let _result = try_spawn_with_bind("t005_forced_panic", bind_err, true).await;
    }
}
