use super::*;

/// [T-SK015]
#[test]
fn t001_no_mentions_returns_empty() {
    let spans = parse_mentions("hello world");
    assert!(spans.is_empty());
}

/// [T-SK016]
#[test]
fn t002_single_mention_returns_one_span() {
    let text = "hi <@U123> bye";
    let spans = parse_mentions(text);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].user_id, "U123");
    assert_eq!(spans[0].start, 3);
    assert_eq!(spans[0].end, 10);
    assert_eq!(&text[spans[0].start..spans[0].end], "<@U123>");
}

/// [T-SK017]
#[test]
fn t003_pipe_label_extracts_user_id_only() {
    let text = "cc <@U123|alice>";
    let spans = parse_mentions(text);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].user_id, "U123");
}

/// [T-SK058]
#[test]
fn t003b_pipe_label_captured() {
    let spans = parse_mentions("cc <@U123|alice>");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].user_id, "U123");
    assert_eq!(spans[0].label, Some("alice"));
}

/// [T-SK059]
#[test]
fn t003c_bare_mention_has_no_label() {
    let spans = parse_mentions("hi <@U123>");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].label, None);
}

/// [T-SK060]
#[test]
fn t003d_empty_label_normalized_to_none() {
    let spans = parse_mentions("x <@U123|>");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].user_id, "U123");
    assert_eq!(spans[0].label, None);
}

/// [T-SK018] parse_mentions captures consecutive adjacent mentions
#[test]
fn t004_multiple_adjacent_mentions() {
    let text = "<@U001><@U002><@U003>";
    let spans = parse_mentions(text);
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].user_id, "U001");
    assert_eq!(spans[1].user_id, "U002");
    assert_eq!(spans[2].user_id, "U003");
    assert_eq!(spans[0].end, spans[1].start);
    assert_eq!(spans[1].end, spans[2].start);
}

/// [T-SK019] parse_mentions stops at unclosed mention token
#[test]
fn t005_unclosed_mention_breaks_early() {
    let text = "<@U001> then <@U002";
    let spans = parse_mentions(text);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].user_id, "U001");
}

/// [T-SK020] parse_mentions yields correct byte offsets across multibyte characters
#[test]
fn t006_multibyte_characters_correct_offsets() {
    let text = "\u{3053}\u{3093}\u{306B}\u{3061}\u{306F}<@UCJK>end";
    let spans = parse_mentions(text);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].user_id, "UCJK");
    // 5 CJK chars x 3 bytes = 15, so <@UCJK> starts at byte 15
    assert_eq!(spans[0].start, 15);
    assert_eq!(&text[spans[0].start..spans[0].end], "<@UCJK>");

    // Emoji (4-byte) surrounding a mention
    let emoji_text = "\u{1F600}<@UEMJ>\u{1F600}";
    let spans2 = parse_mentions(emoji_text);
    assert_eq!(spans2.len(), 1);
    assert_eq!(spans2[0].user_id, "UEMJ");
    assert_eq!(spans2[0].start, 4);
    assert_eq!(&emoji_text[spans2[0].start..spans2[0].end], "<@UEMJ>");
}

/// [T-SK021]
#[test]
fn t007_known_user_replaced_with_display_name() {
    let cache: HashMap<String, String> = [("U100".into(), "Alice".into())].into_iter().collect();
    let result = substitute_mentions("hello <@U100> world", &cache);
    assert_eq!(result, "hello @Alice world");
}

/// [T-SK022] substitute_mentions falls back to @UID when user unknown
#[test]
fn t008_unknown_user_kept_as_at_uid() {
    let cache: HashMap<String, String> = HashMap::new();
    let result = substitute_mentions("hello <@UXXX> world", &cache);
    assert_eq!(result, "hello @UXXX world");
}

/// [T-SK023] substitute_mentions returns text unchanged when no mentions present
#[test]
fn t009_no_mentions_returns_text_unchanged() {
    let cache: HashMap<String, String> = [("U100".into(), "Alice".into())].into_iter().collect();
    let text = "no mentions here";
    let result = substitute_mentions(text, &cache);
    assert_eq!(result, text);
}

/// [T-SK024] substitute_mentions prefers the cached display name over the embedded pipe label
#[test]
fn t009b_pipe_label_substituted_with_display_name() {
    let cache: HashMap<String, String> = [("U123".into(), "Alice".into())].into_iter().collect();
    let result = substitute_mentions("cc <@U123|alice_handle>", &cache);
    assert_eq!(result, "cc @Alice");
}

/// [T-SK061] substitute_mentions falls back to embedded label when the user id
/// is absent from the cache
#[test]
fn t025_cache_miss_renders_embedded_label() {
    let cache: HashMap<String, String> = HashMap::new();
    let result = substitute_mentions("cc <@U123|alice>", &cache);
    assert_eq!(result, "cc @alice");
}

/// [T-SK062] substitute_mentions falls back to the raw user id when the cache
/// misses and the embedded label is empty
#[test]
fn t026_cache_miss_empty_label_renders_user_id() {
    let cache: HashMap<String, String> = HashMap::new();
    let result = substitute_mentions("x <@U123|>", &cache);
    assert_eq!(result, "x @U123");
}

/// [T-SK063]
#[test]
fn t027_empty_cache_value_falls_through_to_label() {
    let cache: HashMap<String, String> = [("U123".into(), String::new())].into_iter().collect();
    let result = substitute_mentions("cc <@U123|alice>", &cache);
    assert_eq!(result, "cc @alice");
}

/// [T-SK064]
#[test]
fn t028_cache_hit_takes_priority_over_label() {
    let cache: HashMap<String, String> = [("U123".into(), "Bob".into())].into_iter().collect();
    let result = substitute_mentions("cc <@U123|alice>", &cache);
    assert_eq!(result, "cc @Bob");
}

/// [T-SK084] `<@…>` holding something that cannot be a user id is left alone
///
/// Slack ids carry no whitespace and no `<`, so these are some other use of the
/// sequence. Substituting them rewrote text the author did not write as a
/// mention (`<@U1 hi>` became `@U1 hi`, losing the brackets) and put the
/// non-id into the lookup queue, where it consumed one of the 50
/// `SLACK_MAX_USER_LOOKUPS` slots and could cap out the real mentions behind it.
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
