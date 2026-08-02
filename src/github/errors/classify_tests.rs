use super::*;

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

/// [T-GHC004] NotFound classifies as NotFound with a "check the repo/path" hint.
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

/// [T-GHC005] RateLimited with retry_after embeds the seconds into next_step.
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

/// [T-GHC006] RateLimited without retry_after still suggests GITHUB_TOKEN.
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
/// peer to Decode (issue #186). End-to-end non-retriability is pinned by T-GH020.
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
