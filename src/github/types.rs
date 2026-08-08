use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

/// Repository metadata from `GET /repos/{owner}/{repo}`.
#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct RepoInfo {
    pub full_name: String,
    pub description: Option<String>,
    pub html_url: String,
    pub default_branch: String,
    pub language: Option<String>,
    pub stargazers_count: u64,
    pub forks_count: u64,
    pub open_issues_count: u64,
    pub topics: Option<Vec<String>>,
    pub license: Option<LicenseInfo>,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct LicenseInfo {
    pub spdx_id: Option<String>,
    pub name: String,
}

/// Response from `GET /repos/{owner}/{repo}/git/trees/{ref}?recursive=1`.
#[derive(Deserialize, Debug)]
pub(crate) struct TreeResponse {
    pub tree: Vec<TreeEntry>,
    pub truncated: bool,
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
    pub path: String,
    #[serde(rename = "type")]
    pub entry_type: EntryType,
    pub size: Option<u64>,
}

/// Response from `GET /repos/{owner}/{repo}/contents/{path}`.
#[derive(Deserialize, Debug)]
pub(crate) struct ContentsResponse {
    pub sha: String,
    pub content: Option<String>,
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
    pub content: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub labels: Vec<LabelInfo>,
    pub user: Option<UserInfo>,
    /// Internal: present when GitHub's issues endpoint returns a PR.
    /// Not part of scout's public JSON output (#67/ADR-0010).
    #[serde(skip_serializing)]
    pub pull_request: Option<serde_json::Value>,
}

/// GitHub's `GET /repos/{owner}/{repo}/issues` endpoint returns PRs alongside
/// issues; callers that only want issues must apply this filter (#67/ADR-0010).
pub(crate) fn real_issues(issues: &[IssueInfo]) -> Vec<&IssueInfo> {
    issues.iter().filter(|i| i.pull_request.is_none()).collect()
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct LabelInfo {
    pub name: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct PullInfo {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub draft: Option<bool>,
    pub user: Option<UserInfo>,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct UserInfo {
    pub login: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub(crate) struct ReleaseInfo {
    pub tag_name: String,
    pub name: Option<String>,
    pub html_url: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-GHT001] real_issues は pull_request を持つ項目を除外し issue だけを返す
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
