use super::*;
use crate::github::types::EntryType;

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

/// [T-GF042] A path carrying link syntax stays byte-identical inside the fence
///
/// GitHub admits `[`, `]`, `(`, `)` in a filename, so an unfenced list would
/// render `docs/[draft](old).md` as a link. The fix is the fence, not
/// `escape_md_inline`: the agent feeds these paths straight back to `repo-read`,
/// where a `\[` would 404. Both halves are asserted — the path is untouched, and
/// it sits inside a fence.
#[test]
fn format_tree_keeps_link_syntax_in_paths_verbatim() {
    let entries = [TreeEntry {
        path: "docs/[draft](old).md".into(),
        entry_type: EntryType::Blob,
        size: None,
    }];
    let refs: Vec<&TreeEntry> = entries.iter().collect();
    let output = format_tree("owner", "repo", "main", &refs, false);

    assert!(
        output.contains("docs/[draft](old).md"),
        "path must survive byte-for-byte, got: {output}"
    );
    assert!(
        !output.contains(r"\["),
        "path must not be escaped — it is passed back to repo-read: {output}"
    );
    let after_header = output.split("\n\n").nth(1).expect("body after header");
    assert!(
        after_header.starts_with("```"),
        "paths must open a fence, got: {after_header}"
    );
}

/// [T-GF043] A path containing a backtick run gets a longer fence
///
/// `fence_delimiter` already does this for file contents; the tree list needs it
/// for the same reason — a path may not close the block it sits in.
#[test]
fn format_tree_fence_outgrows_backticks_in_a_path() {
    let entries = [TreeEntry {
        path: "weird/```name.md".into(),
        entry_type: EntryType::Blob,
        size: None,
    }];
    let refs: Vec<&TreeEntry> = entries.iter().collect();
    let output = format_tree("owner", "repo", "main", &refs, false);

    assert!(
        output.contains("````\nweird/```name.md"),
        "fence must be longer than the run inside it, got: {output}"
    );
}
