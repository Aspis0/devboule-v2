//! Tests for one topic: the text read from an ACP tool call's `content`.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::acp_view::view_from_envelope;

// Element shapes follow `ToolCallContent` and `ContentBlock` in
// agent-client-protocol-schema 1.7.0 (src/v1/tool_call.rs, src/v1/content.rs);
// the crate types are the evidence, no provider capture of these arrays exists.
fn envelope(kind: &str, content: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "s1",
            "update": {
                "sessionUpdate": kind,
                "toolCallId": "tc1",
                "title": "Fetch",
                "content": content
            }
        }
    })
}

fn wrapped(block: Value) -> Value {
    json!({"type": "content", "content": block})
}

fn update_text(content: Value) -> Option<String> {
    let events = view_from_envelope(&envelope("tool_call_update", content), "s1");
    match events.as_slice() {
        [SessionEvent::AgentToolUpdate { text, .. }] => text.clone(),
        other => panic!("expected one tool update, got {other:?}"),
    }
}

#[test]
fn text_array_joins_the_blocks_with_newlines() {
    let content = json!([
        wrapped(json!({"type": "text", "text": "first"})),
        wrapped(json!({"type": "text", "text": "second"}))
    ]);
    assert_eq!(update_text(content).as_deref(), Some("first\nsecond"));
}

#[test]
fn bare_text_block_is_still_read() {
    let content = json!({"type": "text", "text": "plain"});
    assert_eq!(update_text(content).as_deref(), Some("plain"));
}

#[test]
fn resource_link_is_a_line_with_the_title_or_the_name_and_the_uri() {
    let titled = wrapped(json!({
        "type": "resource_link", "name": "readme", "title": "The README",
        "uri": "file:///repo/README.md", "mimeType": "text/markdown"
    }));
    let named = wrapped(json!({
        "type": "resource_link", "name": "readme", "uri": "file:///repo/README.md"
    }));
    assert_eq!(
        update_text(json!([titled])).as_deref(),
        Some("The README: file:///repo/README.md")
    );
    assert_eq!(
        update_text(json!([named])).as_deref(),
        Some("readme: file:///repo/README.md")
    );
}

#[test]
fn embedded_text_resource_is_its_text_and_a_blob_names_the_uri() {
    let text = wrapped(json!({
        "type": "resource",
        "resource": {"uri": "file:///a.txt", "text": "file body"}
    }));
    let blob = wrapped(json!({
        "type": "resource",
        "resource": {"uri": "file:///a.bin", "blob": "QUJDREVGRw=="}
    }));
    assert_eq!(update_text(json!([text])).as_deref(), Some("file body"));
    let shown = update_text(json!([blob])).expect("a line naming the uri");
    assert_eq!(shown, "[binary resource: file:///a.bin]");
    assert!(!shown.contains("QUJD"));
}

#[test]
fn image_and_audio_show_a_placeholder_and_never_the_data() {
    let content = json!([
        wrapped(json!({"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgoAAA"})),
        wrapped(json!({"type": "audio", "mimeType": "audio/wav", "data": "UklGRiQAAABXQVZF"}))
    ]);
    let shown = update_text(content).expect("placeholders");
    assert_eq!(shown, "[image: image/png]\n[audio: audio/wav]");
    assert!(!shown.contains("iVBOR") && !shown.contains("UklGR"));
}

#[test]
fn diff_and_terminal_name_the_path_or_the_terminal() {
    let content = json!([
        {"type": "diff", "path": "src/lib.rs", "oldText": null, "newText": "fn main() {}"},
        {"type": "terminal", "terminalId": "term-7"}
    ]);
    let shown = update_text(content).expect("placeholders");
    assert_eq!(shown, "[diff: src/lib.rs]\n[terminal: term-7]");
    assert!(!shown.contains("fn main"));
}

#[test]
fn mixed_array_keeps_order_and_skips_what_it_cannot_read() {
    let content = json!([
        wrapped(json!({"type": "text", "text": "before"})),
        {"type": "somethingNew", "x": 1},
        null,
        wrapped(json!({"type": "image", "mimeType": "image/jpeg", "data": "/9j/4AAQ"})),
        {"type": "diff", "path": "a.rs", "newText": "x"},
        wrapped(json!({"type": "text", "text": "after"}))
    ]);
    assert_eq!(
        update_text(content).as_deref(),
        Some("before\n[image: image/jpeg]\n[diff: a.rs]\nafter")
    );
}

#[test]
fn empty_or_unreadable_content_adds_no_text() {
    for content in [
        json!([]),
        json!(null),
        json!("text"),
        json!(7),
        json!({}),
        json!([wrapped(json!({"type": "text"}))]),
        json!([{"type": "content"}]),
    ] {
        assert_eq!(update_text(content.clone()), None, "content {content}");
    }
}

#[test]
fn update_without_content_adds_no_text() {
    let mut frame = envelope("tool_call_update", json!([]));
    frame["params"]["update"]
        .as_object_mut()
        .expect("update object")
        .remove("content");
    match view_from_envelope(&frame, "s1").as_slice() {
        [SessionEvent::AgentToolUpdate { text, .. }] => assert_eq!(*text, None),
        other => panic!("expected one tool update, got {other:?}"),
    }
}

#[test]
fn first_tool_call_frame_content_follows_as_a_text_update() {
    let content = json!([wrapped(json!({"type": "text", "text": "starting"}))]);
    let events = view_from_envelope(&envelope("tool_call", content), "s1");
    match events.as_slice() {
        [SessionEvent::AgentToolCall { tool_call_id, .. }, SessionEvent::AgentToolUpdate {
            tool_call_id: update_id,
            status,
            text,
            ..
        }] => {
            assert_eq!(tool_call_id, "tc1");
            assert_eq!(update_id, "tc1");
            assert_eq!(*status, None);
            assert_eq!(text.as_deref(), Some("starting"));
        }
        other => panic!("expected the call then its text, got {other:?}"),
    }
}

#[test]
fn first_tool_call_frame_without_readable_content_is_one_event() {
    let events = view_from_envelope(&envelope("tool_call", json!([])), "s1");
    assert!(matches!(
        events.as_slice(),
        [SessionEvent::AgentToolCall { .. }]
    ));
}
