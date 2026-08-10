use std::time::Duration;

use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

use crate::retry::is_transient_network;
use crate::test_support::{connection_refused_error, try_spawn_mock_server};

use super::*;

/// [T-SLC001]
#[test]
fn token_not_set_is_usage_error_with_token_hint() {
    let c = SlackError::TokenNotSet.classify();
    assert_eq!(c.kind, ErrorCode::UsageError);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("SLACK_TOKEN")),
        "expected SLACK_TOKEN hint, got: {:?}",
        c.next_step
    );
}

/// [T-SLC011] Same caller-facing treatment as `TokenNotSet`: both are a
/// misconfigured credential the user must fix before retrying (issue #261).
#[test]
fn token_wrong_type_is_usage_error_with_token_hint() {
    let c = SlackError::TokenWrongType.classify();
    assert_eq!(c.kind, ErrorCode::UsageError);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("SLACK_TOKEN")),
        "expected SLACK_TOKEN hint, got: {:?}",
        c.next_step
    );
}

/// [T-SLC002] InsecureUrl classifies as DataError (peer to other backends' InsecureUrl).
#[test]
fn insecure_url_is_data_error() {
    let c = SlackError::InsecureUrl.classify();
    assert_eq!(c.kind, ErrorCode::DataError);
}

/// [T-SLC003] scout's internal "message not found" (space form) must classify the same
/// as Slack's `message_not_found` (underscore) — both should land on
/// EX_NOINPUT(66) per issue #114.
#[test]
fn api_not_found_codes_classify_as_not_found() {
    for code in [
        "channel_not_found",
        "message_not_found",
        "thread_not_found",
        "message not found",
    ] {
        let c = SlackError::Api {
            error: code.to_owned(),
        }
        .classify();
        assert_eq!(
            c.kind,
            ErrorCode::NotFound,
            "{code} must classify as NotFound"
        );
    }
}

/// [T-SLC010] scout's ts-bearing "message {ts} not found in thread" string
/// (built at client.rs when `extract_target` misses — target absent or in a
/// truncated page) classifies as NotFound, same as the bare "message not found"
/// form. Guards issue #224: the interpolated `{ts}` made the old exact-match arm
/// miss this string, dropping it to UsageError (exit 64) instead of NotFound
/// (exit 66).
#[test]
fn api_not_found_in_thread_with_ts_classifies_as_not_found() {
    let c = SlackError::Api {
        error: "message 1700000000.123456 not found in thread".to_owned(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::NotFound);
}

/// [T-SLC004] ADR-0003 — internal_error must not be misclassified as UsageError.
#[test]
fn api_temp_failure_codes_classify_as_temp_failure() {
    for code in [
        "internal_error",
        "service_unavailable",
        "fatal_error",
        "team_added_to_org",
    ] {
        let c = SlackError::Api {
            error: code.to_owned(),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::TempFailure, "{code}");
    }
}

/// [T-SLC005]
#[test]
fn api_other_codes_classify_as_usage_error() {
    for code in ["invalid_auth", "missing_scope", "not_authed"] {
        let c = SlackError::Api {
            error: code.to_owned(),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::UsageError, "{code}");
    }
}

/// [T-SLC006]
#[test]
fn rate_limited_is_temp_failure() {
    let c = SlackError::RateLimited { retry_after: None }.classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
}

/// [T-SLC007] Network classifies as TempFailure (network-class hint).
///
/// Superseded in kind coverage by T-SLNET002, which also asserts the network
/// hint; this fixture keeps the original connect-refused case so the
/// `Network` variant's `From<reqwest::Error>` construction stays exercised
/// under its own test id.
#[tokio::test]
async fn network_is_temp_failure() {
    let Some(err) = connection_refused_error("network_is_temp_failure").await else {
        return;
    };
    let c = SlackError::from(err).classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
}

/// [T-SLC008] Timeout classifies as Timeout (exit 124 split from TempFailure).
#[test]
fn timeout_is_timeout_kind() {
    let c = SlackError::Timeout("timed out".into()).classify();
    assert_eq!(c.kind, ErrorCode::Timeout);
}

/// [T-SLC009] Decode (schema drift) classifies as Internal per ADR-0011 priority 5.
#[test]
fn decode_is_internal() {
    let c = SlackError::Decode("schema mismatch".into()).classify();
    assert_eq!(c.kind, ErrorCode::Internal);
}

/// [T-SLNET001] A reqwest error originating in a connect or read timeout
/// classifies as Timeout(124).
///
/// `classify` delegates `Network` to `Classification::from_reqwest`, which
/// checks `is_timeout()` before the transient check.
#[tokio::test]
async fn reqwest_timeout_error_classifies_as_timeout() {
    let Some(server) = try_spawn_mock_server("slack::classify::timeout").await else {
        return; // loopback bind unavailable — skip
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(50))
        .build()
        .expect("client build");
    let err = client
        .get(server.uri())
        .send()
        .await
        .expect_err("request must time out");
    assert!(err.is_timeout(), "fixture must be a timeout: {err}");

    let c = SlackError::Network(err).classify();
    assert_eq!(c.kind, ErrorCode::Timeout);
}

/// [T-SLNET002] A reqwest error originating in a refused connection classifies as
/// TempFailure(75) with the network hint.
///
/// Companion to T-ER009, which drives the same failure through `FetchError`.
#[tokio::test]
async fn reqwest_connection_refused_classifies_as_temp_failure_with_network_hint() {
    let Some(err) = connection_refused_error("reqwest_connection_refused_classifies").await else {
        return;
    };
    let c = SlackError::Network(err).classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("network")),
        "expected network hint, got: {:?}",
        c.next_step
    );
}

/// [T-SLNET003] A reqwest error that is neither timeout nor transient classifies as Unknown(104).
///
/// A body-decode failure on a 2xx response is neither a timeout nor a
/// transient transport fault. Companion to T-ER033.
#[tokio::test]
async fn reqwest_error_neither_timeout_nor_transient_classifies_as_unknown() {
    let Some(server) = try_spawn_mock_server("slack::classify::unknown").await else {
        return; // loopback bind unavailable — skip
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let resp = reqwest::Client::new()
        .get(server.uri())
        .send()
        .await
        .expect("mock server responds");
    let err = resp
        .json::<serde_json::Value>()
        .await
        .expect_err("body is not JSON");
    assert!(!err.is_timeout(), "fixture must not be a timeout");
    assert!(!is_transient_network(&err), "fixture must not be transient");

    let c = SlackError::Network(err).classify();
    assert_eq!(c.kind, ErrorCode::Unknown);
}

/// [T-SLNET004] A URL construction failure classifies as ParseUrl, which maps to Internal(70).
///
/// The why-Internal-not-DataError rationale lives on the `ParseUrl`
/// variant's doc in `src/slack.rs`.
#[test]
fn url_build_failure_classifies_as_parse_url_internal() {
    // `::url` (crate root): the `mod url` declared in `src/slack.rs` shadows
    // the `url` crate name within this module's path resolution.
    let parse_err = ::url::Url::parse("not a url").expect_err("malformed url must fail to parse");
    let c = SlackError::ParseUrl(parse_err).classify();
    assert_eq!(c.kind, ErrorCode::Internal);
}

/// [T-SLAPI001] team_added_to_org classifies as TempFailure and carries the hint to retry
/// after a short delay
#[test]
fn team_added_to_org_classifies_as_temp_failure_with_short_delay_hint() {
    let c = SlackError::Api {
        error: "team_added_to_org".to_owned(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
    assert_eq!(
        c.next_step.as_deref(),
        Some("Retry after a short delay"),
        "team_added_to_org must reuse the shared short-delay retry hint"
    );
}

/// [T-SLAPI002] org_login_required classifies as TempFailure with the hint
/// "Retry after the workspace's Enterprise migration completes"
#[test]
fn org_login_required_classifies_as_temp_failure_with_enterprise_migration_hint() {
    let c = SlackError::Api {
        error: "org_login_required".to_owned(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
    assert_eq!(
        c.next_step.as_deref(),
        Some("Retry after the workspace's Enterprise migration completes")
    );
}

/// [T-SLAPI003] invalid_cursor classifies as TempFailure with the hint
/// "Re-run to restart thread paging from the first page"
#[test]
fn invalid_cursor_classifies_as_temp_failure_with_restart_paging_hint() {
    let c = SlackError::Api {
        error: "invalid_cursor".to_owned(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
    assert_eq!(
        c.next_step.as_deref(),
        Some("Re-run to restart thread paging from the first page")
    );
}

/// [T-SLC012] Every one of the 14 strings the contract enumerates classifies as UsageError
#[test]
fn contract_listed_fourteen_strings_classify_as_usage_error() {
    for code in [
        "access_denied",
        "accesslimited",
        "account_inactive",
        "ekm_access_denied",
        "enterprise_is_restricted",
        "invalid_auth",
        "missing_scope",
        "no_permission",
        "not_allowed_token_type",
        "not_authed",
        "team_access_not_granted",
        "token_expired",
        "token_revoked",
        "two_factor_setup_required",
    ] {
        let c = SlackError::Api {
            error: code.to_owned(),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::UsageError, "{code}");
    }
}

/// [T-SLC013] invalid_arguments classifies as DataError
#[test]
fn invalid_arguments_classifies_as_data_error() {
    let c = SlackError::Api {
        error: "invalid_arguments".to_owned(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::DataError);
}

/// [T-SLC014] invalid_arg_name, deprecated_endpoint and method_deprecated classify as Internal
#[test]
fn invalid_arg_name_and_deprecated_endpoint_and_method_deprecated_classify_as_internal() {
    for code in [
        "invalid_arg_name",
        "deprecated_endpoint",
        "method_deprecated",
    ] {
        let c = SlackError::Api {
            error: code.to_owned(),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::Internal, "{code}");
    }
}

/// [T-SLC015] An unknown string absent from the table classifies as Unknown
#[test]
fn unlisted_unknown_string_classifies_as_unknown() {
    let c = SlackError::Api {
        error: "some_future_slack_error_code".to_owned(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::Unknown);
}

/// [T-SLC016] every non-2xx status is transient, including ones the shared
/// table would read as terminal
///
/// This is the one arm that departs from ADR-0003's HTTP-status table, and the
/// variant's doc says why: Slack reports its own failures as `ok: false` inside
/// a 200, so a non-2xx came from something between scout and Slack. Reading 404
/// through the shared table would report a gateway's error as a missing message,
/// and 401/403 as the caller's credentials rather than the proxy's.
///
/// The declaration existed; nothing held it. "Route Server through
/// `from_http_status` like the other backends" is a plausible tidy-up that this
/// now stops.
#[test]
fn server_status_is_always_transient_never_the_shared_table() {
    for status in [400, 401, 403, 404, 408, 500, 502, 503] {
        let c = SlackError::Server(status).classify();
        assert_eq!(
            c.kind,
            ErrorCode::TempFailure,
            "HTTP {status} came from an intermediary, not from Slack, so it must not \
             take its meaning from the shared status table"
        );
    }
}
