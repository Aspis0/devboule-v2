//! Tests for one topic: the `webSearch` thread item as a tool row.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::codex_view::CodexView;

// Item shapes follow `WebSearchThreadItem` and `WebSearchAction` in
// scout/captures/codex-schema/codex_app_server_protocol.v2.schemas.json
// (codex-cli 0.159.0); the schema is the only evidence, no live frame exists.
fn notification(method: &str, item: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": {"threadId": "th", "turnId": "tu", "item": item}
    })
}

fn call_of(item: Value) -> (String, String, Option<String>) {
    let events = CodexView::new(None).ingest(&notification("item/started", item));
    match events.as_slice() {
        [SessionEvent::AgentToolCall {
            tool_call_id,
            title,
            status,
            kind,
            ..
        }] => {
            assert_eq!(status, "in_progress");
            (tool_call_id.clone(), title.clone(), kind.clone())
        }
        other => panic!("expected one tool call, got {other:?}"),
    }
}

type Update = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn update_of(item: Value) -> Update {
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
fn search_action_is_a_search_row_titled_with_the_query() {
    let item = json!({
        "type": "webSearch", "id": "ws1", "query": "rust 1.90 release",
        "action": {"type": "search", "query": "rust 1.90 release", "queries": ["a", "b"]}
    });
    let (id, title, kind) = call_of(item);
    assert_eq!(id, "ws1");
    assert_eq!(title, "rust 1.90 release");
    assert_eq!(kind.as_deref(), Some("search"));
}

#[test]
fn null_action_is_a_search_row_titled_with_the_query() {
    let item = json!({"type": "webSearch", "id": "ws2", "query": "tauri updater", "action": null});
    let (_, title, kind) = call_of(item);
    assert_eq!(title, "tauri updater");
    assert_eq!(kind.as_deref(), Some("search"));
}

#[test]
fn absent_action_is_a_search_row_titled_with_the_query() {
    let (_, title, kind) = call_of(json!({"type": "webSearch", "id": "ws3", "query": "q"}));
    assert_eq!(title, "q");
    assert_eq!(kind.as_deref(), Some("search"));
}

#[test]
fn open_page_is_a_fetch_row_titled_with_the_url() {
    let item = json!({
        "type": "webSearch", "id": "ws4", "query": "rust blog",
        "action": {"type": "openPage", "url": "https://blog.rust-lang.org/"}
    });
    let (_, title, kind) = call_of(item);
    assert_eq!(title, "https://blog.rust-lang.org/");
    assert_eq!(kind.as_deref(), Some("fetch"));
}

#[test]
fn open_page_without_a_url_falls_back_to_the_search_query() {
    let item = json!({
        "type": "webSearch", "id": "ws5", "query": "rust blog",
        "action": {"type": "openPage", "url": null}
    });
    let (_, title, kind) = call_of(item);
    assert_eq!(title, "rust blog");
    assert_eq!(kind.as_deref(), Some("search"));
}

#[test]
fn find_in_page_is_titled_with_the_url() {
    let item = json!({
        "type": "webSearch", "id": "ws6", "query": "edition",
        "action": {"type": "findInPage", "url": "https://doc.rust-lang.org/", "pattern": "edition"}
    });
    let (_, title, kind) = call_of(item);
    assert_eq!(title, "https://doc.rust-lang.org/");
    assert_eq!(kind.as_deref(), Some("fetch"));
}

#[test]
fn unknown_and_other_actions_fall_back_to_a_search_on_the_top_level_query() {
    for action in [
        json!({"type": "other"}),
        json!({"type": "somethingNew", "url": "https://example.com"}),
        json!({"no": "type"}),
        json!("not an object"),
    ] {
        let item = json!({"type": "webSearch", "id": "ws7", "query": "fallback", "action": action});
        let (_, title, kind) = call_of(item);
        assert_eq!(title, "fallback", "action {action}");
        assert_eq!(kind.as_deref(), Some("search"), "action {action}");
    }
}

#[test]
fn completed_search_counts_the_results_and_never_dumps_them() {
    let item = json!({
        "type": "webSearch", "id": "ws1", "query": "rust",
        "action": {"type": "search", "query": "rust", "queries": null},
        "results": [{"opaque": "secret-url"}, "x", 3]
    });
    let (status, text, title, kind) = update_of(item);
    assert_eq!(status.as_deref(), Some("completed"));
    assert_eq!(text.as_deref(), Some("3 results"));
    assert_eq!(title.as_deref(), Some("rust"));
    assert_eq!(kind.as_deref(), Some("search"));
}

#[test]
fn completed_with_one_result_says_so_in_the_singular() {
    let item = json!({"type": "webSearch", "id": "ws1", "query": "q", "results": [{}]});
    assert_eq!(update_of(item).1.as_deref(), Some("1 result"));
}

#[test]
fn completed_open_page_updates_the_row_to_the_fetch_url() {
    let item = json!({
        "type": "webSearch", "id": "ws4", "query": "rust blog",
        "action": {"type": "openPage", "url": "https://blog.rust-lang.org/"}
    });
    let (status, text, title, kind) = update_of(item);
    assert_eq!(status.as_deref(), Some("completed"));
    assert_eq!(text, None);
    assert_eq!(title.as_deref(), Some("https://blog.rust-lang.org/"));
    assert_eq!(kind.as_deref(), Some("fetch"));
}

#[test]
fn completed_with_null_action_leaves_the_call_title_and_kind_alone() {
    let item = json!({"type": "webSearch", "id": "ws2", "query": "q", "action": null});
    let (_, text, title, kind) = update_of(item);
    assert_eq!((text, title, kind), (None, None, None));
}

#[test]
fn unusable_results_add_no_text() {
    for results in [json!(null), json!([]), json!("text"), json!({"a": 1})] {
        let item = json!({"type": "webSearch", "id": "ws1", "query": "q", "results": results});
        assert_eq!(update_of(item).1, None, "results {results}");
    }
}

#[test]
fn an_item_without_an_id_makes_no_row() {
    let events = CodexView::new(None).ingest(&notification(
        "item/started",
        json!({"type": "webSearch", "query": "q"}),
    ));
    assert!(events.is_empty());
}
