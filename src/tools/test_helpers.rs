use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::redirect::Policy;

use super::*;
use crate::fetch::StaticDnsResolver;

pub(super) fn scout_with_github(brave_uri: &str, github_uri: &str) -> Scout {
    ScoutBuilder::for_test()
        .with_brave_endpoint(brave_uri)
        .with_github_endpoint(github_uri)
        .build()
}

/// Builds a `Scout` whose outer GitHub-command timeout is `timeout`, so a test
/// can trip the `run()`-level guard against a delayed wiremock response without
/// waiting the production 120s (issue #185).
pub(super) fn scout_with_github_timeout(
    brave_uri: &str,
    github_uri: &str,
    timeout: Duration,
) -> Scout {
    ScoutBuilder::for_test()
        .with_brave_endpoint(brave_uri)
        .with_github_endpoint(github_uri)
        .with_github_timeout(timeout)
        .build()
}

pub(super) fn scout_lazy(brave_uri: &str) -> Scout {
    ScoutBuilder::for_test()
        .with_brave_endpoint(brave_uri)
        .build()
}

pub(super) fn scout_with_brave(brave_uri: &str) -> Scout {
    scout_with_github(brave_uri, "http://localhost:0")
}

/// A `Scout` that can reach a loopback wiremock at `addr` without the SSRF
/// contract being switched off.
///
/// `.resolve()` points the test host at the wiremock socket, and the client is
/// built without `SsrfResolver` so the loopback connect is allowed to proceed —
/// the pre-flight still runs, against a resolver that answers with a public
/// address. Installing the guard instead would block loopback, and dropping the
/// pre-flight would stop testing the path under test.
pub(super) fn scout_reaching(addr: SocketAddr) -> Scout {
    let fetch_http = reqwest::Client::builder()
        .redirect(Policy::none())
        .resolve("scout-test.example", addr)
        .build()
        .expect("test client builds");
    ScoutBuilder::for_test()
        .with_dns(Arc::new(StaticDnsResolver::single("93.184.216.34")))
        .with_fetch_http(fetch_http)
        .build()
}
