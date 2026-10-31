use std::borrow::Cow;

mod html;
use serde::Serialize;

use super::extractor::ExtractedArticle;
use crate::markdown::shift_headings;
use crate::yaml::{neutralize_yaml_markers_outside_fences, write_yaml_str};

/// Private fields restrict production construction to media-specific output
/// boundaries. for_test can supply a body directly for formatter fixtures.
#[derive(Debug, Serialize)]
pub(crate) struct FetchResult {
    url: String,
    markdown: String,
    /// Non-HTML source literals must bypass Markdown heading interpretation.
    #[serde(skip_serializing)]
    is_plain_text: bool,
    /// Internal flag: surfaced as a `notes` entry in scout's JSON output, not as data.
    #[serde(skip_serializing)]
    used_raw_fallback: bool,
    /// Internal flag: the body could not be decoded cleanly, so the
    /// markdown is a best-effort lossy rendering. Surfaced as `DECODE_UNCERTAIN`
    /// in `degraded_reasons`, not as data.
    #[serde(skip_serializing)]
    decode_uncertain: bool,
}

impl FetchResult {
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn markdown(&self) -> &str {
        &self.markdown
    }

    /// Shift converted HTML headings while preserving non-HTML source syntax and whitespace.
    pub(crate) fn with_heading_offset(&self, offset: usize) -> Cow<'_, str> {
        if self.is_plain_text {
            Cow::Borrowed(self.markdown())
        } else {
            Cow::Owned(shift_headings(self.markdown(), offset))
        }
    }

    pub(crate) fn used_raw_fallback(&self) -> bool {
        self.used_raw_fallback
    }

    pub(crate) fn decode_uncertain(&self) -> bool {
        self.decode_uncertain
    }

    /// Test-only constructor. Production uses the media-specific constructors.
    #[cfg(test)]
    pub(crate) fn for_test(url: String, markdown: String, used_raw_fallback: bool) -> Self {
        Self {
            url,
            markdown,
            is_plain_text: false,
            used_raw_fallback,
            decode_uncertain: false,
        }
    }

    /// Test builder keeps decode uncertainty separate from the raw-fallback flag.
    #[cfg(test)]
    pub(crate) fn with_decode_uncertain(mut self, decode_uncertain: bool) -> Self {
        self.decode_uncertain = decode_uncertain;
        self
    }
}

pub(crate) const RAW_FALLBACK_NOTE: &str =
    "> Note: Readability extraction failed. Showing raw page conversion.\n\n";

pub(crate) const DECODE_UNCERTAIN_NOTE: &str = "> Note: Character encoding could not be determined; the body is a best-effort decode and may be garbled.\n\n";

const RAW_TEXT_TAGS: [&str; 6] = ["script", "style", "textarea", "iframe", "noscript", "title"];

/// Expand self-closed raw-text tags so HTML parsing of XHTML cannot swallow
/// the remaining body into an element that suppression then removes.
/// This does not implement the rest of XML syntax. The byte scan can also
/// rewrite matches in comments or quoted attributes, where they remain inert.
fn close_self_closed_raw_text_tags(html: &str) -> Cow<'_, str> {
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

pub(super) fn to_fetch_result(
    article: &ExtractedArticle,
    url: String,
    decode_uncertain: bool,
) -> FetchResult {
    let content_html = close_self_closed_raw_text_tags(&article.content_html);
    let markdown = html::convert(&content_html);
    let output = format_with_frontmatter(article, &markdown);

    FetchResult {
        url,
        markdown: output,
        is_plain_text: false,
        used_raw_fallback: article.used_raw_fallback,
        decode_uncertain,
    }
}

/// Preserve decoded non-HTML text without interpreting tags, entities or Markdown
/// syntax. It has no HTML metadata or Readability failure, but shares the same
/// YAML boundary defense and the caller's output cap as converted HTML.
pub(crate) fn plain_text_result(text: &str, url: String, decode_uncertain: bool) -> FetchResult {
    FetchResult {
        url,
        markdown: format_body("", text),
        is_plain_text: true,
        used_raw_fallback: false,
        decode_uncertain,
    }
}

/// Always emit frontmatter, including when metadata is absent.
fn format_with_frontmatter(article: &ExtractedArticle, markdown: &str) -> String {
    let mut fields = String::new();

    if let Some(title) = &article.title {
        write_yaml_str(&mut fields, "title", title);
    }
    if let Some(author) = &article.byline {
        write_yaml_str(&mut fields, "author", author);
    }
    if let Some(date) = &article.published_time {
        write_yaml_str(&mut fields, "date", date);
    }

    format_body(&fields, markdown)
}

fn format_body(fields: &str, markdown: &str) -> String {
    // Neutralize untrusted YAML boundaries; only closed fences preserve literal markers.
    let body = neutralize_yaml_markers_outside_fences(markdown);

    let mut fm = String::from("---\n");
    fm.push_str(fields);
    fm.push_str("---\n\n");
    fm.push_str(&body);
    fm
}

#[cfg(test)]
mod fetch_result_tests;
#[cfg(test)]
mod link_tests;
#[cfg(test)]
mod pre_tests;
#[cfg(test)]
mod suppression_tests;
#[cfg(test)]
mod table_tests;
#[cfg(test)]
mod test_helpers;
#[cfg(test)]
mod whitespace_tests;
