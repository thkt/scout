//! Typo correction utilities for the `error.candidates` JSON envelope field (ADR-0010).
//!
//! Uses Optimal String Alignment (OSA) distance — Levenshtein extended with
//! adjacent transpositions, so "REDAME" matches "README" at distance 1.

use tokio::task::yield_now;

// These limits bound decoding, allocation, sorting and DP work independently.
// Exceeding a limit omits all hints, rather than claiming a global top-N from
// only a prefix. Repo also checks raw tree length before filtering non-blobs.
pub(super) const MAX_POOL_ENTRIES: usize = 4_096;
const MAX_PATH_CHARS: usize = 512;
const MAX_DISTANCE_CELLS: usize = 1_000_000;

async fn osa_distance_chars(a: &[char], b: &[char]) -> usize {
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
        // At most 16 * MAX_PATH_CHARS cells between scheduling points. Dropping
        // this future stops computation; no blocking worker survives timeout.
        if i % 16 == 0 {
            yield_now().await;
        }
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
/// Stable ascending order preserves pool order for equal distances. Hints are
/// best-effort: return empty if a path, pool or total DP work exceeds its cap.
pub(super) async fn closest_matches<'a>(
    target: &str,
    pool: impl IntoIterator<Item = &'a str>,
    max_distance: usize,
    top_n: usize,
) -> Vec<String> {
    let target: Vec<char> = target.chars().take(MAX_PATH_CHARS + 1).collect();
    if target.len() > MAX_PATH_CHARS || top_n == 0 {
        return Vec::new();
    }
    let mut remaining_cells = MAX_DISTANCE_CELLS;
    let mut scored = Vec::new();
    for (index, c) in pool.into_iter().enumerate() {
        if index == MAX_POOL_ENTRIES {
            return Vec::new();
        }
        let candidate: Vec<char> = c.chars().take(MAX_PATH_CHARS + 1).collect();
        if candidate.len() > MAX_PATH_CHARS {
            return Vec::new();
        }
        if target.len().abs_diff(candidate.len()) <= max_distance {
            let cells = target.len() * candidate.len();
            let Some(remaining) = remaining_cells.checked_sub(cells) else {
                return Vec::new();
            };
            remaining_cells = remaining;
            let distance = osa_distance_chars(&target, &candidate).await;
            if distance <= max_distance {
                scored.push((distance, c));
            }
        }
        // Short paths and length-rejected entries must also give timers and
        // shutdown a chance to run, even when the DP never reaches row 16.
        yield_now().await;
    }
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
    use std::iter::repeat_n;
    use tokio::time::{advance, timeout};

    /// [T-TY010] A 400-character pool stops at entry seven and returns no hints.
    #[tokio::test]
    async fn long_path_pool_stops_at_work_limit() {
        let target = "a".repeat(400);
        let mut visited = 0;
        let pool = repeat_n(target.as_str(), 100).inspect(|_| visited += 1);
        let matches = closest_matches(&target, pool, 3, 3).await;
        assert_eq!(visited, 7, "six distances fit; the seventh exceeds the cap");
        assert!(matches.is_empty());
    }

    /// Whether `closest_matches` finds `candidate` for `target` at exactly
    /// `distance`: within that distance, but not within one less.
    async fn matches_at_exactly(target: &str, candidate: &str, distance: usize) -> bool {
        let within =
            |d| async move { !closest_matches(target, [candidate], d, 1).await.is_empty() };
        within(distance).await && (distance == 0 || !within(distance - 1).await)
    }

    /// [T-TY001] Identical strings match at distance 0.
    #[tokio::test]
    async fn identical_strings_have_distance_zero() {
        assert!(matches_at_exactly("README.md", "README.md", 0).await);
        assert!(matches_at_exactly("", "", 0).await);
    }

    /// [T-TY002] An empty string is as far from another as that one is long.
    #[tokio::test]
    async fn empty_input_returns_length() {
        assert!(matches_at_exactly("", "hello", 5).await);
        assert!(matches_at_exactly("hello", "", 5).await);
    }

    /// [T-TY003] Swapping two adjacent characters costs 1.
    #[tokio::test]
    async fn transposition_counts_as_one() {
        assert!(matches_at_exactly("REDAME", "README", 1).await);
        assert!(matches_at_exactly("ab", "ba", 1).await);
    }

    /// [T-TY004] Replacing one character costs 1.
    #[tokio::test]
    async fn substitution_counts_as_one() {
        assert!(matches_at_exactly("kitten", "sitten", 1).await);
    }

    /// [T-TY005] The classic kitten/sitting pair is at distance 3.
    #[tokio::test]
    async fn kitten_sitting_distance_three() {
        assert!(matches_at_exactly("kitten", "sitting", 3).await);
    }

    /// [T-TY006] closest_matches: returns top-N filtered by max_distance
    #[tokio::test]
    async fn closest_matches_filters_by_distance() {
        let pool = ["ab", "abdc", "abcd", "a", "wxyz", "abce"];
        assert_eq!(
            closest_matches("abcd", pool, 3, 3).await,
            ["abcd", "abdc", "abce"]
        );
        assert_eq!(
            closest_matches("abcd", pool, 3, 6).await,
            ["abcd", "abdc", "abce", "ab", "a"]
        );
        assert_eq!(closest_matches("abcd", pool, 3, 2).await, ["abcd", "abdc"]);
    }

    /// [T-TY007]
    #[tokio::test]
    async fn closest_matches_empty_pool_returns_empty() {
        let pool: Vec<&str> = vec![];
        let matches = closest_matches("REDAME.md", pool, 3, 3).await;
        assert!(matches.is_empty());
    }

    /// [T-TY008]
    #[tokio::test]
    async fn closest_matches_all_too_far_returns_empty() {
        let pool = ["totally-different.txt", "completely-unrelated.json"];
        let matches = closest_matches("REDAME.md", pool.iter().copied(), 3, 3).await;
        assert!(matches.is_empty());
    }

    /// [T-TY011] Unicode paths at 512/513 characters and empty-entry pools at
    /// 4,096/4,097 entries check cap boundaries and discard of prefix hints.
    #[tokio::test]
    async fn path_and_pool_limits_omit_hints_only_above_boundary() {
        let path = "猫".repeat(512);
        assert_eq!(
            closest_matches(&path, [&*path], 3, 3).await,
            [path.as_str()]
        );
        let oversized = "猫".repeat(513);
        let mut visited = 0;
        assert!(
            closest_matches(
                &oversized,
                [&*path].into_iter().inspect(|_| visited += 1),
                3,
                3
            )
            .await
            .is_empty()
        );
        assert_eq!(visited, 0);
        assert!(
            closest_matches(&path, [&*path, &*oversized], 3, 3)
                .await
                .is_empty()
        );
        assert_eq!(
            closest_matches("", repeat_n("", 4_096), 3, 3).await,
            ["", "", ""]
        );
        assert!(
            closest_matches("", repeat_n("", 4_097), 3, 3)
                .await
                .is_empty()
        );
        assert!(
            closest_matches("", repeat_n("long", 4_097), 3, 3)
                .await
                .is_empty()
        );
        assert!(closest_matches("abcd", ["abcd"], 3, 0).await.is_empty());
    }

    /// [T-TY012] Matching yields on its first poll and times out after virtual
    /// time advances, with only one pool entry consumed.
    #[tokio::test(start_paused = true)]
    async fn timeout_interrupts_matching_in_progress() {
        use std::task::Poll;
        use std::time::Duration;

        let target = "a".repeat(400);
        let mut visited = 0;
        {
            let pool = repeat_n(target.as_str(), 100).inspect(|_| visited += 1);
            let matching = closest_matches(&target, pool, 3, 3);
            let timed = timeout(Duration::from_secs(5), matching);
            tokio::pin!(timed);
            assert_eq!(futures_util::poll!(timed.as_mut()), Poll::Pending);
            advance(Duration::from_secs(6)).await;
            assert!(timed.await.is_err());
        }
        assert_eq!(visited, 1, "timeout must stop the in-flight pool scan");
    }
}
