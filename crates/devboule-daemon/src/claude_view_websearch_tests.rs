//! Tests for one topic: a WebSearch call and its string-valued result.

use devboule_protocol::SessionEvent;
use serde_json::Value;

use crate::claude_view::test_support::view;

// fixtures/wire/claude-websearch.jsonl: the WebSearch call frame and its
// result frame from a stream-json capture (CLI 2.1.284), sanitized.
const WEBSEARCH_CAPTURE: &str = include_str!("../fixtures/wire/claude-websearch.jsonl");

fn frames() -> Vec<Value> {
    WEBSEARCH_CAPTURE
        .lines()
        .map(|line| serde_json::from_str(line).expect("fixture line is JSON"))
        .collect()
}

#[test]
fn websearch_call_carries_the_query_and_the_search_kind() {
    let mut mapper = view();
    let events = mapper.ingest(&frames()[0]);
    match events.as_slice() {
        [SessionEvent::AgentToolCall {
            tool_call_id,
            title,
            kind,
            ..
        }] => {
            assert_eq!(tool_call_id, "toolu_019icr3BpkEw26Y2EnQgV7Ys");
            assert_eq!(title, "Rust 1.90 release date");
            assert_eq!(kind.as_deref(), Some("search"));
        }
        other => panic!("expected the WebSearch call, got {other:?}"),
    }
}

#[test]
fn websearch_result_string_reaches_the_update_text() {
    let mut mapper = view();
    mapper.ingest(&frames()[0]);
    let events = mapper.ingest(&frames()[1]);
    match events.as_slice() {
        [SessionEvent::AgentToolUpdate {
            tool_call_id,
            status,
            text,
            ..
        }] => {
            assert_eq!(tool_call_id, "toolu_019icr3BpkEw26Y2EnQgV7Ys");
            assert_eq!(status.as_deref(), Some("completed"));
            let text = text.as_deref().expect("the result text is kept");
            assert!(text.starts_with("Web search results for query: \"Rust 1.90 release date\""));
            assert!(text.contains("Links: [{\"title\":"));
        }
        other => panic!("expected the WebSearch result update, got {other:?}"),
    }
}
