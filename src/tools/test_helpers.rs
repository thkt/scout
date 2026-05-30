use super::*;

pub(super) fn scout_with_github(brave_uri: &str, github_uri: &str) -> Scout {
    ScoutBuilder::for_test()
        .with_brave_endpoint(brave_uri)
        .with_github_endpoint(github_uri)
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
