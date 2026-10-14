//! The browser lane under Claude's own permission frame.
//!
//! The CLI reads no consent table: it asks about every act through
//! `can_use_tool`, and the daemon's browser card behind `tools/call` is the
//! same question one road further on. These tests hold the CLI to the second
//! half of that: a browser tool is answered here and never carded, whether or
//! not the daemon's gate is open, while every other tool keeps its own card.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::super::super::{claude_permission_sender_with_writer, ClaudeFrameWriter};
use super::super::{attached, drain, test_reader};
use super::*;

use crate::mcp_broker::McpBroker;

use serde_json::Value;

/// One `can_use_tool` frame for the tool a browser call arrives as: the CLI
/// qualifies it with the MCP server it came from.
fn tool_frame(tool: &str) -> Value {
    serde_json::json!({
        "type": "control_request",
        "request_id": "browser-1",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": tool,
            "display_name": tool,
            "input": {"browserId": "tab-1", "ref": "e3"},
            "tool_use_id": "toolu_browser"
        }
    })
}

/// What one frame produced: the events the session published and the control
/// frame written back to the CLI.
type Answered = (Vec<SessionEvent>, Vec<Value>);

/// Feed `tool` with the daemon's browser gate open or shut, and collect what
/// the session showed the person and what the CLI was told.
fn answer_to_person(tool: &str, gate_open: bool) -> Answered {
    let written: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&written);
    let writer: ClaudeFrameWriter = Arc::new(move |bytes: &[u8]| {
        sink.lock()
            .expect("frames")
            .push(serde_json::from_slice(bytes).unwrap_or(Value::Null));
        Ok(())
    });
    let controls = Arc::new(Mutex::new(HashMap::new()));
    let broker = PermissionBroker::for_test(claude_permission_sender_with_writer(
        Arc::clone(&controls),
        writer,
    ));
    let mut reader = test_reader(Arc::clone(&broker), Arc::clone(&controls));
    let (runtime, conn) = attached(&broker);

    let gate = McpBroker::new(&crate::test_dirs::test_temp_dir(&format!(
        "claude-browser-gate-{tool}-{gate_open}"
    )))
    .expect("gate broker");
    if gate_open {
        gate.open_browser_gate_for_test(&runtime.session_id);
    }

    reader
        .feed(format!("{}\n", tool_frame(tool)).as_bytes(), &runtime)
        .expect("feed the permission frame");
    drop(reader);
    let events = drain(&conn);
    let frames = written.lock().expect("frames").clone();
    (events, frames)
}

/// Every card the session showed the person, whatever else it published.
fn cards(events: &[SessionEvent]) -> Vec<&SessionEvent> {
    events
        .iter()
        .filter(|event| matches!(event, SessionEvent::PermissionRequest { .. }))
        .collect()
}

/// The behaviour the CLI was told, for the one frame written back.
fn behaviour(frames: &[Value]) -> &str {
    assert_eq!(
        frames.len(),
        1,
        "one control response, written without a person: {frames:?}"
    );
    frames[0]
        .pointer("/response/response/behavior")
        .and_then(Value::as_str)
        .expect("a behaviour in the response frame")
}

/// A browser act is answered here and never carded, with the gate open and
/// with it shut: either way the question a person sees is the daemon's own.
#[test]
fn a_browser_tool_is_allowed_without_a_card_in_both_gate_states() {
    for gate_open in [true, false] {
        let (events, frames) = answer_to_person("mcp__devboule__browser_click", gate_open);
        assert!(
            cards(&events).is_empty(),
            "gate open {gate_open}: the CLI's browser question is never a card: {events:?}"
        );
        assert_eq!(
            behaviour(&frames),
            "allow",
            "gate open {gate_open}: the act proceeds to the daemon's gate"
        );
    }
}

/// The whole lane answers this way, including the batch that runs ten acts in
/// one call — the tool whose card-per-act would have been the worst of the lot.
#[test]
fn every_browser_act_tool_is_answered_the_same_way() {
    for tool in [
        "mcp__devboule__browser_new_tab",
        "mcp__devboule__browser_act",
        "mcp__devboule__browser_type",
        "mcp__devboule__browser_resize",
    ] {
        let (events, frames) = answer_to_person(tool, true);
        assert!(cards(&events).is_empty(), "{tool} was carded: {events:?}");
        assert_eq!(behaviour(&frames), "allow", "{tool} was not allowed");
    }
}

/// Every other tool keeps the card it has always had: the rule is the browser
/// lane and nothing wider.
#[test]
fn a_tool_outside_the_browser_lane_is_still_carded() {
    let (events, frames) = answer_to_person("mcp__devboule__devboule_create_workspace", true);
    assert_eq!(
        cards(&events).len(),
        1,
        "a workspace is still the person's decision: {events:?}"
    );
    assert!(
        frames.is_empty(),
        "nothing goes back to the CLI before a person answers: {frames:?}"
    );
}
