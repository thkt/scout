//! Typo correction utilities for the `error.candidates` JSON envelope field (ADR-0010).
//!
//! Uses Optimal String Alignment (OSA) distance — Levenshtein extended with
//! adjacent transpositions, so "REDAME" matches "README" at distance 1.

#[cfg(test)]
fn osa_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    osa_distance_chars(&a, &b)
}

/// Compute the OSA distance between two char slices.
/// Allowed edit operations: insert, delete, substitute, transpose adjacent.
fn osa_distance_chars(a: &[char], b: &[char]) -> usize {
    let m = a.len();
    let n = b.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let mut d = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, val) in d[0].iter_mut().enumerate() {
        *val = j;
    }

    for i in 1..=m {
        for j in 1..=n {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);

            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }

    d[m][n]
}

/// Pick the top-N closest matches by OSA distance, filtered by `max_distance`.
/// Sorted by ascending distance (most similar first).
pub(super) fn closest_matches<'a>(
    target: &str,
    pool: impl IntoIterator<Item = &'a str>,
    max_distance: usize,
    top_n: usize,
) -> Vec<String> {
    let target: Vec<char> = target.chars().collect();
    let mut scored: Vec<(usize, &str)> = pool
        .into_iter()
        .map(|c| {
            let candidate: Vec<char> = c.chars().collect();
            (osa_distance_chars(&target, &candidate), c)
        })
        .filter(|(d, _)| *d <= max_distance)
        .collect();
    scored.sort_by_key(|(d, _)| *d);
    scored
        .into_iter()
        .take(top_n)
        .map(|(_, c)| c.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [T-TY001]
    #[test]
    fn identical_strings_have_distance_zero() {
        assert_eq!(osa_distance("README.md", "README.md"), 0);
        assert_eq!(osa_distance("", ""), 0);
    }

    /// [T-TY002] osa_distance: empty input returns the other's length
    #[test]
    fn empty_input_returns_length() {
        assert_eq!(osa_distance("", "hello"), 5);
        assert_eq!(osa_distance("hello", ""), 5);
    }

    /// [T-TY003]
    #[test]
    fn transposition_counts_as_one() {
        assert_eq!(osa_distance("REDAME", "README"), 1);
        assert_eq!(osa_distance("ab", "ba"), 1);
    }

    /// [T-TY004]
    #[test]
    fn substitution_counts_as_one() {
        assert_eq!(osa_distance("kitten", "sitten"), 1);
    }

    /// [T-TY005] osa_distance: classic kitten/sitting case is 3
    #[test]
    fn kitten_sitting_distance_three() {
        assert_eq!(osa_distance("kitten", "sitting"), 3);
    }

    /// [T-TY006] closest_matches: returns top-N filtered by max_distance
    #[test]
    fn closest_matches_filters_by_distance() {
        let pool = ["README.md", "Cargo.toml", "src/main.rs", "REDAME.md"];
        let matches = closest_matches("REDME.md", pool.iter().copied(), 3, 3);
        assert!(matches.contains(&"REDAME.md".to_owned()));
        assert!(matches.contains(&"README.md".to_owned()));
        assert!(!matches.contains(&"Cargo.toml".to_owned()));
        assert!(!matches.contains(&"src/main.rs".to_owned()));
    }

    /// [T-TY007]
    #[test]
    fn closest_matches_empty_pool_returns_empty() {
        let pool: Vec<&str> = vec![];
        let matches = closest_matches("REDAME.md", pool, 3, 3);
        assert!(matches.is_empty());
    }

    /// [T-TY008] closest_matches: all-too-far returns empty
    #[test]
    fn closest_matches_all_too_far_returns_empty() {
        let pool = ["totally-different.txt", "completely-unrelated.json"];
        let matches = closest_matches("REDAME.md", pool.iter().copied(), 3, 3);
        assert!(matches.is_empty());
    }

    /// [T-TY009] closest_matches: respects top_n cap even with many in-range
    #[test]
    fn closest_matches_respects_top_n_cap() {
        let pool = ["a", "ab", "abc", "abcd", "abcde"];
        let matches = closest_matches("a", pool.iter().copied(), 5, 2);
        assert_eq!(matches.len(), 2);
    }
}
