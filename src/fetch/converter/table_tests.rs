use super::test_helpers::article;
use super::*;

/// [T-FC023] A blank line between header and separator would break the table.
#[test]
fn table_output_includes_a_separator_row_following_the_header_row() {
    let article = article(
        "<table><thead><tr><th>Name</th><th>Age</th></tr></thead>\
            <tbody><tr><td>Alice</td><td>30</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();
    let lines: Vec<&str> = markdown.lines().collect();

    let header_idx = lines
        .iter()
        .position(|line| line.contains("Name") && line.contains("Age"))
        .expect("header row must be present");
    let separator_line = lines
        .get(header_idx + 1)
        .expect("a line must immediately follow the header row");

    assert!(
        !separator_line.is_empty()
            && separator_line.contains('-')
            && separator_line
                .chars()
                .all(|c| c == '|' || c == '-' || c == ' '),
        "the line right after the header row must be a dash separator row:\n{markdown}"
    );
}

/// [T-FC025] Preformatted cell newlines must not split the table row.
#[test]
fn td_pre_does_not_split_the_table_row() {
    let article = article(
        "<table><thead><tr><th>H</th></tr></thead><tbody><tr><td>\
            <pre><code>line1\nline2</code></pre></td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    let data_row = markdown
        .lines()
        .find(|line| line.contains("line1"))
        .expect("the data row must be present");
    assert!(
        data_row.contains("line1") && data_row.contains("line2"),
        "both lines of the <pre> block must land on the same table row:\n{markdown}"
    );
    assert!(
        data_row.starts_with('|') && data_row.ends_with('|'),
        "the row must stay a single well-formed pipe-delimited row:\n{markdown}"
    );
    assert_eq!(
        markdown.lines().filter(|l| l.contains('|')).count(),
        3,
        "the table must still have exactly 3 pipe-bearing lines \
         (header, separator, one data row):\n{markdown}"
    );
}

/// [T-FC060] Mixed header/data cells must both survive in their source row.
#[test]
fn label_and_value_from_a_mixed_th_td_row_land_in_the_same_row_in_separate_cells() {
    let article = article(
        "<table><tbody><tr><th>Name</th><td>Alice</td></tr>\
         <tr><th>Age</th><td>30</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    let name_row = markdown
        .lines()
        .find(|line| line.contains("Name"))
        .expect("a row carrying the Name label must be present");
    assert!(
        name_row.contains("Alice"),
        "the th label and its td value from the same source row must land in the same \
         output row:\n{markdown}"
    );

    let age_row = markdown
        .lines()
        .find(|line| line.contains("Age"))
        .expect("a row carrying the Age label must be present");
    assert!(
        age_row.contains("30"),
        "the th label and its td value from the same source row must land in the same \
         output row:\n{markdown}"
    );
}

/// [T-FC061] Unequal cell widths must not introduce column-alignment padding.
#[test]
fn table_row_between_pipes_has_no_run_of_two_or_more_spaces() {
    let article = article(
        "<table><thead><tr><th>Name</th><th>City</th></tr></thead>\
         <tbody><tr><td>Al</td><td>Springfield</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    let data_row = markdown
        .lines()
        .find(|line| line.contains("Al") && line.contains("Springfield"))
        .expect("the data row must be present");

    assert!(
        !data_row.contains("  "),
        "a table row must carry no run of two or more consecutive spaces \
         (no column-width alignment padding):\n{markdown}"
    );
}

/// [T-FC062] Separator width must remain three dashes per column.
#[test]
fn separator_row_has_exactly_three_dashes_per_cell() {
    let article = article(
        "<table><thead><tr><th>Name</th><th>Age</th></tr></thead>\
         <tbody><tr><td>Alice</td><td>30</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    // Anchored on the pipe so the frontmatter's own `---` delimiter cannot
    // stand in for the separator row.
    let separator_line = markdown
        .lines()
        .find(|line| line.starts_with("| ---"))
        .expect("a dash separator row must be present");

    assert_eq!(
        separator_line, "| --- | --- |",
        "the separator row must carry exactly three dashes per cell, unpadded to column \
         width:\n{markdown}"
    );
}

/// [T-FC063] Cell normalization must preserve literal NBSP.
#[test]
fn nbsp_inside_a_cell_survives_without_collapsing_to_a_space() {
    let article = article(
        "<table><thead><tr><th>Name</th><th>Value</th></tr></thead>\
         <tbody><tr><td>A\u{00A0}B</td><td>x</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("A\u{00A0}B"),
        "a literal NBSP inside a cell must survive unchanged, not collapse to an ASCII \
         space:\n{markdown}"
    );
}

/// [T-FC064] Mixed cells in the first thead row survive on the same line.
#[test]
fn header_promoted_row_keeps_its_td_cells() {
    let article = article(
        "<table><thead><tr><th>Name</th><td>Alice</td></tr></thead>\
         <tbody><tr><td>Bob</td><td>30</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    let header_line = markdown
        .lines()
        .find(|line| line.contains("Name"))
        .expect("the thead's first row must become the header row");
    assert!(
        header_line.contains("Alice"),
        "the row promoted to header must keep its td cell alongside its th cell, \
         not lose it:\n{markdown}"
    );
}

/// [T-FC065] Both cells of a later all-th row must survive on the same line.
/// The position check also matches frontmatter, so it cannot detect header promotion.
#[test]
fn th_only_row_in_the_middle_of_the_body_appears_as_a_data_row() {
    let article = article(
        "<table><tbody><tr><td>Alice</td><td>30</td></tr>\
         <tr><th>Bob</th><th>40</th></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();
    let lines: Vec<&str> = markdown.lines().collect();

    let separator_idx = lines
        .iter()
        .position(|line| {
            !line.is_empty()
                && line.contains('-')
                && line.chars().all(|c| c == '|' || c == '-' || c == ' ')
        })
        .expect("a dash separator row must be present");
    let bob_idx = lines
        .iter()
        .position(|line| line.contains("Bob") && line.contains("40"))
        .expect("the th-only row's cells must land in the same row");

    assert!(
        bob_idx > separator_idx,
        "a th-only row that is not the table's first row must be emitted as a data row \
         after the separator, not promoted to header:\n{markdown}"
    );
}

/// [T-FC066] Both cells of a later thead row must survive on the same line.
#[test]
fn second_and_later_rows_of_a_multi_row_thead_appear_as_data_rows() {
    let article = article(
        "<table><thead><tr><th>Name</th><th>Age</th></tr>\
         <tr><th>Category A</th><th>Category B</th></tr></thead>\
         <tbody><tr><td>Alice</td><td>30</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    markdown
        .lines()
        .find(|line| line.contains("Category A") && line.contains("Category B"))
        .expect(
            "the thead's second row must survive as a data row, not be dropped from the \
             output",
        );
}

/// [T-FC067] Mixed label/value rows retain both cells with an empty header
/// immediately followed by its separator. Data-row placement is not asserted.
#[test]
fn table_with_no_all_th_row_emits_an_empty_header_row_and_separator() {
    let article = article(
        "<table><tbody><tr><th>Name</th><td>Alice</td></tr>\
         <tr><th>Age</th><td>30</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();
    let lines: Vec<&str> = markdown.lines().collect();

    let header_idx = lines
        .iter()
        .position(|line| *line == "|  |  |")
        .expect("an empty header row must be present when no row qualifies as a header");
    let separator_line = lines
        .get(header_idx + 1)
        .expect("a line must immediately follow the empty header row");
    assert_eq!(
        *separator_line, "| --- | --- |",
        "the line right after the empty header row must be the dash separator row:\n{markdown}"
    );
    assert!(
        markdown.contains("| Name | Alice |") && markdown.contains("| Age | 30 |"),
        "both row-heading rows must stay in the body after the empty header, label and \
         value together:\n{markdown}"
    );
}

/// [T-FC069] The first all-header body row must be promoted without a thead.
#[test]
fn all_th_first_body_row_becomes_the_header_without_a_thead() {
    let article = article(
        "<table><tbody><tr><th>Name</th><th>Age</th></tr>\
         <tr><td>Alice</td><td>30</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();
    let lines: Vec<&str> = markdown.lines().collect();

    let header_idx = lines
        .iter()
        .position(|line| *line == "| Name | Age |")
        .expect("the all-th first body row must become the header row");
    assert_eq!(
        lines.get(header_idx + 1).copied(),
        Some("| --- | --- |"),
        "the separator row must follow the promoted header row:\n{markdown}"
    );
    assert_eq!(
        lines.get(header_idx + 2).copied(),
        Some("| Alice | 30 |"),
        "the remaining row must stay a data row under the header:\n{markdown}"
    );
}

/// [T-FC070] Converted caption content must precede the table.
#[test]
fn table_caption_precedes_the_header_row() {
    let article = article(
        "<table><caption>Population</caption>\
         <thead><tr><th>City</th><th>Count</th></tr></thead>\
         <tbody><tr><td>Osaka</td><td>2</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();
    let lines: Vec<&str> = markdown.lines().collect();

    let caption_idx = lines
        .iter()
        .position(|line| line.contains("Population"))
        .expect("the caption's text must reach the output");
    let header_idx = lines
        .iter()
        .position(|line| *line == "| City | Count |")
        .expect("the header row must be present");
    assert!(
        caption_idx < header_idx,
        "the caption must precede the header row:\n{markdown}"
    );
}

/// [T-FC071] A rowless table must return child content, not a zero-column table.
#[test]
fn table_with_no_rows_falls_back_to_its_walked_content() {
    let article = article("<table><caption>Empty</caption></table>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("Empty"),
        "the table's own content must survive the fallback:\n{markdown}"
    );
    assert!(
        !markdown.contains("---|") && !markdown.contains("| ---"),
        "a table with no rows must not emit a separator row:\n{markdown}"
    );
}

/// [T-FC072] Whitespace between cells must not drop either row's cell content
/// or split it across output lines. Header placement is not checked.
#[test]
fn cells_are_extracted_from_a_tr_split_across_source_lines() {
    let article = article(
        "<table>\n  <tr>\n    <th>Name</th>\n    <th>Age</th>\n  </tr>\n\
         \n  <tr>\n    <td>Alice</td>\n    <td>30</td>\n  </tr>\n</table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("| Name | Age |"),
        "the whitespace between cells must not stop the all-th row from becoming the \
         header:\n{markdown}"
    );
    assert!(
        markdown.contains("| Alice | 30 |"),
        "both cells of the data row must be extracted past the whitespace text \
         nodes:\n{markdown}"
    );
}

/// [T-FC078] A pre/code pair inside a table cell must use inline code.
#[test]
fn table_cell_code_block_renders_as_inline_code_with_one_backtick_delimiter() {
    let article = article(
        "<table><thead><tr><th>H</th></tr></thead><tbody><tr><td>\
            <pre><code>let x = 1;</code></pre></td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("| `let x = 1;` |"),
        "a table-cell code block must render as inline code with a \
         1-backtick delimiter:\n{markdown}"
    );
    assert!(
        !markdown.contains("```"),
        "a table-cell code block must not carry a 3-backtick fence:\n{markdown}"
    );
}

/// [T-FC079] Three content backticks require a four-backtick cell-code delimiter.
#[test]
fn table_cell_code_delimiter_widens_to_four_backticks_when_content_has_a_three_backtick_run() {
    let article = article(
        "<table><thead><tr><th>H</th></tr></thead><tbody><tr><td>\
            <pre><code>a ``` b</code></pre></td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("| ````a ``` b```` |"),
        "a 3-backtick run in the cell content must widen the delimiter to \
         4 backticks:\n{markdown}"
    );
}

/// [T-FC080] Inner spaces separate content-edge backticks from delimiters.
#[test]
fn table_cell_code_delimiter_gets_inner_space_when_content_starts_and_ends_with_backtick() {
    let article = article(
        "<table><thead><tr><th>H</th></tr></thead><tbody><tr><td>\
            <pre><code>`code`</code></pre></td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("| `` `code` `` |"),
        "content starting and ending with a backtick must get a single \
         inner space next to each delimiter:\n{markdown}"
    );
}

/// [T-FC082] Caption and table stay adjacent. This asserts source formatting,
/// not equivalent rendering across Markdown consumers.
#[test]
fn table_caption_precedes_the_header_row_without_a_blank_line() {
    let article = article(
        "<table><caption>Cap</caption><thead><tr><th>A</th><th>B</th></tr></thead>\
         <tbody><tr><td>1</td><td>2</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("Cap\n| A | B |"),
        "the caption must sit on the line directly above the header row:\n{markdown}"
    );
}

/// [T-FC093] A bare pre inside a cell must also avoid block fencing.
#[test]
fn table_cell_pre_without_a_code_child_renders_as_inline_code() {
    let article =
        article("<table><tbody><tr><td><pre>x\ny</pre></td><td>b</td></tr></tbody></table>");

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        !markdown.contains("```"),
        "a bare <pre> in a table cell must not leave fence backticks in the \
         cell:\n{markdown}"
    );
    assert!(
        markdown.contains("`x y`"),
        "a bare <pre> in a table cell must render as inline code:\n{markdown}"
    );
}

/// [T-FC094] Cell pre text beside a code child must survive.
#[test]
fn table_cell_pre_keeps_text_outside_its_code_child() {
    let article = article(
        "<table><tbody><tr><td><pre>prefix<code>x</code>suffix</pre></td>\
            <td>b</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("`prefixxsuffix`"),
        "text on either side of a <code> child must survive the cell's \
         inline-code rendering:\n{markdown}"
    );
}

/// [T-FC095] Direct DOM reads for cell pre content must also suppress scripts.
#[test]
fn table_cell_pre_drops_the_body_of_a_suppressed_element() {
    let article = article(
        "<table><tbody><tr><td><pre><code>visible\
            <script>hidden()</script></code></pre></td><td>b</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        !markdown.contains("hidden()"),
        "a <script> body inside a table cell's <pre> must not reach the \
         markdown:\n{markdown}"
    );
    assert!(
        markdown.contains("`visible`"),
        "the cell's visible code text must survive the suppression:\n{markdown}"
    );
}

/// [T-FC096] A br without a Text child must still separate cell pre words.
#[test]
fn table_cell_pre_keeps_a_br_as_a_visible_separator() {
    let article = article(
        "<table><tbody><tr><td><pre><code>a<br>b</code></pre></td>\
            <td>c</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("`a b`"),
        "a <br> inside a table cell's <pre> must separate the two lines \
         rather than joining them:\n{markdown}"
    );
}

/// [T-FC098] Escape cell-code pipes with `\|`; GFM does not decode entities
/// inside code spans.
#[test]
fn table_cell_code_span_escapes_a_pipe_with_a_backslash() {
    let article = article(
        "<table><tbody><tr><td><pre><code>a | b</code></pre></td><td>x</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains(r"`a \| b`"),
        "a pipe inside a cell's code span must be backslash-escaped:\n{markdown}"
    );
    assert!(
        !markdown.contains("&#124;"),
        "an entity reference would show as literal text inside the code span:\n{markdown}"
    );
}

/// [T-FC099] Row padding must keep a trailing content backslash from escaping
/// the next cell delimiter. Inline-code output closes with a backtick, so it
/// cannot produce a bare trailing backslash at that boundary.
#[test]
fn table_row_keeps_a_space_between_a_trailing_backslash_and_the_closing_pipe() {
    // A backslash inside a code span stays unescaped (T-FC015).
    let article = article(
        "<table><tbody><tr><td><pre><code>a\\</code></pre></td><td>second</td></tr></tbody></table>",
    );

    let result = to_fetch_result(&article, "https://example.com".into(), false);
    let markdown = result.markdown();

    assert!(
        markdown.contains("`a\\` | second |"),
        "a trailing backslash must not run into the closing pipe:\n{markdown}"
    );
}
