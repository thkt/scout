use super::*;
use std::collections::HashMap;

// Contract: mirror the proxy environment variables reqwest users expect
// (HTTPS_PROXY / HTTP_PROXY plus their lowercase forms), resolved as data in
// / data out — the environment is passed in as a map so the function never
// reads process env itself. The reqwest 0.13 "System Proxies" section that
// documents these vars was not retrievable via WebFetch this session; the
// four scenarios below are the authoritative spec for the expected outputs.

/// [T-001]
#[test]
fn detects_proxied_with_the_https_proxy_value_when_both_https_proxy_and_http_proxy_are_set() {
    let env = HashMap::from([
        (
            "HTTPS_PROXY".to_owned(),
            "http://proxy.https:8443".to_owned(),
        ),
        ("HTTP_PROXY".to_owned(), "http://proxy.http:8080".to_owned()),
    ]);
    assert_eq!(
        detect_egress_mode(&env),
        EgressMode::Proxied("http://proxy.https:8443".to_owned()),
    );
}

/// [T-002]
#[test]
fn detects_proxied_with_the_http_proxy_value_when_only_http_proxy_is_set() {
    let env = HashMap::from([("HTTP_PROXY".to_owned(), "http://proxy.http:8080".to_owned())]);
    assert_eq!(
        detect_egress_mode(&env),
        EgressMode::Proxied("http://proxy.http:8080".to_owned()),
    );
}

/// [T-003]
#[test]
fn detects_direct_when_no_proxy_env_var_is_present() {
    let env = HashMap::from([("PATH".to_owned(), "/usr/bin".to_owned())]);
    assert_eq!(detect_egress_mode(&env), EgressMode::Direct);
}

/// [T-005]
///
/// `Proxy::all("")` is a relative-URL parse error, so returning `Proxied("")`
/// here would fail client construction and abort every command at startup.
#[test]
fn treats_a_present_but_empty_proxy_value_as_unset() {
    let env = HashMap::from([
        ("HTTPS_PROXY".to_owned(), String::new()),
        ("HTTP_PROXY".to_owned(), "   ".to_owned()),
    ]);
    assert_eq!(detect_egress_mode(&env), EgressMode::Direct);
}

/// [T-006] falls through an empty value to the next candidate that has one
#[test]
fn falls_through_an_empty_value_to_the_next_candidate() {
    let env = HashMap::from([
        ("HTTPS_PROXY".to_owned(), String::new()),
        ("HTTP_PROXY".to_owned(), "http://proxy.http:8080".to_owned()),
    ]);
    assert_eq!(
        detect_egress_mode(&env),
        EgressMode::Proxied("http://proxy.http:8080".to_owned()),
    );
}

/// [T-004]
#[test]
fn detects_proxied_from_lowercase_https_proxy_when_uppercase_forms_are_absent() {
    let env = HashMap::from([(
        "https_proxy".to_owned(),
        "http://proxy.lower:3128".to_owned(),
    )]);
    assert_eq!(
        detect_egress_mode(&env),
        EgressMode::Proxied("http://proxy.lower:3128".to_owned()),
    );
}
