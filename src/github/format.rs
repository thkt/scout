use std::fmt::Write;

use super::types::{IssueInfo, PullInfo, ReleaseInfo, RepoInfo, TreeEntry, UserInfo, real_issues};
use crate::markdown::{escape_md_inline, md_link, shift_headings, truncation_note};

const MAX_README_BYTES: usize = 24_000;

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Build a safe fenced code block delimiter that is longer than any backtick
/// run found in `content`.
fn fence_delimiter(content: &str) -> String {
    let max_run = content
        .bytes()
        .fold((0usize, 0usize), |(longest, run), b| {
            if b == b'`' {
                let next = run + 1;
                (longest.max(next), next)
            } else {
                (longest, 0)
            }
        })
        .0;
    "`".repeat(max_run.max(2) + 1)
}

/// Infer a Markdown language identifier from a file path's extension.
fn lang_for_path(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" => "javascript",
        "jsx" => "jsx",
        "py" => "python",
        "rb" => "ruby",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "hpp" => "cpp",
        "md" => "markdown",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "sh" | "bash" => "bash",
        "sql" => "sql",
        "html" => "html",
        "css" => "css",
        _ => "",
    }
}

/// Format file content as a fenced Markdown code block with a path header.
///
/// `encoding` is shown in the header when the file uses a non-default encoding
/// (e.g. `Some("shift_jis")`). Pass `None` for plain UTF-8 files.
pub(crate) fn format_file_content(
    path: &str,
    total: usize,
    content: &str,
    encoding: Option<&str>,
) -> String {
    let lang = lang_for_path(path);
    let fence = fence_delimiter(content);
    let header = match encoding {
        Some(enc) => format!("{path} ({total} lines) [encoding: {enc}]"),
        None => format!("{path} ({total} lines)"),
    };
    format!("{header}\n\n{fence}{lang}\n{content}\n{fence}")
}

pub(crate) fn format_tree(
    owner: &str,
    repo: &str,
    ref_: &str,
    entries: &[&TreeEntry],
    truncated: bool,
) -> String {
    let mut out = format!("{owner}/{repo} (ref: {ref_})\n");
    let _ = write!(out, "files: {}", entries.len());
    if truncated {
        out.push_str(" (tree truncated by GitHub — repository exceeds API limits)");
    }
    out.push_str("\n\n");

    for entry in entries {
        out.push_str(&entry.path);
        if let Some(size) = entry.size {
            let _ = write!(out, " ({})", format_size(size));
        }
        out.push('\n');
    }

    out
}

pub(crate) fn format_overview(
    repo: &RepoInfo,
    readme: Option<&str>,
    issues: &[IssueInfo],
    pulls: &[PullInfo],
    releases: &[ReleaseInfo],
) -> String {
    let mut out = format!("# {}\n\n", escape_md_inline(&repo.full_name));

    if let Some(ref desc) = repo.description {
        let _ = writeln!(out, "{}\n", escape_md_inline(desc));
    }

    format_metadata_table(repo, &mut out);
    format_readme_section(readme, &mut out);
    format_issues_section(issues, &mut out);
    format_pulls_section(pulls, &mut out);
    format_releases_section(releases, &mut out);

    out
}

fn format_metadata_table(repo: &RepoInfo, out: &mut String) {
    out.push_str("| Attribute | Value |\n|-----------|-------|\n");
    if let Some(ref lang) = repo.language {
        let _ = writeln!(out, "| Language | {} |", escape_md_inline(lang));
    }
    let _ = writeln!(out, "| Stars | {} |", repo.stargazers_count);
    let _ = writeln!(out, "| Forks | {} |", repo.forks_count);
    let _ = writeln!(out, "| Open Issues | {} |", repo.open_issues_count);
    if let Some(ref license) = repo.license {
        let name = license.spdx_id.as_deref().unwrap_or(&license.name);
        let _ = writeln!(out, "| License | {} |", escape_md_inline(name));
    }
    let _ = writeln!(
        out,
        "| Default Branch | {} |",
        escape_md_inline(&repo.default_branch)
    );
    let topics = repo.topics.as_deref().unwrap_or(&[]);
    if !topics.is_empty() {
        let _ = writeln!(out, "| Topics | {} |", escape_md_inline(&topics.join(", ")));
    }
    let _ = writeln!(out, "| URL | {} |\n", escape_md_inline(&repo.html_url));
}

fn format_readme_section(readme: Option<&str>, out: &mut String) {
    let Some(content) = readme else { return };
    out.push_str("## README\n\n");
    if content.len() > MAX_README_BYTES {
        // Not reusing truncate_with_note because shift_headings must run
        // between truncation and note addition.
        let boundary = content.floor_char_boundary(MAX_README_BYTES);
        let end = content[..boundary]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(boundary);
        out.push_str(&shift_headings(&content[..end], 2));
        out.push_str(&truncation_note(end, content.len()));
    } else {
        out.push_str(&shift_headings(content, 2));
    }
    out.push_str("\n\n");
}

/// Render the ` — @login` suffix for an optional author, or an empty string
/// when no author is present.
fn author_suffix(user: Option<&UserInfo>) -> String {
    user.map(|u| format!(" — @{}", escape_md_inline(&u.login)))
        .unwrap_or_default()
}

fn format_issues_section(issues: &[IssueInfo], out: &mut String) {
    let real_issues = real_issues(issues);
    if real_issues.is_empty() {
        return;
    }
    out.push_str("## Recent Issues\n\n");
    for issue in &real_issues {
        let labels = if issue.labels.is_empty() {
            String::new()
        } else {
            format!(
                " ({})",
                issue
                    .labels
                    .iter()
                    .map(|l| l.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let user = author_suffix(issue.user.as_ref());
        let _ = writeln!(
            out,
            "- {} {}{}{}",
            md_link(&format!("#{}", issue.number), &issue.html_url),
            escape_md_inline(&issue.title),
            escape_md_inline(&labels),
            user
        );
    }
    out.push('\n');
}

fn format_pulls_section(pulls: &[PullInfo], out: &mut String) {
    if pulls.is_empty() {
        return;
    }
    out.push_str("## Recent Pull Requests\n\n");
    for pr in pulls {
        let draft = if pr.draft.unwrap_or(false) {
            " [draft]"
        } else {
            ""
        };
        let user = author_suffix(pr.user.as_ref());
        let _ = writeln!(
            out,
            "- {} {}{}{}",
            md_link(&format!("#{}", pr.number), &pr.html_url),
            escape_md_inline(&pr.title),
            draft,
            user
        );
    }
    out.push('\n');
}

fn format_releases_section(releases: &[ReleaseInfo], out: &mut String) {
    if releases.is_empty() {
        return;
    }
    out.push_str("## Recent Releases\n\n");
    for release in releases {
        let name = release.name.as_deref().unwrap_or(&release.tag_name);
        let date = release
            .published_at
            .as_deref()
            .and_then(|d| d.get(..10))
            .unwrap_or("—");
        let pre = if release.prerelease {
            " (pre-release)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "- {} — {}{}",
            md_link(name, &release.html_url),
            date,
            pre
        );
    }
    out.push('\n');
}

#[cfg(test)]
mod file_content_tests;
#[cfg(test)]
mod overview_tests;
#[cfg(test)]
mod size_tests;
#[cfg(test)]
mod tree_tests;
