//! Media-type handling through the real fetch CLI and a local HTTP proxy.

mod common;

use std::sync::atomic::Ordering;

/// [T-C049] HTML parsing must not consume explicit non-HTML source syntax or whitespace;
/// neither HTML heuristic may launch Chrome for these short responses.
#[test]
fn non_html_source_survives_normal_raw_markdown_and_json() {
    for (content_type, body) in [
        (
            "text/plain; charset=utf-8",
            "pub fn id<T>(x: T) -> T { x }\nLiteral: <secret>preserve me</secret> &amp; &\nTitle\n=====\n# comment\n\n",
        ),
        (
            " TEXT/PLAIN ; charset=UTF-8",
            "pub fn id<T>(x: T) -> T { x }\nLiteral: <secret>preserve me</secret> &amp; &\n<script>literal</script>\nTitle\n=====\n# comment\n\n",
        ),
        (
            "application/xml; charset=utf-8",
            "<catalog>\n<item id=\"42\">Primary <symbol>X</symbol> &amp; &#65;</item>\n</catalog>\n\n",
        ),
        (
            "text/markdown; charset=utf-8",
            "# Heading\n\nLiteral: <widget key=\"value\"> &amp; &#65;\n```rust\nlet x = \"<value>\";\n```\n\n",
        ),
        (
            "text/xml",
            "<p id=\"source\">Visible &amp; readable</p>\n<script>literal</script>\n",
        ),
        (
            "APPLICATION/RSS+XML; charset=UTF-8",
            "<rss><channel><title>Source</title><item id=\"42\" /></channel></rss>\n",
        ),
    ] {
        for raw in [false, true] {
            for json in [false, true] {
                let Some((markdown, stderr)) = fetch(content_type, body, raw, json) else {
                    return;
                };
                assert_eq!(markdown, format!("---\n---\n\n{body}"));
                assert!(!stderr.contains("trying JS rendering"), "{stderr}");
                assert!(
                    !stderr.contains("extraction yielded too little content"),
                    "{stderr}"
                );
                assert!(
                    !stderr.contains("Readability extraction failed"),
                    "{stderr}"
                );
            }
        }
    }
}

/// [T-C050] HTML and XHTML still decode entities and suppress active markup.
#[test]
fn html_and_xhtml_still_convert() {
    for content_type in ["text/html", "application/xhtml+xml"] {
        let body = format!(
            "<p>Visible &amp; readable {}</p><script>hidden</script>",
            "ordinary article content ".repeat(8)
        );
        let Some((markdown, _)) = fetch(content_type, &body, true, false) else {
            return;
        };
        assert!(markdown.contains("Visible & readable"), "{markdown}");
        assert!(!markdown.contains("<p>"), "{markdown}");
        assert!(!markdown.contains("hidden"), "{markdown}");
    }
}

/// [T-C051] Non-HTML source output must still neutralize YAML boundaries.
#[test]
fn non_html_source_keeps_yaml_output_defense() {
    for content_type in ["text/plain", "text/markdown", "application/xml"] {
        for json in [false, true] {
            let Some((markdown, _)) = fetch(content_type, "before\n---\n...\nafter\n", false, json)
            else {
                return;
            };
            assert_eq!(markdown, "---\n---\n\nbefore\n***\n***\nafter\n");
        }
    }
}

fn fetch(content_type: &str, body: &str, raw: bool, json: bool) -> Option<(String, String)> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let (proxy, connections, handle) = common::spawn_mock_proxy_raw_response(response.as_bytes())?;
    let mut command = common::scout_with_clean_env();
    command
        .env("HTTP_PROXY", &proxy)
        .args(["fetch", "http://scout-audit.example/plain"]);
    if raw {
        command.arg("--raw");
    }
    if json {
        command.arg("--json");
    }
    let output = command.output().expect("scout should launch");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(connections.load(Ordering::SeqCst), 1);
    handle.join().expect("proxy should finish");
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
    let markdown = if json {
        let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("JSON envelope");
        assert_eq!(envelope["data"]["url"], "http://scout-audit.example/plain");
        assert_eq!(envelope["notes"], serde_json::json!([]));
        assert!(envelope.get("degraded_reasons").is_none(), "{envelope}");
        envelope["data"]["markdown"]
            .as_str()
            .expect("markdown body")
            .to_owned()
    } else {
        stdout
    };
    Some((markdown, stderr))
}
