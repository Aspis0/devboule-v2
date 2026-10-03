//! Tests for one topic: a content snapshot replaces the tool's content, so a
//! repeated, grown or rewritten snapshot must leave the row at that snapshot.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::acp_tool_content::{Extracted, ToolContentMemory};
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

/// What the app is sent, in order, for a sequence of envelopes read through one
/// memory: each update's text and whether it replaces the row.
fn updates_with(memory: &mut ToolContentMemory, frames: &[Value]) -> Vec<(String, bool)> {
    let mut updates = Vec::new();
    for frame in frames {
        for event in view_from_envelope_with(frame, "s1", None, memory) {
            if let SessionEvent::AgentToolUpdate {
                text: Some(text),
                replace,
                ..
            } = event
            {
                updates.push((text, replace));
            }
        }
    }
    updates
}

fn emitted_with(memory: &mut ToolContentMemory, frames: &[Value]) -> Vec<String> {
    updates_with(memory, frames)
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

fn emitted(frames: &[Value]) -> Vec<String> {
    emitted_with(&mut ToolContentMemory::default(), frames)
}

/// The row's output as the app builds it: a replacing update sets it (an
/// empty text clears it), any other non-empty text is joined after a line
/// feed.
fn row_output(updates: &[(String, bool)]) -> String {
    let mut output = String::new();
    for (text, replace) in updates {
        if *replace {
            output.clone_from(text);
        } else if !text.is_empty() {
            output = format!("{output}\n{text}");
        }
    }
    output
}

fn content_of(text: &str) -> Value {
    json!([{"type": "content", "content": {"type": "text", "text": text}}])
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
    let updates = updates_with(&mut ToolContentMemory::default(), &frames);
    assert_eq!(updates, [("the whole output".to_string(), true)]);
}

#[test]
fn a_first_snapshot_replaces_the_row() {
    let frames = [frame("tool_call_update", "tc1", None, "first")];
    let updates = updates_with(&mut ToolContentMemory::default(), &frames);
    assert_eq!(updates, [("first".to_string(), true)]);
}

#[test]
fn an_evicted_calls_next_snapshot_replaces_the_row() {
    let mut memory = ToolContentMemory::default();
    let mut frames: Vec<Value> = (0..65)
        .map(|n| frame("tool_call_update", &format!("tc{n}"), None, "open"))
        .collect();
    // tc0 was dropped when the 65th call arrived, so it has no memory left.
    frames.push(frame("tool_call_update", "tc0", None, "open\nmore"));
    let updates = updates_with(&mut memory, &frames);
    assert_eq!(updates.len(), 66);
    assert_eq!(updates[65], ("open\nmore".to_string(), true));
}

#[test]
fn a_growing_snapshot_gives_only_the_new_suffix() {
    let frames = [
        frame("tool_call_update", "tc1", None, "line1"),
        frame("tool_call_update", "tc1", None, "line1\nline2"),
        frame("tool_call_update", "tc1", None, "line1\nline2"),
        frame("tool_call_update", "tc1", None, "line1\nline2\nline3"),
    ];
    let updates = updates_with(&mut ToolContentMemory::default(), &frames);
    let expected = [("line1", true), ("line2", false), ("line3", false)];
    assert_eq!(
        updates,
        expected.map(|(text, replace)| (text.to_string(), replace))
    );
    assert_eq!(row_output(&updates), "line1\nline2\nline3");
}

#[test]
fn growth_in_the_middle_of_a_line_replaces_the_row() {
    let frames = [
        frame("tool_call_update", "tc1", None, "abc"),
        frame("tool_call_update", "tc1", None, "abcdef"),
    ];
    let updates = updates_with(&mut ToolContentMemory::default(), &frames);
    assert_eq!(
        updates,
        [("abc".to_string(), true), ("abcdef".to_string(), true)]
    );
    assert_eq!(row_output(&updates), "abcdef");
}

#[test]
fn a_snapshot_that_is_not_an_extension_replaces_the_row() {
    let frames = [
        frame("tool_call_update", "tc1", None, "first answer"),
        frame("tool_call_update", "tc1", None, "second answer"),
    ];
    let updates = updates_with(&mut ToolContentMemory::default(), &frames);
    assert_eq!(
        updates,
        [
            ("first answer".to_string(), true),
            ("second answer".to_string(), true)
        ]
    );
    assert_eq!(row_output(&updates), "second answer");
}

#[test]
fn a_replacement_after_line_growth_still_ends_at_the_final_snapshot() {
    let frames = [
        frame("tool_call_update", "tc1", None, "one"),
        frame("tool_call_update", "tc1", None, "one\ntwo"),
        frame("tool_call_update", "tc1", None, "one\ntwo\nthr"),
        frame("tool_call_update", "tc1", None, "rewritten"),
    ];
    let updates = updates_with(&mut ToolContentMemory::default(), &frames);
    assert_eq!(row_output(&updates), "rewritten");
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
    assert_eq!(emitted_with(&mut memory, &failed), ["x", "x y"]);
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

#[test]
fn growth_by_only_a_trailing_newline_replaces_the_row() {
    let frames = [
        frame("tool_call_update", "tc1", None, "abc"),
        frame("tool_call_update", "tc1", None, "abc\n"),
    ];
    let updates = updates_with(&mut ToolContentMemory::default(), &frames);
    assert_eq!(
        updates,
        [("abc".to_string(), true), ("abc\n".to_string(), true)]
    );
    assert_eq!(row_output(&updates), "abc\n");
}

#[test]
fn an_empty_snapshot_clears_the_row_once() {
    let frames = [
        frame("tool_call_update", "tc1", None, "abc"),
        frame("tool_call_update", "tc1", None, ""),
        frame("tool_call_update", "tc1", None, ""),
        frame("tool_call_update", "tc1", None, "again"),
    ];
    let updates = updates_with(&mut ToolContentMemory::default(), &frames);
    assert_eq!(
        updates,
        [
            ("abc".to_string(), true),
            (String::new(), true),
            ("again".to_string(), true)
        ]
    );
    assert_eq!(row_output(&updates[..2]), "");
}

#[test]
fn an_empty_content_array_clears_the_row() {
    let mut cleared = frame("tool_call_update", "tc1", None, "x");
    cleared["params"]["update"]["content"] = json!([]);
    let frames = [frame("tool_call_update", "tc1", None, "abc"), cleared];
    let updates = updates_with(&mut ToolContentMemory::default(), &frames);
    assert_eq!(updates, [("abc".to_string(), true), (String::new(), true)]);
}

#[test]
fn an_empty_first_snapshot_sends_nothing() {
    let frames = [frame("tool_call_update", "tc1", None, "")];
    assert!(updates_with(&mut ToolContentMemory::default(), &frames).is_empty());
}

#[test]
fn crlf_growth_replaces_the_row() {
    let frames = [
        frame("tool_call_update", "tc1", None, "a\r\nb"),
        frame("tool_call_update", "tc1", None, "a\r\nb\r\nc"),
        frame("tool_call_update", "tc1", None, "a\r\nb\r\nc\r\n"),
    ];
    let updates = updates_with(&mut ToolContentMemory::default(), &frames);
    assert_eq!(
        updates,
        [
            ("a\r\nb".to_string(), true),
            ("a\r\nb\r\nc".to_string(), true),
            ("a\r\nb\r\nc\r\n".to_string(), true)
        ]
    );
}

#[test]
fn growth_of_a_capped_old_snapshot_replaces_the_row() {
    let mut memory = ToolContentMemory::default();
    memory.calls.push((
        "tc1".to_string(),
        Extracted {
            body: "abc".to_string(),
            truncated: true,
        },
    ));
    let added = memory
        .new_text("tc1", Some(&content_of("abc\ndef")), None)
        .expect("a changed snapshot");
    assert_eq!((added.text.as_str(), added.replace), ("abc\ndef", true));
}

#[test]
fn a_remembered_snapshot_grown_by_a_line_sends_the_suffix_only() {
    let mut memory = ToolContentMemory::default();
    memory.new_text("tc1", Some(&content_of("a\nb")), None);
    let added = memory
        .new_text("tc1", Some(&content_of("a\nb\nc")), None)
        .expect("a changed snapshot");
    assert_eq!((added.text.as_str(), added.replace), ("c", false));
}

/// Whatever the sequence, the row the app rebuilds ends at the last snapshot.
#[test]
fn the_row_always_ends_at_the_last_snapshot() {
    let sequences: [&[&str]; 8] = [
        &["a", "a\nb", "a\nb\nc"],
        &["a", "ab", "abc\n", "abc\n\nd"],
        &["abc", "", "abc", "abc\ndef", ""],
        &["a\r\nb", "a\r\nb\r\nc", "a\r\nb\r\nc\n", "z"],
        &["", "", "x", "x\n\ny", "x\n\ny\n", "x\n\ny\nz"],
        &["one\ntwo", "one", "one\ntwo", "ONE\ntwo"],
        &["\nlead", "\nlead\nmore", "lead"],
        &["a\nb", "a\nb", "a\nb\n", "a\nb\n\n", "a\nb\n\nc"],
    ];
    for snapshots in sequences {
        let frames: Vec<Value> = snapshots
            .iter()
            .map(|text| frame("tool_call_update", "tc1", None, text))
            .collect();
        let updates = updates_with(&mut ToolContentMemory::default(), &frames);
        let last = snapshots.last().expect("a sequence");
        assert_eq!(row_output(&updates), *last, "sequence {snapshots:?}");
    }
}
