use std::time::{SystemTime, UNIX_EPOCH};

const ANGLES: &[&str] = &[
    "latest developments {year}",
    "benchmarks comparison",
    "case studies real world",
    "challenges limitations",
];

pub(crate) fn expand_topical(query: &str, breadth: u8) -> Vec<String> {
    if breadth <= 1 {
        return vec![query.to_string()];
    }

    let skip_temporal = contains_year(query);
    let year = current_year().to_string();

    let mut queries = Vec::with_capacity(breadth as usize);
    queries.push(query.to_string());

    for angle in ANGLES {
        if queries.len() >= breadth as usize {
            break;
        }
        if skip_temporal && angle.contains("{year}") {
            continue;
        }
        let expanded = angle.replace("{year}", &year);
        queries.push(format!("{query} {expanded}"));
    }

    queries
}

fn contains_year(query: &str) -> bool {
    query.as_bytes().windows(4).any(|w| {
        w.iter().all(|b| b.is_ascii_digit())
            && ((w[0] == b'1' && w[1] == b'9') || (w[0] == b'2' && w[1] == b'0'))
    })
}

fn current_year() -> u16 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    (1970 + secs / 31_557_600) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_001_breadth_3_returns_original_plus_2_angle_variants() {
        let queries = expand_topical("WebAssembly", 3);

        assert_eq!(queries.len(), 3, "breadth=3 should produce 3 queries");
        assert_eq!(queries[0], "WebAssembly", "first query is the original topic");
        for q in &queries[1..] {
            assert_ne!(q, "WebAssembly", "angle variant should differ from original");
            assert!(
                q.contains("WebAssembly"),
                "angle variant should contain the topic: got {q:?}"
            );
        }
    }

    #[test]
    fn t_002_breadth_1_returns_original_only() {
        let queries = expand_topical("WebAssembly", 1);

        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0], "WebAssembly");
    }

    #[test]
    fn t_003_breadth_7_clamped_to_template_count() {
        let queries = expand_topical("WebAssembly", 7);

        assert_eq!(
            queries.len(),
            ANGLES.len() + 1,
            "breadth=7 should use all available templates"
        );
        assert_eq!(queries[0], "WebAssembly");
    }

    fn has_temporal_query(queries: &[String]) -> bool {
        queries
            .iter()
            .any(|q| q.contains("latest") && contains_year(q))
    }

    #[test]
    fn t_004_query_with_year_skips_temporal_template() {
        let queries = expand_topical("COVID-19 origin 2019", 7);

        assert!(
            !has_temporal_query(&queries),
            "temporal template should be absent when query contains a 4-digit year, got: {queries:?}"
        );
        assert_eq!(
            queries.len(),
            ANGLES.len(),
            "temporal skip should reduce count by 1"
        );
    }

    #[test]
    fn t_005_query_without_year_includes_temporal_template() {
        let queries = expand_topical("WebAssembly", 7);

        assert!(
            has_temporal_query(&queries),
            "temporal template should be present when query has no year, got: {queries:?}"
        );
    }

    #[test]
    fn contains_year_matches_20xx() {
        assert!(contains_year("topic 2024"));
        assert!(contains_year("topic 2099"));
    }

    #[test]
    fn contains_year_matches_19xx() {
        assert!(contains_year("topic 1984"));
        assert!(contains_year("topic 1900"));
    }

    #[test]
    fn contains_year_rejects_other_centuries() {
        assert!(!contains_year("topic 3000"));
        assert!(!contains_year("topic 1899"));
        assert!(!contains_year("topic 1234"));
        assert!(!contains_year("topic 2100"));
    }

    #[test]
    fn contains_year_rejects_short_digits() {
        assert!(!contains_year("topic 20"));
        assert!(!contains_year("topic 202"));
    }

    #[test]
    fn contains_year_embedded_in_text() {
        // "COVID-19" has only 2 consecutive digits, not 4
        assert!(!contains_year("COVID-19"));
        // Year embedded without spaces still matches
        assert!(contains_year("v2024beta"));
    }
}
