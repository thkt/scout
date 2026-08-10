use super::*;

/// [T-ER001a] UsageError errors surface with exit 64 (EX_USAGE per ADR-0002)
#[test]
fn usage_errors_have_exit_code_64() {
    let cases: Vec<ScoutError> = vec![
        github::GitHubError::Forbidden("denied".into()).into(),
        FetchError::BrowserNotFound("not installed".into()).into(),
        FetchError::Status(401).into(),
        FetchError::Status(403).into(),
        SlackError::TokenNotSet.into(),
        SlackError::TokenWrongType.into(),
        SlackError::Api {
            error: "invalid_auth".into(),
        }
        .into(),
        BraveError::ApiKeyNotSet.into(),
        BraveError::Unauthorized.into(),
    ];
    for err in &cases {
        assert_eq!(err.error_kind(), ErrorCode::UsageError, "{err}");
        assert_eq!(err.exit_code(), 64, "expected EX_USAGE (64): {err}");
    }
}

/// [T-ER001b] DataError errors surface with exit 65 (EX_DATAERR per ADR-0002).
/// Per ADR-0011 priority 2, `*Error::Api { code }` 4xx (other than 401/403/404) now
/// routes to DataError instead of folding onto IoError via `internal()`. The three
/// `Insecure*` variants (one per backend) belong here because a plain-HTTP URL is a
/// caller-supplied config defect, not a transient runtime failure.
#[test]
fn data_errors_have_exit_code_65() {
    let cases: Vec<ScoutError> = vec![
        github::GitHubError::InvalidRepo("bad".into()).into(),
        github::GitHubError::Api {
            code: 400,
            message: "bad request".into(),
        }
        .into(),
        github::GitHubError::Api {
            code: 422,
            message: "unprocessable entity".into(),
        }
        .into(),
        github::GitHubError::InsecureUrl.into(),
        FetchError::InvalidScheme.into(),
        FetchError::InternalHost.into(),
        FetchError::UnsupportedContentType("image/png".into()).into(),
        FetchError::RedirectMissingLocation.into(),
        FetchError::Status(400).into(),
        FetchError::Status(499).into(),
        FetchError::TooLarge.into(),
        FetchError::TooManyRedirects(10).into(),
        BraveError::Api {
            code: 400,
            message: "err".into(),
        }
        .into(),
        BraveError::InsecureBaseUrl.into(),
        SlackError::InsecureUrl.into(),
    ];
    for err in &cases {
        assert_eq!(err.error_kind(), ErrorCode::DataError, "{err}");
        assert_eq!(err.exit_code(), 65, "expected EX_DATAERR (65): {err}");
    }
}

/// [T-ER001c] NotFound errors surface with exit 66 (EX_NOINPUT per ADR-0002)
#[test]
fn not_found_errors_have_exit_code_66() {
    let cases: Vec<ScoutError> = vec![
        github::GitHubError::NotFound("/test".into()).into(),
        FetchError::Status(404).into(),
    ];
    for err in &cases {
        assert_eq!(err.error_kind(), ErrorCode::NotFound, "{err}");
        assert_eq!(err.exit_code(), 66, "expected EX_NOINPUT (66): {err}");
    }
}

/// [T-ER002] IoError errors surface with exit 74 (EX_IOERR) and are non-retryable.
/// Reserved for external-tool IO failures (browser); scout-side schema bugs
/// route to `Internal(70)` (T-ER025) and unclassifiable Api codes to
/// `Unknown(104)` (T-ER026).
#[test]
fn io_errors_have_exit_code_74() {
    let cases: Vec<ScoutError> =
        vec![FetchError::BrowserFailed("CDP protocol error".into()).into()];
    for err in &cases {
        assert_eq!(err.error_kind(), ErrorCode::IoError, "{err}");
        assert_eq!(err.exit_code(), 74, "expected EX_IOERR (74): {err}");
        assert!(!err.retryable(), "IoError should not be retryable: {err}");
    }
}

/// [T-ER003] TempFailure errors are retryable, display retry hint, exit 75 (EX_TEMPFAIL).
/// Timeout cases moved to T-ER027 with exit 124 per ADR-0002.
///
/// [T-ER035] A Slack connection-refused error becomes exit code 75 with a retry
/// hint through ScoutError — pinned by the `SlackError::Network` row, which carries a real
/// connection-refused `reqwest::Error`.
#[tokio::test]
async fn temp_failure_errors_have_exit_code_75() {
    use crate::test_support::connection_refused_error;
    let Some(slack_network_err) = connection_refused_error("temp_failure_exit_code_75").await
    else {
        return;
    };

    let cases: Vec<ScoutError> = vec![
        FetchError::Status(408).into(),
        FetchError::Status(429).into(),
        FetchError::Status(500).into(),
        FetchError::Status(503).into(),
        FetchError::DnsResolution("dns failed".into()).into(),
        github::GitHubError::RateLimited { retry_after: None }.into(),
        github::GitHubError::Api {
            code: 502,
            message: "bad gateway".into(),
        }
        .into(),
        BraveError::RateLimited { retry_after: None }.into(),
        BraveError::Api {
            code: 503,
            message: "unavailable".into(),
        }
        .into(),
        SlackError::RateLimited { retry_after: None }.into(),
        SlackError::Network(slack_network_err).into(),
    ];
    for err in &cases {
        assert_eq!(err.error_kind(), ErrorCode::TempFailure, "{err}");
        assert!(err.retryable(), "expected retryable: {err}");
        assert_eq!(err.exit_code(), 75, "expected EX_TEMPFAIL (75): {err}");
        assert!(
            err.to_string().contains("retry may succeed"),
            "should include retry hint: {err}"
        );
    }
}

/// [T-ER027] Exit 124 (GNU coreutils `timeout`) is split from TempFailure(75) so
/// caller scripts can apply a longer retry backoff than for rate-limit / 5xx; a
/// timeout implies an unknown counterparty load condition.
#[test]
fn timeout_errors_have_exit_code_124() {
    let cases: Vec<ScoutError> = vec![
        FetchError::Timeout("timed out".into()).into(),
        SlackError::Timeout("timed out".into()).into(),
    ];
    for err in &cases {
        assert_eq!(err.error_kind(), ErrorCode::Timeout, "{err}");
        assert!(err.retryable(), "Timeout must be retryable: {err}");
        assert_eq!(err.exit_code(), 124, "expected 124 (GNU timeout): {err}");
        assert!(
            err.to_string().contains("retry may succeed"),
            "should include retry hint: {err}"
        );
    }
}

/// [T-ER004] Non-transient errors are not retryable and omit retry hint
#[test]
fn non_transient_errors_are_not_retryable() {
    let cases: Vec<ScoutError> = vec![
        FetchError::InvalidScheme.into(),
        FetchError::Status(404).into(),
        FetchError::BrowserFailed("err".into()).into(),
        github::GitHubError::Decode("err".into()).into(),
        SlackError::Decode("err".into()).into(),
    ];
    for err in &cases {
        assert!(!err.retryable(), "expected not retryable: {err}");
        assert!(
            !err.to_string().contains("retry may succeed"),
            "should not include retry hint: {err}"
        );
    }
}

/// [T-ER009]
#[tokio::test]
async fn fetch_error_http_connection_refused_is_transient() {
    use crate::retry::is_transient_network;
    use crate::test_support::connection_refused_error;

    let Some(reqwest_err) = connection_refused_error("fetch_error_http_connection_refused").await
    else {
        return;
    };

    assert!(
        is_transient_network(&reqwest_err),
        "expected transient network error, got: {reqwest_err}"
    );

    let fetch_err = FetchError::Http(reqwest_err);
    let scout_err = ScoutError::from(fetch_err);

    assert!(
        scout_err.retryable(),
        "connection-refused FetchError::Http should produce transient ScoutError"
    );
    assert!(
        scout_err.to_string().contains("retry may succeed"),
        "transient error should contain retry hint: {}",
        scout_err
    );
}

/// [T-ER033] every backend sends a reqwest error that is neither timeout nor
/// transient to Unknown(104), not TempFailure(75)
///
/// A rising `Unknown` rate is the signal ADR-0011 asks for when the
/// classification has missed a case; calling an unrecognized transport failure
/// retryable buries it instead. github and brave used to blanket-map this to
/// TempFailure, each in its own arm — the shared `Classification::from_reqwest`
/// is what keeps the three answers the same from here on.
#[tokio::test]
async fn unclassifiable_reqwest_error_is_unknown_across_backends() {
    use reqwest::Client;

    use wiremock::matchers::method;
    use wiremock::{Mock, ResponseTemplate};

    use crate::brave::client::BraveError;
    use crate::envelope::ErrorCode;
    use crate::github::GitHubError;
    use crate::retry::is_transient_network;
    use crate::test_support::try_spawn_mock_server;

    // A body-decode failure: the request itself succeeded, so this is neither a
    // timeout nor a connect-level fault.
    let Some(server) = try_spawn_mock_server("errors::unclassifiable").await else {
        return;
    };
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let resp = Client::new()
        .get(server.uri())
        .send()
        .await
        .expect("mock server responds");
    let reqwest_err = resp
        .json::<serde_json::Value>()
        .await
        .expect_err("body is not JSON");

    assert!(!reqwest_err.is_timeout(), "fixture must not be a timeout");
    assert!(
        !is_transient_network(&reqwest_err),
        "fixture must not be transient: {reqwest_err}"
    );

    assert_eq!(
        FetchError::Http(reqwest_err).classify().kind,
        ErrorCode::Unknown
    );

    for (label, kind) in [
        ("github", {
            let e = Client::new()
                .get(server.uri())
                .send()
                .await
                .expect("send")
                .json::<serde_json::Value>()
                .await
                .expect_err("not json");
            GitHubError::Network(e).classify().kind
        }),
        ("brave", {
            let e = Client::new()
                .get(server.uri())
                .send()
                .await
                .expect("send")
                .json::<serde_json::Value>()
                .await
                .expect_err("not json");
            BraveError::Network(e).classify().kind
        }),
    ] {
        assert_eq!(
            kind,
            ErrorCode::Unknown,
            "{label} should report an unclassifiable transport failure as Unknown"
        );
    }
}

/// [T-ER036] An internal_error that persists after retries becomes exit code 75 with a
/// retry hint through ScoutError
///
/// The seam from a persistently-failing real HTTP call through to the
/// process exit code: a `fetch_message` whose responder always answers
/// `internal_error` must exhaust the retry loop and reach `ScoutError` as
/// exit 75 (EX_TEMPFAIL) with the retry hint. The request count proves at
/// least one retry actually ran.
#[tokio::test]
async fn slack_persistent_internal_error_reaches_exit_code_75_via_scout_error() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    use crate::slack::{SlackClient, parse_slack_url};
    use crate::test_support::try_spawn_mock_server;

    let Some(server) = try_spawn_mock_server("errors::slack_persistent_internal_error").await
    else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"ok": false, "error": "internal_error"})),
        )
        .mount(&server)
        .await;

    let client = SlackClient::with_base_url(reqwest::Client::new(), &server.uri());
    let url = parse_slack_url("https://acme.slack.com/archives/C1/p1000000001")
        .expect("valid slack permalink parses");
    let result = client.fetch_message(&url).await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("internal_error persisting past every retry attempt must still fail"),
    };
    assert!(
        matches!(err, SlackError::Api { ref error } if error == "internal_error"),
        "expected the persistent internal_error to propagate, got: {err:?}"
    );
    let requests = server
        .received_requests()
        .await
        .expect("request recording is on by default");
    assert!(
        requests.len() >= 2,
        "a retryable internal_error must be retried at least once before giving up, \
         got {} request(s)",
        requests.len()
    );

    let scout_err = ScoutError::from(err);
    assert_eq!(
        scout_err.exit_code(),
        75,
        "expected EX_TEMPFAIL (75): {scout_err}"
    );
    assert!(scout_err.retryable(), "expected retryable: {scout_err}");
    assert!(
        scout_err.next_step().is_some_and(|h| h.contains("Retry")),
        "expected a retry hint, got: {:?}",
        scout_err.next_step()
    );
    assert!(
        scout_err.to_string().contains("retry may succeed"),
        "should include retry hint in Display: {scout_err}"
    );
}
