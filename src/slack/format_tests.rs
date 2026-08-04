use super::*;

/// [T-SK008]
#[test]
fn format_output_uses_reply_as_primary_when_targeted() {
    let slack_url = parse_slack_url(
        "https://team.slack.com/archives/C123/p1111111111222222?thread_ts=1234567890.123456",
    )
    .expect("URL fixture should parse");
    let reply = ResolvedMessage {
        author: "reply-author".into(),
        text: "reply body".into(),
        ts: "1111111111.222222".into(),
    };
    let parent = ResolvedMessage {
        author: "parent-author".into(),
        text: "parent body".into(),
        ts: "1234567890.123456".into(),
    };
    let output = format_slack_output(&slack_url, "#general", &reply, &[parent]);
    let expected = "\
---
workspace: \"team\"
channel: \"#general\"
author: \"reply-author\"
ts: \"1111111111.222222\"
context_messages: 1
url: \"https://team.slack.com/archives/C123/p1111111111222222?thread_ts=1234567890.123456\"
---

reply body

---

parent-author (1234567890.123456):
parent body
";
    assert_eq!(output, expected);
}

/// [T-SK070] an untrusted message body starting with `---` cannot inject a YAML
/// document boundary into scout's frontmatter output (a naive multi-document YAML
/// reader splits on bare `---`/`...` lines; the body must contribute none).
#[test]
fn body_cannot_inject_yaml_document_marker() {
    let slack_url = parse_slack_url("https://team.slack.com/archives/C123/p1111111111222222")
        .expect("URL fixture should parse");
    let first = ResolvedMessage {
        author: "attacker".into(),
        text: "---\ninjected: pwned\nreal body".into(),
        ts: "1111111111.222222".into(),
    };
    let output = format_slack_output(&slack_url, "#general", &first, &[]);

    let body = output
        .split("---\n\n")
        .nth(1)
        .expect("body follows the frontmatter close delimiter");
    assert!(
        !body.lines().any(|l| l == "---" || l == "..."),
        "untrusted body must not introduce a bare YAML document marker, got:\n{output}"
    );
    assert!(
        body.contains("injected: pwned"),
        "body content is preserved (only the marker line is rewritten):\n{output}"
    );
}

/// [T-SK071] a reply author's untrusted display name (user-settable) cannot inject
/// a YAML document marker into the body either
#[test]
fn reply_author_cannot_inject_yaml_document_marker() {
    let slack_url = parse_slack_url("https://team.slack.com/archives/C123/p1111111111222222")
        .expect("URL fixture should parse");
    let first = ResolvedMessage {
        author: "alice".into(),
        text: "hello".into(),
        ts: "1111111111.222222".into(),
    };
    let reply = ResolvedMessage {
        author: "evil\n---\ninjected: pwned".into(),
        text: "reply".into(),
        ts: "1234567890.123456".into(),
    };
    let output = format_slack_output(&slack_url, "#general", &first, &[reply]);

    // The only bare `---` lines scout emits are structural: the frontmatter open and
    // close, plus one separator per reply. Untrusted content must add none.
    let bare_markers = output
        .lines()
        .filter(|l| *l == "---" || *l == "...")
        .count();
    assert_eq!(
        bare_markers, 3,
        "expected 2 frontmatter delimiters + 1 reply separator and no injected marker, got:\n{output}"
    );
}

fn msg(ts: &str, author: &str) -> ResolvedMessage {
    ResolvedMessage {
        author: author.into(),
        text: format!("text by {author}"),
        ts: ts.into(),
    }
}

/// [T-SK009] extract_target picks the reply matching target ts from a thread
#[test]
fn extract_target_picks_reply_from_thread() {
    let messages = vec![
        msg("1000.000000", "parent"),
        msg("1001.000000", "reply-1"),
        msg("1002.000000", "reply-2"),
    ];
    let (first, rest) = extract_target(messages, "1001.000000").unwrap();
    assert_eq!(first.ts, "1001.000000");
    assert_eq!(rest.len(), 2);
    assert_eq!(rest[0].ts, "1000.000000");
    assert_eq!(rest[1].ts, "1002.000000");
}

/// [T-SK010]
#[test]
fn extract_target_returns_none_when_ts_missing() {
    let messages = vec![msg("1000.000000", "parent"), msg("1001.000000", "reply-1")];
    assert!(extract_target(messages, "9999.999999").is_none());
}

/// [T-SK011]
#[test]
fn extract_target_matches_ts_for_non_thread() {
    let messages = vec![msg("1000.000000", "author")];
    let (first, rest) = extract_target(messages, "1000.000000").unwrap();
    assert_eq!(first.ts, "1000.000000");
    assert!(rest.is_empty());
}

/// [T-SK069]
///
/// `conversations.history` is probed with `latest` as an upper bound, so a
/// deleted or absent ts yields the *previous* message rather than an empty list.
/// Taking index 0 unconditionally rendered that neighbour's author and body under
/// the requested ts, which the frontmatter then asserts as fact.
#[test]
fn extract_target_rejects_a_neighbour_returned_for_a_missing_ts() {
    let messages = vec![msg("1000.000000", "earlier-author")];
    assert!(
        extract_target(messages, "1500.000000").is_none(),
        "a message with a different ts is not the requested one"
    );
}
