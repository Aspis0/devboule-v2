//! Tests for one topic: the row summary of pi's web tools.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use super::events_from_line;

// Argument names follow the tool definitions in `@juicesharp/rpiv-web-tools`
// 2.10.1 (`web_search(query, max_results?, provider?)`, `web_fetch(url, raw?)`);
// the call frame is the `toolcall_end` shape the other pi tests carry.
fn toolcall_end(name: &str, arguments: Value) -> Value {
    json!({
        "type": "message_update",
        "assistantMessageEvent": {
            "type": "toolcall_end",
            "contentIndex": 0,
            "toolCall": {"type": "toolCall", "id": "call_1", "name": name, "arguments": arguments}
        }
    })
}

fn row(name: &str, arguments: Value) -> (Option<String>, Option<String>) {
    match events_from_line(&toolcall_end(name, arguments)).as_slice() {
        [SessionEvent::AgentToolUpdate { title, kind, .. }] => (title.clone(), kind.clone()),
        other => panic!("expected one tool update, got {other:?}"),
    }
}

#[test]
fn web_search_is_summarised_by_its_query_and_is_a_search() {
    let (title, kind) = row(
        "web_search",
        json!({"query": "rust 1.90 release", "max_results": 5, "provider": "brave"}),
    );
    assert_eq!(title.as_deref(), Some("rust 1.90 release"));
    assert_eq!(kind.as_deref(), Some("search"));
}

#[test]
fn web_fetch_is_summarised_by_its_url_and_is_a_fetch() {
    let (title, kind) = row(
        "web_fetch",
        json!({"url": "https://blog.rust-lang.org/", "raw": false}),
    );
    assert_eq!(title.as_deref(), Some("https://blog.rust-lang.org/"));
    assert_eq!(kind.as_deref(), Some("fetch"));
}

#[test]
fn web_tools_without_their_argument_keep_the_tool_name() {
    for (name, arguments) in [
        ("web_search", json!({"max_results": 3})),
        ("web_search", json!({"query": ""})),
        ("web_search", json!({"query": 7})),
        ("web_fetch", json!({"raw": true})),
        ("web_fetch", json!(null)),
    ] {
        let (title, _) = row(name, arguments.clone());
        assert_eq!(title, None, "{name} {arguments}");
    }
}

#[test]
fn other_tools_keep_the_command_path_pattern_summary() {
    assert_eq!(
        row(
            "bash",
            json!({"command": "ls -la", "url": "https://x.test"})
        )
        .0,
        Some("ls -la".to_string())
    );
    assert_eq!(
        row("read", json!({"path": "src/a.rs", "query": "q"})).0,
        Some("src/a.rs".to_string())
    );
    assert_eq!(
        row("grep", json!({"pattern": "todo"})).0,
        Some("todo".to_string())
    );
    // A foreign tool's `query` or `url` is not a summary.
    assert_eq!(
        row("custom", json!({"query": "q", "url": "https://x.test"})).0,
        None
    );
}
