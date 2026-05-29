use super::*;

#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
struct DummyBody {
    ok: bool,
}

mod http_tests {
    use super::*;
    use crate::test_support::{spawn_mid_stream_drop_server, try_spawn_mock_server};
    use reqwest::Client;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    /// [T-SK001] HTTP 429 response maps to SlackError::RateLimited
    #[tokio::test]
    async fn api_get_once_429_returns_rate_limited() {
        let Some(server) = try_spawn_mock_server("slack::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/test.method"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(Client::new(), &server.uri());
        let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
        assert!(matches!(
            result,
            Err(SlackError::RateLimited { retry_after: None })
        ));
    }

    /// [T-SK002] HTTP 429 with Retry-After header preserves header value
    #[tokio::test]
    async fn api_get_once_429_with_retry_after_header() {
        let Some(server) = try_spawn_mock_server("slack::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/test.method"))
            .respond_with(ResponseTemplate::new(429).append_header("Retry-After", "30"))
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(Client::new(), &server.uri());
        let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
        assert!(matches!(
            result,
            Err(SlackError::RateLimited {
                retry_after: Some(30)
            })
        ));
    }

    /// [T-SK003] Body-level ratelimited error maps to SlackError::RateLimited
    #[tokio::test]
    async fn api_get_once_body_ratelimited_returns_rate_limited() {
        let Some(server) = try_spawn_mock_server("slack::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/test.method"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": false, "error": "ratelimited"})),
            )
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(Client::new(), &server.uri());
        let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
        assert!(matches!(result, Err(SlackError::RateLimited { .. })));
    }

    /// [T-SK004] Non-ratelimited Slack API error maps to SlackError::Api
    #[tokio::test]
    async fn api_get_once_api_error_returns_api_variant() {
        let Some(server) = try_spawn_mock_server("slack::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/test.method"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": false, "error": "channel_not_found"})),
            )
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(Client::new(), &server.uri());
        let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
        assert!(matches!(
            result,
            Err(SlackError::Api { error }) if error == "channel_not_found"
        ));
    }

    /// [T-SK031] ok:false without an `error` field surfaces as SlackError::Decode
    /// (issue #114 condition 5). The previous code substituted the literal
    /// "unknown" string and mapped to UsageError; a missing `error` is a
    /// Slack API contract violation, not a user-fixable failure.
    #[tokio::test]
    async fn api_get_once_ok_false_without_error_field_returns_decode() {
        let Some(server) = try_spawn_mock_server("slack::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/test.method"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": false})),
            )
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(Client::new(), &server.uri());
        let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
        assert!(
            matches!(result, Err(SlackError::Decode(_))),
            "expected SlackError::Decode for ok:false without error field, got: {result:?}"
        );
    }

    /// [T-SK032] (issue #165 / CHX-009)
    /// Setup: wiremock returns a 2xx whose body exceeds
    /// `MAX_API_RESPONSE_BYTES` (1 MiB), simulating a runaway Slack
    /// thread/channel response.
    /// Action: `api_get_once::<DummyBody>("test.method", &[])` is invoked.
    /// Expected: returns `SlackError::Decode` (terminal — Slack contract
    /// violation, retry will not recover). Body message contains
    /// "too large" to surface the cap in the user-facing error.
    #[tokio::test]
    async fn api_get_once_oversized_body_returns_decode() {
        let Some(server) = try_spawn_mock_server("slack::http").await else {
            return;
        };
        let body = vec![b'x'; (1024 * 1024) + 1];
        Mock::given(method("GET"))
            .and(path("/test.method"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .expect(1)
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(Client::new(), &server.uri());
        let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
        match result {
            Err(SlackError::Decode(msg)) => assert!(
                msg.contains("too large"),
                "expected size-cap message, got: {msg}"
            ),
            other => panic!("expected SlackError::Decode for oversized body, got: {other:?}"),
        }
    }

    /// [T-SK030] Mid-stream body drop on 2xx routes through SlackError::Network
    /// (transient, retry path) rather than SlackError::Decode (terminal). reqwest
    /// 0.13 reports the drop as `is_decode() == true`; `is_transient_decode`
    /// distinguishes it from a schema fail via the io::Error source chain (issue #113).
    #[tokio::test]
    async fn api_get_once_2xx_mid_stream_drop_returns_network() {
        let Some((url, _counter, handle)) = spawn_mid_stream_drop_server(1) else {
            return;
        };
        let client = SlackClient::with_base_url(Client::new(), &url);
        let result: Result<DummyBody, _> = client.api_get_once("test.method", &[]).await;
        assert!(
            matches!(result, Err(SlackError::Network(_))),
            "expected SlackError::Network for mid-stream drop, got: {result:?}"
        );
        let _ = handle.join();
    }
}

/// [T-SK005] parse_slack_url accepts a standard archive permalink
#[test]
fn parse_standard_url() {
    let url = "https://myteam.slack.com/archives/C0656BJSFL7/p1773819598273499";
    let parsed = parse_slack_url(url).unwrap();
    assert_eq!(parsed.workspace, "myteam");
    assert_eq!(parsed.channel, "C0656BJSFL7");
    assert_eq!(parsed.ts, "1773819598.273499");
    assert!(parsed.thread_ts.is_none());
}

/// [T-SK006] parse_slack_url extracts thread_ts from parent permalink
#[test]
fn parse_parent_permalink_with_thread_ts() {
    let url = "https://team.slack.com/archives/C123/p1234567890123456?thread_ts=1234567890.123456&cid=C123";
    let parsed = parse_slack_url(url).unwrap();
    assert_eq!(parsed.channel, "C123");
    assert_eq!(parsed.ts, "1234567890.123456");
    assert_eq!(parsed.thread_ts.as_deref(), Some("1234567890.123456"));
}

/// [T-SK007] Reply permalink carries distinct ts and thread_ts
#[test]
fn parse_reply_permalink_has_different_ts_and_thread_ts() {
    let url = "https://team.slack.com/archives/C123/p1111111111222222?thread_ts=1234567890.123456&cid=C123";
    let parsed = parse_slack_url(url).unwrap();
    assert_eq!(parsed.ts, "1111111111.222222");
    assert_eq!(parsed.thread_ts.as_deref(), Some("1234567890.123456"));
}

/// [T-SK008] format_slack_output uses targeted reply as primary message
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
    let (first, rest) = extract_target(messages, "1001.000000", true).unwrap();
    assert_eq!(first.ts, "1001.000000");
    assert_eq!(rest.len(), 2);
    assert_eq!(rest[0].ts, "1000.000000");
    assert_eq!(rest[1].ts, "1002.000000");
}

/// [T-SK010] extract_target returns None when target ts not present
#[test]
fn extract_target_returns_none_when_ts_missing() {
    let messages = vec![msg("1000.000000", "parent"), msg("1001.000000", "reply-1")];
    assert!(extract_target(messages, "9999.999999", true).is_none());
}

/// [T-SK011] extract_target ignores ts for non-thread messages and picks first
#[test]
fn extract_target_ignores_ts_for_non_thread() {
    let messages = vec![msg("1000.000000", "author")];
    let (first, rest) = extract_target(messages, "9999.999999", false).unwrap();
    assert_eq!(first.ts, "1000.000000");
    assert!(rest.is_empty());
}

/// [T-SK012] parse_slack_url rejects URLs outside *.slack.com
#[test]
fn parse_rejects_non_slack_url() {
    assert!(parse_slack_url("https://example.com/page").is_none());
}

/// [T-SK013] parse_slack_url rejects paths outside /archives
#[test]
fn parse_rejects_non_archives_path() {
    assert!(parse_slack_url("https://team.slack.com/messages/C123/p111111222222333").is_none());
}

/// [T-SK014] parse_slack_url rejects timestamps shorter than micro-precision
#[test]
fn parse_rejects_short_timestamp() {
    assert!(parse_slack_url("https://team.slack.com/archives/C123/p12345").is_none());
}

mod mention_tests {
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
        let cache: HashMap<String, String> =
            [("U100".into(), "Alice".into())].into_iter().collect();
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
        let cache: HashMap<String, String> =
            [("U100".into(), "Alice".into())].into_iter().collect();
        let text = "no mentions here";
        let result = substitute_mentions(text, &cache);
        assert_eq!(result, text);
    }

    /// [T-SK024] substitute_mentions replaces pipe-labeled mention with display name
    #[test]
    fn t009b_pipe_label_substituted_with_display_name() {
        let cache: HashMap<String, String> =
            [("U123".into(), "Alice".into())].into_iter().collect();
        let result = substitute_mentions("cc <@U123|alice_handle>", &cache);
        assert_eq!(result, "cc @Alice");
    }
}

mod constructor_tests {
    use super::*;
    use crate::test_support::try_spawn_mock_server;
    use reqwest::Client;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    /// [T-SK025] SlackClient::with_base_url constructs a client that reaches a wiremock server
    #[tokio::test]
    async fn t010_with_base_url_constructs_usable_client() {
        let Some(server) = try_spawn_mock_server("slack::http").await else {
            return;
        };
        Mock::given(method("GET"))
            .and(path("/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(Client::new(), &server.uri());

        let result: Result<DummyBody, _> = client.api_get_once("auth.test", &[]).await;
        assert!(result.is_ok());
    }
}

mod resolve_messages_tests {
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
}

mod classify_tests {
    use super::*;

    /// [T-SLC001] TokenNotSet classifies as UsageError with SLACK_TOKEN hint.
    #[test]
    fn token_not_set_is_usage_error_with_token_hint() {
        let c = SlackError::TokenNotSet.classify();
        assert_eq!(c.kind, ErrorCode::UsageError);
        assert!(
            c.next_step
                .as_deref()
                .is_some_and(|h| h.contains("SLACK_TOKEN")),
            "expected SLACK_TOKEN hint, got: {:?}",
            c.next_step
        );
    }

    /// [T-SLC002] InsecureUrl classifies as DataError (peer to other backends' InsecureUrl).
    #[test]
    fn insecure_url_is_data_error() {
        let c = SlackError::InsecureUrl.classify();
        assert_eq!(c.kind, ErrorCode::DataError);
    }

    /// [T-SLC003] Slack-native NOT_FOUND error codes classify as NotFound.
    /// scout's internal "message not found" (space form) must classify the same
    /// as Slack's `message_not_found` (underscore) — both should land on
    /// EX_NOINPUT(66) per issue #114.
    #[test]
    fn api_not_found_codes_classify_as_not_found() {
        for code in [
            "channel_not_found",
            "message_not_found",
            "thread_not_found",
            "message not found",
        ] {
            let c = SlackError::Api {
                error: code.to_owned(),
            }
            .classify();
            assert_eq!(
                c.kind,
                ErrorCode::NotFound,
                "{code} must classify as NotFound"
            );
        }
    }

    /// [T-SLC004] Slack TEMP_FAILURE error codes classify as TempFailure
    /// (ADR-0003 — internal_error must not be misclassified as UsageError).
    #[test]
    fn api_temp_failure_codes_classify_as_temp_failure() {
        for code in ["internal_error", "service_unavailable", "fatal_error"] {
            let c = SlackError::Api {
                error: code.to_owned(),
            }
            .classify();
            assert_eq!(c.kind, ErrorCode::TempFailure, "{code}");
        }
    }

    /// [T-SLC005] Other Slack API error codes (e.g., invalid_auth) classify as UsageError.
    #[test]
    fn api_other_codes_classify_as_usage_error() {
        for code in ["invalid_auth", "missing_scope", "not_authed"] {
            let c = SlackError::Api {
                error: code.to_owned(),
            }
            .classify();
            assert_eq!(c.kind, ErrorCode::UsageError, "{code}");
        }
    }

    /// [T-SLC006] RateLimited classifies as TempFailure.
    #[test]
    fn rate_limited_is_temp_failure() {
        let c = SlackError::RateLimited { retry_after: None }.classify();
        assert_eq!(c.kind, ErrorCode::TempFailure);
    }

    /// [T-SLC007] Network classifies as TempFailure (network-class hint).
    #[test]
    fn network_is_temp_failure() {
        let c = SlackError::Network("connection reset".into()).classify();
        assert_eq!(c.kind, ErrorCode::TempFailure);
    }

    /// [T-SLC008] Timeout classifies as Timeout (exit 124 split from TempFailure).
    #[test]
    fn timeout_is_timeout_kind() {
        let c = SlackError::Timeout("timed out".into()).classify();
        assert_eq!(c.kind, ErrorCode::Timeout);
    }

    /// [T-SLC009] Decode (schema drift) classifies as Internal per ADR-0011 priority 5.
    #[test]
    fn decode_is_internal() {
        let c = SlackError::Decode("schema mismatch".into()).classify();
        assert_eq!(c.kind, ErrorCode::Internal);
    }
}
