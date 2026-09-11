//! Shared JSON-wire helpers for the provider view adapters.
//!
//! The Claude (`claude_view`) and Pi (`pi_view`) adapters grew the same small
//! helpers independently: joining text from `{text}` blocks, mapping a tool
//! name to a display kind, and mapping an error flag to a status string. They
//! live here so both callers share one implementation.
//!
//! `tool_kind_from_name` is the union of the two old name tables, matched
//! case-insensitively, and that union is a deliberate widening, not a pure
//! move. Every name the two old tables listed maps as before, but names
//! outside them are now shared across providers: the old Claude table was
//! case-sensitive (lowercase `read`, `grep`, `task`, … fell to `other`, as
//! did the Pi-only `find`/`ls`), and the old Pi table knew nothing of
//! `glob`, `websearch`, `notebookedit`, `webfetch`, `agent`, or `task`.
//! Those names now resolve to the same kind on both wires, so an MCP tool
//! named like a built-in gets the same icon whichever provider serves it.
//! The `tool_kind_widening_is_deliberate` test pins this; do not "fix" it
//! back to two tables without renaming that test first.

use serde_json::Value;

/// Join text from a content value: a bare string passes through, an array of
/// `{text}` blocks concatenates, anything else is empty.
///
/// This is the exact body the two adapters carried separately (`claude_view`
/// `tool_result_text`, `pi_view` `tool_result_text` minus its parent-object
/// lookup and empty filter, which stay at the Pi call site so its
/// missing/empty/non-array cases still yield `None`).
pub(crate) fn blocks_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(blocks) = content.as_array() {
        return blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

/// Case-insensitive tool name to display kind: the union of the Claude table
/// (`Read`, `Edit`/`Write`/`NotebookEdit`, `Bash`/`PowerShell`, `Glob`/`Grep`,
/// `WebFetch`, `WebSearch`, `Agent`/`Task`, `Skill`/other) and the Pi table
/// (`bash`/`powershell`, `read`, `edit`/`write`, `grep`/`find`/`ls`, other).
pub(crate) fn tool_kind_from_name(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "read" => "read",
        "edit" | "write" | "notebookedit" => "edit",
        "bash" | "powershell" => "execute",
        "glob" | "grep" | "websearch" | "find" | "ls" => "search",
        "webfetch" => "fetch",
        "agent" | "task" => "think",
        _ => "other",
    }
}

/// Error flag to tool status string: the ternary both adapters carried inline.
pub(crate) fn tool_status(is_error: bool) -> &'static str {
    if is_error {
        "failed"
    } else {
        "completed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blocks_text_passes_strings_and_joins_blocks() {
        assert_eq!(blocks_text(&json!("hello")), "hello");
        assert_eq!(
            blocks_text(&json!([{"type": "text", "text": "a"}, {"type": "text", "text": "b"}])),
            "ab"
        );
        assert_eq!(blocks_text(&json!([])), "");
        assert_eq!(blocks_text(&json!({})), "");
        assert_eq!(blocks_text(&json!(null)), "");
    }

    #[test]
    fn tool_kind_covers_every_name_both_callers_mapped_before() {
        // From `claude_view::tool_kind` (case-sensitive there, so both the
        // original spelling and its lowercase must land the same here).
        let claude_cases = [
            ("Read", "read"),
            ("Edit", "edit"),
            ("Write", "edit"),
            ("NotebookEdit", "edit"),
            ("Bash", "execute"),
            ("PowerShell", "execute"),
            ("Glob", "search"),
            ("Grep", "search"),
            ("WebFetch", "fetch"),
            ("WebSearch", "search"),
            ("Agent", "think"),
            ("Task", "think"),
            ("Skill", "other"),
        ];
        // From `pi_view::tool_kind` (already lowercase there).
        let pi_cases = [
            ("bash", "execute"),
            ("powershell", "execute"),
            ("read", "read"),
            ("edit", "edit"),
            ("write", "edit"),
            ("grep", "search"),
            ("find", "search"),
            ("ls", "search"),
        ];
        for (name, expected) in claude_cases.into_iter().chain(pi_cases) {
            assert_eq!(tool_kind_from_name(name), expected, "tool {name}");
            assert_eq!(
                tool_kind_from_name(&name.to_ascii_lowercase()),
                expected,
                "tool {name} (lowercase)"
            );
        }
        for name in ["mcp__probe__ping", "custom_tool"] {
            assert_eq!(tool_kind_from_name(name), "other", "tool {name}");
        }
    }

    #[test]
    fn tool_kind_widening_is_deliberate() {
        // Names neither old table mapped now resolve through the shared
        // union: lowercase built-in spellings on the Claude wire (`read`,
        // `grep`, `task` were `other` under its case-sensitive table), and
        // Claude-table names on the Pi wire (`glob`, `websearch`,
        // `notebookedit`, `webfetch`, `agent`, `task` were `other` there).
        // An MCP tool named like a built-in gets the same kind on both.
        let widened = [
            ("read", "read"),
            ("grep", "search"),
            ("task", "think"),
            ("glob", "search"),
            ("websearch", "search"),
            ("webfetch", "fetch"),
            ("agent", "think"),
            ("notebookedit", "edit"),
        ];
        for (name, expected) in widened {
            assert_eq!(tool_kind_from_name(name), expected, "tool {name}");
        }
    }

    #[test]
    fn tool_status_maps_the_error_flag() {
        assert_eq!(tool_status(true), "failed");
        assert_eq!(tool_status(false), "completed");
    }
}
