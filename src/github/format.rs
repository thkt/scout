use std::fmt::Write;

use super::types::{IssueInfo, PullInfo, ReleaseInfo, RepoInfo, TreeEntry};
use crate::markdown::{escape_md_inline, escape_md_link, shift_headings};

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

/// Format a comprehensive repository overview with metadata, README, issues, PRs, and releases.
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
        let _ = write!(
            out,
            "\n\n(truncated: showing {end} / {} bytes)",
            content.len()
        );
    } else {
        out.push_str(&shift_headings(content, 2));
    }
    out.push_str("\n\n");
}

fn format_issues_section(issues: &[IssueInfo], out: &mut String) {
    let real_issues: Vec<_> = issues.iter().filter(|i| i.pull_request.is_none()).collect();
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
        let user = issue
            .user
            .as_ref()
            .map(|u| format!(" — @{}", escape_md_inline(&u.login)))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- [#{}]({}) {}{}{}",
            issue.number,
            escape_md_link(&issue.html_url),
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
        let user = pr
            .user
            .as_ref()
            .map(|u| format!(" — @{}", escape_md_inline(&u.login)))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- [#{}]({}) {}{}{}",
            pr.number,
            escape_md_link(&pr.html_url),
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
            "- [{}]({}) — {}{}",
            escape_md_inline(name),
            escape_md_link(&release.html_url),
            date,
            pre
        );
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::types::{EntryType, LabelInfo, LicenseInfo, UserInfo};

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(500), "500 B");
    }

    #[test]
    fn format_size_kilobytes() {
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn format_size_megabytes() {
        assert_eq!(format_size(2_621_440), "2.5 MB");
    }

    #[test]
    fn format_tree_basic() {
        let entries = [
            TreeEntry {
                path: "src/main.rs".into(),
                entry_type: EntryType::Blob,
                size: Some(1024),
            },
            TreeEntry {
                path: "README.md".into(),
                entry_type: EntryType::Blob,
                size: Some(256),
            },
        ];
        let refs: Vec<&TreeEntry> = entries.iter().collect();
        let output = format_tree("owner", "repo", "main", &refs, false);
        assert!(output.contains("owner/repo (ref: main)"));
        assert!(output.contains("files: 2"));
        assert!(output.contains("src/main.rs (1.0 KB)"));
        assert!(output.contains("README.md (256 B)"));
    }

    #[test]
    fn format_tree_truncated() {
        let output = format_tree("o", "r", "main", &[], true);
        assert!(output.contains("truncated"));
    }

    fn sample_repo() -> RepoInfo {
        RepoInfo {
            full_name: "owner/repo".into(),
            description: Some("A test repo".into()),
            html_url: "https://github.com/owner/repo".into(),
            default_branch: "main".into(),
            language: Some("Rust".into()),
            stargazers_count: 42,
            forks_count: 5,
            open_issues_count: 3,
            topics: Some(vec!["rust".into(), "cli".into()]),
            license: Some(LicenseInfo {
                spdx_id: Some("MIT".into()),
                name: "MIT License".into(),
            }),
        }
    }

    #[test]
    fn format_overview_minimal() {
        let repo = RepoInfo {
            full_name: "o/r".into(),
            description: None,
            html_url: "https://github.com/o/r".into(),
            default_branch: "main".into(),
            language: None,
            stargazers_count: 0,
            forks_count: 0,
            open_issues_count: 0,
            topics: None,
            license: None,
        };
        let output = format_overview(&repo, None, &[], &[], &[]);
        assert!(output.contains("# o/r"));
        assert!(output.contains("| Stars | 0 |"));
        assert!(!output.contains("## README"));
        assert!(!output.contains("## Recent Issues"));
    }

    #[test]
    fn format_overview_with_metadata() {
        let repo = sample_repo();
        let output = format_overview(&repo, None, &[], &[], &[]);
        assert!(output.contains("| Language | Rust |"));
        assert!(output.contains("| License | MIT |"));
        assert!(output.contains("| Topics | rust, cli |"));
        assert!(output.contains("A test repo"));
    }

    #[test]
    fn format_overview_truncates_long_readme() {
        let repo = sample_repo();
        let total = MAX_README_BYTES + 1_000;
        let long_readme = make_readme_bytes(total);
        let output = format_overview(&repo, Some(&long_readme), &[], &[], &[]);
        assert!(output.contains("## README"));
        let shown_bytes = parse_shown_bytes(&output);
        assert!(shown_bytes > 0 && shown_bytes <= MAX_README_BYTES);
        assert!(output.contains(&format!("/ {total} bytes)")));
    }

    #[test]
    fn format_overview_filters_issues_from_prs() {
        let repo = sample_repo();
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
        let output = format_overview(&repo, None, &issues, &[], &[]);
        assert!(output.contains("Real issue"));
        assert!(!output.contains("PR as issue"));
    }

    #[test]
    fn format_overview_shows_draft_prs() {
        let repo = sample_repo();
        let pulls = vec![PullInfo {
            number: 10,
            title: "WIP feature".into(),
            html_url: "https://github.com/o/r/pull/10".into(),
            draft: Some(true),
            user: Some(UserInfo {
                login: "dev".into(),
            }),
        }];
        let output = format_overview(&repo, None, &[], &pulls, &[]);
        assert!(output.contains("[draft]"));
        assert!(output.contains("@dev"));
    }

    #[test]
    fn format_overview_shows_prerelease() {
        let repo = sample_repo();
        let releases = vec![ReleaseInfo {
            tag_name: "v0.1.0-beta".into(),
            name: Some("Beta".into()),
            html_url: "https://github.com/o/r/releases/tag/v0.1.0-beta".into(),
            published_at: Some("2026-01-15T00:00:00Z".into()),
            prerelease: true,
        }];
        let output = format_overview(&repo, None, &[], &[], &releases);
        assert!(output.contains("(pre-release)"));
        assert!(output.contains("2026-01-15"));
    }

    #[test]
    fn format_overview_shows_issue_labels() {
        let repo = sample_repo();
        let issues = vec![IssueInfo {
            number: 5,
            title: "Bug".into(),
            html_url: "https://github.com/o/r/issues/5".into(),
            labels: vec![
                LabelInfo { name: "bug".into() },
                LabelInfo {
                    name: "urgent".into(),
                },
            ],
            user: Some(UserInfo {
                login: "reporter".into(),
            }),
            pull_request: None,
        }];
        let output = format_overview(&repo, None, &issues, &[], &[]);
        assert!(output.contains(r"\(bug, urgent\)"));
        assert!(output.contains("@reporter"));
    }

    #[test]
    fn format_overview_shifts_readme_headings() {
        let repo = sample_repo();
        let readme = "# Getting Started\n## Install\nRun `cargo install`\n### Config";
        let output = format_overview(&repo, Some(readme), &[], &[], &[]);
        assert!(
            output.contains("### Getting Started"),
            "h1 should shift to h3"
        );
        assert!(output.contains("#### Install"), "h2 should shift to h4");
        assert!(output.contains("##### Config"), "h3 should shift to h5");
    }

    #[test]
    fn format_overview_shifts_headings_in_truncated_readme() {
        let repo = sample_repo();
        let heading = "# Title\n";
        let padding = make_readme_bytes(MAX_README_BYTES + 500 - heading.len());
        let readme = format!("{heading}{padding}");
        let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
        assert!(
            output.contains("### Title"),
            "h1 should shift to h3 even when truncated"
        );
        assert!(output.contains("(truncated: showing"));
    }

    #[test]
    fn format_overview_escapes_description_with_pipe() {
        let mut repo = sample_repo();
        repo.description = Some("col1 | col2".into());
        let output = format_overview(&repo, None, &[], &[], &[]);
        assert!(
            !output.contains("col1 | col2"),
            "raw pipe should be escaped"
        );
        assert!(output.contains(r"col1 \| col2"));
    }

    #[test]
    fn format_overview_escapes_table_cell_metadata() {
        let mut repo = sample_repo();
        repo.language = Some("Rust | Go".into());
        let output = format_overview(&repo, None, &[], &[], &[]);
        assert!(output.contains(r"Rust \| Go"));
    }

    #[test]
    fn format_overview_escapes_default_branch() {
        let mut repo = sample_repo();
        repo.default_branch = "feat|injection".into();
        let output = format_overview(&repo, None, &[], &[], &[]);
        assert!(output.contains(r"feat\|injection"));
    }

    #[test]
    fn format_overview_escapes_issue_title() {
        let repo = sample_repo();
        let issues = vec![IssueInfo {
            number: 1,
            title: "bug [click](http://evil)".into(),
            html_url: "https://github.com/o/r/issues/1".into(),
            labels: vec![],
            user: None,
            pull_request: None,
        }];
        let output = format_overview(&repo, None, &issues, &[], &[]);
        assert!(!output.contains("[click](http://evil)"));
    }

    #[test]
    fn format_overview_escapes_pr_title() {
        let repo = sample_repo();
        let pulls = vec![PullInfo {
            number: 1,
            title: "feat | [link](http://evil)".into(),
            html_url: "https://github.com/o/r/pull/1".into(),
            draft: None,
            user: None,
        }];
        let output = format_overview(&repo, None, &[], &pulls, &[]);
        assert!(!output.contains("[link](http://evil)"));
    }

    fn make_readme_bytes(n: usize) -> String {
        let mut buf = "x\n".repeat(n);
        buf.truncate(n);
        buf
    }

    fn parse_shown_bytes(output: &str) -> usize {
        let prefix = "(truncated: showing ";
        let pos = output.find(prefix).expect("should have truncation note");
        output[pos + prefix.len()..]
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .expect("shown bytes should be a number")
    }

    #[test]
    fn readme_no_truncation_under_limit() {
        let repo = sample_repo();
        let readme = make_readme_bytes(super::MAX_README_BYTES - 100);

        let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
        assert!(output.contains("## README"));
        assert!(!output.contains("truncated"));
    }

    #[test]
    fn readme_no_truncation_at_exact_limit() {
        let repo = sample_repo();
        let readme = make_readme_bytes(super::MAX_README_BYTES);
        assert_eq!(readme.len(), super::MAX_README_BYTES);

        let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
        assert!(!output.contains("truncated"));
    }

    #[test]
    fn readme_truncation_snaps_to_last_newline() {
        let repo = sample_repo();
        let short_line = "short\n";
        let repeat_count = (super::MAX_README_BYTES - 200) / short_line.len();
        let prefix: String = short_line.repeat(repeat_count);
        let filler_len = super::MAX_README_BYTES + 500 - prefix.len();
        let filler: String = "X".repeat(filler_len);
        let readme = format!("{prefix}{filler}");
        assert!(readme.len() > super::MAX_README_BYTES);

        let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
        assert!(output.contains("truncated"));

        let shown_bytes = parse_shown_bytes(&output);
        assert!(
            shown_bytes <= super::MAX_README_BYTES,
            "shown bytes ({shown_bytes}) should be <= MAX_README_BYTES"
        );
        assert_eq!(
            shown_bytes,
            prefix.len(),
            "should snap to last newline position"
        );
    }

    #[test]
    fn readme_truncation_no_newline_no_panic() {
        let repo = sample_repo();
        let total = super::MAX_README_BYTES + 500;
        let readme = "A".repeat(total);
        assert!(!readme.contains('\n'));

        let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
        assert!(output.contains("truncated"));
        assert!(output.contains(&format!("/ {total} bytes)")));
    }

    #[test]
    fn readme_truncation_multibyte_no_mid_char_cut() {
        let repo = sample_repo();
        let cjk_char = '\u{4E16}'; // '世' = 3 bytes
        let char_count = (super::MAX_README_BYTES / 3) + 100;
        let readme: String = std::iter::repeat_n(cjk_char, char_count).collect();
        assert!(readme.len() > super::MAX_README_BYTES);

        let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
        assert!(output.contains("truncated"));
        let shown_bytes = parse_shown_bytes(&output);
        assert_eq!(shown_bytes % 3, 0, "must land on a char boundary");
    }

    #[test]
    fn readme_truncation_multibyte_with_newlines() {
        // TC-003 fix: exercises both floor_char_boundary AND rfind('\n') on CJK
        let repo = sample_repo();
        let line = "\u{4E16}\u{754C}\u{3053}\n"; // "世界こ\n" = 10 bytes
        let repeat_count = (super::MAX_README_BYTES / line.len()) + 50;
        let readme: String = line.repeat(repeat_count);
        assert!(readme.len() > super::MAX_README_BYTES);

        let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
        assert!(output.contains("truncated"));
        let shown_bytes = parse_shown_bytes(&output);
        assert!(shown_bytes <= super::MAX_README_BYTES);
        assert_eq!(
            shown_bytes % line.len(),
            0,
            "should snap to last newline on char boundary"
        );
    }

    #[test]
    fn readme_truncation_note_not_heading_shifted() {
        // TC-004 fix: ordering invariant — note appended after shift_headings
        let repo = sample_repo();
        let readme = format!("# Title\n{}", "x\n".repeat(super::MAX_README_BYTES));
        let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
        assert!(output.contains("(truncated: showing"));
        assert!(!output.contains("### (truncated"));
        assert!(!output.contains("## (truncated"));
    }
}
