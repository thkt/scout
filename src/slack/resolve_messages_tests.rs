use super::*;
use tracing_test::traced_test;

fn make_msg(user: Option<&str>, text: &str, ts: Option<&str>) -> Message {
    Message {
        user: user.map(String::from),
        text: text.into(),
        ts: ts.map(String::from),
        reply_count: None,
    }
}

#[traced_test]
/// [T-SK026] resolve_messages logs debug and falls back to "(no author)" when user is None
#[test]
fn t012_user_none_emits_debug_and_falls_back_to_no_author() {
    let messages = vec![make_msg(None, "hello", Some("1000.000"))];
    let users = HashMap::new();

    let resolved = resolve_messages(&messages, &users);

    assert_eq!(resolved[0].author, "(no author)");
    assert!(logs_contain("msg.user is None"));
    assert!(logs_contain("DEBUG"));
}

#[traced_test]
/// [T-SK027] resolve_messages logs warn and falls back to empty ts when missing
#[test]
fn t013_ts_none_emits_warn_and_falls_back_to_empty() {
    let messages = vec![make_msg(Some("U1"), "hi", None)];
    let users = HashMap::from([("U1".into(), "Alice".into())]);

    let resolved = resolve_messages(&messages, &users);

    assert_eq!(resolved[0].ts, "");
    assert!(logs_contain("msg.ts is None"));
    assert!(logs_contain("WARN"));
}

#[traced_test]
/// [T-SK028] resolve_messages resolves both author and mention via user map
#[test]
fn t014_mention_resolved_and_user_mapped() {
    let messages = vec![make_msg(Some("U1"), "cc <@U2>", Some("1000.000"))];
    let users = HashMap::from([("U1".into(), "Alice".into()), ("U2".into(), "Bob".into())]);

    let resolved = resolve_messages(&messages, &users);

    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].author, "Alice");
    assert_eq!(resolved[0].text, "cc @Bob");
    assert_eq!(resolved[0].ts, "1000.000");
}

/// [T-SK029] resolve_messages keeps unknown user id as the author label
#[test]
fn t015_unknown_user_id_kept_as_author() {
    let messages = vec![make_msg(Some("UXXX"), "text", Some("1000.000"))];
    let users = HashMap::new();

    let resolved = resolve_messages(&messages, &users);

    assert_eq!(resolved[0].author, "UXXX");
}
