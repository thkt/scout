use serde::Deserialize;

/// Repository metadata from `GET /repos/{owner}/{repo}`.
#[derive(Deserialize, Debug)]
pub struct RepoInfo {
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

#[derive(Deserialize, Debug)]
pub struct LicenseInfo {
    pub spdx_id: Option<String>,
    pub name: String,
}

/// Response from `GET /repos/{owner}/{repo}/git/trees/{ref}?recursive=1`.
#[derive(Deserialize, Debug)]
pub struct TreeResponse {
    pub tree: Vec<TreeEntry>,
    pub truncated: bool,
}

/// Git object type. `Other` captures unknown types via `#[serde(other)]` for forward compat.
#[derive(Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    Blob,
    Tree,
    Commit,
    #[serde(other)]
    Other,
}

/// A single entry in a git tree (file, directory, or submodule).
#[derive(Deserialize, Debug)]
pub struct TreeEntry {
    pub path: String,
    #[serde(rename = "type")]
    pub entry_type: EntryType,
    pub size: Option<u64>,
}

/// Response from `GET /repos/{owner}/{repo}/contents/{path}`.
#[derive(Deserialize, Debug)]
pub struct ContentsResponse {
    pub sha: String,
    pub content: Option<String>,
}

/// Response from `GET /repos/{owner}/{repo}/git/blobs/{sha}`.
#[derive(Deserialize, Debug)]
pub struct BlobResponse {
    pub content: String,
}

#[derive(Deserialize, Debug)]
pub struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub labels: Vec<LabelInfo>,
    pub user: Option<UserInfo>,
    pub pull_request: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
pub struct LabelInfo {
    pub name: String,
}

#[derive(Deserialize, Debug)]
pub struct PullInfo {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub draft: Option<bool>,
    pub user: Option<UserInfo>,
}

#[derive(Deserialize, Debug)]
pub struct UserInfo {
    pub login: String,
}

#[derive(Deserialize, Debug)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub name: Option<String>,
    pub html_url: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
}
