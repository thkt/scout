use std::time::Duration;

use super::*;

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
