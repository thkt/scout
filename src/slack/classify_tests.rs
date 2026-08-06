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
/// Superseded in kind coverage by [T-002], which also asserts the network
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

/// [T-001] connect または read の timeout に由来する reqwest error は Timeout(124) に分類される。
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

/// [T-002] 接続拒否に由来する reqwest error は TempFailure(75) と network hint に分類される。
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

/// [T-003] timeout でも transient でもない reqwest error は Unknown(104) に分類される。
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

/// [T-004] URL 構築失敗は ParseUrl として Internal(70) に分類される。
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

/// [T-001] team_added_to_org は TempFailure に分類され短い待機後の再試行を促す hint を持つ
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

/// [T-002] org_login_required は TempFailure に分類され hint が
/// "Retry after the workspace's Enterprise migration completes" になる
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

/// [T-003] invalid_cursor は TempFailure に分類され hint が
/// "Re-run to restart thread paging from the first page" になる
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

/// [T-001] contract が列挙した 14 文字列はいずれも UsageError に分類される
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

/// [T-002] invalid_arguments は DataError に分類される
#[test]
fn invalid_arguments_classifies_as_data_error() {
    let c = SlackError::Api {
        error: "invalid_arguments".to_owned(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::DataError);
}

/// [T-003] invalid_arg_name と deprecated_endpoint と method_deprecated は Internal に分類される
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

/// [T-004] 表に無い未知の文字列は Unknown に分類される
#[test]
fn unlisted_unknown_string_classifies_as_unknown() {
    let c = SlackError::Api {
        error: "some_future_slack_error_code".to_owned(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::Unknown);
}
