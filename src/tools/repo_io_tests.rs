use super::test_helpers::*;
use super::*;
use crate::test_support::try_spawn_mock_server;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, ResponseTemplate};

/// [T-TS017] repo_read: the --encoding hint reaches the decoder. Auto-detection
/// rejects these windows-1252 bytes (T-GE018), so only the hint can decode them.
#[tokio::test]
async fn repo_read_decodes_with_encoding_hint() {
    let Some(server) = try_spawn_mock_server("tools::t_008").await else {
        return;
    };

    // "café crème brûlée à la française, déjà vu" in windows-1252, base64-encoded.
    let windows_1252_b64 = "Y2Fm6SBjcuhtZSBicvts6WUg4CBsYSBmcmFu52Fpc2UsIGTpauAgdnU=";

    Mock::given(method("GET"))
        .and(path_regex(r"^/repos/owner/repo/contents/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sha": "abc123",
            "content": windows_1252_b64
        })))
        .mount(&server)
        .await;

    let s = scout_with_github("http://localhost:0", &server.uri());
    let params = RepoReadParams {
        repository: Some("owner/repo".into()),
        path: Some("test.txt".into()),
        ref_: None,
        lines: None,
        encoding: Some("windows-1252".into()),
    };

    let result = s.repo_read(params).await.unwrap();
    assert!(
        result.markdown().contains("café crème brûlée"),
        "output should contain the decoded windows-1252 text, got: {result:?}"
    );
    assert!(
        result.markdown().contains("[encoding: windows-1252]"),
        "header should include encoding label, got: {result:?}"
    );
}

/// [T-TS018] repo_tree: --path filter is wired through RepoTreeParams to
/// filter_tree_entries; files outside the prefix are excluded from output.
#[tokio::test]
async fn repo_tree_path_filter_excludes_non_matching_files() {
    let Some(server) = try_spawn_mock_server("tools::t_009").await else {
        return;
    };

    mount_repo_info(&server).await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/repos/owner/repo/git/trees/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "tree": [
                {"path": "src/main.rs", "type": "blob", "size": 100},
                {"path": "src/lib.rs", "type": "blob", "size": 200},
                {"path": "README.md", "type": "blob", "size": 50},
                {"path": "Cargo.toml", "type": "blob", "size": 80},
            ],
            "truncated": false
        })))
        .mount(&server)
        .await;

    let s = scout_with_github("http://localhost:0", &server.uri());
    let params = RepoTreeParams {
        repository: Some("owner/repo".into()),
        ref_: None,
        path: Some("src/".into()),
        pattern: None,
    };

    let result = s.repo_tree(params).await.unwrap();
    assert!(
        result.markdown().contains("src/main.rs"),
        "path filter should include src/main.rs, got:\n{result:?}"
    );
    assert!(
        !result.markdown().contains("README.md"),
        "path filter should exclude README.md, got:\n{result:?}"
    );
    assert!(
        !result.markdown().contains("Cargo.toml"),
        "path filter should exclude Cargo.toml, got:\n{result:?}"
    );
}

/// [T-TS033] a malformed --lines is rejected before any GitHub request
///
/// `parse_line_range` only ever inspects the string's shape — empty, a bad
/// number, a zero start, an end below the start — so none of it needs the file.
/// Running it after `get_contents`, and after the blob fetch and the decode
/// that can follow, spends one or two rate-limited calls to report a typo the
/// caller made. Every other caller-supplied value (`repository`, `path`,
/// `ref`) is already checked on this side of the network.
#[tokio::test]
async fn repo_read_invalid_line_range_fails_before_any_request() {
    let Some(server) = try_spawn_mock_server("tools::t_033").await else {
        return;
    };

    // Deliberately no mock: reaching the network at all would 404 here, which
    // would surface as NotFound rather than the range error asserted below.
    let s = scout_with_github("http://localhost:0", &server.uri());
    let params = RepoReadParams {
        repository: Some("owner/repo".into()),
        path: Some("test.txt".into()),
        ref_: None,
        lines: Some("not-a-range".into()),
        encoding: None,
    };

    let err = s
        .repo_read(params)
        .await
        .expect_err("a malformed --lines must fail");

    assert!(
        err.message().contains("not-a-range"),
        "error should name the offending range, got: {}",
        err.message()
    );
    let requests = server.received_requests().await.unwrap_or_default();
    assert!(
        requests.is_empty(),
        "no GitHub request should be made for a range that cannot parse, got {} request(s)",
        requests.len()
    );
}
