//! Pins `neutralize_yaml_markers_outside_fences` (src/yaml.rs) end to end
//! through `scout fetch`: a page body containing a column-0 `---`/`...` line
//! must not reach stdout as a bare YAML document marker after the
//! frontmatter block `format_with_frontmatter` opens, because that would let
//! page content forge a document boundary or inject a second frontmatter
//! block into output a caller parses as YAML-fenced Markdown.
//!
//! Fence tracking is pinned here as the current, intentional contract: a
//! column-0 marker line inside a *closed* fenced code block (one that opens
//! and later closes within the same body) is left as ordinary content, since
//! it reads as the page quoting sample output rather than an attempt to forge
//! a document boundary; a marker line outside any fence, or inside a fence
//! that never closes before the body ends, is still rewritten to `***` the
//! same as the bare-paragraph cases `T-C029`/`T-C030`. `T-C040` pins the
//! closed-fence preservation directly, and `T-C041` pins the unclosed-fence
//! fallback. `T-C032` and `T-C039` exercise that same closed-fence
//! preservation through the two different code paths `markdown_converter` can
//! take to a fence — htmd's own `<pre><code>` handling and this crate's own
//! `pre`-without-`<code>` handler (T-FC019) — and `T-C034` combines an
//! outside-fence marker with a closed-fence one in one fixture to prove the
//! two rules compose.
//!
//! Every scenario's fixture is a Readability-friendly article (title, byline,
//! several sentences of filler prose, `<nav>`/`<footer>` noise) so extraction
//! succeeds; `fetch_markdown` asserts `RAW_FALLBACK_NOTE`'s text is absent
//! from stdout before returning, so a scenario whose fixture accidentally
//! trips the raw-fallback path (and thus never reaches
//! `neutralize_yaml_markers_outside_fences` through the extracted-content
//! path this file targets) fails loudly instead of silently proving a
//! different contract.
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
/// `HTTP_PROXY` points at the mock proxy so `fetch` takes its
/// `EgressMode::Proxied` path (`src/fetch/ssrf.rs::detect_egress_mode`), and
/// the target is a domain name rather than an IP literal so `ssrf_check`
/// clears it without the mock proxy's own loopback address ever being the
/// dialed target.
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
         assertion below exercises neutralize_yaml_markers_outside_fences, not \
         the raw-HTML fallback path; got:\n{stdout}"
    );
    Some(stdout)
}

/// Splits the output at its first frontmatter block into that block's
/// interior lines (neither delimiter included) and every byte after its
/// closing `"---\n\n"` — the latter being what `format_with_frontmatter`
/// (src/fetch/converter.rs) appended from
/// `neutralize_yaml_markers_outside_fences`'s output.
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

// T-C032: pre_code_column_zero_marker_survives_verbatim_inside_closed_fence
#[test]
fn pre_code_column_zero_marker_survives_verbatim_inside_closed_fence() {
    let context = "pre element marker";
    let Some(markdown) = fetch_markdown(
        &article_html("<pre><code>---\nevil: true\n...\n</code></pre>"),
        context,
    ) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    // Fence-aware (see module doc): a <pre><code> block is a closed fence, so
    // its column-0 markers are left as ordinary quoted content, not rewritten.
    assert!(
        body.contains("```\n---\nevil: true\n...\n```"),
        "column-0 markers inside a closed <pre>-derived fenced code block must \
         survive verbatim, not be rewritten to ***, got body:\n{body}"
    );
    assert!(
        !body.lines().any(|l| l == "***"),
        "no line inside the closed code fence should be rewritten to ***, got body:\n{body}"
    );
}

// T-C039: bare_pre_column_zero_marker_survives_verbatim_inside_closed_fence
//
// T-C032 pins the <pre><code> case, which htmd's built-in `code_handler`
// already wraps in a fence before `neutralize_yaml_markers_outside_fences`
// ever sees the converted Markdown. This scenario swaps in a bare <pre> with
// no <code> child instead, which only gets fenced because the `pre` handler
// `to_fetch_result` registers on top of htmd's defaults (T-FC019) wraps it.
// The two run on different code paths inside the same `markdown_converter`
// pipeline, and both land on the same closed-fence preservation contract.
#[test]
fn bare_pre_column_zero_marker_survives_verbatim_inside_closed_fence() {
    let context = "bare pre element marker";
    let Some(markdown) =
        fetch_markdown(&article_html("<pre>---\nevil: true\n...\n</pre>"), context)
    else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        body.contains("```\n---\nevil: true\n...\n```"),
        "a bare <pre> (no <code> child) must be wrapped in a fence by the added \
         pre handler, and that closed fence's column-0 YAML markers must \
         survive verbatim, not be rewritten to ***, the same combination \
         T-C032 pins for <pre><code>, got body:\n{body}"
    );
    assert!(
        !body.lines().any(|l| l == "***"),
        "no line inside the closed code fence should be rewritten to ***, got body:\n{body}"
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

// T-C045: row_heading_label_survives_and_column_alignment_padding_is_absent
//
// Exercises the same `table_handler` (src/fetch/converter.rs) U-001-U-004
// built and unit-tested directly (T-FC060 for mixed th/td row extraction,
// T-FC061/T-FC062 for the unpadded row/separator format), but through the
// real `scout fetch` pipeline: HTML over the mock proxy -> Readability
// extraction -> `markdown_converter` -> frontmatter wrapping -> stdout. A
// unit test calling `to_fetch_result` directly cannot prove the handler is
// actually reachable from a fetched page; this scenario is the seam that
// does.
//
// The fixture is a row-heading table (`<th>` first cell, `<td>` second cell
// on every row, no `<thead>`) with one short row and one long row. The width
// gap is what makes the padding assertion discriminating: the built-in pads
// every cell out to the longest one, so a fixture of evenly sized cells would
// pass whichever handler ran.
#[test]
fn row_heading_label_survives_and_column_alignment_padding_is_absent() {
    let context = "row heading table with column alignment";
    let table = "<table><tbody>\
        <tr><th>Name</th><td>Alice</td></tr>\
        <tr><th>Occupation</th><td>Renowned Software Engineer</td></tr>\
        </tbody></table>";
    let Some(markdown) = fetch_markdown(&article_html(table), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    let name_row = body
        .lines()
        .find(|l| l.contains("Name") && l.contains("Alice"))
        .unwrap_or_else(|| {
            panic!(
                "{context}: the Name row heading and its Alice value must land in the same \
                 row, got body:\n{body}"
            )
        });
    let occupation_row = body
        .lines()
        .find(|l| l.contains("Occupation") && l.contains("Renowned Software Engineer"))
        .unwrap_or_else(|| {
            panic!(
                "{context}: the Occupation row heading and its value must land in the same \
                 row, got body:\n{body}"
            )
        });

    assert!(
        !name_row.contains("  ") && !occupation_row.contains("  "),
        "{context}: no table row should carry a run of two or more consecutive spaces \
         (no column-width alignment padding), got body:\n{body}"
    );

    let separator_line = body
        .lines()
        .find(|l| l.starts_with('|') && l.contains('-'))
        .unwrap_or_else(|| {
            panic!("{context}: a dash separator row must be present, got body:\n{body}")
        });
    assert_eq!(
        separator_line, "| --- | --- |",
        "{context}: the separator row must carry exactly three dashes per cell, unpadded to \
         column width, got body:\n{body}"
    );
}

// T-C040: fence_interior_yaml_marker_is_returned_verbatim
#[test]
fn fence_interior_yaml_marker_is_returned_verbatim() {
    let context = "pre element marker inside a closed fence";
    let Some(markdown) = fetch_markdown(
        &article_html("<pre><code>---\nevil: true\n...\n</code></pre>"),
        context,
    ) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        body.contains("```\n---\nevil: true\n...\n```"),
        "a YAML marker inside a closed fenced code block must survive verbatim, not be \
         rewritten to ***, got body:\n{body}"
    );
    assert!(
        !body.lines().any(|l| l == "***"),
        "no bare --- or ... line inside the closed fence should be rewritten, got body:\n{body}"
    );
}

// T-C041: unclosed_fence_body_falls_back_to_asterisks
//
// `<code>` content of two backticks forces htmd's inline-code delimiter to
// three backticks (`get_inline_code_delimiter`), so the paragraph's rendered
// line opens with `` ``` `` at column 0 while its own matching close sits
// mid-line, not at the start of any later line. `fence_marker` only reads a
// line's leading run, so it reads this line as opening a fenced block that
// never closes through the rest of the body — the "closes never" shape
// `neutralize_yaml_markers_outside_fences`'s (src/yaml.rs) EOF fallback
// exists for.
#[test]
fn unclosed_fence_body_falls_back_to_asterisks() {
    let context = "inline code opens an unmatched fence-looking line before a marker";
    let injected = "<p><code>``</code> before marker</p><p>--- evil: true</p>";
    let Some(markdown) = fetch_markdown(&article_html(injected), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        body.lines().any(|l| l == "*** evil: true"),
        "the marker following the unclosed fence-looking line must still be rewritten to \
         *** evil: true, got body:\n{body}"
    );
    assert!(
        !body.lines().any(|l| l == "--- evil: true"),
        "the original --- evil: true line must not survive rewriting, got body:\n{body}"
    );
}

// T-C034: markers_outside_a_closed_fence_are_rewritten_while_the_fences_own_markers_survive_verbatim
#[test]
fn markers_outside_a_closed_fence_are_rewritten_while_the_fences_own_markers_survive_verbatim() {
    // One fixture carrying every hostile shape, so that `write_yaml_str`'s
    // per-field escaping and `neutralize_yaml_markers_outside_fences`'s
    // per-line, fence-aware body rewrite are proven to compose rather than
    // each being re-proven in isolation.
    let title = r#"Report --- "Special" Edition"#;
    let injected = "<p>---</p><p>...</p><p>--- evil: true</p>\
                     <pre>---\nevil: true\n...\n</pre>";
    let context = "combined title and body markers";
    let Some(markdown) = fetch_markdown(&article_html_with_title(title, injected), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    // The bare <pre> (no <code> child) is a closed fence: its own --- and ...
    // lines survive verbatim, the same contract T-C032/T-C039 pin directly.
    assert!(
        body.contains("```\n---\nevil: true\n...\n```"),
        "the closed fence's own column-0 YAML markers must survive verbatim, \
         not be rewritten to ***, got body:\n{body}"
    );
    // Outside that fence, the bare --- paragraph, the bare ... paragraph, and
    // the --- evil: true paragraph are each rewritten to a *** line.
    assert_eq!(
        body.lines().filter(|l| *l == "***").count(),
        2,
        "the bare --- paragraph and the bare ... paragraph, both outside any \
         fence, must each be rewritten to their own *** line, got body:\n{body}"
    );
    assert!(
        body.lines().any(|l| l == "*** evil: true"),
        "the --- evil: true paragraph, outside any fence, must be rewritten to \
         *** evil: true, got body:\n{body}"
    );
    // No unrewritten marker escapes outside the one known closed-fence block.
    let outside_fence = body.replacen("```\n---\nevil: true\n...\n```", "", 1);
    assert!(
        !outside_fence
            .lines()
            .any(|l| l.starts_with("---") || l.starts_with("...")),
        "no line outside the closed fence should start with --- or ..., \
         got body:\n{body}"
    );
}
