use super::*;
use crate::github::types::{EntryType, LabelInfo, LicenseInfo, UserInfo};
use std::iter;

/// [T-GF001] format_size returns byte suffix for values under 1 KiB
#[test]
fn format_size_bytes() {
    assert_eq!(format_size(500), "500 B");
}

/// [T-GF002] format_size returns KB suffix with one decimal place
#[test]
fn format_size_kilobytes() {
    assert_eq!(format_size(1536), "1.5 KB");
}

/// [T-GF003] format_size returns MB suffix with one decimal place
#[test]
fn format_size_megabytes() {
    assert_eq!(format_size(2_621_440), "2.5 MB");
}

/// [T-GF004] format_tree renders owner/repo header and entry sizes
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

/// [T-GF005] format_tree emits truncation notice when GitHub marks tree truncated
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

/// [T-GF006] format_overview omits README and Issues sections when inputs are empty
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

/// [T-GF007] format_overview renders language, license, topics, and description rows
#[test]
fn format_overview_with_metadata() {
    let repo = sample_repo();
    let output = format_overview(&repo, None, &[], &[], &[]);
    assert!(output.contains("| Language | Rust |"));
    assert!(output.contains("| License | MIT |"));
    assert!(output.contains("| Topics | rust, cli |"));
    assert!(output.contains("A test repo"));
}

/// [T-GF008] format_overview truncates README larger than MAX_README_BYTES
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

/// [T-GF009] format_overview filters PR-backed issues out of the Recent Issues section
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

/// [T-GF010] format_overview marks draft PRs and shows author handle
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

/// [T-GF011] format_overview annotates prerelease flag and publish date
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

/// [T-GF012] format_overview renders issue labels list and reporter handle
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

/// [T-GF013] format_overview shifts README headings by two levels (h1 to h3)
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

/// [T-GF014] format_overview shifts headings even when README is truncated
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

/// [T-GF015] format_overview escapes pipe characters in the description
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

/// [T-GF016] format_overview escapes pipes inside metadata table cells
#[test]
fn format_overview_escapes_table_cell_metadata() {
    let mut repo = sample_repo();
    repo.language = Some("Rust | Go".into());
    let output = format_overview(&repo, None, &[], &[], &[]);
    assert!(output.contains(r"Rust \| Go"));
}

/// [T-GF017] format_overview escapes pipes in the default branch name
#[test]
fn format_overview_escapes_default_branch() {
    let mut repo = sample_repo();
    repo.default_branch = "feat|injection".into();
    let output = format_overview(&repo, None, &[], &[], &[]);
    assert!(output.contains(r"feat\|injection"));
}

/// [T-GF018] format_overview escapes markdown link syntax in issue titles
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

/// [T-GF019] format_overview escapes markdown link syntax in PR titles
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

/// [T-GF020] README passes through intact when below MAX_README_BYTES
#[test]
fn readme_no_truncation_under_limit() {
    let repo = sample_repo();
    let readme = make_readme_bytes(super::MAX_README_BYTES - 100);

    let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
    assert!(output.contains("## README"));
    assert!(!output.contains("truncated"));
}

/// [T-GF021] README passes through intact at exactly MAX_README_BYTES
#[test]
fn readme_no_truncation_at_exact_limit() {
    let repo = sample_repo();
    let readme = make_readme_bytes(super::MAX_README_BYTES);
    assert_eq!(readme.len(), super::MAX_README_BYTES);

    let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
    assert!(!output.contains("truncated"));
}

/// [T-GF022] README truncation snaps back to the last newline before the limit
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

/// [T-GF023] README truncation does not panic when no newline exists
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

/// [T-GF024] README truncation lands on a character boundary for multibyte content
#[test]
fn readme_truncation_multibyte_no_mid_char_cut() {
    let repo = sample_repo();
    let cjk_char = '\u{4E16}'; // '世' = 3 bytes
    let char_count = (super::MAX_README_BYTES / 3) + 100;
    let readme: String = iter::repeat_n(cjk_char, char_count).collect();
    assert!(readme.len() > super::MAX_README_BYTES);

    let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
    assert!(output.contains("truncated"));
    let shown_bytes = parse_shown_bytes(&output);
    assert_eq!(shown_bytes % 3, 0, "must land on a char boundary");
}

/// [T-GF025] README truncation snaps to newline on multibyte character boundary
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

// ── format_file_content / lang_for_path / fence_delimiter ──

/// [T-GF026] format_file_content wraps a Rust file in a ```rust fenced block
#[test]
fn format_file_content_wraps_rust_file_in_fenced_code_block() {
    // FR-001, FR-002
    let output = format_file_content("src/main.rs", 3, "    1\tfn main() {}\n", None);
    assert!(
        output.starts_with("src/main.rs (3 lines)\n\n```rust\n"),
        "should start with path header and ```rust fence, got:\n{output}"
    );
    assert!(
        output.ends_with("\n```"),
        "should end with closing fence, got:\n{output}"
    );
}

/// [T-GF027] lang_for_path returns the canonical identifier for known extensions
#[test]
fn lang_for_path_returns_canonical_identifier_for_known_extensions() {
    // FR-002
    let cases: &[(&str, &str)] = &[
        ("main.rs", "rust"),
        ("app.ts", "typescript"),
        ("app.tsx", "tsx"),
        ("app.js", "javascript"),
        ("app.jsx", "jsx"),
        ("app.py", "python"),
        ("app.rb", "ruby"),
        ("app.go", "go"),
        ("App.java", "java"),
        ("main.c", "c"),
        ("main.h", "c"),
        ("main.cpp", "cpp"),
        ("main.hpp", "cpp"),
        ("README.md", "markdown"),
        ("data.json", "json"),
        ("config.yaml", "yaml"),
        ("config.yml", "yaml"),
        ("config.toml", "toml"),
        ("run.sh", "bash"),
        ("run.bash", "bash"),
        ("query.sql", "sql"),
        ("index.html", "html"),
        ("style.css", "css"),
    ];
    for &(path, expected) in cases {
        assert_eq!(
            lang_for_path(path),
            expected,
            "lang_for_path({path:?}) should return {expected:?}"
        );
    }
}

/// [T-GF028] lang_for_path returns empty string for missing or unknown extensions
#[test]
fn lang_for_path_returns_empty_for_no_extension_and_unknown() {
    // FR-002
    assert_eq!(lang_for_path("Makefile"), "", "no dot in path");
    assert_eq!(lang_for_path("file.xyz"), "", "unknown extension");
}

/// [T-GF029] fence_delimiter grows past three backticks when content contains a triple-backtick run
#[test]
fn fence_delimiter_returns_longer_fence_when_content_has_triple_backticks() {
    // FR-003
    let content = "some\n```\ncode\n```\n";
    let delim = fence_delimiter(content);
    assert!(
        delim.len() >= 4,
        "delimiter should be >= 4 when content has 3 backticks, got len={}",
        delim.len()
    );
    assert!(
        delim.chars().all(|c| c == '`'),
        "delimiter should consist only of backticks"
    );
}

/// [T-GF030] fence_delimiter defaults to triple backticks for content without backticks
#[test]
fn fence_delimiter_returns_triple_backtick_for_plain_content() {
    // FR-003, FR-004
    assert_eq!(fence_delimiter("hello world"), "```");
}

/// [T-GF031] fence_delimiter grows past five backticks when content has a five-backtick run
#[test]
fn fence_delimiter_returns_longer_fence_when_content_has_five_backticks() {
    // FR-003
    let content = "text `````more";
    let delim = fence_delimiter(content);
    assert!(
        delim.len() >= 6,
        "delimiter should be >= 6 when content has 5 backticks, got len={}",
        delim.len()
    );
    assert!(
        delim.chars().all(|c| c == '`'),
        "delimiter should consist only of backticks"
    );
}

/// [T-GF032] format_file_content picks a fence longer than any inner backtick run
#[test]
fn format_file_content_fence_does_not_collide_with_inner_backticks() {
    // FR-001, FR-003
    let inner = "    1\t```\n    2\tsome code\n    3\t```\n";
    let output = format_file_content("doc.md", 3, inner, None);

    let lines: Vec<&str> = output.lines().collect();
    // Structure: line 0 = header, line 1 = blank, line 2 = opening fence, ..., last = closing fence
    let opening_fence_line = lines[2];
    let fence_backticks: String = opening_fence_line
        .chars()
        .take_while(|&c| c == '`')
        .collect();

    // The opening fence must not appear as a standalone line within the content body
    let content_lines = &lines[3..lines.len() - 1];
    for line in content_lines {
        assert_ne!(
            *line, &*fence_backticks,
            "opening fence delimiter should not appear as standalone line in content"
        );
    }
}

/// [T-GF033] format_file_content emits a bare triple-backtick fence for paths without extensions
#[test]
fn format_file_content_uses_empty_lang_for_extensionless_path() {
    // FR-001, FR-002
    let output = format_file_content("config", 1, "    1\tkey=val", None);
    let lines: Vec<&str> = output.lines().collect();
    // line 2 is the opening fence
    assert_eq!(
        lines[2], "```",
        "fence line should be exactly ``` with no language suffix"
    );
}

/// [T-GF034] format_file_content appends the encoding label to the header when provided
#[test]
fn format_file_content_includes_encoding_label_in_header() {
    // FR-009: encoding label appended to header when provided
    let output = format_file_content("file.txt", 2, "    1\thello\n", Some("shift_jis"));
    assert!(
        output.starts_with("file.txt (2 lines) [encoding: shift_jis]\n\n"),
        "header should include encoding label, got:\n{output}"
    );
}

/// [T-GF035] format_file_content omits the encoding label when none is given
#[test]
fn format_file_content_omits_encoding_when_none() {
    // FR-009: no encoding label when None
    let output = format_file_content("file.txt", 1, "    1\thello\n", None);
    assert!(
        output.starts_with("file.txt (1 lines)\n\n"),
        "header should omit encoding when None, got:\n{output}"
    );
    assert!(
        !output.contains("[encoding"),
        "should not contain encoding label"
    );
}

/// [T-GF036] README truncation note is appended after heading shift so it is not rewritten
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
