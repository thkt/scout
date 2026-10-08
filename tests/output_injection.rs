//! End-to-end YAML-boundary defense through `scout fetch` (src/yaml.rs).
//! Outside closed fences, column-0 `---`/`...` markers become `***`; an
//! unclosed conservative fence makes the entire body subject to rewriting.
//! Both htmd's pre/code fences (T-C032) and scout's bare-pre fences
//! (T-C039, T-FC019) must preserve quoted markers when closed.
//!
//! Fixtures need enough article prose for Readability extraction. The helper
//! rejects RAW_FALLBACK_NOTE so fallback cannot silently satisfy these checks.
//! T-C033/T-C034 exercise quoted title values through write_yaml_str. Their
//! short titles rely on Readability's separator-cleanup fallback retaining the
//! original title; neither fixture tests literal title newlines. Newline
//! escaping is covered by escapes_yaml_special_chars and
//! escapes_combined_special_chars in src/yaml.rs.

mod common;

use std::process::Output;
use std::time::Duration;

/// Fetch fixture HTML through a forward proxy in Markdown mode. A domain
/// target avoids IP-literal SSRF rejection; the proxy owns DNS and dialing.
/// Returns None on unavailable loopback bind unless SCOUT_NETWORK_TESTS
/// requires assertions, matching tests/common/mod.rs's shared policy.
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

/// Require successful Readability extraction and return Markdown stdout.
/// The fallback-note text is repeated because its pub(crate) constant is
/// inaccessible to this integration binary. None preserves the bind skip.
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

/// Split the first frontmatter block, allowing a preamble before its opener.
/// Inspect the entire remaining body: stopping at a second delimiter could
/// hide the very injected boundary T-C034 detects. Literal closing-delimiter
/// matching is safe because escaped field values cannot contain raw newlines.
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

/// Article prose, byline and surrounding chrome make Readability select the
/// injected region. Title fixtures must survive its title cleanup unchanged
/// to exercise write_yaml_str; see the module-level fixture constraints.
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
// Unlike T-C032's htmd pre/code path, this exercises scout's bare-pre
// handler (T-FC019) through Readability and the CLI.
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
    // Keep the short title so Readability retains it verbatim; see module docs.
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
// Exercises table_handler through Readability and the CLI, beyond the direct
// T-FC060/T-FC061/T-FC062 converter checks. Unequal cell widths distinguish
// scout's unpadded rows from htmd's column-alignment padding.
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

// T-C041: unclosed_fence_body_falls_back_to_asterisks
//
// Three content backticks force a four-backtick inline delimiter. Its opener
// is at column 0, but its close is mid-line; the conservative YAML tracker
// therefore sees an unclosed fence. No later line has four closing backticks.
#[test]
fn unclosed_fence_body_falls_back_to_asterisks() {
    let context = "inline code opens an unmatched fence-looking line before a marker";
    let injected = "<p><code>```</code> before marker</p><p>--- evil: true</p>";
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

// T-C043: on a page whose pre holds one span per line, no fetch output line carries a backslash
//
// Extends T-FC053 through Readability and the CLI. Direct pre text and span
// children use different converter paths; extraction must preserve the final
// code text across that seam. Leading tilde/backtick bytes expose htmd escapes
// that plain text would not, so the no-backslash assertion is discriminating.
#[test]
fn pre_with_one_span_per_line_produces_no_backslash_in_fetch_output() {
    let context = "syntax-highlighted pre with one span per line";
    let injected = "<pre><span data-line=\"1\">~/project$ ls -la\n</span>\
                     <span data-line=\"2\">`echo hello`\n</span>\
                     <span data-line=\"3\">plain trailing line</span></pre>";
    let Some(markdown) = fetch_markdown(&article_html(injected), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        body.contains("```\n~/project$ ls -la\n`echo hello`\nplain trailing line\n```"),
        "{context}: each line-span's content must land on its own output line, in order, and \
         match the source text verbatim, got body:\n{body}"
    );
    assert!(
        !body.lines().any(|l| l.contains('\\')),
        "{context}: no line of fetch output should contain a backslash, since neither \
         raw_pre_content's direct-Text-child path nor span_handler's walk_children path may \
         introduce htmd's leading `~`/`` ` `` escape once the per-line <span> structure survives \
         Readability extraction intact, got body:\n{body}"
    );
}

// T-C042: closed_fence_and_paragraph_and_unclosed_fence_in_one_page_converge_to_one_output
//
// A later unclosed fence removes marker protection from the whole body,
// including the earlier closed pre fence (T-C032/T-C039). Put the unclosed
// T-C041 trick last to distinguish global fallback from a tail-only rewrite.
#[test]
fn closed_fence_and_paragraph_and_unclosed_fence_in_one_page_converge_to_one_output() {
    let context = "closed fence, paragraph, and unclosed fence combined";
    let closed_fence = "<pre>---\nevil: true\n...\n</pre>";
    let outside_marker = "<p>--- evil: true</p>";
    let unclosed_trick = "<p><code>```</code> before marker</p><p>... evil: unclosed</p>";
    let injected = format!("{closed_fence}{outside_marker}{unclosed_trick}");
    let Some(markdown) = fetch_markdown(&article_html(&injected), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        body.contains("```\n***\nevil: true\n***\n```"),
        "the <pre> block must still render as a fenced code block, got body:\n{body}"
    );
    assert!(
        !body.contains("```\n---\nevil: true\n...\n```"),
        "the closed fence's own markers must not survive verbatim once a \
         later fence in the same page never closes, got body:\n{body}"
    );
    // T-C031's outside-fence marker must still be rewritten.
    assert!(
        body.lines().any(|l| l == "*** evil: true"),
        "the outside-fence paragraph marker must be rewritten to \
         *** evil: true, got body:\n{body}"
    );
    // T-C041's post-opener marker must also be rewritten.
    assert!(
        body.lines().any(|l| l == "*** evil: unclosed"),
        "the marker following the unclosed fence-looking line must be \
         rewritten to *** evil: unclosed, got body:\n{body}"
    );
    assert!(
        !body
            .lines()
            .any(|l| l.starts_with("---") || l.starts_with("...")),
        "no line anywhere in the page should start with --- or ... once the \
         page carries an unclosed fence, got body:\n{body}"
    );
}

// T-C044: a page mixing paragraphs, a table and a list shows folding, hard breaks and the counter-examples in one output
//
// Compose DR-0027's line-break cases through Readability and the CLI:
// paragraph folding/hard breaks (T-FC041/T-FC042), and table/list exceptions
// (T-FC045/T-FC046). Direct converter tests do not exercise this seam.
#[test]
fn paragraph_fold_hard_break_and_table_and_list_counter_examples_converge_in_one_output() {
    let context = "paragraph fold, hard break, and table/list counter-examples combined";
    let fold_paragraph = "<p>river bank line one\nriver bank line two</p>";
    let hard_break_paragraph = "<p>bridge deck line one<br>bridge deck line two</p>";
    // Row-heading shape (<tbody>, <th> label cell, two rows) mirrors the
    // fixture T-C045 already proves Readability keeps as a genuine data
    // table rather than unwrapping as a layout table; a single-row, no-<th>
    // table does not survive extraction intact.
    let table = "<table><tbody>\
        <tr><th>Harbor</th><td>harbor cell line one<br>harbor cell line two</td></tr>\
        <tr><th>Note</th><td>second row keeps this a genuine data table</td></tr>\
        </tbody></table>";
    let list = "<ul><li>lighthouse item line one<br>lighthouse item line two</li></ul>";
    let injected = format!("{fold_paragraph}{hard_break_paragraph}{table}{list}");
    let Some(markdown) = fetch_markdown(&article_html(&injected), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    // Contract form 1 (T-FC041): the paragraph's own wrapped newline folds
    // to a single space.
    assert!(
        body.contains("river bank line one river bank line two"),
        "{context}: a newline inside a paragraph's text must fold to a single space, got \
         body:\n{body}"
    );
    assert!(
        !body.contains("river bank line one\nriver bank line two"),
        "{context}: the source newline must not survive as a literal line break, got \
         body:\n{body}"
    );

    // Contract form 2 (T-FC042): the paragraph's own <br> survives as a hard
    // break (two trailing spaces then a newline).
    assert!(
        body.contains("bridge deck line one  \nbridge deck line two"),
        "{context}: a <br> inside a paragraph must leave two trailing spaces before the \
         newline it introduces, got body:\n{body}"
    );

    // Counter-example (T-FC045): the table cell's <br> loses the hard break
    // and collapses to a run of spaces instead.
    assert!(
        body.contains("| Harbor | harbor cell line one   harbor cell line two |"),
        "{context}: a <br> inside a table cell must collapse to a run of spaces, not survive \
         as a line break, got body:\n{body}"
    );
    assert!(
        !body.contains("harbor cell line one  \nharbor cell line two"),
        "{context}: the paragraph's hard-break form must not survive inside a table cell, got \
         body:\n{body}"
    );

    // Counter-example (T-FC046): the list item's <br> loses its trailing
    // spaces and becomes an indented newline instead.
    assert!(
        body.contains("lighthouse item line one\n    lighthouse item line two"),
        "{context}: a <br> inside a list item must leave no trailing spaces on the line \
         before it, and the text after it must reappear indented on its own line, got \
         body:\n{body}"
    );
    assert!(
        !body.contains("lighthouse item line one  \n"),
        "{context}: the paragraph's hard-break form must not survive inside a list item, got \
         body:\n{body}"
    );
}

// T-C046: a page holding both an empty anchor and a titled link shows the suppression and the removal in one output
//
// Compose empty highlighter anchors and titled prose links through the fetched
// page pipeline. T-FC048/T-FC073 test the converter directly; this catches
// extraction or registration changes that leave those isolated checks passing.
#[test]
fn empty_anchor_suppression_and_link_title_deletion_converge_in_one_fetch_output() {
    let context = "empty per-line code anchor and titled prose link combined";
    let code_with_line_anchors = "<pre><a href=\"#__codelineno-0-1\"></a>    def foo():\n\
         <a href=\"#__codelineno-0-2\"></a>        return 1\n</pre>";
    let titled_link = "<p>See <a href=\"https://example.com/target\" title=\"My Title\">link text</a> \
         for details.</p>";
    let injected = format!("{code_with_line_anchors}{titled_link}");
    let Some(markdown) = fetch_markdown(&article_html(&injected), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        !body.contains("__codelineno"),
        "an empty anchor pointing only at a fragment must leave no trace of its href, \
         got body:\n{body}"
    );
    assert!(
        body.lines().any(|l| l == "    def foo():"),
        "the first original code line and its indentation must survive with the anchor \
         removed, got body:\n{body}"
    );
    assert!(
        body.lines().any(|l| l == "        return 1"),
        "the second original code line and its indentation must survive with the anchor \
         removed, got body:\n{body}"
    );

    assert!(
        body.contains("[link text](https://example.com/target)"),
        "a titled link with link text must lose its title, leaving a bare [text](url), \
         got body:\n{body}"
    );
    assert!(
        !body.contains("My Title"),
        "the title text must not survive anywhere in the output, got body:\n{body}"
    );
}

// T-C048: fetch_output_truncated_at_the_cap_leaves_no_live_yaml_document_marker
//
// Conversion preserves the marker inside a closed pre fence (T-C032). Filler
// exceeds the 100,000-byte output cap, removing that fence's close; truncation
// must re-neutralize the now-exposed marker. The private output cap cannot be
// imported by this integration binary.
#[test]
fn fetch_output_truncated_at_the_cap_leaves_no_live_yaml_document_marker() {
    let context = "marker inside a fence whose close falls past the truncation cap";
    let filler = "x".repeat(80) + "\n";
    let pre_content = format!("---\nevil: true\n{}", filler.repeat(1_500));
    let injected = format!("<pre>{pre_content}</pre>");
    let Some(markdown) = fetch_markdown(&article_html(&injected), context) else {
        return;
    };
    let (_, body) = split_frontmatter(&markdown, context);

    assert!(
        body.len() < pre_content.len(),
        "{context}: output must actually be truncated for this scenario to be \
         meaningful, got {} bytes of body against a {}-byte <pre>, body:\n{body}",
        body.len(),
        pre_content.len()
    );
    assert!(
        body.contains("(truncated: showing"),
        "{context}: output should carry the truncation note, got body:\n{body}"
    );
    assert!(
        !body.lines().any(|l| l == "---"),
        "{context}: a marker that survived verbatim only because its fence \
         looked closed must be re-neutralized once truncation removes that \
         fence's own closing delimiter, got body:\n{body}"
    );
}
