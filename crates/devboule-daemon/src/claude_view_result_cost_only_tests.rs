//! A result frame that carries a cost but no usable usage object.

use devboule_protocol::SessionEvent;
use serde_json::json;

use crate::claude_view::test_support::view;

#[test]
fn a_result_with_a_cost_but_no_usage_keeps_the_cost() {
    // The running total is the only cost the frame carries: usage absence
    // must not take the turn's billing down with it.
    let result = json!({"type": "result", "subtype": "success", "is_error": false,
        "total_cost_usd": 0.25});
    let events = view().ingest(&result);
    let finishes: Vec<&SessionEvent> = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::AgentFinished { .. }))
        .collect();
    match finishes.as_slice() {
        [SessionEvent::AgentFinished {
            usage: Some(usage), ..
        }] => {
            assert_eq!(usage.cost_usd, Some(0.25));
            assert_eq!(usage.input_tokens, None);
            assert_eq!(usage.output_tokens, None);
        }
        other => panic!("expected one finish carrying the cost, got {other:?}"),
    }
}
