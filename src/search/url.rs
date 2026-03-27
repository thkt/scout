use std::collections::HashMap;

use url::Url;

use crate::gemini::types::Source;

const TRACKING_PARAMS: &[&str] = &["fbclid", "gclid"];
const TRACKING_PREFIXES: &[&str] = &["utm_"];

pub(crate) fn canonicalize_url(raw: &str) -> String {
    let Ok(mut parsed) = Url::parse(raw) else {
        return raw.to_string();
    };

    parsed.set_fragment(None);

    let mut clean_pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| {
            !TRACKING_PARAMS.contains(&k.as_ref())
                && !TRACKING_PREFIXES.iter().any(|p| k.starts_with(p))
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    clean_pairs.sort_by(|(a, _), (b, _)| a.cmp(b));

    if clean_pairs.is_empty() {
        parsed.set_query(None);
    } else {
        let qs = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&clean_pairs)
            .finish();
        parsed.set_query(Some(&qs));
    }

    let path = parsed.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        parsed.set_path(&path[..path.len() - 1]);
    }

    parsed.to_string()
}

pub(crate) fn select_diverse_sources(sources: Vec<Source>, max_per_domain: usize) -> Vec<Source> {
    let mut domain_counts: HashMap<String, usize> = HashMap::new();
    let mut selected = Vec::new();

    for source in sources {
        let domain = extract_domain(&source.url);
        let count = domain_counts.entry(domain).or_insert(0);
        if *count < max_per_domain {
            *count += 1;
            selected.push(source);
        }
    }

    selected
}

fn extract_domain(raw: &str) -> String {
    Url::parse(raw)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini::types::Source;

    #[test]
    fn t_009_removes_tracking_params_preserves_others() {
        let result = canonicalize_url("https://example.com/page?utm_source=google&id=1");
        assert_eq!(result, "https://example.com/page?id=1");
    }

    #[test]
    fn t_010_removes_hash_fragment() {
        let result = canonicalize_url("https://example.com/page#section");
        assert_eq!(result, "https://example.com/page");
    }

    #[test]
    fn t_011_removes_trailing_slash() {
        let result = canonicalize_url("https://example.com/page/");
        assert_eq!(result, "https://example.com/page");
    }

    #[test]
    fn t_012_lowercases_host_preserves_path_case() {
        let result = canonicalize_url("https://Example.COM/Page");
        assert_eq!(result, "https://example.com/Page");
    }

    #[test]
    fn t_013_unparseable_url_returns_original() {
        let result = canonicalize_url("not-a-url");
        assert_eq!(result, "not-a-url");
    }

    #[test]
    fn t_009b_removes_fbclid_and_gclid() {
        let result =
            canonicalize_url("https://example.com/page?fbclid=abc&gclid=xyz&utm_medium=email&k=v");
        assert_eq!(result, "https://example.com/page?k=v");
    }

    #[test]
    fn t_009c_all_params_tracking_yields_no_query() {
        let result =
            canonicalize_url("https://example.com/page?utm_source=x&utm_medium=y&fbclid=z");
        assert_eq!(result, "https://example.com/page");
    }

    fn source(domain: &str, n: usize) -> Source {
        Source {
            url: format!("https://{domain}/page{n}"),
            title: format!("{domain} page {n}"),
        }
    }

    #[test]
    fn t_014_caps_per_domain_overflow_fills_remaining() {
        let sources = vec![
            source("a.com", 1),
            source("a.com", 2),
            source("a.com", 3),
            source("b.com", 1),
            source("b.com", 2),
        ];

        let result = select_diverse_sources(sources, 2);
        assert_eq!(result.len(), 4);

        let a_count = result.iter().filter(|s| s.url.contains("a.com")).count();
        let b_count = result.iter().filter(|s| s.url.contains("b.com")).count();
        assert_eq!(a_count, 2);
        assert_eq!(b_count, 2);
    }

    #[test]
    fn t_015_overflow_fills_up_to_take_limit() {
        let sources = vec![
            source("a.com", 1),
            source("a.com", 2),
            source("a.com", 3),
            source("b.com", 1),
        ];

        let result = select_diverse_sources(sources, 2);
        assert_eq!(result.len(), 3);

        let a_count = result.iter().filter(|s| s.url.contains("a.com")).count();
        let b_count = result.iter().filter(|s| s.url.contains("b.com")).count();
        assert_eq!(a_count, 2);
        assert_eq!(b_count, 1);
    }

    #[test]
    fn t_016_single_domain_capped_to_max() {
        let sources = vec![source("a.com", 1), source("a.com", 2), source("a.com", 3)];

        let result = select_diverse_sources(sources, 2);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|s| s.url.contains("a.com")));
    }
}
