//! `Scout` repository commands: tree listing, file read, and overview.

use std::io::{IsTerminal, stdin};
use std::time::Duration;

use tokio::time::timeout;
use tracing::{info, warn};

use crate::envelope::{CommandOutput, Degradation, DegradedReason};
use crate::github::types::ContentsResponse;
use crate::github::{self, GitHubClient, PerPage};

use super::errors::{parse_repo_param, unwrap_or_degraded};
use super::params::{RepoOverviewParams, RepoReadParams, RepoTreeParams};
use super::{Scout, ScoutError, StdinResolver, StdinState, read_stdin, resolve_stdin_arg, typo};

const OVERVIEW_ITEMS: PerPage = PerPage::new(5);
const OVERVIEW_RELEASES: PerPage = PerPage::new(3);

impl Scout {
    pub(super) async fn repo_tree(
        &self,
        params: RepoTreeParams,
    ) -> Result<CommandOutput, ScoutError> {
        let repository = resolve_stdin_arg(params.repository, "repository", "<OWNER/REPO>").await?;

        let (owner, repo) = parse_repo_param(&repository)?;

        info!(repository = %repository, "repo_tree");

        let github = self.github().await;

        let ref_ = match params.ref_ {
            Some(r) => {
                github::validate_ref(&r)?;
                r
            }
            None => github.get_repo(owner, repo).await?.default_branch,
        };

        if let Some(ref p) = params.path {
            github::validate_path(p)?;
        }

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
        let mut resolver = StdinResolver {
            is_terminal,
            state: match content {
                Some(s) => StdinState::Available(s),
                None => StdinState::NotPiped,
            },
        };
        let repository = resolver.resolve(params.repository, "repository", "<OWNER/REPO>")?;
        let path = resolver.resolve(params.path, "path", "<FILE_PATH>")?;

        let (owner, repo) = parse_repo_param(&repository)?;

        info!(repository = %repository, path = %path, "repo_read");

        github::validate_path(&path)?;
        if let Some(ref r) = params.ref_ {
            github::validate_ref(r)?;
        }

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
        let content = if let Some(ref range) = params.lines {
            let (start, end) = github::parse_line_range(range)?;
            github::apply_line_range(&raw, start, end)
        } else {
            github::apply_line_range(&raw, 1, None)
        };

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

        let (owner, repo) = parse_repo_param(&repository)?;

        info!(repository = %repository, "repo_overview");

        let github = self.github().await;

        // Verify repo exists first: a 404 here avoids 4 wasted parallel API calls (#18).
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
            markdown.push_str("\n> **Note:** ");
            markdown.push_str(&degradation.notes().join(". "));
            markdown.push_str(".\n");
        }

        // GitHub's issues endpoint returns PRs too; filter them out so JSON
        // consumers don't see PRs duplicated under issues.
        let real_issues: Vec<&github::types::IssueInfo> =
            issues.iter().filter(|i| i.pull_request.is_none()).collect();
        let data = serde_json::json!({
            "repository": repo_info,
            "readme": readme_content,
            "issues": real_issues,
            "pulls": pulls,
            "releases": releases,
        });

        info!(
            issues = issues.len(),
            pulls = pulls.len(),
            releases = releases.len(),
            has_readme = readme_content.is_some(),
            "repo_overview complete"
        );
        Ok(CommandOutput::with_degradation(markdown, data, degradation))
    }
}

/// Maximum time spent on best-effort candidate generation in the NotFound
/// error path. The user is already waiting on a failure; we'd rather skip
/// candidates than block them on a slow tree fetch.
const CANDIDATE_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum OSA distance and number of suggestions returned per error.
const CANDIDATE_MAX_DISTANCE: usize = 3;
const CANDIDATE_TOP_N: usize = 3;

/// Best-effort: fetch the repo tree and return up to `CANDIDATE_TOP_N` paths
/// most similar to `target` (OSA distance ≤ `CANDIDATE_MAX_DISTANCE`).
/// Returns empty on any API failure or if the fetch exceeds
/// `CANDIDATE_FETCH_TIMEOUT`.
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
        let entries = tree
            .tree
            .iter()
            .filter(|e| matches!(e.entry_type, github::types::EntryType::Blob))
            .map(|e| e.path.as_str());
        typo::closest_matches(target, entries, CANDIDATE_MAX_DISTANCE, CANDIDATE_TOP_N)
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
