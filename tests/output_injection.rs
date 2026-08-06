//! Pins `neutralize_yaml_markers` (src/yaml.rs) end to end through
//! `scout fetch`: a page body containing a column-0 `---`/`...` line must not
//! reach stdout as a bare YAML document marker after the frontmatter block
//! `format_with_frontmatter` opens, because that would let page content forge
//! a document boundary or inject a second frontmatter block into output a
//! caller parses as YAML-fenced Markdown.
//!
//! Fence non-consideration is pinned here as the current, intentional
//! contract, not a gap to close: `neutralize_yaml_markers` rewrites every
//! column-0 marker line by its literal text alone, with no state tracking
//! whether the line sits inside a fenced code block. `T-C032` proves this by
//! injecting a marker inside a `<pre>` element (which `html2md::rewrite_html`
//! renders as a fenced code block) and asserting it is rewritten exactly like
//! the bare-paragraph cases `T-C029`/`T-C030` are.
//!
//! Every scenario's fixture is a Readability-friendly article (title, byline,
//! several sentences of filler prose, `<nav>`/`<footer>` noise) so extraction
//! succeeds; `fetch_markdown` asserts `RAW_FALLBACK_NOTE`'s text is absent
//! from stdout before returning, so a scenario whose fixture accidentally
//! trips the raw-fallback path (and thus never reaches
//! `neutralize_yaml_markers` through the extracted-content path this file
//! targets) fails loudly instead of silently proving a different contract.
//!
//! `T-C033`/`T-C034` pin the sibling contract `write_yaml_str`
//! (src/yaml.rs) owns: a frontmatter *field value* (the article
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
//! `src/yaml.rs` (`escapes_yaml_special_chars`,
//! `escapes_combined_special_chars`), not by anything in this file.

mod common;

use std::process::Output;
use std::time::Duration;

/// Runs `scout fetch http://example.com/` (Markdown stdout, not `--json`)
/// against a mock forward proxy serving `html` as the upstream page body.
/// Builds the child's environment via `common::scout_with_clean_env` — the
/// same env_clear plus `PATH`/`HOME`/`LLVM_PROFILE_FILE` carry-through
/// `run_scout_fetch` (`tests/exit_code_contract.rs`) uses — with `HTTP_PROXY`
/// pointed at the mock proxy, so `fetch`'s `EgressMode::Proxied` path
/// (`src/fetch/ssrf.rs::detect_egress_mode`) is taken and the domain-name
/// target (not an IP literal) clears `ssrf_check` without the mock proxy's
/// own loopback address ever being the dialed target.
///
/// Returns `None` when `spawn_mock_proxy` reports an unavailable loopback
/// bind, which `guard_loopback_bind` (tests/common/mod.rs) defines as a skip
/// unless `SCOUT_NETWORK_TESTS` forces a panic — the same
/// `else { return; }` treatment `assert_proxy_status_maps_to`
/// (`tests/exit_code_contract.rs`) gives it, so a bind-restricted environment
/// skips this file the way it skips the other two `tests/*.rs` binaries
/// instead of reddening the suite here alone.
fn run_scout_fetch_via_proxy(html: &str, context: &str) -> Option<Output> {
    let (proxy_url, connection_count, _handle) =
        common::spawn_mock_proxy(200, Duration::ZERO, html.as_bytes())?;

    let mut cmd = common::scout_with_clean_env();
    cmd.env("HTTP_PROXY", &proxy_url)
        .args(["fetch", "http://example.com/"]);
    let output = cmd.output().expect("scout fetch failed to run");

    common::assert_proxy_was_dialed(
        &connection_count,
        context,
        "the stdout asserted below did not come from the fixture",
    );
    Some(output)
}

/// Runs the fixture through `scout fetch` and returns stdout as Markdown,
/// after asserting the run succeeded and Readability extraction did not fall
/// back to raw HTML. The fallback note's text is duplicated here rather than
/// imported: `RAW_FALLBACK_NOTE` (src/fetch/converter.rs) is `pub(crate)`,
/// which scopes to the `scout` library crate and does not reach this
/// separately-compiled integration-test binary.
///
/// `None` carries `run_scout_fetch_via_proxy`'s skip decision, which every
/// caller below turns into an early `return`.
fn fetch_markdown(html: &str, context: &str) -> Option<String> {
    let output = run_scout_fetch_via_proxy(html, context)?;
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
    Some(stdout)
}

/// Splits the output at its first frontmatter block into that block's
/// interior lines (neither delimiter included) and every byte after its
/// closing `"---\n\n"` — the latter being what `format_with_frontmatter`
/// (src/fetch/converter.rs) appended from `neutralize_yaml_markers`'s output.
///
/// Both slices are located rather than assumed:
///
/// - The opening `"---\n"` is searched for at a line start instead of
///   required at byte 0, so output carrying a preamble ahead of the block
///   (`RAW_FALLBACK_NOTE`, which `fetch_markdown` rejects, is the one this
///   file could hit) still splits into the same two slices rather than
///   panicking on an absent prefix.
/// - The body slice runs to the end of the output rather than to the next
///   `"---\n\n"`, so a marker that survived rewriting and opened a second
///   block cannot push the text behind it outside what `T-C034` inspects.
///   Reading only up to a second delimiter would assume the very property
///   these tests exist to falsify.
///
/// Finding the closing delimiter as a literal is safe against a field value
/// containing a `-`-only run followed by non-`\n` text (`T-C033`'s title
/// does), because `escape_yaml` never lets a field value carry a raw `\n`.
fn split_frontmatter<'a>(markdown: &'a str, context: &str) -> (&'a str, &'a str) {
    let open_at = if markdown.starts_with("---\n") {
        0
    } else {
        markdown.find("\n---\n").map_or_else(
            || panic!("{context}: output should contain an opening --- line, got:\n{markdown}"),
            |at| at + 1,
        )
    };
    // Both search patterns are ASCII-only, which is what keeps `open_at` and
    // the length added to it on a char boundary.
    let after_open = &markdown[open_at + "---\n".len()..];
    after_open.split_once("---\n\n").unwrap_or_else(|| {
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

/// `article_html_with_title` with the fixed title `T-C029`-`T-C032` share.
fn article_html(injected: &str) -> String {
    article_html_with_title("Marker Injection Post", injected)
}

// T-C029: body_originated_bare_dash_line_does_not_appear_after_frontmatter_close
#[test]
fn body_originated_bare_dash_line_does_not_appear_after_frontmatter_close() {
    let context = "bare dash body line";
    let Some(markdown) = fetch_markdown(&article_html("<p>---</p>"), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        !body.lines().any(|l| l == "---"),
        "a body-originated column-0 --- line must not appear as a bare --- \
         line after the frontmatter close, got body:\n{body}"
    );
}

// T-C030: body_originated_bare_dots_line_does_not_appear_after_frontmatter_close
#[test]
fn body_originated_bare_dots_line_does_not_appear_after_frontmatter_close() {
    let context = "bare dots body line";
    let Some(markdown) = fetch_markdown(&article_html("<p>...</p>"), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        !body.lines().any(|l| l == "..."),
        "a body-originated column-0 ... line must not appear as a bare ... \
         line after the frontmatter close, got body:\n{body}"
    );
}

// T-C031: body_dash_evil_true_line_is_rewritten_to_asterisks_evil_true
#[test]
fn body_dash_evil_true_line_is_rewritten_to_asterisks_evil_true() {
    let context = "dash marker with inline content";
    let Some(markdown) = fetch_markdown(&article_html("<p>--- evil: true</p>"), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        body.lines().any(|l| l == "*** evil: true"),
        "--- evil: true must be rewritten to a *** evil: true line, got body:\n{body}"
    );
    assert!(
        !body.lines().any(|l| l == "--- evil: true"),
        "the original --- evil: true line must not survive rewriting, got body:\n{body}"
    );
}

// T-C032: pre_element_column_zero_marker_is_rewritten_to_asterisks
#[test]
fn pre_element_column_zero_marker_is_rewritten_to_asterisks() {
    let context = "pre element marker";
    let Some(markdown) =
        fetch_markdown(&article_html("<pre>---\nevil: true\n...\n</pre>"), context)
    else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    // Not fence-aware on purpose (see module doc).
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

// T-C033: title_with_double_quotes_and_dashes_is_escaped_without_creating_a_new_line
#[test]
fn title_with_double_quotes_and_dashes_is_escaped_without_creating_a_new_line() {
    // The module doc's `T-C033`/`T-C034` paragraph records why this exact
    // string survives `get_article_title`'s separator cleanup unchanged, and
    // that the source is a throwaway probe rather than crate docs.
    let title = r#"Report --- "Special" Edition"#;
    let context = "quoted-dash title";
    let Some(markdown) = fetch_markdown(
        &article_html_with_title(title, "<p>Injected content placeholder.</p>"),
        context,
    ) else {
        return;
    };
    let (frontmatter, _) = split_frontmatter(&markdown, context);

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

// T-C034: no_line_after_first_frontmatter_block_starts_with_a_yaml_document_marker
#[test]
fn no_line_after_first_frontmatter_block_starts_with_a_yaml_document_marker() {
    // One fixture carrying both hostile shapes, so that `write_yaml_str`'s
    // per-field escaping and `neutralize_yaml_markers`'s per-line body rewrite
    // are proven to compose rather than each being re-proven in isolation.
    let title = r#"Report --- "Special" Edition"#;
    let injected = "<p>---</p><p>...</p><p>--- evil: true</p>\
                     <pre>---\nevil: true\n...\n</pre>";
    let context = "combined title and body markers";
    let Some(markdown) = fetch_markdown(&article_html_with_title(title, injected), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        !body
            .lines()
            .any(|l| l.starts_with("---") || l.starts_with("...")),
        "no line anywhere in the output after the first frontmatter block's \
         close should start with --- or ..., got body:\n{body}"
    );
}
