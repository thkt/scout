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
//!
//! `T-005`/`T-006` pin the sibling contract `write_yaml_str`
//! (src/fetch/converter.rs) owns: a frontmatter *field value* (the article
//! title, here) is wrapped in double quotes and escaped through
//! `escape_yaml` as one contract, not two independent steps, so a title
//! carrying `"` or a `---`-shaped substring is written back out as ordinary
//! text on the single `title: "..."` line rather than breaking out of the
//! quotes into a new line. The fixture titles below are proven, empirically
//! (a throwaway `dom_smoothie` probe, not read off crate docs), to survive
//! `dom_smoothie::Readability::get_article_title`'s separator-driven
//! cleanup unchanged: their word count after that cleanup's split-on-`-`
//! step is still <= 4, so `get_article_title` reverts to the original
//! `<title>` text verbatim before `to_fetch_result` ever sees it.
//!
//! A title containing a literal newline is out of reach through this
//! HTML-driven, end-to-end path on purpose: `get_article_title` runs
//! `normalize_spaces` (collapsing any whitespace run, including a
//! newline, to a single space) on every title it does not revert
//! verbatim, and the revert-to-original branch these fixtures take can only
//! be reached by a `<title>` text node, which an HTML parser never lets
//! contain a raw `\n` byte in the first place. `escape_yaml`'s `'\n' =>
//! "\\n"` arm therefore stays pinned only by the crate-internal tests in
//! `src/fetch/converter.rs` (`escapes_yaml_special_chars`,
//! `escapes_combined_special_chars`), not by anything in this file.

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
/// caller-supplied `<title>`, byline, and several sentences of filler prose
/// on either side — the same shape `src/fetch/extractor.rs`'s own
/// `BLOG_HTML` test fixture uses — so `dom_smoothie::Readability` scores the
/// article region high enough to extract it rather than falling back to raw
/// HTML. `title` lands verbatim in the `<title>` element, which
/// `get_article_title` (`dom_smoothie`) reads to produce `article.title`,
/// the value `format_with_frontmatter` (src/fetch/converter.rs) passes to
/// `write_yaml_str` — the module doc above records why `title` must stay
/// free of raw newlines for that value to round-trip unchanged.
fn article_html_with_title(title: &str, injected: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head><title>{title}</title></head>
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

/// `article_html_with_title` with the fixed title `T-001`-`T-004` share.
fn article_html(injected: &str) -> String {
    article_html_with_title("Marker Injection Post", injected)
}

/// The frontmatter block's interior lines — everything between the opening
/// `"---\n"` `format_with_frontmatter` (src/fetch/converter.rs) writes and
/// the closing `"---\n\n"` it writes after the last field — with neither
/// delimiter included. `T-005` reads a single field line out of this; `find`
/// (not a `"---\n"` split like `body_after_frontmatter` uses) is safe against
/// a field value that itself contains a `-`-only run followed by non-`\n`
/// text (`T-005`'s title does), because `escape_yaml` never lets a field
/// value contain a raw `\n`, so `"---\n\n"` cannot occur before the real
/// closing delimiter.
fn frontmatter_block<'a>(markdown: &'a str, context: &str) -> &'a str {
    let after_open = markdown.strip_prefix("---\n").unwrap_or_else(|| {
        panic!("{context}: output should start with an opening --- line, got:\n{markdown}")
    });
    let close_at = after_open.find("---\n\n").unwrap_or_else(|| {
        panic!("{context}: output should contain a closed frontmatter block, got:\n{markdown}")
    });
    &after_open[..close_at]
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

// T-005: 二重引用符と --- を含む title は frontmatter の title 値としてエスケープされ新しい行を作らない
#[test]
fn title_with_double_quotes_and_dashes_is_escaped_without_creating_a_new_line() {
    // Empirically confirmed (throwaway `dom_smoothie` probe against the
    // pinned 0.18.0, not read off crate docs, `unverified` no docs page
    // documents this cleanup's exact behavior): this exact string reverts to
    // itself, unchanged, through `get_article_title`'s separator cleanup —
    // see the module doc's `T-005`/`T-006` paragraph for why.
    let title = r#"Report --- "Special" Edition"#;
    let markdown = fetch_markdown(
        &article_html_with_title(title, "<p>Injected content placeholder.</p>"),
        "quoted-dash title",
    );
    let frontmatter = frontmatter_block(&markdown, "quoted-dash title");

    let title_lines: Vec<&str> = frontmatter
        .lines()
        .filter(|l| l.starts_with("title:"))
        .collect();
    assert_eq!(
        title_lines,
        vec![r#"title: "Report --- \"Special\" Edition""#],
        "the title's \" must be escaped to \\\" and the whole value must stay \
         on write_yaml_str's single title: \"...\" line, got frontmatter:\n{frontmatter}"
    );
    assert!(
        !frontmatter.lines().any(|l| l == "---" || l == "..."),
        "an escaped title must not produce a bare --- or ... line inside the \
         frontmatter block, got frontmatter:\n{frontmatter}"
    );
}

// T-006: 最初の frontmatter block を除いた出力全体に --- または ... で始まる行が存在しない
#[test]
fn no_line_after_first_frontmatter_block_starts_with_a_yaml_document_marker() {
    // Combines T-005's hostile title with T-001/T-002/T-003/T-004's hostile
    // body markers in one fixture, so this scenario proves the two
    // mechanisms (write_yaml_str's per-field escaping and
    // neutralize_yaml_markers's per-line body rewrite) compose without
    // leaving a gap at their boundary, rather than re-proving either one in
    // isolation.
    let title = r#"Report --- "Special" Edition"#;
    let injected = "<p>---</p><p>...</p><p>--- evil: true</p>\
                     <pre>---\nevil: true\n...\n</pre>";
    let markdown = fetch_markdown(
        &article_html_with_title(title, injected),
        "combined title and body markers",
    );
    let body = body_after_frontmatter(&markdown, "combined title and body markers");

    assert!(
        !body
            .lines()
            .any(|l| l.starts_with("---") || l.starts_with("...")),
        "no line anywhere in the output after the first frontmatter block's \
         close should start with --- or ..., got body:\n{body}"
    );
}
