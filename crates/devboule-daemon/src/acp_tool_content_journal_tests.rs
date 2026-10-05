//! Tests for one topic: the `replace` flag of a tool update survives the
//! journal, so a rebuilt row ends where the live one did.

use devboule_protocol::{SessionEvent, SessionKind};
use serde_json::{json, Value};

use crate::acp_tool_content::ToolContentMemory;
use crate::acp_view::view_from_envelope_with;
use crate::journal::{
    acp_envelope_record, agent_report_record, new_session_record, tmp_journal, Journal,
};

fn update_frame(text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "s1", "update": {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc1",
            "status": "in_progress",
            "content": [{"type": "content", "content": {"type": "text", "text": text}}]
        }}
    })
}

fn tool_updates(events: &[SessionEvent]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentToolUpdate {
                text: Some(text),
                replace,
                ..
            } => Some((text.clone(), *replace)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_rebuilt_acp_row_carries_the_same_replacements_as_the_live_one() {
    let frames = [
        update_frame("abc"),
        update_frame("abcdef"),
        update_frame("abcdef\nnext"),
        update_frame("rewritten"),
        update_frame(""),
    ];
    let mut memory = ToolContentMemory::default();
    let live: Vec<SessionEvent> = frames
        .iter()
        .flat_map(|frame| view_from_envelope_with(frame, "s1", None, &mut memory))
        .collect();

    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let id = "s.acp.replace";
    journal
        .create_session(new_session_record(
            id,
            "owner",
            None,
            SessionKind::Acp,
            "Acp",
        ))
        .expect("birth");
    for (index, frame) in frames.iter().enumerate() {
        let record = acp_envelope_record(id, 1, index as u64 + 1, frame).expect("record");
        journal.append_blocking(record).expect("append");
    }
    let replayed = journal.replay(id).expect("replay");

    // The last snapshot is empty: it clears the row, and no stage on the way
    // to the journal or back drops the empty text.
    let expected = [
        ("abc", true),
        ("abcdef", true),
        ("next", false),
        ("rewritten", true),
        ("", true),
    ];
    assert_eq!(
        tool_updates(&live),
        expected.map(|(text, replace)| (text.to_string(), replace))
    );
    assert_eq!(tool_updates(&replayed.events), tool_updates(&live));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_replay_that_starts_mid_call_replaces_the_row() {
    use crate::claude_view::{drive_replay, ClaudeView};

    let mut view = ClaudeView::new(None);
    // The earlier snapshots are before the replay's start, so the view has no
    // memory of the call: a snapshot that would be a suffix live is sent whole.
    let mut frame = update_frame("abc\ndef");
    let events = drive_replay(&mut view, &mut frame);
    assert_eq!(tool_updates(&events), [("abc\ndef".to_string(), true)]);

    let mut next = update_frame("abc\ndef\nghi");
    let events = drive_replay(&mut view, &mut next);
    assert_eq!(tool_updates(&events), [("ghi".to_string(), false)]);
}

#[test]
fn a_journaled_update_report_keeps_replace() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let id = "s.acp.report";
    journal
        .create_session(new_session_record(
            id,
            "owner",
            None,
            SessionKind::Acp,
            "Acp",
        ))
        .expect("birth");
    for (seq, replace) in [(1, true), (2, false)] {
        let event = SessionEvent::AgentToolUpdate {
            tool_call_id: "tc1".to_string(),
            status: None,
            text: Some(format!("body {seq}")),
            title: None,
            kind: None,
            locations: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
            replace,

            images: Vec::new(),
        };
        let record = agent_report_record(id, 1, seq, &event).expect("record");
        journal.append_blocking(record).expect("append");
    }
    let replayed = journal.replay(id).expect("replay");
    assert_eq!(
        tool_updates(&replayed.events),
        [("body 1".to_string(), true), ("body 2".to_string(), false)]
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
