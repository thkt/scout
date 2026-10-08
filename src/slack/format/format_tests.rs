use super::*;
use crate::slack::parse_slack_url;

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

/// [T-SK088] Fenced body marker is rewritten to `***` even inside a closed fence
///
/// `format_slack_output` neutralizes the body through
/// [`crate::yaml::neutralize_yaml_markers`], not the fence-aware
/// `neutralize_yaml_markers_outside_fences` (see that function's doc comment):
/// a `---` line stays a rewrite target even when it sits inside a closed
/// ```` ``` ```` fence in the message text.
#[test]
fn fenced_body_marker_is_rewritten_even_inside_closed_fence() {
    let slack_url = parse_slack_url("https://team.slack.com/archives/C123/p1111111111222222")
        .expect("URL fixture should parse");
    let first = ResolvedMessage {
        author: "author".into(),
        text: "before\n```\n---\n```\nafter".into(),
        ts: "1111111111.222222".into(),
    };
    let output = format_slack_output(&slack_url, "#general", &first, &[]);

    let body = output
        .split("---\n\n")
        .nth(1)
        .expect("body follows the frontmatter close delimiter");
    assert_eq!(body, "before\n```\n***\n```\nafter\n");
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
/// Taking index 0 unconditionally renders that neighbour's author and body under
/// the requested ts, which the frontmatter then asserts as fact.
#[test]
fn extract_target_rejects_a_neighbour_returned_for_a_missing_ts() {
    let messages = vec![msg("1000.000000", "earlier-author")];
    assert!(
        extract_target(messages, "1500.000000").is_none(),
        "a message with a different ts is not the requested one"
    );
}

/// [T-SK091] Message fences cannot absorb reply labels or subsequent bodies.
#[test]
fn message_fences_preserve_reply_boundaries() {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    let url = parse_slack_url("https://team.slack.com/archives/C123/p1111111111222222")
        .expect("URL fixture should parse");
    for text in [
        "```rust\nlet x = 1;",
        "~~~~\nlet x = 1;\n~~~",
        "`````rust\nlet x = 1;\n```",
        "```rust\nlet x = 1;\n```",
        "~~~rust\nlet x = 1;\n~~~",
        "```rust\nlet x = 1;\n\tdata\t\n```\t",
        "```rust\nlet x = 1;\r```\t",
        "~~~rust\rlet x = 1;\r---\r...\r~~~\t\rafter close",
        "```rust\r\nlet x = 1;\r\n```\t\r\nafter close",
        "````rust\nlet x = 1;\n```\t\n````\t\nafter close\t",
        "~~~~rust\nlet x = 1;\n\tdata\t\n ~~~~~\t \t\nafter close",
    ] {
        for author in ["reply-author", "```", "~~~", "evil\n```\n---"] {
            let first = ResolvedMessage {
                author: "parent".into(),
                text: text.into(),
                ts: "1111111111.222222".into(),
            };
            let replies = [
                ResolvedMessage {
                    author: author.into(),
                    text: "reply body\n~~~\nreply code".into(),
                    ts: "2.000001".into(),
                },
                ResolvedMessage {
                    author: "last-author".into(),
                    text: "last body".into(),
                    ts: "3.000001".into(),
                },
            ];
            let output = format_slack_output(&url, "#general", &first, &replies);
            let body = output.split_once("---\n\n").unwrap().1;
            let mut in_code = false;
            let mut prose = String::new();
            let mut code = String::new();
            for event in Parser::new(body) {
                match event {
                    Event::Start(Tag::CodeBlock(_)) => in_code = true,
                    Event::End(TagEnd::CodeBlock) => in_code = false,
                    Event::Text(text) if in_code => code.push_str(&text),
                    Event::Text(text) => prose.push_str(&text),
                    _ => {}
                }
            }
            assert!(
                prose.contains("reply body"),
                "reply body must be outside code: {author:?}, {text:?}"
            );
            assert!(prose.contains("last-author (3.000001):") && prose.contains("last body"));
            assert!(
                prose.contains("(2.000001):"),
                "timestamp must remain outside code"
            );
            let expected_author = if author == "evil\n```\n---" {
                "evil ``` ***"
            } else {
                author
            };
            assert!(
                prose.contains(expected_author),
                "display name must remain visible"
            );
            assert!(code.contains("let x = 1;"));
            if text.contains("data") {
                assert!(code.contains("\tdata\t\n"), "code tabs must be preserved");
                assert!(!code.contains("```\t") && !code.contains("~~~~~\t"));
            }
            if text.starts_with("````rust\n") {
                assert!(
                    code.contains("```\t\n"),
                    "a short decoy close must retain its tab"
                );
                assert!(
                    body.contains("after close\t"),
                    "prose tabs must be preserved"
                );
            }
            if text.contains("after close") {
                assert!(
                    prose.contains("after close"),
                    "prose after a close must stay prose"
                );
            }
            assert!(
                !body.lines().any(|line| line == "..."),
                "normalization must not expose YAML end markers"
            );
            if text.contains("\r---\r") {
                assert!(
                    code.contains("***"),
                    "CR-delimited YAML markers must be neutralized"
                );
                assert!(!code.contains("---"));
            }
            assert!(!code.contains("reply body") && !code.contains("last-author"));
            if text.ends_with("```") || text.ends_with("~~~rust\nlet x = 1;\n~~~") {
                assert!(body.starts_with(text), "closed source must be preserved");
            }
        }
    }
}

/// [T-SK090] Truncation closes code before the note within the source byte cap.
#[test]
fn truncated_slack_fences_keep_note_outside_code_and_bound_closure() {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    let closed = "```rust\nlet x = 1;\n```\n";
    assert!(matches!(
        truncate_slack_output(closed, closed.len()),
        Cow::Borrowed(_)
    ));
    assert_eq!(truncate_slack_output(closed, closed.len()), closed);
    for (marker, width) in [('`', 3), ('~', 5), ('`', 90_000), ('~', 100_001)] {
        let fence = marker.to_string().repeat(width);
        for source in [
            format!("metadata\n\n{fence}\n{}\n{fence}\n", "éé\n".repeat(32_000)),
            format!(
                "metadata\n\n{fence}\n\tlet x = 1;\t\n{fence}\t \t\n{}\n",
                "outside ".repeat(20_000)
            ),
            format!(
                "metadata\n\n{fence}\n\tlet x = 1;\t\r{fence}\t\n{}\n",
                "outside ".repeat(20_000)
            ),
        ] {
            let output = truncate_slack_output(&source, 100_000);
            let (retained, note) = output.split_once("\n\n(truncated: showing").unwrap();
            assert!(
                retained.len() <= 100_000,
                "closure must fit in the original byte cap"
            );
            assert!(note.contains(&format!(" / {} bytes)", source.len())));
            let mut in_code = false;
            let mut note_in_prose = false;
            let mut code = String::new();
            for event in Parser::new(&output) {
                match event {
                    Event::Start(Tag::CodeBlock(_)) => in_code = true,
                    Event::End(TagEnd::CodeBlock) => in_code = false,
                    Event::Text(text) if text.contains("(truncated:") => {
                        assert!(!in_code, "truncation note must not become code");
                        note_in_prose = true;
                    }
                    Event::Text(text) if in_code => code.push_str(&text),
                    _ => {}
                }
            }
            assert!(note_in_prose);
            if width < 10 && source.contains("let x = 1;") {
                assert_eq!(
                    code, "\tlet x = 1;\t\n",
                    "only source code belongs in the block"
                );
            }
            if width < 10 {
                assert!(
                    retained.contains("éé") || retained.contains("let x = 1;"),
                    "retain primary-source code when closure fits"
                );
            } else {
                assert!(
                    !retained.contains(&fence),
                    "drop an opener whose closure cannot fit"
                );
            }
        }
    }
}
