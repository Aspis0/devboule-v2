//! Tests for one topic: the `mcpToolCall` thread item as a tool row.
//!
//! Item shapes follow `McpToolCallThreadItem` in the schema the installed
//! codex-cli 0.159.0 generates (`codex app-server generate-json-schema`).
//! A live turn against that CLI's own MCP server carried the same fields in the
//! exec protocol's spelling (`mcp_tool_call`, `in_progress`,
//! `structured_content`); every case below is the app-server spelling, which is
//! the wire this daemon reads.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::codex_view::CodexView;

fn notification(method: &str, item: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": {"threadId": "th", "turnId": "tu", "item": item}
    })
}

/// A started item as one call: the id, the row's title, its kind and status.
fn call_of(item: Value) -> (String, String, Option<String>, String) {
    let events = CodexView::new(None).ingest(&notification("item/started", item));
    match events.as_slice() {
        [SessionEvent::AgentToolCall {
            tool_call_id,
            title,
            status,
            kind,
            ..
        }] => (
            tool_call_id.clone(),
            title.clone(),
            kind.clone(),
            status.clone(),
        ),
        other => panic!("expected one tool call, got {other:?}"),
    }
}

/// A completed item as one update: status, output, title and kind.
fn update_of(
    item: Value,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let events = CodexView::new(None).ingest(&notification("item/completed", item));
    match events.as_slice() {
        [SessionEvent::AgentToolUpdate {
            status,
            text,
            title,
            kind,
            ..
        }] => (status.clone(), text.clone(), title.clone(), kind.clone()),
        other => panic!("expected one tool update, got {other:?}"),
    }
}

#[test]
fn a_browser_call_from_the_brokers_server_is_a_browser_row() {
    let item = json!({
        "type": "mcpToolCall", "id": "mcp-1", "server": "devboule",
        "tool": "browser_click", "arguments": {"ref": "e33"},
        "status": "inProgress", "result": null, "error": null
    });
    let (id, title, kind, status) = call_of(item);
    assert_eq!(id, "mcp-1");
    // The browser lane's own line, which the app reads as the row's summary
    // beside the lane's own name.
    assert_eq!(title, "click e33");
    assert_eq!(kind.as_deref(), Some("browser"));
    assert_eq!(status, "in_progress");
}

#[test]
fn a_completed_call_shows_the_result_text() {
    let item = json!({
        "type": "mcpToolCall", "id": "mcp-1", "server": "devboule",
        "tool": "browser_list_tabs", "arguments": {},
        "status": "completed",
        "result": {"content": [{"type": "text", "text": "2 tabs"}]}
    });
    let (status, text, title, kind) = update_of(item);
    assert_eq!(status.as_deref(), Some("completed"));
    assert_eq!(text.as_deref(), Some("2 tabs"));
    assert_eq!(title.as_deref(), Some("list tabs"));
    assert_eq!(kind.as_deref(), Some("browser"));
}

#[test]
fn a_failed_call_shows_its_own_error_words() {
    let item = json!({
        "type": "mcpToolCall", "id": "mcp-2", "server": "devboule",
        "tool": "browser_click", "arguments": {"ref": "e9"},
        "status": "failed", "error": {"message": "no tab has ref e9"}
    });
    let (status, text, _, kind) = update_of(item);
    assert_eq!(status.as_deref(), Some("failed"));
    assert_eq!(text.as_deref(), Some("no tab has ref e9"));
    assert_eq!(kind.as_deref(), Some("browser"));
}

#[test]
fn another_servers_tool_keeps_its_own_name() {
    let item = json!({
        "type": "mcpToolCall", "id": "mcp-3", "server": "probe", "tool": "ping",
        "arguments": {"n": 1}, "status": "inProgress"
    });
    let (_, title, kind, _) = call_of(item);
    // Named as the other provider that serves this server would name it, and
    // so not read as this daemon's own browser lane.
    assert_eq!(title, "mcp__probe__ping");
    assert_eq!(kind.as_deref(), Some("other"));
}

#[test]
fn this_daemons_own_tool_outside_the_browser_lane_keeps_its_name() {
    let item = json!({
        "type": "mcpToolCall", "id": "mcp-4", "server": "devboule",
        "tool": "devboule_send_message", "arguments": {"session": "s.child.1"},
        "status": "inProgress"
    });
    let (_, title, kind, _) = call_of(item);
    assert_eq!(title, "mcp__devboule__devboule_send_message");
    assert_eq!(kind.as_deref(), Some("other"));
}

#[test]
fn an_image_in_the_result_is_not_text() {
    let item = json!({
        "type": "mcpToolCall", "id": "mcp-5", "server": "devboule",
        "tool": "browser_screenshot", "arguments": {},
        "status": "completed",
        "result": {"content": [
            {"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgo="},
            {"type": "text", "text": "image/png 800 px, viewport 800 css px"}
        ]}
    });
    let (_, text, _, _) = update_of(item);
    assert_eq!(
        text.as_deref(),
        Some("image/png 800 px, viewport 800 css px")
    );
}

#[test]
fn an_answer_with_no_text_leaves_the_rows_output_alone() {
    for result in [
        json!(null),
        json!({}),
        json!({"content": []}),
        json!({"content": [{"type": "text", "text": ""}]}),
        json!({"content": [{"type": "image", "mimeType": "image/png", "data": "iVBO"}]}),
    ] {
        let item = json!({
            "type": "mcpToolCall", "id": "mcp-6", "server": "probe", "tool": "pic",
            "arguments": {}, "status": "completed", "result": result
        });
        assert_eq!(update_of(item).1, None, "result {result}");
    }
}

#[test]
fn an_item_with_no_tool_makes_no_row() {
    for item in [
        json!({"type": "mcpToolCall", "id": "mcp-7", "server": "devboule", "status": "inProgress"}),
        json!({"type": "mcpToolCall", "id": "mcp-7", "server": "devboule", "tool": ""}),
    ] {
        for method in ["item/started", "item/completed"] {
            let events = CodexView::new(None).ingest(&notification(method, item.clone()));
            assert!(events.is_empty(), "{method} of {item}");
        }
    }
}

#[test]
fn a_call_with_no_server_name_keeps_the_bare_tool() {
    let item = json!({
        "type": "mcpToolCall", "id": "mcp-9", "tool": "browser_click",
        "arguments": {"ref": "e1"}, "status": "inProgress"
    });
    let (_, title, kind, _) = call_of(item);
    assert_eq!(title, "click e1");
    assert_eq!(kind.as_deref(), Some("browser"));
}

#[test]
fn a_started_item_is_the_call_its_completion_finishes() {
    // One view for both frames: the pair is what the app receives live.
    let mut view = CodexView::new(None);
    let call = view.ingest(&notification(
        "item/started",
        json!({
            "type": "mcpToolCall", "id": "mcp-8", "server": "devboule",
            "tool": "browser_fill", "arguments": {"ref": "e3", "text": "WebView2"},
            "status": "inProgress"
        }),
    ));
    let update = view.ingest(&notification(
        "item/completed",
        json!({
            "type": "mcpToolCall", "id": "mcp-8", "server": "devboule",
            "tool": "browser_fill", "arguments": {"ref": "e3", "text": "WebView2"},
            "status": "completed",
            "result": {"content": [{"type": "text", "text": "filled"}]}
        }),
    ));
    match call.as_slice() {
        [SessionEvent::AgentToolCall {
            tool_call_id,
            title,
            status,
            kind,
            ..
        }] => {
            assert_eq!(tool_call_id, "mcp-8");
            assert_eq!(title, "fill e3 (8 chars)");
            assert_eq!(status, "in_progress");
            assert_eq!(kind.as_deref(), Some("browser"));
        }
        other => panic!("expected the call row, got {other:?}"),
    }
    match update.as_slice() {
        [SessionEvent::AgentToolUpdate {
            tool_call_id,
            status,
            text,
            ..
        }] => {
            assert_eq!(tool_call_id, "mcp-8");
            assert_eq!(status.as_deref(), Some("completed"));
            assert_eq!(text.as_deref(), Some("filled"));
        }
        other => panic!("expected the update row, got {other:?}"),
    }
}
