use globset::Glob;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use super::GitHubError;
use super::encoding::DecodeResult;
use super::types::{EntryType, TreeEntry};

/// Characters to percent-encode in URL path segments.
///
/// Preserves `/` for path structure but encodes query/fragment delimiters and special chars.
/// `:` is intentionally NOT in this set because GitHub API paths may legitimately contain
/// it (e.g., refs in URL form). `validate_ref` separately rejects `:` in ref-only positions
/// per `git-check-ref-format`. The asymmetry reflects two distinct rule surfaces:
/// URL path encoding (this set) vs git ref name validation (`validate_ref`).
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
pub(crate) fn parse_repo(repository: &str) -> Result<(&str, &str), GitHubError> {
    let stripped = repository
        .strip_prefix("https://github.com/")
        .or_else(|| repository.strip_prefix("http://github.com/"))
        .unwrap_or(repository)
        .trim_end_matches('/');
    let repo_str = stripped.strip_suffix(".git").unwrap_or(stripped);

    let parts: Vec<&str> = repo_str.splitn(3, '/').collect();
    if parts.len() < 2 || !is_valid_github_name(parts[0]) || !is_valid_github_name(parts[1]) {
        return Err(GitHubError::InvalidRepo(repository.to_owned()));
    }
    Ok((parts[0], parts[1]))
}

/// Validate a git ref (branch, tag, or SHA).
///
/// Rejects empty, control characters, and `..` sequences (git-check-ref-format).
pub(crate) fn validate_ref(ref_: &str) -> Result<(), GitHubError> {
    if ref_.is_empty()
        || ref_.contains(['\0', '\n', '\r', ' ', '~', '^', ':', '\\', '*', '?', '['])
        || ref_.contains("..")
        || ref_.ends_with('.')
        || ref_.ends_with(".lock")
    {
        return Err(GitHubError::InvalidRef(ref_.to_owned()));
    }
    Ok(())
}

/// Validate a file path within a repository.
///
/// Rejects empty, absolute paths, control characters, and `..` path traversal.
pub(crate) fn validate_path(path: &str) -> Result<(), GitHubError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(['\0', '\n', '\r'])
        || path.split('/').any(|s| s == "..")
    {
        return Err(GitHubError::InvalidPath(path.to_owned()));
    }
    Ok(())
}

/// Decode base64-encoded content from the GitHub Contents/Blob API.
///
/// `hint` is an optional encoding label (e.g. `"shift_jis"`) passed by the caller.
/// When `None`, chardetng auto-detects the encoding (BOM → chardetng → UTF-8 fallback).
///
/// Returns a [`DecodeResult`] containing the decoded text, encoding label, and detection source.
pub(crate) fn decode_content(
    encoded: &str,
    hint: Option<&str>,
) -> Result<DecodeResult, GitHubError> {
    let bytes = super::encoding::decode_base64(encoded)?;
    super::encoding::decode_bytes(&bytes, hint)
}

/// Parse a line range string: `"1-80"` (range), `"50-"` (open end), `"100"` (first N lines).
pub(crate) fn parse_line_range(range: &str) -> Result<(usize, Option<usize>), GitHubError> {
    let range = range.trim();
    let err = || GitHubError::InvalidLineRange(range.to_owned());

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
pub(crate) fn apply_line_range(content: &str, start: usize, end: Option<usize>) -> String {
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
pub(crate) fn filter_tree_entries<'a>(
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

    let dir_prefix = path.filter(|p| !p.ends_with('/')).map(|p| format!("{p}/"));

    Ok(entries
        .iter()
        .filter(|e| e.entry_type == EntryType::Blob)
        .filter(|e| {
            path.is_none_or(|prefix| {
                e.path == prefix || e.path.starts_with(dir_prefix.as_deref().unwrap_or(prefix))
            })
        })
        .filter(|e| {
            // ADR-0004 Rule 3: glob matches against the full repo-relative path
            // (e.g., `src/*.rs` matches `src/main.rs`). Previously matched only
            // the filename component, causing path-scoped patterns to silently
            // produce zero results.
            matcher.as_ref().is_none_or(|m| m.is_match(&e.path))
        })
        .collect())
}

#[cfg(test)]
mod tests;
