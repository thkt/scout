//! Slack message resolution (mention substitution, author/ts lookup) and the
//! YAML-frontmatter + body rendering of a resolved permalink.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write;
use std::iter::repeat_n;
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use serde::Deserialize;
use tracing::{debug, warn};

use super::{SlackUrl, resolved_display_name, substitute_mentions};
use crate::markdown::{report_parser_input, truncation_note};
use crate::yaml::{neutralize_yaml_markers, write_yaml_str};

#[derive(Deserialize)]
pub(in crate::slack) struct Message {
    pub(in crate::slack) user: Option<String>,
    #[serde(default)]
    pub(in crate::slack) text: String,
    pub(in crate::slack) ts: Option<String>,
    pub(in crate::slack) reply_count: Option<u32>,
}

pub(in crate::slack) struct ResolvedMessage {
    pub(in crate::slack) author: String,
    pub(in crate::slack) text: String,
    pub(in crate::slack) ts: String,
}

pub(in crate::slack) fn resolve_messages(
    messages: &[Message],
    users: &HashMap<String, String>,
) -> Vec<ResolvedMessage> {
    let mut resolved = Vec::with_capacity(messages.len());
    for msg in messages {
        let author = match &msg.user {
            Some(uid) => resolved_display_name(users, uid)
                .map(str::to_owned)
                .unwrap_or_else(|| uid.clone()),
            None => {
                debug!("msg.user is None, falling back to \"(no author)\"");
                "(no author)".into()
            }
        };
        let text = substitute_mentions(&msg.text, users);
        let ts = match &msg.ts {
            Some(t) => t.clone(),
            None => {
                warn!("msg.ts is None, falling back to empty string");
                String::new()
            }
        };
        resolved.push(ResolvedMessage { author, text, ts });
    }
    resolved
}

/// Extract the message matching `target_ts` from `messages`, returning it and
/// the remaining messages in their original order.
///
/// The match is by ts for a channel fetch too, not just within a thread:
/// `conversations.history` is probed with `latest` as an *upper* bound, so a ts
/// that no longer exists answers with the preceding message instead of an empty
/// list. Returning `None` lets the caller report a miss, rather than rendering a
/// neighbour's author and body under the ts the caller asked for.
pub(in crate::slack) fn extract_target(
    mut messages: Vec<ResolvedMessage>,
    target_ts: &str,
) -> Option<(ResolvedMessage, Vec<ResolvedMessage>)> {
    let idx = messages.iter().position(|m| m.ts == target_ts)?;
    let first = messages.remove(idx);
    Some((first, messages))
}

/// Render a resolved Slack permalink as YAML-frontmatter + body, the stable
/// output schema agent consumers parse.
///
/// Frontmatter keys are emitted in a fixed order: `workspace`, `channel`,
/// `author`, `ts`, then `context_messages` only when `replies` is non-empty
/// (omitted, not zero, so a parser feature-detects threads via key presence),
/// then `url`. Every frontmatter value flows through `escape_yaml`, and every
/// body segment through `neutralize_yaml_markers`, so a message whose text
/// contains a line `---` or `key: value` cannot break out of the body and forge
/// frontmatter the consumer would trust (output-injection defense, ADR-0014).
/// Reply blocks are separated by a `---` line and prefix the author (and `ts`
/// when present) before the neutralized text.
pub(in crate::slack) fn format_slack_output(
    slack_url: &SlackUrl,
    channel_name: &str,
    first: &ResolvedMessage,
    replies: &[ResolvedMessage],
) -> String {
    let mut out = String::from("---\n");
    write_yaml_str(&mut out, "workspace", &slack_url.workspace);
    write_yaml_str(&mut out, "channel", channel_name);
    write_yaml_str(&mut out, "author", &first.author);
    write_yaml_str(&mut out, "ts", &slack_url.ts);
    if !replies.is_empty() {
        let _ = writeln!(out, "context_messages: {}", replies.len());
    }
    write_yaml_str(&mut out, "url", &slack_url.raw_url);
    out.push_str("---\n\n");

    out.push_str(&finish_message(&first.text));

    for msg in replies {
        let ts_suffix = if msg.ts.is_empty() {
            String::new()
        } else {
            format!(" ({})", reply_label(&msg.ts))
        };
        out.push_str(&format!(
            "\n\n---\n\n{}{}:\n{}",
            reply_label(&msg.author),
            ts_suffix,
            finish_message(&msg.text)
        ));
    }

    if !out.ends_with('\n') {
        out.push('\n');
    }

    out
}

fn finish_message(text: &str) -> String {
    // Normalize line boundaries before YAML defense so CR cannot expose a marker.
    let text = slack_line_endings(text);
    let mut out = neutralize_yaml_markers(&text);
    let fences = observe_slack_fences(&out);
    out = normalize_closing_tabs(out, &fences.closes);
    if let Some((_, marker, width)) = fences.dangling {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.extend(repeat_n(marker, width));
    }
    out
}

fn slack_line_endings(text: &str) -> Cow<'_, str> {
    let has_lone_cr = text
        .bytes()
        .enumerate()
        .any(|(i, b)| b == b'\r' && text.as_bytes().get(i + 1) != Some(&b'\n'));
    if !has_lone_cr {
        return Cow::Borrowed(text);
    }
    let mut bytes = text.as_bytes().to_vec();
    for i in 0..bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) != Some(&b'\n') {
            bytes[i] = b'\n';
        }
    }
    Cow::Owned(String::from_utf8(bytes).expect("ASCII replacements preserve UTF-8"))
}

struct SlackFences {
    closes: Vec<Range<usize>>,
    dangling: Option<(usize, char, usize)>,
}

/// Observe closes and the dangling opener together on the normalized input.
/// Offsets remain valid when lone CR and closing tabs are emitted as spaces/LF.
fn observe_slack_fences(body: &str) -> SlackFences {
    let parsed = report_parser_input(body);
    let mut fences = SlackFences {
        closes: Vec::new(),
        dangling: None,
    };
    let mut depth = 0;
    for (event, range) in Parser::new(&parsed).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 && matches!(tag, Tag::CodeBlock(CodeBlockKind::Fenced(_))) {
                    let block = &parsed[range.clone()];
                    let opener = block.trim_start_matches(' ');
                    let marker = opener.as_bytes()[0];
                    let width = opener.bytes().take_while(|&b| b == marker).count();
                    let close = block.split_once('\n').and_then(|(_, rest)| {
                        let line = rest.lines().last()?;
                        let indent = line.bytes().take_while(|&b| b == b' ').count();
                        let trimmed = &line[indent..];
                        let close_width = trimmed.bytes().take_while(|&b| b == marker).count();
                        if indent > 3
                            || close_width < width
                            || !trimmed[close_width..].trim_matches([' ', '\r']).is_empty()
                        {
                            return None;
                        }
                        let end = range.end
                            - usize::from(block.ends_with('\n'))
                            - usize::from(block.ends_with("\r\n"));
                        Some(end - line.len()..end)
                    });
                    if let Some(close) = close {
                        if body[close.clone()].contains('\t') {
                            fences.closes.push(close);
                        }
                    } else {
                        let start = parsed[..range.start].rfind('\n').map_or(0, |p| p + 1);
                        fences.dangling = Some((start, char::from(marker), width));
                        break;
                    }
                }
                depth += 1;
            }
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
    fences
}

/// Rewrite observed closing delimiters only, preserving code and prose tabs.
fn normalize_closing_tabs(body: String, closes: &[Range<usize>]) -> String {
    if closes.is_empty() {
        return body;
    }
    let mut bytes = body.into_bytes();
    for range in closes {
        for byte in &mut bytes[range.clone()] {
            if *byte == b'\t' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(bytes).expect("ASCII replacements preserve UTF-8")
}

/// Keep reply labels on one line and prevent delimiter runs from opening code.
fn reply_label(text: &str) -> String {
    let mut out = String::new();
    for c in neutralize_yaml_markers(text).chars() {
        match c {
            '\n' | '\r' => out.push(' '),
            '\\' | '`' | '~' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Complete a cut Slack body before its note. Synthetic delimiters consume the
/// source budget; if an opener and its close cannot fit, omit that block.
/// Notes and degradation preambles retain their existing overhead allowance.
pub(crate) fn truncate_slack_output(source: &str, max_bytes: usize) -> Cow<'_, str> {
    if source.len() <= max_bytes {
        return Cow::Borrowed(source);
    }
    let mut end = slack_cut_boundary(source, max_bytes);
    let fences = loop {
        let fences = observe_slack_fences(&source[..end]);
        let Some((start, _, width)) = fences.dangling else {
            break fences;
        };
        let overhead = width.saturating_add(2);
        if overhead <= max_bytes.saturating_sub(end) {
            break fences;
        }
        let reduced = slack_cut_boundary(source, max_bytes.saturating_sub(overhead));
        end = if reduced <= start { start } else { reduced };
    };
    let mut out = normalize_closing_tabs(
        slack_line_endings(&source[..end]).into_owned(),
        &fences.closes,
    );
    if let Some((_, marker, width)) = fences.dangling {
        out.push('\n');
        out.extend(repeat_n(marker, width));
        out.push('\n');
    }
    out.push_str(&truncation_note(end, source.len()));
    Cow::Owned(out)
}

fn slack_cut_boundary(source: &str, budget: usize) -> usize {
    let boundary = source.floor_char_boundary(budget);
    source[..boundary].rfind('\n').map_or(boundary, |p| p + 1)
}

#[cfg(test)]
mod format_tests;
#[cfg(test)]
mod resolve_messages_tests;
