pub fn expand_bilingual(query: &str) -> Vec<String> {
    if contains_japanese(query) {
        let eng = to_english_query(query);
        if eng == query {
            vec![query.to_owned()]
        } else {
            vec![query.to_owned(), eng]
        }
    } else {
        vec![query.to_owned()]
    }
}

fn contains_japanese(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c,
            '\u{3040}'..='\u{309F}' |
            '\u{30A0}'..='\u{30FF}' |
            '\u{4E00}'..='\u{9FFF}' |
            '\u{3400}'..='\u{4DBF}'
        )
    })
}

/// Extracts ASCII tokens (technical terms) from a Japanese query as a best-effort English query.
fn to_english_query(query: &str) -> String {
    let ascii_words: Vec<&str> = query
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.')
        .filter(|w| w.len() >= 2)
        .collect();

    if ascii_words.is_empty() {
        query.to_owned()
    } else {
        ascii_words.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-SB001] Japanese+ASCII query expands to original plus English-only variant
    #[test]
    fn japanese_query_expands_to_two() {
        let queries = expand_bilingual("型安全 TypeScript");
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0], "型安全 TypeScript");
        assert!(queries[1].contains("TypeScript"));
    }

    /// [T-SB002] English-only query is not expanded
    #[test]
    fn english_query_stays_single() {
        let queries = expand_bilingual("React hooks best practices");
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0], "React hooks best practices");
    }

    /// [T-SB003] Pure Japanese query returns a single element
    #[test]
    fn pure_japanese_query_returns_single() {
        let queries = expand_bilingual("型安全とは");
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0], "型安全とは");
    }

    /// [T-SB004] Mixed query extracts ASCII tech terms into English variant
    #[test]
    fn mixed_query_extracts_tech_terms() {
        let queries = expand_bilingual("Rust MCP SDK の使い方");
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0], "Rust MCP SDK の使い方");
        assert!(queries[1].contains("Rust"));
        assert!(queries[1].contains("MCP"));
        assert!(queries[1].contains("SDK"));
    }

    /// [T-SB005] Hiragana characters detected as Japanese
    #[test]
    fn detects_hiragana() {
        assert!(contains_japanese("あいうえお"));
    }

    /// [T-SB006] Katakana characters detected as Japanese
    #[test]
    fn detects_katakana() {
        assert!(contains_japanese("カタカナ"));
    }

    /// [T-SB007] Kanji characters detected as Japanese
    #[test]
    fn detects_kanji() {
        assert!(contains_japanese("漢字"));
    }

    /// [T-SB008] ASCII-only text not detected as Japanese
    #[test]
    fn no_japanese_in_ascii() {
        assert!(!contains_japanese("hello world"));
    }

    /// [T-SB009] Katakana+ASCII query expands to two variants
    #[test]
    fn katakana_mixed_query_expands_to_two() {
        let queries = expand_bilingual("タイプスクリプト TypeScript");
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0], "タイプスクリプト TypeScript");
        assert!(queries[1].contains("TypeScript"));
    }
}
