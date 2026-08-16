use super::*;
use crate::test_support::connection_refused_error;

/// [T-GHC001]
#[test]
fn forbidden_is_usage_error_with_scope_hint() {
    let c = GitHubError::Forbidden("denied".into()).classify();
    assert_eq!(c.kind, ErrorCode::UsageError);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("GITHUB_TOKEN")),
        "expected GITHUB_TOKEN hint, got: {:?}",
        c.next_step
    );
}

/// [T-GHC002] Api { code: 401 } classifies as UsageError (priority 1 over 4xx fallback).
/// Regression guard: a reorder that moves the 4xx arm above 401 would flip this to
/// DataError(65) without `match` exhaustiveness catching it.
#[test]
fn api_401_is_usage_error_not_data_error() {
    let c = GitHubError::Api {
        code: 401,
        message: "Bad credentials".into(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::UsageError, "401 must precede 4xx arm");
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("gh auth login")),
        "expected gh auth login hint, got: {:?}",
        c.next_step
    );
}

/// [T-GHC003] All priority-2 DataError variants classify as DataError.
#[test]
fn data_error_variants_classify_as_data_error() {
    let cases: Vec<GitHubError> = vec![
        GitHubError::InvalidRepo("bad".into()),
        GitHubError::InvalidRef("bad".into()),
        GitHubError::InvalidPath("bad".into()),
        GitHubError::InvalidLineRange("bad".into()),
        GitHubError::InvalidPattern("bad".into()),
        GitHubError::NonUtf8("bad".into()),
        // Was the one DataError variant no test named, so its arm could have
        // been moved or its code changed without a failure.
        GitHubError::PathIsDirectory("src/".into()),
        GitHubError::InsecureUrl,
        GitHubError::Api {
            code: 400,
            message: "bad request".into(),
        },
        GitHubError::Api {
            code: 422,
            message: "unprocessable".into(),
        },
    ];
    for case in &cases {
        assert_eq!(
            case.classify().kind,
            ErrorCode::DataError,
            "{case:?} must classify as DataError"
        );
    }
}

/// [T-GHC004]
#[test]
fn not_found_is_not_found_with_hint() {
    let c = GitHubError::NotFound("/x".into()).classify();
    assert_eq!(c.kind, ErrorCode::NotFound);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("repository or path")),
        "expected repo/path hint, got: {:?}",
        c.next_step
    );
}

/// [T-GHC005]
#[test]
fn rate_limited_with_retry_after_embeds_seconds() {
    let c = GitHubError::RateLimited {
        retry_after: Some(42),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("42 seconds")),
        "expected 42 seconds in hint, got: {:?}",
        c.next_step
    );
}

/// [T-GHC006]
#[test]
fn rate_limited_without_retry_after_suggests_token() {
    let c = GitHubError::RateLimited { retry_after: None }.classify();
    assert_eq!(c.kind, ErrorCode::TempFailure);
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("GITHUB_TOKEN")),
        "expected GITHUB_TOKEN hint, got: {:?}",
        c.next_step
    );
}

/// [T-GHC007] Api 5xx classifies as TempFailure (priority 4 over Unknown fallback).
#[test]
fn api_5xx_is_temp_failure() {
    for code in [500u16, 502, 503, 599] {
        let c = GitHubError::Api {
            code,
            message: "x".into(),
        }
        .classify();
        assert_eq!(c.kind, ErrorCode::TempFailure, "code {code}");
    }
}

/// [T-GHC008] Decode (schema drift) classifies as Internal per ADR-0011 priority 5.
#[test]
fn decode_is_internal() {
    let c = GitHubError::Decode("schema mismatch".into()).classify();
    assert_eq!(c.kind, ErrorCode::Internal);
}

/// [T-GHC010] ResponseTooLarge classifies as Internal per ADR-0011 priority 5,
/// peer to Decode. End-to-end non-retriability is pinned by T-GH020.
#[test]
fn response_too_large_is_internal() {
    let c = GitHubError::ResponseTooLarge.classify();
    assert_eq!(c.kind, ErrorCode::Internal);
}

/// [T-GHC009] Api codes outside 4xx/5xx (e.g., 1xx/3xx leak) land on Unknown.
#[test]
fn api_non_4xx_5xx_is_unknown() {
    let c = GitHubError::Api {
        code: 304,
        message: "not modified".into(),
    }
    .classify();
    assert_eq!(c.kind, ErrorCode::Unknown);
}

/// [T-GHC011] a transport failure delegates to the shared reqwest classifier
///
/// `Network` was the last variant no test reached. Every other backend pins its
/// transport arm (T-SLNET001-003 for Slack, and the fetch and Brave classify
/// suites), so GitHub's delegation to `Classification::from_reqwest` rested on
/// reading the code.
#[tokio::test]
async fn network_delegates_to_shared_reqwest_classification() {
    let Some(err) = connection_refused_error("github::classify").await else {
        return; // loopback bind unavailable — skip
    };

    let c = GitHubError::from(err).classify();

    assert_eq!(
        c.kind,
        ErrorCode::TempFailure,
        "a refused connection is transient, per Classification::from_reqwest"
    );
    assert!(
        c.next_step
            .as_deref()
            .is_some_and(|h| h.contains("network")),
        "expected the shared network hint, got: {:?}",
        c.next_step
    );
}

/// [T-GHC012] the reqwest conversion strips the request URL
///
/// `From<reqwest::Error>` calls `without_url` because reqwest's `Display`
/// appends `for url (…)` with the query string, which is where a token would
/// sit. The comment said so; nothing checked it, so dropping the call would
/// have looked like a simplification.
#[tokio::test]
async fn reqwest_conversion_drops_the_url() {
    let Some(err) = connection_refused_error("github::url_strip").await else {
        return; // loopback bind unavailable — skip
    };
    let with_url = err.to_string();

    let converted = GitHubError::from(err).to_string();

    assert!(
        with_url.contains("should-refuse"),
        "the fixture must start out carrying its URL, got: {with_url}"
    );
    assert!(
        !converted.contains("should-refuse"),
        "the converted error must not carry the URL, got: {converted}"
    );
}
