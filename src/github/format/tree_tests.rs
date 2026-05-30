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
