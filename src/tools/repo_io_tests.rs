use super::test_helpers::*;
use super::*;
use crate::test_support::try_spawn_mock_server;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, ResponseTemplate};

/// [T-TS017] repo_read: --encoding hint is passed to decode_content and
/// used to decode non-UTF-8 content correctly.
#[tokio::test]
async fn repo_read_decodes_with_encoding_hint() {
    let Some(server) = try_spawn_mock_server("tools::t_008").await else {
        return;
    };

    // "テスト" in Shift_JIS ([0x83, 0x65, 0x83, 0x58, 0x83, 0x67]), base64-encoded.
    // Without --encoding, chardetng auto-detects Shift_JIS for 6 bytes too.
    // With --encoding shift_jis, decode_explicit is used (deterministic).
    let shift_jis_b64 = "g2WDWINn";

    Mock::given(method("GET"))
        .and(path_regex(r"^/repos/owner/repo/contents/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sha": "abc123",
            "content": shift_jis_b64
        })))
        .mount(&server)
        .await;

    let s = scout_with_github("http://localhost:0", &server.uri());
    let params = RepoReadParams {
        repository: Some("owner/repo".into()),
        path: Some("test.txt".into()),
        ref_: None,
        lines: None,
        encoding: Some("shift_jis".into()),
    };

    let result = s.repo_read(params).await.unwrap();
    assert!(
        result.markdown().contains("テスト"),
        "output should contain decoded Shift_JIS text, got: {result:?}"
    );
    assert!(
        result.markdown().contains("[encoding: shift_jis]"),
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

    Mock::given(method("GET"))
        .and(path("/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "full_name": "owner/repo",
            "description": null,
            "html_url": "https://github.com/owner/repo",
            "default_branch": "main",
            "language": null,
            "stargazers_count": 0,
            "forks_count": 0,
            "open_issues_count": 0,
            "topics": null,
            "license": null
        })))
        .mount(&server)
        .await;

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
