use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};

/// [T-GHH001] parse_repo accepts slug, HTTPS URL, subpath URL, and .git-suffixed variants
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

/// [T-GHH002] parse_repo rejects empty, incomplete, and unsafe repository strings
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

/// [T-GHH003] validate_ref accepts a normal branch-like ref
#[test]
fn validate_ref_valid() {
    assert!(validate_ref("feature/my-branch").is_ok());
}

/// [T-GHH004] validate_ref rejects refs violating git-check-ref-format rules
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

/// [T-GHH005] validate_path accepts relative paths and embedded ".." inside filenames
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

/// [T-GHH006] validate_path rejects absolute paths and ".." traversal segments
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

/// [T-GHH007] encode_path percent-encodes reserved characters in path segments
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

/// [T-GHH008] encode_path preserves forward slashes and encodes non-ASCII bytes
#[test]
fn encode_path_preserves_slashes_and_encodes_non_ascii() {
    assert_eq!(encode_path("feature/my-branch"), "feature/my-branch");
    let encoded = encode_path("docs/日本語.md");
    assert!(encoded.starts_with("docs/"));
    assert!(!encoded.contains("日本語"));
}

/// [T-GHH009] parse_line_range accepts range, open-ended, and first-N forms
#[test]
fn parse_line_range_valid() {
    assert_eq!(parse_line_range("1-80").unwrap(), (1, Some(80)));
    assert_eq!(parse_line_range("50-").unwrap(), (50, None));
    assert_eq!(parse_line_range("100").unwrap(), (1, Some(100)));
}

/// [T-GHH010] parse_line_range rejects zero start, inverted bounds, and zero count
#[test]
fn parse_line_range_invalid() {
    for input in ["0", "80-1", "0-10"] {
        assert!(parse_line_range(input).is_err(), "should reject: {input}");
    }
}

/// [T-GHH011] apply_line_range selects the inclusive line subset bounded by start and end
#[test]
fn apply_line_range_subset() {
    let result = apply_line_range("line1\nline2\nline3\nline4\nline5", 2, Some(4));
    assert!(result.contains("line2") && result.contains("line4"));
    assert!(!result.contains("line1") && !result.contains("line5"));
}

/// [T-GHH012] apply_line_range extends from start to end of file when end is None
#[test]
fn apply_line_range_open_end() {
    let result = apply_line_range("line1\nline2\nline3", 2, None);
    assert!(result.contains("line2") && !result.contains("line1"));
}

/// [T-GHH013] apply_line_range reports total lines when start exceeds file length
#[test]
fn apply_line_range_beyond_file() {
    assert!(apply_line_range("line1\nline2", 5, None).contains("2 lines"));
}

/// [T-GHH014] decode_content decodes base64 payloads with and without embedded newlines
#[test]
fn decode_content_handles_base64() {
    assert_eq!(
        decode_content(&STANDARD.encode("hello world"), None)
            .unwrap()
            .text,
        "hello world"
    );
    assert_eq!(
        decode_content("aGVs\nbG8g\nd29y\nbGQ=\n", None)
            .unwrap()
            .text,
        "hello world"
    );
}

/// [T-GHH015] decode_content auto-detects Shift_JIS without a hint
#[test]
fn decode_content_decodes_shift_jis_without_hint() {
    // [Phase 1-B] delegate to decode_bytes: chardetng auto-detects Shift_JIS
    // "テスト" in Shift_JIS — would fail with old UTF-8-only decode_content
    let shift_jis_bytes: &[u8] = &[0x83, 0x65, 0x83, 0x58, 0x83, 0x67];
    let result = decode_content(&STANDARD.encode(shift_jis_bytes), None).unwrap();
    assert_eq!(result.text, "テスト");
}

/// [T-GHH016] decode_content honors an explicit EUC-JP encoding hint
#[test]
fn decode_content_decodes_euc_jp_with_hint() {
    // [Phase 1-B] delegate to decode_bytes: explicit encoding hint passed through
    // "日本語" in EUC-JP
    let euc_jp_bytes: &[u8] = &[0xC6, 0xFC, 0xCB, 0xDC, 0xB8, 0xEC];
    let result = decode_content(&STANDARD.encode(euc_jp_bytes), Some("euc-jp")).unwrap();
    assert_eq!(result.text, "日本語");
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

/// [T-GHH017] filter_tree_entries keeps entries matching a directory path prefix
#[test]
fn filter_by_path_prefix() {
    let entries = vec![blob("src/main.rs"), blob("tests/test.rs"), tree("src")];
    let filtered = filter_tree_entries(&entries, Some("src/"), None).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].path, "src/main.rs");
}

/// [T-GHH018] filter_tree_entries treats bare prefixes as directory boundaries
#[test]
fn filter_by_path_prefix_respects_directory_boundary() {
    let entries = vec![
        blob("src/main.rs"),
        blob("src-old/legacy.rs"),
        blob("src2/other.rs"),
    ];
    let filtered = filter_tree_entries(&entries, Some("src"), None).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].path, "src/main.rs");
}

/// [T-GHH019] filter_tree_entries matches an exact file path without prefix overreach
#[test]
fn filter_by_path_exact_file() {
    let entries = vec![
        blob("README.md"),
        blob("README.md.bak"),
        blob("src/main.rs"),
    ];
    let filtered = filter_tree_entries(&entries, Some("README.md"), None).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].path, "README.md");
}

/// [T-GHH020] filter_tree_entries applies glob patterns against entry filenames
#[test]
fn filter_by_glob_pattern() {
    let entries = vec![blob("src/main.rs"), blob("src/lib.ts"), blob("README.md")];
    let filtered = filter_tree_entries(&entries, None, Some("*.rs")).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].path, "src/main.rs");
}

/// [T-GHH023] filter_tree_entries glob matches against full repo-relative path (ADR-0004 Rule 3)
#[test]
fn filter_by_glob_matches_full_path() {
    let entries = vec![
        blob("src/main.rs"),
        blob("tests/integration.rs"),
        blob("src/lib.rs"),
    ];
    let filtered = filter_tree_entries(&entries, None, Some("src/*.rs")).unwrap();
    assert_eq!(filtered.len(), 2);
    let paths: Vec<&str> = filtered.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"src/main.rs"));
    assert!(paths.contains(&"src/lib.rs"));
}

/// [T-GHH021] filter_tree_entries excludes directory tree entries
#[test]
fn filter_excludes_tree_entries() {
    let entries = vec![tree("src"), blob("src/main.rs")];
    let filtered = filter_tree_entries(&entries, None, None).unwrap();
    assert_eq!(filtered.len(), 1);
}

/// [T-GHH022] filter_tree_entries excludes submodule commit entries
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

/// [T-GHH026] extract_error_message pulls the message field from GitHub JSON payloads
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

/// [T-GHH024] is_valid_github_name accepts alphanumerics, hyphens, underscores, and dots
#[test]
fn is_valid_github_name_accepts_normal() {
    for name in ["facebook", "my-repo.js", "repo_name"] {
        assert!(is_valid_github_name(name), "should accept: {name}");
    }
}

/// [T-GHH025] is_valid_github_name rejects empty, "..", and names with special characters
#[test]
fn is_valid_github_name_rejects_special() {
    for name in ["", "..", "repo?q", "repo#frag", "a/b"] {
        assert!(!is_valid_github_name(name), "should reject: {name}");
    }
}
