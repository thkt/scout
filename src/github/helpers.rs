use base64::{Engine as _, engine::general_purpose::STANDARD};
use globset::Glob;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use super::GitHubError;
use super::types::{EntryType, TreeEntry};

/// Characters to percent-encode in URL path segments.
/// Preserves `/` for path structure but encodes query/fragment delimiters and special chars.
const PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'?')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'+')
    .add(b'@')
    .add(b'[')
    .add(b']')
    .add(b';')
    .add(b'=');

pub(super) fn encode_path(s: &str) -> String {
    utf8_percent_encode(s, PATH_ENCODE_SET).to_string()
}

fn is_valid_github_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && s != ".."
        && s != "."
}

/// Parse a repository identifier into `(owner, repo)`.
///
/// Accepts `"owner/repo"`, full GitHub URLs, and `.git` suffixed URLs.
pub fn parse_repo(repository: &str) -> Result<(&str, &str), GitHubError> {
    let stripped = repository
        .strip_prefix("https://github.com/")
        .or_else(|| repository.strip_prefix("http://github.com/"))
        .unwrap_or(repository)
        .trim_end_matches('/');
    let repo_str = stripped.strip_suffix(".git").unwrap_or(stripped);

    let parts: Vec<&str> = repo_str.splitn(3, '/').collect();
    if parts.len() < 2 || !is_valid_github_name(parts[0]) || !is_valid_github_name(parts[1]) {
        return Err(GitHubError::InvalidRepo(repository.to_string()));
    }
    Ok((parts[0], parts[1]))
}

/// Validate a git ref (branch, tag, or SHA).
///
/// Rejects empty, control characters, and `..` sequences (git-check-ref-format).
pub fn validate_ref(ref_: &str) -> Result<(), GitHubError> {
    if ref_.is_empty()
        || ref_.contains(['\0', '\n', '\r', ' ', '~', '^', ':', '\\', '*', '?', '['])
        || ref_.contains("..")
        || ref_.ends_with('.')
        || ref_.ends_with(".lock")
    {
        return Err(GitHubError::InvalidRef(ref_.to_string()));
    }
    Ok(())
}

/// Validate a file path within a repository.
///
/// Rejects empty, absolute paths, control characters, and `..` path traversal.
pub fn validate_path(path: &str) -> Result<(), GitHubError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(['\0', '\n', '\r'])
        || path.split('/').any(|s| s == "..")
    {
        return Err(GitHubError::InvalidPath(path.to_string()));
    }
    Ok(())
}

/// Decode base64-encoded content from the GitHub Contents/Blob API.
pub fn decode_content(encoded: &str) -> Result<String, GitHubError> {
    let clean: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = STANDARD
        .decode(&clean)
        .map_err(|e| GitHubError::Decode(e.to_string()))?;
    String::from_utf8(bytes)
        .map_err(|_| GitHubError::Decode("file appears to be binary (not valid UTF-8)".into()))
}

/// Parse a line range string: `"1-80"` (range), `"50-"` (open end), `"100"` (first N lines).
pub fn parse_line_range(range: &str) -> Result<(usize, Option<usize>), GitHubError> {
    let range = range.trim();
    let err = || GitHubError::InvalidLineRange(range.to_string());

    if range.is_empty() {
        return Err(err());
    }

    if let Some((start, end)) = range.split_once('-') {
        let start: usize = start.trim().parse().map_err(|_| err())?;
        if start == 0 {
            return Err(err());
        }
        if end.trim().is_empty() {
            Ok((start, None))
        } else {
            let end: usize = end.trim().parse().map_err(|_| err())?;
            if end < start {
                return Err(err());
            }
            Ok((start, Some(end)))
        }
    } else {
        let n: usize = range.parse().map_err(|_| err())?;
        if n == 0 {
            return Err(err());
        }
        Ok((1, Some(n)))
    }
}

/// Extract a line range from content, returning numbered lines.
pub fn apply_line_range(content: &str, start: usize, end: Option<usize>) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let start_idx = start.saturating_sub(1);
    let end_idx = end.map(|e| e.min(total)).unwrap_or(total);

    if start_idx >= total {
        return format!("(file has {total} lines, requested start at {start})");
    }

    lines[start_idx..end_idx]
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:>5}\t{}", start_idx + i + 1, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Filter tree entries to blobs matching an optional path prefix and glob pattern.
pub fn filter_tree_entries<'a>(
    entries: &'a [TreeEntry],
    path: Option<&str>,
    pattern: Option<&str>,
) -> Result<Vec<&'a TreeEntry>, GitHubError> {
    let matcher = pattern
        .map(|p| {
            Glob::new(p)
                .map_err(|e| GitHubError::InvalidPattern(e.to_string()))
                .map(|g| g.compile_matcher())
        })
        .transpose()?;

    Ok(entries
        .iter()
        .filter(|e| e.entry_type == EntryType::Blob)
        .filter(|e| path.is_none_or(|prefix| e.path.starts_with(prefix)))
        .filter(|e| {
            matcher.as_ref().is_none_or(|m| {
                let filename = e.path.rsplit('/').next().unwrap_or(&e.path);
                m.is_match(filename)
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;

    #[test]
    fn parse_repo_valid_formats() {
        for (input, owner, repo) in [
            ("facebook/react", "facebook", "react"),
            ("https://github.com/facebook/react", "facebook", "react"),
            (
                "https://github.com/facebook/react/tree/main/src",
                "facebook",
                "react",
            ),
            ("https://github.com/facebook/react.git", "facebook", "react"),
            ("owner/repo.git", "owner", "repo"),
            ("user/user.github.io", "user", "user.github.io"),
        ] {
            let (o, r) = parse_repo(input).unwrap_or_else(|_| panic!("should parse: {input}"));
            assert_eq!((o, r), (owner, repo), "input: {input}");
        }
    }

    #[test]
    fn parse_repo_rejects_invalid() {
        for input in [
            "",
            "facebook",
            "owner?/repo",
            "../repo",
            "owner#/repo",
            "owner/repo?q=1",
            "owner/..",
        ] {
            assert!(parse_repo(input).is_err(), "should reject: {input}");
        }
    }

    #[test]
    fn validate_ref_valid() {
        assert!(validate_ref("feature/my-branch").is_ok());
    }

    #[test]
    fn validate_ref_invalid() {
        for input in [
            "",
            "main\0",
            "refs/../../HEAD",
            "main..develop",
            "ref with space",
            "ref~1",
            "ref^2",
            "ref:path",
            "ref\\path",
            "ref*glob",
            "ref?wild",
            "ref[bracket",
            "branch.",
            "refs/heads/main.lock",
        ] {
            assert!(validate_ref(input).is_err(), "should reject ref: {input}");
        }
    }

    #[test]
    fn validate_path_valid() {
        for input in [
            "src/lib.rs",
            ".github/workflows/ci.yml",
            "path/to/file..name",
        ] {
            assert!(validate_path(input).is_ok(), "should accept path: {input}");
        }
    }

    #[test]
    fn validate_path_invalid() {
        for input in [
            "",
            "/etc/passwd",
            "../etc/passwd",
            "src/../../secret",
            "a/..",
        ] {
            assert!(validate_path(input).is_err(), "should reject path: {input}");
        }
    }

    #[test]
    fn encode_path_encodes_special_chars() {
        assert_eq!(encode_path("main?recursive=0"), "main%3Frecursive%3D0");
        assert_eq!(encode_path("ref#frag"), "ref%23frag");
        assert_eq!(encode_path("a b"), "a%20b");
        assert_eq!(encode_path("100%"), "100%25");
        assert_eq!(encode_path("a&b"), "a%26b");
        assert!(encode_path("ref+1").contains("%2B"));
        assert!(encode_path("a@b").contains("%40"));
        assert!(encode_path("a[0]").contains("%5B"));
        assert!(encode_path("a;b").contains("%3B"));
    }

    #[test]
    fn encode_path_preserves_slashes_and_encodes_non_ascii() {
        assert_eq!(encode_path("feature/my-branch"), "feature/my-branch");
        let encoded = encode_path("docs/日本語.md");
        assert!(encoded.starts_with("docs/"));
        assert!(!encoded.contains("日本語"));
    }

    #[test]
    fn parse_line_range_valid() {
        assert_eq!(parse_line_range("1-80").unwrap(), (1, Some(80)));
        assert_eq!(parse_line_range("50-").unwrap(), (50, None));
        assert_eq!(parse_line_range("100").unwrap(), (1, Some(100)));
    }

    #[test]
    fn parse_line_range_invalid() {
        for input in ["0", "80-1", "0-10"] {
            assert!(parse_line_range(input).is_err(), "should reject: {input}");
        }
    }

    #[test]
    fn apply_line_range_subset() {
        let result = apply_line_range("line1\nline2\nline3\nline4\nline5", 2, Some(4));
        assert!(result.contains("line2") && result.contains("line4"));
        assert!(!result.contains("line1") && !result.contains("line5"));
    }

    #[test]
    fn apply_line_range_open_end() {
        let result = apply_line_range("line1\nline2\nline3", 2, None);
        assert!(result.contains("line2") && !result.contains("line1"));
    }

    #[test]
    fn apply_line_range_beyond_file() {
        assert!(apply_line_range("line1\nline2", 5, None).contains("2 lines"));
    }

    #[test]
    fn decode_content_handles_base64() {
        assert_eq!(
            decode_content(&STANDARD.encode("hello world")).unwrap(),
            "hello world"
        );
        assert_eq!(
            decode_content("aGVs\nbG8g\nd29y\nbGQ=\n").unwrap(),
            "hello world"
        );
    }

    fn blob(path: &str) -> TreeEntry {
        TreeEntry {
            path: path.into(),
            entry_type: EntryType::Blob,
            size: Some(100),
        }
    }

    fn tree(path: &str) -> TreeEntry {
        TreeEntry {
            path: path.into(),
            entry_type: EntryType::Tree,
            size: None,
        }
    }

    #[test]
    fn filter_by_path_prefix() {
        let entries = vec![blob("src/main.rs"), blob("tests/test.rs"), tree("src")];
        let filtered = filter_tree_entries(&entries, Some("src/"), None).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "src/main.rs");
    }

    #[test]
    fn filter_by_glob_pattern() {
        let entries = vec![blob("src/main.rs"), blob("src/lib.ts"), blob("README.md")];
        let filtered = filter_tree_entries(&entries, None, Some("*.rs")).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "src/main.rs");
    }

    #[test]
    fn filter_excludes_tree_entries() {
        let entries = vec![tree("src"), blob("src/main.rs")];
        let filtered = filter_tree_entries(&entries, None, None).unwrap();
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn filter_excludes_commit_entries() {
        let entries = vec![
            blob("src/main.rs"),
            TreeEntry {
                path: "submodule".into(),
                entry_type: EntryType::Commit,
                size: None,
            },
        ];
        let filtered = filter_tree_entries(&entries, None, None).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "src/main.rs");
    }

    #[test]
    fn extract_error_message_from_json() {
        assert_eq!(
            super::super::extract_error_message(r#"{"message": "Not Found"}"#),
            "Not Found"
        );
        assert_eq!(
            super::super::extract_error_message("plain text"),
            "plain text"
        );
    }

    #[test]
    fn is_valid_github_name_accepts_normal() {
        for name in ["facebook", "my-repo.js", "repo_name"] {
            assert!(is_valid_github_name(name), "should accept: {name}");
        }
    }

    #[test]
    fn is_valid_github_name_rejects_special() {
        for name in ["", "..", "repo?q", "repo#frag", "a/b"] {
            assert!(!is_valid_github_name(name), "should reject: {name}");
        }
    }
}
