//! Tests for one topic: the size bound on the text one extraction produces.

use devboule_protocol::{SessionEvent, MAX_FRAME_BYTES};
use serde_json::{json, Value};

use crate::acp_tool_content::ToolContentMemory;
use crate::acp_view::{view_from_envelope, view_from_envelope_with};

const CAP: usize = 64 * 1024;
const MARKER: &str = "\n[output truncated]";

fn update_with(content: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tc1",
                "content": content
            }
        }
    })
}

fn text_of(frame: &Value) -> String {
    match view_from_envelope(frame, "s1").as_slice() {
        [SessionEvent::AgentToolUpdate {
            text: Some(text), ..
        }] => text.clone(),
        other => panic!("expected one update with text, got {other:?}"),
    }
}

fn text_block(text: &str) -> Value {
    json!({"type": "content", "content": {"type": "text", "text": text}})
}

#[test]
fn a_five_megabyte_embedded_resource_is_cut_at_the_cap_with_a_marker() {
    let body = "é".repeat(2_500_000);
    let frame = update_with(json!([{
        "type": "content",
        "content": {"type": "resource", "resource": {"uri": "file:///big.txt", "text": body}}
    }]));
    let text = text_of(&frame);
    assert!(text.len() <= CAP + MARKER.len(), "got {} bytes", text.len());
    assert!(text.ends_with(MARKER));
    assert!(text.starts_with("éé"));
}

#[test]
fn a_hundred_thousand_small_blocks_stop_at_the_cap() {
    let blocks: Vec<Value> = (0..100_000)
        .map(|n| text_block(&format!("line {n}")))
        .collect();
    let text = text_of(&update_with(Value::Array(blocks)));
    assert!(text.len() <= CAP + MARKER.len());
    assert!(text.ends_with(MARKER));
    assert!(text.starts_with("line 0\nline 1\n"));
    assert!(!text.contains("line 99999"));
}

#[test]
fn a_huge_uri_or_title_is_cut_too() {
    let frame = update_with(json!([{
        "type": "content",
        "content": {"type": "resource_link", "name": "n", "uri": "u".repeat(5_000_000)}
    }]));
    let text = text_of(&frame);
    assert!(text.len() <= CAP + MARKER.len(), "got {} bytes", text.len());
}

#[test]
fn text_exactly_at_the_cap_is_not_marked() {
    let text = text_of(&update_with(json!([text_block(&"a".repeat(CAP))])));
    assert_eq!(text.len(), CAP);
    assert!(!text.contains("truncated"));
}

#[test]
fn a_worst_case_escaped_event_stays_far_under_the_frame_cap() {
    let frame = update_with(json!([text_block(&"\u{1}".repeat(5_000_000))]));
    let events = view_from_envelope(&frame, "s1");
    let wire = serde_json::to_string(&events).expect("events serialize");
    assert!(
        wire.len() < MAX_FRAME_BYTES / 2,
        "{} bytes against {MAX_FRAME_BYTES}",
        wire.len()
    );
}

#[test]
fn a_truncated_snapshot_that_only_grows_past_the_cut_adds_nothing() {
    let mut memory = ToolContentMemory::default();
    let mut shown = Vec::new();
    for size in [100_000, 200_000, 300_000] {
        let frame = update_with(json!([text_block(&"a".repeat(size))]));
        for event in view_from_envelope_with(&frame, "s1", None, &mut memory) {
            if let SessionEvent::AgentToolUpdate {
                text: Some(text), ..
            } = event
            {
                shown.push(text);
            }
        }
    }
    assert_eq!(shown.len(), 1);
    assert!(shown[0].ends_with(MARKER));
}
