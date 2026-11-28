//! The self-closed XHTML pre-pass for raw-text elements.

use std::borrow::Cow;

/// Elements whose contents never reach the Markdown body. `suppressed` drops
/// them, and the pre-pass closes their self-closed XHTML form first.
pub(super) const RAW_TEXT_TAGS: [&str; 7] = [
    "script", "style", "textarea", "iframe", "noscript", "title", "template",
];

/// Expand self-closed raw-text tags so HTML parsing of XHTML cannot swallow
/// the remaining body into an element that suppression then removes.
/// This does not implement the rest of XML syntax. The byte scan can also
/// rewrite matches in comments or quoted attributes, where they remain inert.
pub(in crate::fetch::converter) fn close_self_closed_raw_text_tags(html: &str) -> Cow<'_, str> {
    let bytes = html.as_bytes();
    let mut rewritten: Option<String> = None;
    let mut copied_to = 0;
    let mut cursor = 0;

    while cursor < bytes.len() {
        if bytes[cursor] != b'<' {
            cursor += 1;
            continue;
        }
        let Some(tag) = raw_text_tag_at(bytes, cursor + 1) else {
            cursor += 1;
            continue;
        };
        let Some(tag_end) = start_tag_end(bytes, cursor + 1 + tag.len()) else {
            break;
        };
        if bytes[tag_end - 1] == b'/' {
            let out = rewritten.get_or_insert_with(String::new);
            out.push_str(&html[copied_to..tag_end - 1]);
            out.push_str("></");
            out.push_str(tag);
            out.push('>');
            copied_to = tag_end + 1;
            cursor = tag_end + 1;
            continue;
        }
        // Skip raw-text contents: inserting a closing tag into a JS string would
        // end the element early and leak the remaining script into the body.
        cursor = end_tag_at_or_after(bytes, tag_end + 1, tag).unwrap_or(bytes.len());
    }

    match rewritten {
        Some(mut out) => {
            out.push_str(&html[copied_to..]);
            Cow::Owned(out)
        }
        None => Cow::Borrowed(html),
    }
}

/// Match raw-text tag names case-insensitively at a tokenizer name boundary
/// so `<scriptlet>` does not match `script`.
fn raw_text_tag_at(bytes: &[u8], from: usize) -> Option<&'static str> {
    RAW_TEXT_TAGS.into_iter().find(|tag| {
        let end = from + tag.len();
        bytes.len() > end
            && bytes[from..end].eq_ignore_ascii_case(tag.as_bytes())
            && matches!(
                bytes[end],
                b' ' | b'\t' | b'\n' | b'\r' | 0x0c | b'/' | b'>'
            )
    })
}

/// Find the matching end tag; start tags inside raw text cannot close it.
fn end_tag_at_or_after(bytes: &[u8], from: usize, tag: &str) -> Option<usize> {
    (from..bytes.len().saturating_sub(1)).find(|&index| {
        bytes[index] == b'<'
            && bytes[index + 1] == b'/'
            && raw_text_tag_at(bytes, index + 2) == Some(tag)
    })
}

/// Find the start-tag terminator, ignoring `>` inside quoted attributes.
fn start_tag_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (offset, &byte) in bytes[from..].iter().enumerate() {
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(from + offset),
            None => {}
        }
    }
    None
}
