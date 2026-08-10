use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

/// Repository metadata from `GET /repos/{owner}/{repo}`.
#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct RepoInfo {
    pub(super) full_name: String,
    pub(super) description: Option<String>,
    pub(super) html_url: String,
    pub(crate) default_branch: String,
    pub(super) language: Option<String>,
    pub(super) stargazers_count: u64,
    pub(super) forks_count: u64,
    pub(super) open_issues_count: u64,
    pub(super) topics: Option<Vec<String>>,
    pub(super) license: Option<LicenseInfo>,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct LicenseInfo {
    pub(super) spdx_id: Option<String>,
    pub(super) name: String,
}

/// Response from `GET /repos/{owner}/{repo}/git/trees/{ref}?recursive=1`.
#[derive(Deserialize, Debug)]
pub(crate) struct TreeResponse {
    pub(crate) tree: Vec<TreeEntry>,
    pub(crate) truncated: bool,
}

/// Git object type. `Other` captures unknown types via `#[serde(other)]` for forward compat.
#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EntryType {
    Blob,
    Tree,
    Commit,
    #[serde(other)]
    Other,
}

/// A single entry in a git tree (file, directory, or submodule).
#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct TreeEntry {
    pub(crate) path: String,
    #[serde(rename = "type")]
    pub(crate) entry_type: EntryType,
    pub(super) size: Option<u64>,
}

/// Response from `GET /repos/{owner}/{repo}/contents/{path}`.
#[derive(Deserialize, Debug)]
pub(crate) struct ContentsResponse {
    pub(crate) sha: String,
    pub(crate) content: Option<String>,
}

/// Either shape the contents endpoint can answer with. A file yields an object,
/// a directory a listing array; `untagged` picks by shape, so a body matching
/// neither still surfaces as a decode failure rather than being misread as one
/// of the two. The listing's contents are not modeled — `repo-tree` is what
/// reads directories, and here the shape alone answers "was this a file?".
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum ContentsPayload {
    File(ContentsResponse),
    Directory(IgnoredAny),
}

/// Response from `GET /repos/{owner}/{repo}/git/blobs/{sha}`.
#[derive(Deserialize, Debug)]
pub(crate) struct BlobResponse {
    pub(crate) content: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct IssueInfo {
    pub(super) number: u64,
    pub(super) title: String,
    pub(super) html_url: String,
    pub(super) labels: Vec<LabelInfo>,
    pub(super) user: Option<UserInfo>,
    /// Internal: present when GitHub's issues endpoint returns a PR.
    /// Not part of scout's public JSON output (#67/ADR-0010).
    #[serde(skip_serializing)]
    pub(super) pull_request: Option<serde_json::Value>,
}

/// GitHub's `GET /repos/{owner}/{repo}/issues` endpoint returns PRs alongside
/// issues; callers that only want issues must apply this filter (#67/ADR-0010).
pub(crate) fn real_issues(issues: &[IssueInfo]) -> Vec<&IssueInfo> {
    issues.iter().filter(|i| i.pull_request.is_none()).collect()
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct LabelInfo {
    pub(super) name: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct PullInfo {
    pub(super) number: u64,
    pub(super) title: String,
    pub(super) html_url: String,
    pub(super) draft: Option<bool>,
    pub(super) user: Option<UserInfo>,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct UserInfo {
    pub(super) login: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct ReleaseInfo {
    pub(super) tag_name: String,
    pub(super) name: Option<String>,
    pub(super) html_url: String,
    pub(super) published_at: Option<String>,
    pub(super) prerelease: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-GHT001] real_issues drops entries carrying pull_request and returns issues only
    #[test]
    fn real_issues_excludes_pull_requests_and_returns_issues_only() {
        let issues = vec![
            IssueInfo {
                number: 1,
                title: "Real issue".into(),
                html_url: "https://github.com/o/r/issues/1".into(),
                labels: vec![],
                user: None,
                pull_request: None,
            },
            IssueInfo {
                number: 2,
                title: "PR as issue".into(),
                html_url: "https://github.com/o/r/issues/2".into(),
                labels: vec![],
                user: None,
                pull_request: Some(serde_json::json!({})),
            },
        ];

        let result = real_issues(&issues);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].number, 1);
    }
}
