use super::*;

/// [T-SK018] Adjacent mentions are each replaced, with nothing lost between them.
#[test]
fn adjacent_mentions_are_each_replaced() {
    let cache: HashMap<String, String> = [
        ("U001".into(), "A".into()),
        ("U002".into(), "B".into()),
        ("U003".into(), "C".into()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        substitute_mentions("<@U001><@U002><@U003>", &cache),
        "@A@B@C"
    );
}

/// [T-SK019] An unclosed mention is left as written after the earlier mentions
/// are replaced.
#[test]
fn unclosed_mention_is_left_as_written() {
    let cache: HashMap<String, String> = [("U001".into(), "A".into())].into_iter().collect();
    assert_eq!(
        substitute_mentions("<@U001> then <@U002", &cache),
        "@A then <@U002"
    );
}

/// [T-SK020] Mentions after and between multibyte characters are replaced
/// without splitting those characters.
#[test]
fn mentions_next_to_multibyte_text_are_replaced() {
    let cache: HashMap<String, String> = [
        ("UCJK".into(), "cjk".into()),
        ("UEMJ".into(), "emoji".into()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        substitute_mentions("\u{3053}\u{3093}\u{306B}\u{3061}\u{306F}<@UCJK>end", &cache),
        "\u{3053}\u{3093}\u{306B}\u{3061}\u{306F}@cjkend"
    );
    assert_eq!(
        substitute_mentions("\u{1F600}<@UEMJ>\u{1F600}", &cache),
        "\u{1F600}@emoji\u{1F600}"
    );
}

/// [T-SK021]
#[test]
fn cached_user_renders_as_display_name() {
    let cache: HashMap<String, String> = [("U100".into(), "Alice".into())].into_iter().collect();
    let result = substitute_mentions("hello <@U100> world", &cache);
    assert_eq!(result, "hello @Alice world");
}

/// [T-SK022] substitute_mentions falls back to @UID when user unknown
#[test]
fn uncached_user_without_label_renders_as_user_id() {
    let cache: HashMap<String, String> = HashMap::new();
    let result = substitute_mentions("hello <@UXXX> world", &cache);
    assert_eq!(result, "hello @UXXX world");
}

/// [T-SK023] substitute_mentions returns text unchanged when no mentions present
#[test]
fn text_without_mentions_is_returned_unchanged() {
    let cache: HashMap<String, String> = [("U100".into(), "Alice".into())].into_iter().collect();
    let text = "no mentions here";
    let result = substitute_mentions(text, &cache);
    assert_eq!(result, text);
}

/// [T-SK024] substitute_mentions prefers the cached display name over the embedded pipe label
#[test]
fn cached_display_name_replaces_embedded_label() {
    let cache: HashMap<String, String> = [("U123".into(), "Alice".into())].into_iter().collect();
    let result = substitute_mentions("cc <@U123|alice_handle>", &cache);
    assert_eq!(result, "cc @Alice");
}

/// [T-SK061] substitute_mentions falls back to embedded label when the user id
/// is absent from the cache
#[test]
fn uncached_user_renders_embedded_label() {
    let cache: HashMap<String, String> = HashMap::new();
    let result = substitute_mentions("cc <@U123|alice>", &cache);
    assert_eq!(result, "cc @alice");
}

/// [T-SK062] substitute_mentions falls back to the raw user id when the cache
/// misses and the embedded label is empty
#[test]
fn uncached_user_with_empty_label_renders_user_id() {
    let cache: HashMap<String, String> = HashMap::new();
    let result = substitute_mentions("x <@U123|>", &cache);
    assert_eq!(result, "x @U123");
}

/// [T-SK063]
#[test]
fn empty_cached_name_falls_back_to_embedded_label() {
    let cache: HashMap<String, String> = [("U123".into(), String::new())].into_iter().collect();
    let result = substitute_mentions("cc <@U123|alice>", &cache);
    assert_eq!(result, "cc @alice");
}

/// [T-SK084] `<@…>` holding something that cannot be a user id is left alone
///
/// Slack ids carry no whitespace and no `<`, so these are some other use of the
/// sequence. Substituting them rewrites text the author did not write as a
/// mention (`<@U1 hi>` becoming `@U1 hi`, losing the brackets) and puts the
/// non-id into the lookup queue, where it consumes one of the 50
/// `SLACK_MAX_USER_LOOKUPS` slots and can cap out the real mentions behind it.
#[test]
fn malformed_mention_tokens_are_left_verbatim() {
    let cache = HashMap::new();
    for text in [
        "hi <@U123 hello> there",
        "hi <@> there",
        "hi <@U123<@U456> there",
    ] {
        assert_eq!(
            substitute_mentions(text, &cache),
            text,
            "must pass through untouched: {text}"
        );
    }
}

/// [T-SK085] a malformed token contributes no id to the lookup queue
#[test]
fn malformed_mention_tokens_are_not_collected() {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    collect_mention_ids_ordered("<@U1 hi> <@> <@REAL>", &mut seen, &mut out);

    assert_eq!(out, vec!["REAL".to_owned()], "only the real id is queued");
}

/// [T-SK086] a well-formed mention after a malformed one still resolves
///
/// The scan resumes past the rejected token rather than abandoning the line.
#[test]
fn mention_after_malformed_token_still_substitutes() {
    let mut cache = HashMap::new();
    cache.insert("U456".to_owned(), "Alice".to_owned());

    assert_eq!(
        substitute_mentions("<@U123 bad> and <@U456>", &cache),
        "<@U123 bad> and @Alice"
    );
}

/// [T-SK089] Unicode whitespace (and ASCII vertical tab) cannot be part of an
/// ID. Preserve malformed text and keep it out of the user lookup queue.
#[test]
fn whitespace_in_mention_id_is_preserved_and_not_looked_up() {
    let whitespace = [
        '\t', '\n', '\u{000b}', '\u{000c}', '\r', ' ', '\u{0085}', '\u{00a0}', '\u{1680}',
        '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}',
        '\u{2007}', '\u{2008}', '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}',
        '\u{205f}', '\u{3000}',
    ];
    let cache = [("U100".to_owned(), "Alice".to_owned())]
        .into_iter()
        .collect();
    for space in whitespace {
        for suffix in ["", "|label"] {
            let malformed = format!("<@U{space}123{suffix}>");
            let text = format!("{malformed} <@U100>");
            assert_eq!(
                substitute_mentions(&text, &cache),
                format!("{malformed} @Alice"),
                "whitespace {space:?} must not become an ID"
            );
            let mut seen = HashSet::new();
            let mut ids = Vec::new();
            collect_mention_ids_ordered(&text, &mut seen, &mut ids);
            assert_eq!(ids, ["U100"], "invalid ID must not consume a lookup");
        }
    }
}
