//! The text of an ACP tool call's `content`.
//!
//! The schema makes `content` an array of `ToolCallContent` (`content`,
//! `diff`, `terminal`); a bare content block in place of the array is read
//! too, and an element of an unknown type is skipped. Nothing here builds a
//! structured view: every element becomes at most one line of text, and
//! binary payloads (`data`, `blob`) are never copied.
//!
//! Two bounds hold for every extraction. The text is cut at
//! `text_cap::MAX_TEXT_BYTES`, which keeps the mapped event far under the 1 MiB
//! frame cap even when every byte JSON-escapes to six (64 KiB becomes at most
//! 384 KiB). Reading stops at the cut, and a long line is sliced before it is
//! copied.

use std::borrow::Cow;

use devboule_protocol::SessionEvent;
use serde_json::Value;

use crate::text_cap::{self, MAX_TEXT_BYTES, TRUNCATION_MARKER};

/// How much of one field is read: the budget plus one UTF-8 character, so a
/// field longer than the budget still arrives longer than it and the cut is
/// marked.
const MAX_FIELD_BYTES: usize = MAX_TEXT_BYTES + 4;
/// Calls whose last snapshot is remembered at once; the oldest is dropped
/// first, so a call that never completes cannot grow the memory.
const MAX_TRACKED_CALLS: usize = 64;

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(|text| text_cap::clip(text, MAX_FIELD_BYTES))
}

fn media_line(kind: &str, block: &Value) -> String {
    match str_field(block, "mimeType") {
        Some(mime) => format!("[{kind}: {mime}]"),
        None => format!("[{kind}]"),
    }
}

fn resource_link_line(block: &Value) -> Option<Cow<'_, str>> {
    let label = str_field(block, "title").or_else(|| str_field(block, "name"));
    match (label, str_field(block, "uri")) {
        (Some(label), Some(uri)) => Some(Cow::Owned(format!("{label}: {uri}"))),
        (Some(only), None) | (None, Some(only)) => Some(Cow::Borrowed(only)),
        (None, None) => None,
    }
}

fn embedded_resource_line(block: &Value) -> Option<Cow<'_, str>> {
    let resource = block.get("resource")?;
    if let Some(text) = str_field(resource, "text") {
        return Some(Cow::Borrowed(text));
    }
    resource.get("blob").and_then(Value::as_str)?;
    Some(Cow::Owned(match str_field(resource, "uri") {
        Some(uri) => format!("[binary resource: {uri}]"),
        None => "[binary resource]".to_string(),
    }))
}

fn block_line(block: &Value) -> Option<Cow<'_, str>> {
    match block.get("type").and_then(Value::as_str)? {
        "text" => str_field(block, "text").map(Cow::Borrowed),
        "image" => Some(Cow::Owned(media_line("image", block))),
        "audio" => Some(Cow::Owned(media_line("audio", block))),
        "resource_link" => resource_link_line(block),
        "resource" => embedded_resource_line(block),
        _ => None,
    }
}

fn element_line(element: &Value) -> Option<Cow<'_, str>> {
    match element.get("type").and_then(Value::as_str)? {
        "content" => block_line(element.get("content")?),
        "diff" => str_field(element, "path").map(|path| Cow::Owned(format!("[diff: {path}]"))),
        "terminal" => {
            str_field(element, "terminalId").map(|id| Cow::Owned(format!("[terminal: {id}]")))
        }
        _ => block_line(element),
    }
}

/// What one extraction read: the lines joined with newlines, cut at the
/// budget.
#[derive(PartialEq)]
struct Extracted {
    body: String,
    truncated: bool,
}

impl Extracted {
    fn push_line(&mut self, line: &str) {
        let separator = usize::from(!self.body.is_empty());
        let room = MAX_TEXT_BYTES - self.body.len();
        if separator + line.len() <= room {
            if separator == 1 {
                self.body.push('\n');
            }
            self.body.push_str(line);
            return;
        }
        if room > separator {
            if separator == 1 {
                self.body.push('\n');
            }
            self.body.push_str(text_cap::clip(line, room - separator));
        }
        self.truncated = true;
    }

    /// The text a row shows for this snapshot.
    fn shown(&self) -> String {
        text_cap::shown(&self.body, self.truncated)
    }

    /// What a row showing `self` can append so that it shows `next`: the app
    /// rebuilds `shown`, a line feed and `suffix`, so this is `Some` only when that is
    /// byte-exact. A truncated or empty `self` shows more or less than its
    /// body, and a `next` that is only a trailing newline or differs in its
    /// line endings has no suffix to give.
    fn suffix_to(&self, next: &Self) -> Option<String> {
        if self.truncated || self.body.is_empty() {
            return None;
        }
        let added = next
            .body
            .strip_prefix(self.body.as_str())?
            .strip_prefix('\n')?;
        if added.is_empty() {
            return None;
        }
        let mut suffix = added.to_string();
        if next.truncated {
            suffix.push_str(TRUNCATION_MARKER);
        }
        Some(suffix)
    }
}

fn extract(content: &Value) -> Extracted {
    let mut extracted = Extracted {
        body: String::new(),
        truncated: false,
    };
    match content.as_array() {
        Some(elements) => {
            for element in elements {
                if extracted.truncated {
                    break;
                }
                if let Some(line) = element_line(element) {
                    extracted.push_line(&line);
                }
            }
        }
        None => {
            if let Some(line) = element_line(content) {
                extracted.push_line(&line);
            }
        }
    }
    extracted
}

fn is_finished(status: Option<&str>) -> bool {
    matches!(status, Some("completed" | "failed"))
}

/// What one snapshot sends to the row.
pub(crate) struct ToolText {
    /// The whole snapshot when `replace`, else a suffix to append. Empty with
    /// `replace` means the snapshot is empty and clears the row.
    pub(crate) text: String,
    pub(crate) replace: bool,
}

/// The last content snapshot of each open tool call.
///
/// ACP content replaces the call's content, but the app appends every update
/// text to the row. Remembering the snapshot already sent lets a repeat send
/// nothing. A suffix goes out without the flag only when the app's own join
/// (the old text, a line feed and the suffix) rebuilds the new snapshot byte for byte; every other
/// change is sent whole, flagged as a replacement, and so is a snapshot for a
/// call with no memory (first, evicted, or a replay that began mid-call). An
/// empty first snapshot sends nothing: the row has nothing to clear.
#[derive(Default)]
pub(crate) struct ToolContentMemory {
    calls: Vec<(String, Extracted)>,
}

impl ToolContentMemory {
    /// What this snapshot sends to the row, or `None` when it adds nothing.
    /// A finished call is forgotten after its snapshot is read. A `null` or
    /// absent `content` is no snapshot and leaves the memory alone.
    pub(crate) fn new_text(
        &mut self,
        call_id: &str,
        content: Option<&Value>,
        status: Option<&str>,
    ) -> Option<ToolText> {
        let text = content
            .filter(|content| !content.is_null())
            .and_then(|content| self.send_for(call_id, extract(content)));
        if is_finished(status) {
            self.calls.retain(|(known, _)| known != call_id);
        }
        text
    }

    fn send_for(&mut self, call_id: &str, snapshot: Extracted) -> Option<ToolText> {
        let slot = self.calls.iter().position(|(known, _)| known == call_id);
        let change = match slot.map(|index| &self.calls[index].1) {
            Some(old) if *old == snapshot => None,
            Some(old) => Some(match old.suffix_to(&snapshot) {
                Some(suffix) => ToolText {
                    text: suffix,
                    replace: false,
                },
                None => ToolText {
                    text: snapshot.shown(),
                    replace: true,
                },
            }),
            None if snapshot.body.is_empty() => return None,
            None => Some(ToolText {
                text: snapshot.shown(),
                replace: true,
            }),
        };
        match slot {
            Some(index) => self.calls[index].1 = snapshot,
            None => {
                if self.calls.len() == MAX_TRACKED_CALLS {
                    self.calls.remove(0);
                }
                self.calls.push((call_id.to_string(), snapshot));
            }
        }
        change
    }

    #[cfg(test)]
    pub(crate) fn tracked_calls(&self) -> usize {
        self.calls.len()
    }
}

/// The content a first `tool_call` frame carries, as the text update the
/// call event has no field for. Empty when the frame is not a call or its
/// content has no readable text.
pub(crate) fn call_content_update(
    update: &Value,
    events: &[SessionEvent],
    memory: &mut ToolContentMemory,
) -> Option<SessionEvent> {
    let [SessionEvent::AgentToolCall {
        tool_call_id,
        status,
        ..
    }] = events
    else {
        return None;
    };
    let added = memory.new_text(tool_call_id, update.get("content"), Some(status.as_str()))?;
    Some(SessionEvent::AgentToolUpdate {
        tool_call_id: tool_call_id.clone(),
        status: None,
        text: Some(added.text),
        title: None,
        kind: None,
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: None,
        exit_code: None,
        replace: added.replace,
    })
}

#[cfg(test)]
#[path = "acp_tool_content_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "acp_tool_content_replace_tests.rs"]
mod replace_tests;

#[cfg(test)]
#[path = "acp_tool_content_bounds_tests.rs"]
mod bounds_tests;

#[cfg(test)]
#[path = "acp_tool_content_journal_tests.rs"]
mod journal_tests;
