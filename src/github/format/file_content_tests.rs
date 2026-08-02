use super::*;

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

/// [T-GF028]
#[test]
fn lang_for_path_returns_empty_for_no_extension_and_unknown() {
    // FR-002
    assert_eq!(lang_for_path("Makefile"), "", "no dot in path");
    assert_eq!(lang_for_path("file.xyz"), "", "unknown extension");
}

/// [T-GF029]
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
