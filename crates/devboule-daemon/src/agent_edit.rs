//! The file changes one tool call carried, read off the provider's frame.
//!
//! Claude names the text it edited between, Codex sends its own unified diff,
//! ACP sends `diff` content elements; each reader here turns one of those into
//! [`AgentToolEdit`]s. The stored text of one call is budgeted by the JSON it
//! encodes to, not by its raw bytes: a control byte costs six, so the call's
//! frame stays far under the protocol's 1 MiB cap whatever the text holds.

use std::path::Path;

use devboule_protocol::{AgentEditKind, AgentEditText, AgentToolEdit};
use serde_json::Value;

use crate::line_diff::{changed_counts, replace_lines, unified_lines};
use crate::tool_paths::relativize_tool_path;

/// How much text one tool call keeps across all of its edits, counted as the
/// bytes that text takes once JSON-encoded.
pub(crate) const MAX_EDIT_CALL_BYTES: usize = 256 * 1024;

fn text_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The path an edit names, as the wire spells it: inside the workspace it is
/// relative with forward slashes; anywhere else, the provider's own spelling.
fn edit_path(path: &str, cwd: Option<&Path>) -> String {
    let shown = relativize_tool_path(path, cwd);
    if !Path::new(&shown).is_relative() {
        return shown;
    }
    Path::new(&shown)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// One edit with both sides optional, absent sides read as empty. A provider
/// that sent neither text still names the file it changed, so the record
/// keeps the path and carries no text.
fn replace_or_none(
    path: String,
    kind: AgentEditKind,
    old: Option<&str>,
    new: Option<&str>,
) -> AgentToolEdit {
    let text = (old.is_some() || new.is_some()).then(|| AgentEditText::Replace {
        old: old.unwrap_or_default().to_string(),
        new: new.unwrap_or_default().to_string(),
    });
    let (added, removed) = changed_counts(&replace_lines(
        old.unwrap_or_default(),
        new.unwrap_or_default(),
    ));
    AgentToolEdit {
        path,
        kind,
        text,
        truncated: false,
        added,
        removed,
    }
}

/// Claude's file tools: `Edit` (one replace), `MultiEdit` (one replace per
/// entry, all on the named file) and `Write` (the whole file, whose previous
/// text the frame does not carry).
pub(crate) fn claude_tool_edits(
    name: &str,
    input: &Value,
    cwd: Option<&Path>,
) -> Option<Vec<AgentToolEdit>> {
    let path = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(|path| edit_path(path, cwd))?;
    let edits = match name {
        "Edit" => vec![replace_or_none(
            path,
            AgentEditKind::Edit,
            text_field(input, "old_string"),
            text_field(input, "new_string"),
        )],
        "MultiEdit" => input
            .get("edits")
            .and_then(Value::as_array)?
            .iter()
            .map(|edit| {
                replace_or_none(
                    path.clone(),
                    AgentEditKind::Edit,
                    text_field(edit, "old_string"),
                    text_field(edit, "new_string"),
                )
            })
            .collect(),
        "Write" => vec![replace_or_none(
            path,
            AgentEditKind::Write,
            None,
            text_field(input, "content"),
        )],
        _ => return None,
    };
    (!edits.is_empty()).then(|| capped(edits))
}

/// Codex's `fileChange` item: one entry per changed file, each with its own
/// unified diff. A change without a path is no file this daemon can name and
/// is skipped; one without a diff keeps its path and kind.
pub(crate) fn codex_file_change_edits(
    changes: Option<&Value>,
    cwd: Option<&Path>,
) -> Option<Vec<AgentToolEdit>> {
    let changes = changes?.as_array()?;
    let edits: Vec<AgentToolEdit> = changes
        .iter()
        .filter_map(|change| {
            let path = change
                .get("path")
                .and_then(Value::as_str)
                .filter(|path| !path.is_empty())?;
            let kind = match change.get("kind").and_then(Value::as_str) {
                Some("add") => AgentEditKind::Write,
                Some("delete") => AgentEditKind::Delete,
                _ => AgentEditKind::Edit,
            };
            let diff = change
                .get("diff")
                .and_then(Value::as_str)
                .filter(|diff| !diff.is_empty());
            let (added, removed) = changed_counts(&diff.map(unified_lines).unwrap_or_default());
            Some(AgentToolEdit {
                path: edit_path(path, cwd),
                kind,
                text: diff.map(|text| AgentEditText::Diff {
                    text: text.to_string(),
                }),
                truncated: false,
                added,
                removed,
            })
        })
        .collect();
    (!edits.is_empty()).then(|| capped(edits))
}

/// The `diff` elements of an ACP tool call's `content`. The element carries
/// `oldText` and `newText`; whether the call removed the file is the call's
/// own `kind`, since the element itself says only that a file changed.
pub(crate) fn acp_diff_edits(
    content: Option<&Value>,
    tool_kind: Option<&str>,
    cwd: Option<&Path>,
) -> Option<Vec<AgentToolEdit>> {
    let kind = match tool_kind {
        Some("delete") => AgentEditKind::Delete,
        _ => AgentEditKind::Edit,
    };
    let edits: Vec<AgentToolEdit> = content?
        .as_array()?
        .iter()
        .filter(|element| element.get("type").and_then(Value::as_str) == Some("diff"))
        .filter_map(|element| {
            let path = element
                .get("path")
                .and_then(Value::as_str)
                .filter(|path| !path.is_empty())?;
            Some(replace_or_none(
                edit_path(path, cwd),
                kind,
                text_field(element, "oldText"),
                text_field(element, "newText"),
            ))
        })
        .collect();
    (!edits.is_empty()).then(|| capped(edits))
}

/// Cut the texts to the call's budget, in the order the provider sent them.
/// The first text that overflows keeps what fits and is flagged; the ones
/// after it keep nothing, but every edit still names its path and kind.
fn capped(edits: Vec<AgentToolEdit>) -> Vec<AgentToolEdit> {
    let mut remaining = MAX_EDIT_CALL_BYTES;
    edits
        .into_iter()
        .map(|mut edit| {
            let (text, cut) = match edit.text.take() {
                None => (None, false),
                Some(AgentEditText::Replace { old, new }) => {
                    let (old, old_cut) = keep(&mut remaining, old);
                    let (new, new_cut) = keep(&mut remaining, new);
                    (
                        Some(AgentEditText::Replace { old, new }),
                        old_cut || new_cut,
                    )
                }
                Some(AgentEditText::Diff { text }) => {
                    let (text, cut) = keep(&mut remaining, text);
                    (Some(AgentEditText::Diff { text }), cut)
                }
            };
            edit.text = text;
            edit.truncated = cut;
            edit
        })
        .collect()
}

/// One text under the call's remaining budget: what it keeps, and whether it
/// keeps less than it was sent.
fn keep(remaining: &mut usize, text: String) -> (String, bool) {
    let (kept, used) = encoded_prefix(&text, *remaining);
    *remaining -= used;
    (kept.to_string(), kept.len() < text.len())
}

/// The longest prefix of `text` whose JSON string encoding fits in `budget`
/// bytes, cut on a character boundary, with the bytes that prefix takes.
fn encoded_prefix(text: &str, budget: usize) -> (&str, usize) {
    let mut used = 0;
    for (index, ch) in text.char_indices() {
        let size = encoded_len(ch);
        if used + size > budget {
            return (&text[..index], used);
        }
        used += size;
    }
    (text, used)
}

/// The bytes one character takes inside a JSON string: serde_json writes
/// short escapes for these seven, `\u00XX` for other control bytes, and the
/// UTF-8 form of everything else.
fn encoded_len(ch: char) -> usize {
    match ch {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        ch if (ch as u32) < 0x20 => 6,
        ch => ch.len_utf8(),
    }
}

#[cfg(test)]
#[path = "agent_edit_tests.rs"]
mod tests;
