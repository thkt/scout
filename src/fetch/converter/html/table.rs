//! Positional table rendering, including mixed header/data cells and code spans.

use super::{block, children, render as render_node, tag, trim};
use dom_query::NodeRef;

pub(super) fn render(table: NodeRef<'_>) -> String {
    let mut captions = Vec::new();
    let mut headers = Vec::new();
    let mut rows = Vec::new();
    let mut decided = false;
    for child in table.children_it(false) {
        match tag(child).as_str() {
            "caption" => captions.push(trim(&children(child, false)).to_owned()),
            "thead" => {
                let mut thead = child.children_it(false).filter(|node| tag(*node) == "tr");
                if let Some(row) = thead.next() {
                    headers = cells(row);
                    decided = true;
                }
                for row in thead {
                    let row = cells(row);
                    if !row.is_empty() {
                        rows.push(row);
                    }
                }
            }
            "tbody" | "tfoot" => {
                for row in child.children_it(false).filter(|node| tag(*node) == "tr") {
                    add_row(row, &mut decided, &mut headers, &mut rows);
                }
            }
            "tr" => add_row(child, &mut decided, &mut headers, &mut rows),
            _ => {}
        }
    }
    let width = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if width == 0 {
        return block(trim(&children(table, false)));
    }
    let mut out = String::new();
    for caption in captions {
        out.push_str(&caption);
        out.push('\n');
    }
    out.push_str(&format_row(&headers, width));
    out.push('|');
    for _ in 0..width {
        out.push_str(" --- |");
    }
    out.push('\n');
    for row in rows {
        out.push_str(&format_row(&row, width));
    }
    block(out.trim_end_matches('\n'))
}

fn add_row(
    row: NodeRef<'_>,
    decided: &mut bool,
    headers: &mut Vec<String>,
    rows: &mut Vec<Vec<String>>,
) {
    let values = cells(row);
    if values.is_empty() {
        return;
    }
    if !*decided {
        *decided = true;
        let all_headers = row
            .children_it(false)
            .filter(|node| matches!(tag(*node).as_str(), "th" | "td"))
            .all(|node| tag(node) == "th");
        if all_headers {
            *headers = values;
            return;
        }
    }
    rows.push(values);
}

fn cells(row: NodeRef<'_>) -> Vec<String> {
    row.children_it(false)
        .filter(|cell| matches!(tag(*cell).as_str(), "th" | "td"))
        .map(|cell| {
            let content = render_node(cell, false)
                .replace('\n', " ")
                .replace('\r', "")
                .replace('|', "\\|");
            trim(&content).to_owned()
        })
        .collect()
}

fn format_row(values: &[String], width: usize) -> String {
    let mut out = String::from("|");
    for index in 0..width {
        out.push(' ');
        out.push_str(values.get(index).map(String::as_str).unwrap_or(""));
        out.push_str(" |");
    }
    out.push('\n');
    out
}
