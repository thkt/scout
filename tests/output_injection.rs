//! Pins `neutralize_yaml_markers` (src/fetch/converter.rs) end to end through
//! `scout fetch`: a page body containing a column-0 `---`/`...` line must not
//! reach stdout as a bare YAML document marker after the frontmatter block
//! `format_with_frontmatter` opens, because that would let page content forge
//! a document boundary or inject a second frontmatter block into output a
//! caller parses as YAML-fenced Markdown.
//!
//! Fence non-consideration is pinned here as the current, intentional
//! contract, not a gap to close: `neutralize_yaml_markers` rewrites every
//! column-0 marker line by its literal text alone, with no state tracking
//! whether the line sits inside a fenced code block. `T-004` proves this by
//! injecting a marker inside a `<pre>` element (which `html2md::rewrite_html`
//! renders as a fenced code block) and asserting it is rewritten exactly like
//! the bare-paragraph cases `T-001`/`T-002` are.
//!
//! Every scenario's fixture is a Readability-friendly article (title, byline,
//! several sentences of filler prose, `<nav>`/`<footer>` noise) so extraction
//! succeeds; `fetch_markdown` asserts `RAW_FALLBACK_NOTE`'s text is absent
//! from stdout before returning, so a scenario whose fixture accidentally
//! trips the raw-fallback path (and thus never reaches
//! `neutralize_yaml_markers` through the extracted-content path this file
//! targets) fails loudly instead of silently proving a different contract.

mod common;

use std::env;
use std::process::Output;
use std::time::Duration;

use common::scout;

/// Runs `scout fetch http://example.com/` (Markdown stdout, not `--json`)
/// against a mock forward proxy serving `html` as the upstream page body.
/// Mirrors `run_scout_fetch` in `tests/exit_code_contract.rs`: a from-scratch
/// environment (`PATH`/`HOME` restored, everything else cleared) plus
/// `HTTP_PROXY` pointed at the mock proxy, so `fetch`'s `EgressMode::Proxied`
/// path (src/fetch/ssrf.rs::detect_egress_mode) is taken and the domain-name
/// target (not an IP literal) clears `ssrf_check` without the mock proxy's
/// own loopback address ever being the dialed target.
fn run_scout_fetch_via_proxy(html: &str) -> Option<Output> {
    let (proxy_url, _connection_count, _handle) =
        common::spawn_mock_proxy(200, Duration::ZERO, html.as_bytes())?;

    let mut cmd = scout();
    cmd.env_clear()
        .env("PATH", env::var("PATH").unwrap_or_default())
        .env("HOME", env::var("HOME").unwrap_or_default())
        .env("HTTP_PROXY", &proxy_url)
        .args(["fetch", "http://example.com/"]);
    Some(cmd.output().expect("scout fetch failed to run"))
}

/// Runs the fixture through `scout fetch` and returns stdout as Markdown,
/// after asserting the run succeeded and Readability extraction did not fall
/// back to raw HTML. The fallback note's text is duplicated here rather than
/// imported: `RAW_FALLBACK_NOTE` (src/fetch/converter.rs) is `pub(crate)`,
/// which scopes to the `scout` library crate and does not reach this
/// separately-compiled integration-test binary.
fn fetch_markdown(html: &str, context: &str) -> String {
    let Some(output) = run_scout_fetch_via_proxy(html) else {
        panic!("{context}: loopback bind unavailable, cannot run this scenario");
    };
    assert!(
        output.status.success(),
        "{context}: scout fetch should exit 0, got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|e| panic!("{context}: stdout should be valid UTF-8: {e}"));
    assert!(
        !stdout.contains("Readability extraction failed"),
        "{context}: fixture must extract cleanly (no RAW_FALLBACK_NOTE) so the \
         assertion below exercises neutralize_yaml_markers, not the raw-HTML \
         fallback path; got:\n{stdout}"
    );
    stdout
}

/// The body Markdown after the frontmatter block's closing `---` line, i.e.
/// everything `format_with_frontmatter` (src/fetch/converter.rs) appended
/// from `neutralize_yaml_markers`'s output. Splitting on `"---\n\n"` (the
/// closing marker followed by the blank line before the body) mirrors
/// `frontmatter_body_cannot_inject_document_marker` in that file's own test
/// module: the body itself can no longer contain a bare `"---\n\n"` sequence
/// once `neutralize_yaml_markers` has run, so the first occurrence is
/// necessarily the frontmatter boundary, not a body-originated one.
fn body_after_frontmatter<'a>(markdown: &'a str, context: &str) -> &'a str {
    markdown.split("---\n\n").nth(1).unwrap_or_else(|| {
        panic!("{context}: output should contain a closed frontmatter block, got:\n{markdown}")
    })
}

/// Wraps `injected` inside a `<nav>`/`<article>`/`<footer>` shell with a
/// title, byline, and several sentences of filler prose on either side —
/// the same shape `src/fetch/extractor.rs`'s own `BLOG_HTML` test fixture
/// uses — so `dom_smoothie::Readability` scores the article region high
/// enough to extract it rather than falling back to raw HTML.
fn article_html(injected: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head><title>Marker Injection Post</title></head>
<body>
<nav>Navigation links here</nav>
<article>
<h1>Marker Injection Post</h1>
<p class="author">By Jane Doe</p>
<p>This article body demonstrates why untrusted page content must never be
trusted to define its own document structure, since a hostile page could
otherwise smuggle a forged YAML boundary into the rendered output.</p>
{injected}
<p>The paragraph above closes out the demonstration with more genuine prose
so that Readability keeps scoring this block as the main content region
rather than discarding it as boilerplate noise.</p>
</article>
<footer>Site footer</footer>
</body>
</html>"#
    )
}

// T-001: 本文由来の column-0 の --- 行は frontmatter 閉じ以降の出力に現れない
#[test]
fn body_originated_bare_dash_line_does_not_appear_after_frontmatter_close() {
    let markdown = fetch_markdown(&article_html("<p>---</p>"), "bare dash body line");
    let body = body_after_frontmatter(&markdown, "bare dash body line");

    assert!(
        !body.lines().any(|l| l == "---"),
        "a body-originated column-0 --- line must not appear as a bare --- \
         line after the frontmatter close, got body:\n{body}"
    );
}

// T-002: 本文由来の column-0 の ... 行は frontmatter 閉じ以降の出力に現れない
#[test]
fn body_originated_bare_dots_line_does_not_appear_after_frontmatter_close() {
    let markdown = fetch_markdown(&article_html("<p>...</p>"), "bare dots body line");
    let body = body_after_frontmatter(&markdown, "bare dots body line");

    assert!(
        !body.lines().any(|l| l == "..."),
        "a body-originated column-0 ... line must not appear as a bare ... \
         line after the frontmatter close, got body:\n{body}"
    );
}

// T-003: 本文の --- evil: true 行は *** evil: true の行として出力される
#[test]
fn body_dash_evil_true_line_is_rewritten_to_asterisks_evil_true() {
    let markdown = fetch_markdown(
        &article_html("<p>--- evil: true</p>"),
        "dash marker with inline content",
    );
    let body = body_after_frontmatter(&markdown, "dash marker with inline content");

    assert!(
        body.lines().any(|l| l == "*** evil: true"),
        "--- evil: true must be rewritten to a *** evil: true line, got body:\n{body}"
    );
    assert!(
        !body.lines().any(|l| l == "--- evil: true"),
        "the original --- evil: true line must not survive rewriting, got body:\n{body}"
    );
}

// T-004: pre 要素内の column-0 marker も *** に書き換えられる
#[test]
fn pre_element_column_zero_marker_is_rewritten_to_asterisks() {
    let markdown = fetch_markdown(
        &article_html("<pre>---\nevil: true\n...\n</pre>"),
        "pre element marker",
    );
    let body = body_after_frontmatter(&markdown, "pre element marker");

    // Not fence-aware on purpose (see module doc): the marker lines inside
    // the fenced code block `<pre>` becomes must be rewritten exactly like
    // the bare-paragraph cases above, not left as YAML markers just because
    // they sit inside ``` fences.
    assert!(
        body.contains("```\n***\nevil: true\n***\n```"),
        "column-0 markers inside a <pre>-derived fenced code block must be \
         rewritten to *** the same as markers outside a code fence, got body:\n{body}"
    );
    assert!(
        !body.lines().any(|l| l == "---" || l == "..."),
        "no bare --- or ... line should survive anywhere in the body, \
         inside or outside the code fence, got body:\n{body}"
    );
}
