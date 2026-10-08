//! `Scout` repository commands: tree listing, file read, and overview.

use std::io::{IsTerminal, stdin};
use std::time::Duration;

use tokio::time::timeout;
use tracing::{info, warn};

use crate::envelope::{CommandOutput, Degradation, DegradedReason};
use crate::github::types::ContentsResponse;
use crate::github::{self, GitHubClient, PerPage};

use super::errors::unwrap_or_degraded;
use super::params::{RepoOverviewParams, RepoReadParams, RepoTreeParams};
use super::{Scout, ScoutError, StdinResolver, read_stdin, resolve_stdin_arg, typo};

const OVERVIEW_ITEMS: PerPage = PerPage::new(5);
const OVERVIEW_RELEASES: PerPage = PerPage::new(3);

impl Scout {
    pub(super) async fn repo_tree(
        &self,
        params: RepoTreeParams,
    ) -> Result<CommandOutput, ScoutError> {
        let repository = resolve_stdin_arg(params.repository, "repository", "<OWNER/REPO>").await?;

        let (owner, repo) = github::parse_repo(&repository)?;

        info!(repository = %repository, "repo_tree");

        // Reject invalid arguments before token resolution or network requests.
        if let Some(ref p) = params.path {
            github::validate_path(p)?;
        }
        if let Some(ref r) = params.ref_ {
            github::validate_ref(r)?;
        }

        let github = self.github().await;

        let ref_ = match params.ref_ {
            Some(r) => r,
            None => github.get_repo(owner, repo).await?.default_branch,
        };

        let tree = github.get_tree(owner, repo, &ref_).await?;

        let filtered = github::filter_tree_entries(
            &tree.tree,
            params.path.as_deref(),
            params.pattern.as_deref(),
        )?;

        let markdown = github::format::format_tree(owner, repo, &ref_, &filtered, tree.truncated);
        let data = serde_json::json!({
            "owner": owner,
            "repo": repo,
            "ref": ref_,
            "entries": filtered,
            "truncated": tree.truncated,
        });

        info!(files = filtered.len(), "repo_tree complete");
        Ok(CommandOutput::ok(markdown, data))
    }

    pub(super) async fn repo_read(
        &self,
        params: RepoReadParams,
    ) -> Result<CommandOutput, ScoutError> {
        let is_terminal = stdin().is_terminal();
        let content = read_stdin(
            params.repository.as_deref() == Some("-")
                || params.path.as_deref() == Some("-")
                || (!is_terminal && (params.repository.is_none() || params.path.is_none())),
        )
        .await?;
        let mut resolver = StdinResolver::with_content(is_terminal, content);
        let repository = resolver.resolve(params.repository, "repository", "<OWNER/REPO>")?;
        let path = resolver.resolve(params.path, "path", "<FILE_PATH>")?;

        let (owner, repo) = github::parse_repo(&repository)?;

        info!(repository = %repository, path = %path, "repo_read");

        github::validate_path(&path)?;
        if let Some(ref r) = params.ref_ {
            github::validate_ref(r)?;
        }
        // Line-range syntax needs no file data; reject it before fetching content.
        let line_range = params
            .lines
            .as_deref()
            .map(github::parse_line_range)
            .transpose()?;

        let github = self.github().await;

        let contents = match github
            .get_contents(owner, repo, &path, params.ref_.as_deref())
            .await
        {
            Ok(c) => c,
            Err(github::GitHubError::NotFound(_)) => {
                let candidates =
                    collect_path_candidates(github, owner, repo, params.ref_.as_deref(), &path)
                        .await;
                let mut err = ScoutError::from(github::GitHubError::NotFound(path.clone()));
                if !candidates.is_empty() {
                    err = err.with_candidates(candidates);
                }
                return Err(err);
            }
            Err(e) => return Err(ScoutError::from(e)),
        };

        let hint = params.encoding.as_deref();
        let decode_result =
            if let Some(encoded) = contents.content.as_ref().filter(|c| !c.is_empty()) {
                github::decode_content(encoded, hint)?
            } else {
                let blob = github.get_blob(owner, repo, &contents.sha).await?;
                github::decode_content(&blob.content, hint)?
            };
        let encoding_label = match decode_result.source {
            github::encoding::DetectionSource::AssumedUtf8 => None,
            github::encoding::DetectionSource::Detected if decode_result.encoding == "utf-8" => {
                None
            }
            _ => Some(decode_result.encoding.clone()),
        };
        let raw = decode_result.text;

        let total = raw.lines().count();
        let (start, end) = line_range.unwrap_or((1, None));
        let content = github::apply_line_range(&raw, start, end);

        let markdown =
            github::format::format_file_content(&path, total, &content, encoding_label.as_deref());
        let data = serde_json::json!({
            "path": path,
            "total_lines": total,
            "content": content,
            "encoding": encoding_label,
        });

        info!(path = %path, lines = total, "repo_read complete");
        Ok(CommandOutput::ok(markdown, data))
    }

    pub(super) async fn repo_overview(
        &self,
        params: RepoOverviewParams,
    ) -> Result<CommandOutput, ScoutError> {
        let repository = resolve_stdin_arg(params.repository, "repository", "<OWNER/REPO>").await?;

        let (owner, repo) = github::parse_repo(&repository)?;

        info!(repository = %repository, "repo_overview");

        let github = self.github().await;

        // Verify repo exists first: a 404 here avoids 4 wasted parallel API calls.
        let repo_info = github.get_repo(owner, repo).await?;

        let (readme, issues, pulls, releases) = tokio::join!(
            github.get_readme(owner, repo),
            github.get_issues(owner, repo, OVERVIEW_ITEMS),
            github.get_pulls(owner, repo, OVERVIEW_ITEMS),
            github.get_releases(owner, repo, OVERVIEW_RELEASES),
        );

        let mut degradation = Degradation::default();
        let readme_content = resolve_readme(github, owner, repo, readme, &mut degradation).await;
        let issues =
            unwrap_or_degraded(issues, DegradedReason::IssuesFetchFailed, &mut degradation);
        let pulls = unwrap_or_degraded(pulls, DegradedReason::PullsFetchFailed, &mut degradation);
        let releases = unwrap_or_degraded(
            releases,
            DegradedReason::ReleasesFetchFailed,
            &mut degradation,
        );

        let mut markdown = github::format::format_overview(
            &repo_info,
            readme_content.as_deref(),
            &issues,
            &pulls,
            &releases,
        );

        if !degradation.is_empty() {
            markdown.push_str("\n> Note: ");
            markdown.push_str(&degradation.notes().join(". "));
            markdown.push_str(".\n");
        }

        // GitHub's issues endpoint also returns PRs.
        let real_issues = github::types::real_issues(&issues);
        let data = serde_json::json!({
            "repository": repo_info,
            "readme": readme_content,
            "issues": real_issues,
            "pulls": pulls,
            "releases": releases,
        });

        info!(
            issues = real_issues.len(),
            pulls = pulls.len(),
            releases = releases.len(),
            has_readme = readme_content.is_some(),
            "repo_overview complete"
        );
        Ok(CommandOutput::with_degradation(markdown, data, degradation))
    }
}

/// Budget for best-effort not-found hints, including tree lookup and matching.
/// Visible to the config test that checks the outer GitHub timeout exceeds it.
pub(super) const CANDIDATE_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

const CANDIDATE_MAX_DISTANCE: usize = 3;
const CANDIDATE_TOP_N: usize = 3;

/// Omit hints on API failure, timeout, or tree/matching work-cap exhaustion.
async fn collect_path_candidates(
    github: &GitHubClient,
    owner: &str,
    repo: &str,
    ref_: Option<&str>,
    target: &str,
) -> Vec<String> {
    let fut = async {
        let resolved_ref = match ref_ {
            Some(r) => r.to_owned(),
            None => match github.get_repo(owner, repo).await {
                Ok(info) => info.default_branch,
                Err(e) => {
                    warn!(%e, "candidate fetch: get_repo failed");
                    return Vec::new();
                }
            },
        };
        let tree = match github.get_tree(owner, repo, &resolved_ref).await {
            Ok(t) => t,
            Err(e) => {
                warn!(%e, "candidate fetch: get_tree failed");
                return Vec::new();
            }
        };
        if tree.truncated {
            warn!("candidate fetch: tree truncated (>100k entries); candidates may be incomplete");
        }
        // Count all entries, including directories, before the lazy blob filter:
        // otherwise an arbitrarily large directory-only tree is one sync scan.
        if tree.tree.len() > typo::MAX_POOL_ENTRIES {
            return Vec::new();
        }
        let entries = tree
            .tree
            .iter()
            .filter(|e| matches!(e.entry_type, github::types::EntryType::Blob))
            .map(|e| e.path.as_str());
        typo::closest_matches(target, entries, CANDIDATE_MAX_DISTANCE, CANDIDATE_TOP_N).await
    };
    timeout(CANDIDATE_FETCH_TIMEOUT, fut)
        .await
        .unwrap_or_default()
}

async fn resolve_readme(
    github: &GitHubClient,
    owner: &str,
    repo: &str,
    readme: Result<ContentsResponse, github::GitHubError>,
    degradation: &mut Degradation,
) -> Option<String> {
    let entry = match readme {
        Ok(r) => Some(r),
        Err(e) => {
            if !matches!(e, github::GitHubError::NotFound(_)) {
                warn!(%e, "failed to fetch README");
                degradation.push(
                    format!("Could not fetch README ({e})"),
                    DegradedReason::ReadmeFetchFailed,
                );
            }
            None
        }
    };

    let encoded = match entry {
        None => None,
        Some(r) if r.content.as_ref().is_some_and(|c| !c.is_empty()) => r.content,
        Some(r) => match github.get_blob(owner, repo, &r.sha).await {
            Ok(blob) => Some(blob.content).filter(|c| !c.is_empty()),
            Err(e) => {
                warn!(%e, "failed to fetch README blob");
                degradation.push(
                    format!("README could not be fetched ({e})"),
                    DegradedReason::ReadmeBlobFetchFailed,
                );
                None
            }
        },
    };

    encoded.and_then(|c| match github::decode_content(&c, None) {
        Ok(result) => Some(result.text),
        Err(e) => {
            warn!(%e, "failed to decode README");
            degradation.push(
                format!("README could not be decoded ({e})"),
                DegradedReason::ReadmeDecodeFailed,
            );
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::try_spawn_mock_server;
    use crate::tools::test_helpers::scout_with_github;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, ResponseTemplate};

    /// [T-TS040] Repo-read preserves ranked small-tree hints and not-found output;
    /// long-path work and raw tree limits (including directories) omit hints.
    #[tokio::test]
    async fn repo_read_not_found_preserves_contract_with_bounded_candidates() {
        let Some(server) = try_spawn_mock_server("repo_read_bounded_candidates").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path_regex(r"^/repos/owner/repo/contents/"))
            .respond_with(ResponseTemplate::new(404))
            .expect(3)
            .mount(&server)
            .await;

        let blob = |p: &str| serde_json::json!({"path": p, "type": "blob"});
        let small = vec![
            serde_json::json!({"path": "REDME.md", "type": "tree"}),
            blob("REDXX.md"),
            blob("README.md"),
            blob("REDAME.md"),
            blob("REDME.mx"),
            blob("totally-different.txt"),
        ];
        let long_path = "a".repeat(400);
        let long = vec![blob(&long_path); 100];
        let mut large = vec![blob("README.md")];
        large.extend(
            (0..4_096)
                .map(|i| serde_json::json!({"path": format!("directory-{i}"), "type": "tree"})),
        );

        let scout = scout_with_github("http://localhost:0", &server.uri());
        for (ref_, target, tree, candidates) in [
            (
                "small",
                "REDME.md",
                small,
                Some(vec!["README.md", "REDAME.md", "REDME.mx"]),
            ),
            ("long", long_path.as_str(), long, None),
            ("large", "REDME.md", large, None),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/repos/owner/repo/git/trees/{ref_}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "tree": tree,
                    "truncated": false
                })))
                .expect(1)
                .mount(&server)
                .await;
            let err = scout
                .run(super::super::Command::RepoRead(RepoReadParams {
                    repository: Some("owner/repo".into()),
                    path: Some(target.into()),
                    ref_: Some(ref_.into()),
                    lines: None,
                    encoding: None,
                }))
                .await
                .unwrap_err();
            assert_eq!(err.exit_code(), 66, "{ref_}");
            let mut expected = serde_json::json!({
                "error": {
                    "code": "NOT_FOUND",
                    "message": format!("Not found: {target}"),
                    "next_step": "Check that the repository or path exists, and that you have access",
                    "retryable": false
                }
            });
            if let Some(candidates) = candidates {
                expected["error"]["candidates"] = serde_json::json!(candidates);
            }
            let actual: serde_json::Value =
                serde_json::from_str(&crate::render_json_error(&err)).unwrap();
            assert_eq!(actual, expected, "{ref_}");
            assert_eq!(
                err.to_string(),
                format!(
                    "Not found: {target} — Check that the repository or path exists, and that you have access"
                )
            );
        }
    }

    /// [T-TS032] Repo-overview excludes the PR-backed fixture from Markdown and JSON.
    #[tokio::test]
    async fn repo_overview_markdown_and_json_agree_on_pr_backed_issue_exclusion() {
        let Some(server) = try_spawn_mock_server("tools::repo::t_seam_004").await else {
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
            .and(path("/repos/owner/repo/readme"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex(r"^/repos/owner/repo/issues"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "number": 1,
                    "title": "Real issue",
                    "html_url": "https://github.com/owner/repo/issues/1",
                    "labels": [],
                    "user": null,
                    "pull_request": null
                },
                {
                    "number": 2,
                    "title": "PR as issue",
                    "html_url": "https://github.com/owner/repo/issues/2",
                    "labels": [],
                    "user": null,
                    "pull_request": {}
                }
            ])))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex(r"^/repos/owner/repo/pulls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex(r"^/repos/owner/repo/releases"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let s = scout_with_github(&server.uri(), &server.uri());
        let params = RepoOverviewParams {
            repository: Some("owner/repo".into()),
        };

        let result = s
            .repo_overview(params)
            .await
            .expect("repo_overview should succeed against the mocked GitHub API");

        assert!(result.markdown().contains("Real issue"));
        assert!(!result.markdown().contains("PR as issue"));

        let issues_json = result.data()["issues"]
            .as_array()
            .expect("data.issues should be a JSON array");
        assert_eq!(
            issues_json.len(),
            1,
            "JSON issues array should exclude the same PR-backed entry the Markdown \
             Recent Issues section excludes"
        );
        assert_eq!(issues_json[0]["number"], 1);
    }

    /// Mock a failed issues endpoint and a missing README; return None if binding fails.
    async fn overview_with_failed_issues(label: &str) -> Option<CommandOutput> {
        let server = try_spawn_mock_server(label).await?;

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
            .and(path("/repos/owner/repo/readme"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // 422 rather than 500: it classifies as a data error, so the retry loop
        // does not run and the test does not wait out two backoffs.
        Mock::given(method("GET"))
            .and(path_regex(r"^/repos/owner/repo/issues"))
            .respond_with(ResponseTemplate::new(422))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/repos/owner/repo/pulls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/repos/owner/repo/releases"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let s = scout_with_github(&server.uri(), &server.uri());
        Some(
            s.repo_overview(RepoOverviewParams {
                repository: Some("owner/repo".into()),
            })
            .await
            .expect("a failed section degrades the overview rather than failing it"),
        )
    }

    /// [T-TS037] Failed issues produce a Markdown note and a typed degradation reason.
    #[tokio::test]
    async fn repo_overview_states_a_failed_section_in_the_body() {
        let Some(result) = overview_with_failed_issues("tools::repo::degraded_note").await else {
            return;
        };

        assert!(
            result.markdown().contains("> Note: Could not fetch issues"),
            "the failed section must be stated in the body, got: {}",
            result.markdown()
        );
        assert!(
            result
                .degraded_reasons()
                .contains(&DegradedReason::IssuesFetchFailed),
            "the envelope must carry the typed reason, got: {:?}",
            result.degraded_reasons()
        );
    }

    /// [T-TS038] Serialize repo-overview's issues failure as degraded with its reason.
    #[tokio::test]
    async fn a_partial_failure_reaches_the_json_output_as_degraded() {
        let Some(result) = overview_with_failed_issues("tools::repo::degraded_json").await else {
            return;
        };

        let json = serde_json::to_value(result.into_envelope())
            .expect("the success envelope must serialize");

        assert_eq!(
            json["degraded"],
            serde_json::json!(true),
            "a partial failure must set the degraded flag, got: {json}"
        );
        assert_eq!(
            json["degraded_reasons"],
            serde_json::json!(["ISSUES_FETCH_FAILED"]),
            "the typed reason must reach the wire format, got: {json}"
        );
    }
}
