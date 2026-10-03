//! Tests for one topic: a content snapshot replaces the tool's content, so a
//! repeated or grown snapshot must not append itself again.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::acp_tool_content::ToolContentMemory;
use crate::acp_view::view_from_envelope_with;

fn frame(kind: &str, call_id: &str, status: Option<&str>, text: &str) -> Value {
    let mut update = json!({
        "sessionUpdate": kind,
        "toolCallId": call_id,
        "title": "Run",
        "content": [{"type": "content", "content": {"type": "text", "text": text}}]
    });
    if let Some(status) = status {
        update["status"] = json!(status);
    }
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "s1", "update": update}
    })
}

/// The texts the app would append, in order, for a sequence of envelopes read
/// through one memory.
fn emitted_with(memory: &mut ToolContentMemory, frames: &[Value]) -> Vec<String> {
    let mut texts = Vec::new();
    for frame in frames {
        for event in view_from_envelope_with(frame, "s1", None, memory) {
            if let SessionEvent::AgentToolUpdate {
                text: Some(text), ..
            } = event
            {
                texts.push(text);
            }
        }
    }
    texts
}

fn emitted(frames: &[Value]) -> Vec<String> {
    emitted_with(&mut ToolContentMemory::default(), frames)
}

/// The row's output as `updateTool` builds it: each text joined with a newline.
fn row_output(texts: &[String]) -> String {
    texts.join("\n")
}

#[test]
fn fifty_identical_full_content_updates_give_the_content_once() {
    let frames: Vec<Value> = (0..50)
        .map(|_| {
            frame(
                "tool_call_update",
                "tc1",
                Some("in_progress"),
                "the whole output",
            )
        })
        .collect();
    assert_eq!(row_output(&emitted(&frames)), "the whole output");
}

#[test]
fn a_growing_snapshot_gives_only_the_new_suffix() {
    let frames = [
        frame("tool_call_update", "tc1", None, "line1"),
        frame("tool_call_update", "tc1", None, "line1\nline2"),
        frame("tool_call_update", "tc1", None, "line1\nline2"),
        frame("tool_call_update", "tc1", None, "line1\nline2\nline3"),
    ];
    let texts = emitted(&frames);
    assert_eq!(texts, ["line1", "line2", "line3"]);
    assert_eq!(row_output(&texts), "line1\nline2\nline3");
}

#[test]
fn a_replacement_that_is_not_an_extension_still_appends_whole() {
    // The known limit: the row cannot be rewritten without a protocol field.
    let frames = [
        frame("tool_call_update", "tc1", None, "first answer"),
        frame("tool_call_update", "tc1", None, "second answer"),
    ];
    assert_eq!(emitted(&frames), ["first answer", "second answer"]);
}

#[test]
fn calls_do_not_share_snapshots() {
    let frames = [
        frame("tool_call_update", "tc1", None, "same"),
        frame("tool_call_update", "tc2", None, "same"),
    ];
    assert_eq!(emitted(&frames), ["same", "same"]);
}

#[test]
fn the_first_frame_content_is_not_repeated_by_an_identical_update() {
    let frames = [
        frame("tool_call", "tc1", None, "starting"),
        frame("tool_call_update", "tc1", Some("in_progress"), "starting"),
    ];
    assert_eq!(emitted(&frames), ["starting"]);
}

#[test]
fn an_update_without_content_keeps_what_was_remembered() {
    let mut without = frame("tool_call_update", "tc1", Some("in_progress"), "x");
    without["params"]["update"]
        .as_object_mut()
        .expect("update object")
        .remove("content");
    let frames = [
        frame("tool_call_update", "tc1", None, "kept"),
        without,
        frame("tool_call_update", "tc1", None, "kept"),
    ];
    assert_eq!(emitted(&frames), ["kept"]);
}

#[test]
fn a_finished_call_is_forgotten_after_its_last_snapshot() {
    let mut memory = ToolContentMemory::default();
    let running = [frame("tool_call_update", "tc1", Some("in_progress"), "out")];
    assert_eq!(emitted_with(&mut memory, &running), ["out"]);
    assert_eq!(memory.tracked_calls(), 1);

    let done = [frame("tool_call_update", "tc1", Some("completed"), "out")];
    assert!(emitted_with(&mut memory, &done).is_empty());
    assert_eq!(memory.tracked_calls(), 0);

    let failed = [
        frame("tool_call_update", "tc2", Some("in_progress"), "x"),
        frame("tool_call_update", "tc2", Some("failed"), "x y"),
    ];
    assert_eq!(emitted_with(&mut memory, &failed), ["x", " y"]);
    assert_eq!(memory.tracked_calls(), 0);
}

#[test]
fn the_memory_holds_a_bounded_number_of_open_calls() {
    let mut memory = ToolContentMemory::default();
    let frames: Vec<Value> = (0..500)
        .map(|n| frame("tool_call_update", &format!("tc{n}"), None, "open"))
        .collect();
    emitted_with(&mut memory, &frames);
    assert_eq!(memory.tracked_calls(), 64);
}

#[test]
fn replaying_the_journal_gives_the_content_once_as_live_does() {
    use crate::claude_view::{drive_replay, ClaudeView};

    let mut view = ClaudeView::new(None);
    let mut texts = Vec::new();
    for _ in 0..50 {
        let mut envelope = frame("tool_call_update", "tc1", Some("in_progress"), "the output");
        for event in drive_replay(&mut view, &mut envelope) {
            if let SessionEvent::AgentToolUpdate {
                text: Some(text), ..
            } = event
            {
                texts.push(text);
            }
        }
    }
    assert_eq!(texts, ["the output"]);
}
