use super::*;
use crate::github::types::{LabelInfo, LicenseInfo, UserInfo};
use std::iter;

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
        topics: vec!["rust".into(), "cli".into()],
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
        topics: vec![],
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

/// [T-GF023]
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

/// [T-GF024]
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

/// [T-GF036] README truncation note is appended after heading shift so it is not rewritten
#[test]
fn readme_truncation_note_not_heading_shifted() {
    let repo = sample_repo();
    let readme = format!("# Title\n{}", "x\n".repeat(super::MAX_README_BYTES));
    let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
    assert!(output.contains("(truncated: showing"));
    assert!(!output.contains("### (truncated"));
    assert!(!output.contains("## (truncated"));
}

/// [T-GF044] フェンス外の行頭 `---` を持つ README が repo-overview 出力で `***` になる
#[test]
fn readme_outside_fence_dashes_become_asterisks() {
    let repo = sample_repo();
    let readme = "Line one.\n\n---\n\nLine two.\n";
    let output = format_overview(&repo, Some(readme), &[], &[], &[]);
    assert!(
        output.contains("\n***\n"),
        "column-0 --- outside a fence should be neutralized to ***"
    );
    assert!(
        !output.contains("\n---\n"),
        "raw --- marker should not survive unmodified in the output"
    );
}

/// [T-GF045] 24,000 バイトを超えて打ち切られた README でもフェンス外のマーカーが `***` になる
#[test]
fn readme_truncated_outside_fence_dashes_become_asterisks() {
    let repo = sample_repo();
    let padding = make_readme_bytes(super::MAX_README_BYTES + 1_000);
    let readme = format!("---\n\n{padding}");
    let output = format_overview(&repo, Some(&readme), &[], &[], &[]);
    assert!(output.contains("(truncated: showing"));
    assert!(
        output.contains("\n***\n"),
        "column-0 --- should be neutralized to *** even when the README is truncated"
    );
    assert!(
        !output.contains("\n---\n"),
        "raw --- marker should not survive unmodified in truncated output"
    );
}

/// [T-GF046] 閉じないフェンスを含む README ではフェンス以降のマーカーも `***` になる
#[test]
fn readme_unclosed_fence_marker_becomes_asterisks() {
    let repo = sample_repo();
    let readme = "```\nfence opens here\n---\n";
    let output = format_overview(&repo, Some(readme), &[], &[], &[]);
    assert!(
        output.contains("\n***\n"),
        "a marker after an unclosed fence should still be neutralized to ***"
    );
    assert!(
        !output.contains("\n---\n"),
        "raw --- marker should not survive unmodified when its fence never closes"
    );
}

/// [T-GF047] 閉じたフェンスの内側にある行頭 `---` は原文のまま残る
#[test]
fn readme_closed_fence_dashes_stay_verbatim() {
    let repo = sample_repo();
    let readme = "```\n---\n```\n";
    let output = format_overview(&repo, Some(readme), &[], &[], &[]);
    assert!(
        output.contains("```\n---\n```"),
        "a --- inside a closed fence is source code, not a YAML marker, and should not be rewritten"
    );
}
