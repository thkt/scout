//! Slack `<@UID>` mention parsing and display-name substitution.

use std::collections::{HashMap, HashSet};

/// A `<@UID>` or `<@UID|label>` mention span within a text.
struct MentionSpan<'a> {
    user_id: &'a str,
    /// Human-readable label Slack embedded as `<@UID|label>`, `None` when absent
    /// or empty. Used as a best-effort render fallback when the user id is
    /// unresolved; it is the send-time display name and may be stale.
    label: Option<&'a str>,
    /// Byte range covering the entire `<@…>` token.
    start: usize,
    end: usize,
}

fn parse_mentions(text: &str) -> Vec<MentionSpan<'_>> {
    let mut spans = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find("<@") {
        let abs_start = search_from + rel;
        let after = abs_start + 2;
        let Some(rel_end) = text[after..].find('>') else {
            break;
        };
        let abs_end = after + rel_end + 1;
        let inner = &text[after..after + rel_end];
        let (user_id, label) = match inner.split_once('|') {
            Some((id, label)) => (id, Some(label).filter(|l| !l.is_empty())),
            None => (inner, None),
        };
        spans.push(MentionSpan {
            user_id,
            label,
            start: abs_start,
            end: abs_end,
        });
        search_from = abs_end;
    }
    spans
}

/// Append mention IDs from `text` to `out` in first-occurrence order, skipping
/// any already in `seen`. Sharing `seen` across calls (and with an author pass)
/// dedupes a dual-role ID so it consumes a single lookup slot.
pub(in crate::slack) fn collect_mention_ids_ordered(
    text: &str,
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
) {
    for span in parse_mentions(text) {
        if seen.insert(span.user_id.to_owned()) {
            out.push(span.user_id.to_owned());
        }
    }
}

/// Look up a display name for `user_id`, treating an empty value as a failed
/// resolution rather than a name. The Slack users map carries that convention
/// in its values and `HashMap<String, String>` cannot express it, so the
/// substitution below and the author resolution in `format` both read the map
/// through here rather than each re-deriving the rule.
pub(in crate::slack) fn resolved_display_name<'a>(
    users: &'a HashMap<String, String>,
    user_id: &str,
) -> Option<&'a str> {
    users
        .get(user_id)
        .map(String::as_str)
        .filter(|name| !name.is_empty())
}

pub(in crate::slack) fn substitute_mentions(text: &str, cache: &HashMap<String, String>) -> String {
    let spans = parse_mentions(text);
    if spans.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for span in &spans {
        out.push_str(&text[pos..span.start]);
        out.push('@');
        out.push_str(
            resolved_display_name(cache, span.user_id)
                .or(span.label)
                .unwrap_or(span.user_id),
        );
        pos = span.end;
    }
    out.push_str(&text[pos..]);
    out
}

#[cfg(test)]
mod mention_tests;
