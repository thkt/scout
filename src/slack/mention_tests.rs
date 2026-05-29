use super::*;

/// [T-SK015] parse_mentions on plain text returns no spans
#[test]
fn t001_no_mentions_returns_empty() {
    let spans = parse_mentions("hello world");
    assert!(spans.is_empty());
}

/// [T-SK016] parse_mentions captures one span with byte offsets for a single mention
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

/// [T-SK017] parse_mentions extracts user id only from pipe-labeled mention
#[test]
fn t003_pipe_label_extracts_user_id_only() {
    let text = "cc <@U123|alice>";
    let spans = parse_mentions(text);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].user_id, "U123");
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
    // CJK characters are 3 bytes each in UTF-8
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

/// [T-SK021] substitute_mentions replaces known user id with display name
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

/// [T-SK024] substitute_mentions replaces pipe-labeled mention with display name
#[test]
fn t009b_pipe_label_substituted_with_display_name() {
    let cache: HashMap<String, String> = [("U123".into(), "Alice".into())].into_iter().collect();
    let result = substitute_mentions("cc <@U123|alice_handle>", &cache);
    assert_eq!(result, "cc @Alice");
}
