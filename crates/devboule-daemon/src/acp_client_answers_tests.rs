//! Tests that every grok question id gets an answer: pending cards at
//! session close, and requests that arrive when the broker already refuses
//! new cards. Each test reads the exact frame off the capturing sender —
//! deleting the write fails the test on timeout instead of passing silently.

use devboule_protocol::SessionEvent;

use super::question_support::{enveloped, has_notice, live_turn, Harness, SESSION};

#[test]
fn pending_question_answered_on_close() {
    let harness = Harness::new();
    live_turn(&harness.reader);
    harness.dispatch(&enveloped(0));
    let _ = harness.conn.pull_events();
    assert_eq!(harness.broker.pending_len(), 1);
    harness.broker.close();
    let captured = harness.captured.lock().expect("captured");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0],
        serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "result": { "outcome": "cancelled" },
        })
    );
}

#[test]
fn closed_broker_answers_at_once() {
    let harness = Harness::new();
    live_turn(&harness.reader);
    harness.broker.close();
    harness.dispatch(&enveloped(0));
    assert_eq!(harness.broker.pending_len(), 0);
    assert!(!harness
        .conn
        .pull_events()
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })));
    let captured = harness.captured.lock().expect("captured");
    assert_eq!(captured.len(), 1, "the id is answered, not dropped");
    assert_eq!(
        captured[0]["result"],
        serde_json::json!({ "outcome": "cancelled" })
    );
}

#[test]
fn duplicate_tool_call_id_answers_at_once() {
    let harness = Harness::new();
    live_turn(&harness.reader);
    harness.dispatch(&enveloped(0));
    let _ = harness.conn.pull_events();
    let second = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "_x.ai/ask_user_question",
        "params": {
            "sessionId": SESSION,
            "toolCallId": "call-fence-0",
            "questions": [{"question": "Another?"}],
            "mode": "default",
        },
    });
    harness.dispatch(&second);
    assert_eq!(harness.broker.pending_len(), 1, "no second card");
    let cards = harness
        .conn
        .pull_events()
        .into_iter()
        .filter(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. }))
        .count();
    assert_eq!(cards, 0, "the repeat never becomes a card");
    let captured = harness.captured.lock().expect("captured");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0],
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": { "outcome": "cancelled" },
        })
    );
}

#[test]
fn unparseable_frame_answers_at_once() {
    // Nothing the person could answer: the id is answered now, not carded.
    let harness = Harness::new();
    live_turn(&harness.reader);
    harness.dispatch(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "_x.ai/ask_user_question",
        "params": {
            "sessionId": SESSION,
            "toolCallId": "call-empty-3",
            "questions": [{}],
            "mode": "default",
        },
    }));
    assert_eq!(harness.broker.pending_len(), 0);
    assert!(!harness
        .conn
        .pull_events()
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })));
    let captured = harness.captured.lock().expect("captured");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0]["result"],
        serde_json::json!({ "outcome": "cancelled" })
    );
}

#[test]
fn foreign_session_question_answers_at_once() {
    let harness = Harness::new();
    live_turn(&harness.reader);
    let mut frame = enveloped(0);
    frame["params"]["sessionId"] = serde_json::json!("another-session");
    harness.dispatch(&frame);
    assert_eq!(harness.broker.pending_len(), 0);
    assert!(has_notice(&harness.conn.pull_events()));
    let captured = harness.captured.lock().expect("captured");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0]["result"],
        serde_json::json!({ "outcome": "cancelled" })
    );
}

#[test]
fn question_after_turn_end_answers_at_once() {
    let harness = Harness::new();
    harness.dispatch(&enveloped(0));
    assert_eq!(harness.broker.pending_len(), 0);
    assert!(has_notice(&harness.conn.pull_events()));
    let captured = harness.captured.lock().expect("captured");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0]["result"],
        serde_json::json!({ "outcome": "cancelled" })
    );
}
