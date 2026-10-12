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

/// How much one tool call keeps across all of its edits, counted as the JSON
/// they encode to: paths, structure and text together.
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
    capped(edits)
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
    capped(edits)
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
            // A delete leaves nothing behind, whatever `newText` the element sent.
            let new = match kind {
                AgentEditKind::Delete => None,
                _ => text_field(element, "newText"),
            };
            Some(replace_or_none(
                edit_path(path, cwd),
                kind,
                text_field(element, "oldText"),
                new,
            ))
        })
        .collect();
    capped(edits)
}

/// Most edits one call keeps. A call that ran more keeps these and flags the
/// last one, so the list still says the call changed more files.
pub(crate) const MAX_EDITS_PER_CALL: usize = 64;

/// The bytes a `"text":` key takes inside an edit, and the separating comma.
const TEXT_KEY_BYTES: usize = "\"text\":".len() + 1;
/// The brackets of the edits array, and the comma each edit may add to it.
const ARRAY_BYTES: usize = 2;
const COMMA_BYTES: usize = 1;

/// Pi's file tools, read from the arguments its `toolcall_end` carries: `edit`
/// names one `oldText`/`newText` replace per `edits` entry on the file, and
/// `write` names the whole file's `content`.
pub(crate) fn pi_tool_edits(
    name: &str,
    arguments: Option<&Value>,
    cwd: Option<&Path>,
) -> Option<Vec<AgentToolEdit>> {
    let arguments = arguments?;
    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(|path| edit_path(path, cwd))?;
    let edits = match name {
        "edit" => arguments
            .get("edits")
            .and_then(Value::as_array)?
            .iter()
            .map(|entry| {
                replace_or_none(
                    path.clone(),
                    AgentEditKind::Edit,
                    text_field(entry, "oldText"),
                    text_field(entry, "newText"),
                )
            })
            .collect(),
        "write" => vec![replace_or_none(
            path,
            AgentEditKind::Write,
            None,
            text_field(arguments, "content"),
        )],
        _ => return None,
    };
    capped(edits)
}

/// Cut the call's edits to its budget, in the order the provider sent them. The
/// budget is the JSON the edits encode to: an edit's path, kind, counts and
/// structure come off it before its text does. An edit the budget cannot hold
/// is dropped and the last kept edit is flagged truncated; `None` when no edit
/// fits at all.
fn capped(edits: Vec<AgentToolEdit>) -> Option<Vec<AgentToolEdit>> {
    let total = edits.len();
    let mut remaining = MAX_EDIT_CALL_BYTES - ARRAY_BYTES;
    let mut kept: Vec<AgentToolEdit> = Vec::new();
    for mut edit in edits.into_iter().take(MAX_EDITS_PER_CALL) {
        let text = edit.text.take();
        let frame = frame_len(&edit)
            .saturating_add(text.as_ref().map_or(0, text_frame_len))
            .saturating_add(COMMA_BYTES);
        if frame > remaining {
            break;
        }
        remaining -= frame;
        let (text, cut) = match text {
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
        kept.push(edit);
    }
    if kept.len() < total {
        if let Some(last) = kept.last_mut() {
            last.truncated = true;
        }
    }
    (!kept.is_empty()).then_some(kept)
}

/// The bytes an edit takes without its text: path, kind, counts and structure.
fn frame_len(edit: &AgentToolEdit) -> usize {
    serde_json::to_vec(edit).map_or(usize::MAX, |bytes| bytes.len())
}

/// The bytes a text takes inside its edit, key included, with both strings
/// empty: their content is budgeted by `keep`.
fn text_frame_len(text: &AgentEditText) -> usize {
    let empty = match text {
        AgentEditText::Replace { .. } => AgentEditText::Replace {
            old: String::new(),
            new: String::new(),
        },
        AgentEditText::Diff { .. } => AgentEditText::Diff {
            text: String::new(),
        },
    };
    serde_json::to_vec(&empty).map_or(usize::MAX, |bytes| bytes.len() + TEXT_KEY_BYTES)
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
