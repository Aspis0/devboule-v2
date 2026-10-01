//! Tests for one topic: the replay road — `drive_replay` honours the
//! withheld-finish marker.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::claude_view::{drive_replay, ClaudeView, WITHHELD_FINISH_MARKER_TYPE};

#[test]
fn replay_honours_the_withheld_finish_marker() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/claude-aborted-result.json"))
            .expect("fixture");
    let mut mapper = ClaudeView::new(None);
    let mut marker = json!({"type": WITHHELD_FINISH_MARKER_TYPE});
    assert!(drive_replay(&mut mapper, &mut marker).is_empty());
    let mut frame = fixture.clone();
    assert!(!drive_replay(&mut mapper, &mut frame)
        .iter()
        .any(|event| matches!(event, SessionEvent::AgentFinished { .. })));
    let mut ordinary = json!({"type": "result", "stop_reason": "end_turn"});
    assert!(drive_replay(&mut mapper, &mut ordinary)
        .iter()
        .any(|event| matches!(event, SessionEvent::AgentFinished { .. })));
}
