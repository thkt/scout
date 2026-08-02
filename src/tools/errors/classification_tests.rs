use super::*;

/// [T-ER012] io_error returns ErrorCode::IoError
#[test]
fn io_error_kind_is_io_error() {
    let err = ScoutError::io_error("test");
    assert_eq!(err.error_kind(), ErrorCode::IoError);
}

/// [T-CD001] Errors default to empty candidates list
#[test]
fn default_candidates_empty() {
    let err = ScoutError::not_found("test");
    assert!(err.candidates().is_empty());
}

/// [T-CD002] with_candidates attaches correction suggestions
#[test]
fn with_candidates_attaches_list() {
    let err = ScoutError::not_found("path not found")
        .with_candidates(vec!["README.md".into(), "REDAME.md".into()]);
    assert_eq!(err.candidates(), &["README.md", "REDAME.md"]);
}

/// [T-NS001] ApiKeyNotSet sets next_step pointing to BRAVE_SEARCH_API_KEY env var
#[test]
fn brave_api_key_not_set_has_next_step() {
    let err = ScoutError::from(BraveError::ApiKeyNotSet);
    assert_eq!(
        err.next_step(),
        Some("Set BRAVE_SEARCH_API_KEY environment variable")
    );
}

/// [T-NS002] GitHubError::Forbidden separates GITHUB_TOKEN hint into next_step (not message)
#[test]
fn github_forbidden_separates_hint_into_next_step() {
    let err = ScoutError::from(github::GitHubError::Forbidden("denied".into()));
    assert_eq!(
        err.next_step(),
        Some("Check that your GITHUB_TOKEN has the required scopes")
    );
}

/// [T-NS003] GitHubError::NotFound has actionable next_step
#[test]
fn github_not_found_has_next_step() {
    let err = ScoutError::from(github::GitHubError::NotFound("/test".into()));
    assert!(
        err.next_step()
            .is_some_and(|h| h.contains("Check that the repository or path exists"))
    );
}

/// [T-NS004] FetchError::Status(404) has next_step about the URL
#[test]
fn fetch_404_has_next_step() {
    let err = ScoutError::from(FetchError::Status(404));
    assert!(
        err.next_step()
            .is_some_and(|h| h.contains("Check that the URL is correct"))
    );
}

/// [T-NS005] BraveError::Unauthorized points users at the Brave dashboard
#[test]
fn brave_unauthorized_separates_dashboard_hint() {
    let err = ScoutError::from(BraveError::Unauthorized);
    assert!(
        err.next_step()
            .is_some_and(|h| h.contains("api-dashboard.search.brave.com"))
    );
}

/// [T-NS006] GitHubError::RateLimited with retry_after embeds the duration in next_step
#[test]
fn github_rate_limited_with_retry_after_embeds_duration() {
    let err = ScoutError::from(github::GitHubError::RateLimited {
        retry_after: Some(42),
    });
    assert!(
        err.next_step().is_some_and(|h| h.contains("42 seconds")),
        "next_step should mention retry_after seconds, got: {:?}",
        err.next_step()
    );
}

/// [T-NS007] GitHubError::RateLimited without retry_after still suggests setting GITHUB_TOKEN
#[test]
fn github_rate_limited_without_retry_after_suggests_token() {
    let err = ScoutError::from(github::GitHubError::RateLimited { retry_after: None });
    assert!(err.next_step().is_some_and(|h| h.contains("GITHUB_TOKEN")));
}

/// [T-NS008] Display includes next_step appended to message
#[test]
fn display_includes_next_step() {
    let err = ScoutError::user_error("Something is wrong").with_next_step("Try X");
    let display = err.to_string();
    assert!(display.contains("Something is wrong"));
    assert!(display.contains("Try X"));
}

/// [T-NS009] Errors without next_step omit the hint from Display
#[test]
fn display_omits_next_step_when_absent() {
    let err = ScoutError::io_error("io failure");
    let display = err.to_string();
    assert_eq!(display, "io failure");
}

/// [T-ER020] SlackError::Api with internal_error classifies as TempFailure (ADR-0003)
#[test]
fn slack_internal_error_classifies_as_temp_failure() {
    use crate::slack::SlackError;
    let err = ScoutError::from(SlackError::Api {
        error: "internal_error".to_owned(),
    });
    assert_eq!(err.error_kind(), ErrorCode::TempFailure);
}

/// [T-ER021] SlackError::Api with channel_not_found classifies as NotFound (ADR-0003)
#[test]
fn slack_channel_not_found_classifies_as_not_found() {
    use crate::slack::SlackError;
    let err = ScoutError::from(SlackError::Api {
        error: "channel_not_found".to_owned(),
    });
    assert_eq!(err.error_kind(), ErrorCode::NotFound);
}

/// [T-ER022] SlackError::Api with other error codes (e.g., invalid_auth) classifies as UsageError
#[test]
fn slack_other_api_error_classifies_as_usage_error() {
    use crate::slack::SlackError;
    let err = ScoutError::from(SlackError::Api {
        error: "invalid_auth".to_owned(),
    });
    assert_eq!(err.error_kind(), ErrorCode::UsageError);
}

/// [T-ER031] scout-internal "message not found" (space form) classifies as NotFound
/// (issue #114 condition 4). scout returns this string from `fetch_message`
/// when the resolved messages list is empty; it must classify as the same
/// NotFound(66) class as Slack's native `message_not_found` (underscore form).
#[test]
fn slack_space_message_not_found_classifies_as_not_found() {
    use crate::slack::SlackError;
    let err = ScoutError::from(SlackError::Api {
        error: "message not found".to_owned(),
    });
    assert_eq!(err.error_kind(), ErrorCode::NotFound);
    assert_eq!(err.exit_code(), 66, "expected EX_NOINPUT (66)");
}

/// [T-ER034] every backend answers the ADR-0003 status table the same way
///
/// The table was re-derived per backend and had already drifted: a GitHub 408
/// reported DataError(65, retryable=false) instead of TempFailure, and a Brave
/// 404 reported DataError instead of NotFound. `Classification::from_http_status`
/// is now the one copy; this pins each row against the DR so the next divergence
/// fails here rather than in a caller's exit-code branch.
#[test]
fn http_status_table_is_answered_identically_across_backends() {
    use crate::brave::client::BraveError;
    use crate::github::GitHubError;

    for (status, expected) in [
        (500, ErrorCode::TempFailure),
        (503, ErrorCode::TempFailure),
        (408, ErrorCode::TempFailure),
        (429, ErrorCode::TempFailure),
        (404, ErrorCode::NotFound),
        (401, ErrorCode::UsageError),
        (403, ErrorCode::UsageError),
        (400, ErrorCode::DataError),
        (422, ErrorCode::DataError),
        (301, ErrorCode::Unknown),
    ] {
        assert_eq!(
            Classification::from_http_status(status).kind,
            expected,
            "ADR-0003 table: HTTP {status}"
        );
        assert_eq!(
            GitHubError::Api {
                code: status,
                message: "x".into()
            }
            .classify()
            .kind,
            expected,
            "github should follow the table for HTTP {status}"
        );
        assert_eq!(
            BraveError::Api {
                code: status,
                message: "x".into()
            }
            .classify()
            .kind,
            expected,
            "brave should follow the table for HTTP {status}"
        );
        assert_eq!(
            FetchError::Status(status).classify().kind,
            expected,
            "fetch should follow the table for HTTP {status}"
        );
    }
}

/// [T-ER023] ADR-0011 priority 2 wins over priority 5 for Api 4xx codes.
/// Prior to the priority rule reflection, `GitHubError::Api { code: 4xx }` and
/// `BraveError::Api { code: 4xx }` folded onto `internal()` (IoError, exit 74).
/// Per ADR-0011 priority 2 they must classify as DataError (exit 65 per ADR-0002).
#[test]
fn api_4xx_classifies_as_data_error_per_priority_2() {
    let github_400 = ScoutError::from(github::GitHubError::Api {
        code: 400,
        message: "bad request".into(),
    });
    let github_422 = ScoutError::from(github::GitHubError::Api {
        code: 422,
        message: "unprocessable entity".into(),
    });
    let brave_400 = ScoutError::from(BraveError::Api {
        code: 400,
        message: "err".into(),
    });
    for err in [&github_400, &github_422, &brave_400] {
        assert_eq!(err.error_kind(), ErrorCode::DataError, "{err}");
        assert_eq!(err.exit_code(), 65, "{err}");
        assert!(!err.retryable(), "4xx must not be retryable: {err}");
    }
}

/// [T-ER024] ADR-0011 priority 4 (TEMP_FAILURE) takes precedence for `Api { 5xx }`
/// even though priority 5 (INTERNAL) could match the bare `Api { .. }` arm.
/// Match-arm ordering enforces the priority ranking.
#[test]
fn api_5xx_classifies_as_temp_failure_per_priority_4() {
    let github_502 = ScoutError::from(github::GitHubError::Api {
        code: 502,
        message: "bad gateway".into(),
    });
    let brave_503 = ScoutError::from(BraveError::Api {
        code: 503,
        message: "unavailable".into(),
    });
    for err in [&github_502, &brave_503] {
        assert_eq!(err.error_kind(), ErrorCode::TempFailure, "{err}");
        assert_eq!(err.exit_code(), 75, "{err}");
        assert!(err.retryable(), "5xx must be retryable: {err}");
    }
}

/// [T-ER025] INTERNAL (70) reserved for scout-side schema bugs.
/// `Decode` / `ParseJson` variants from GitHub, Slack, and Brave APIs signal
/// an unexpected response shape — by ADR-0011 priority 5 these are scout's
/// invariant violation, not external IO failure (which maps to IoError 74).
#[test]
fn schema_decode_classifies_as_internal_exit_70() {
    let serde_err =
        serde_json::from_str::<serde_json::Value>("{not valid").expect_err("malformed json");
    let cases: Vec<ScoutError> = vec![
        github::GitHubError::Decode("decode error".into()).into(),
        SlackError::Decode("err".into()).into(),
        BraveError::ParseJson(serde_err).into(),
    ];
    for err in &cases {
        assert_eq!(err.error_kind(), ErrorCode::Internal, "{err}");
        assert_eq!(err.exit_code(), 70, "expected EX_SOFTWARE (70): {err}");
        assert!(!err.retryable(), "Internal must not be retryable: {err}");
    }
}

/// [T-ER032] `ScoutError::internal_bug` (direct constructor for scout-side
/// serialize/invariant violations, e.g. `serde_json::to_value` failure in a
/// handler) classifies as Internal / exit 70 (EX_SOFTWARE) and is non-retryable,
/// matching the `Decode`-routed Internal path pinned by T-ER025.
#[test]
fn internal_bug_constructor_classifies_as_internal_exit_70() {
    let err = ScoutError::internal_bug("failed to serialize fetch result");
    assert_eq!(err.error_kind(), ErrorCode::Internal);
    assert_eq!(err.exit_code(), 70, "expected EX_SOFTWARE (70)");
    assert!(!err.retryable(), "Internal must not be retryable");
}

/// [T-ER030] GitHub `Api { code: 401 }` classifies as UsageError(64) with auth hint
/// (issue #101).
///
/// Prior to this fix, 401 fell through the generic `(400..500)` DataError arm
/// (exit 65) because the GitHubClient surfaces every non-special 4xx as
/// `GitHubError::Api`. 401 is an auth-class failure — the user must set
/// `GITHUB_TOKEN` or run `gh auth login` — so ADR-0011 priority 1 (USAGE_ERROR)
/// is the correct landing, peer to `GitHubError::Forbidden`.
#[test]
fn github_401_classifies_as_usage_error_with_auth_hint() {
    let err = ScoutError::from(github::GitHubError::Api {
        code: 401,
        message: "Bad credentials".into(),
    });
    assert_eq!(err.error_kind(), ErrorCode::UsageError);
    assert_eq!(err.exit_code(), 64, "expected EX_USAGE (64)");
    assert!(
        err.next_step().is_some_and(|h| h.contains("GITHUB_TOKEN")),
        "expected auth hint mentioning GITHUB_TOKEN, got: {:?}",
        err.next_step()
    );
}

/// [T-ER026] UNKNOWN (104) is the escape hatch for Api codes that match
/// neither 4xx (priority 2) nor 5xx (priority 4). Exit 104 is the PJ
/// extension reserved per ADR-0002, populated via ADR-0011 §Classification
/// Priority Table 退避 slot. A rising rate of Unknown signals the
/// classification design needs revisiting.
#[test]
fn unclassified_api_classifies_as_unknown_exit_104() {
    let cases: Vec<ScoutError> = vec![
        github::GitHubError::Api {
            code: 304,
            message: "not modified".into(),
        }
        .into(),
        BraveError::Api {
            code: 304,
            message: "not modified".into(),
        }
        .into(),
    ];
    for err in &cases {
        assert_eq!(err.error_kind(), ErrorCode::Unknown, "{err}");
        assert_eq!(err.exit_code(), 104, "expected PJ extension (104): {err}");
        assert!(!err.retryable(), "Unknown must not be retryable: {err}");
    }
}
