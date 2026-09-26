//! Tests that every Codex input-request id gets an answer: pending cards at
//! session close, and requests that arrive when the broker already refuses
//! new cards. The immediate bytes equal the dismissal shapes the topic files
//! pin; what these tests pin is that no id waits.

use super::input_test_support::{
    elicitation_line, fence_question_params, question_harness, user_input_line,
};

#[test]
fn pending_question_answered_on_close() {
    let (broker, captured, runtime, conn, mut reader) = question_harness();
    reader.dispatch_value(
        user_input_line("item/tool/requestUserInput", fence_question_params()),
        &runtime,
    );
    let _ = conn.pull_events();
    assert_eq!(broker.pending_len(), 1);
    broker.close();
    let frames = captured.lock().expect("captured");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["result"], serde_json::json!({ "answers": {} }));
}

#[test]
fn closed_broker_answers_a_question_at_once() {
    let (broker, captured, runtime, conn, mut reader) = question_harness();
    broker.close();
    reader.dispatch_value(
        user_input_line("item/tool/requestUserInput", fence_question_params()),
        &runtime,
    );
    // The card never exists: the id is answered at once and nothing waits.
    assert_eq!(broker.pending_len(), 0);
    assert!(!conn.pull_events().iter().any(|event| matches!(
        event.envelope.event,
        devboule_protocol::SessionEvent::PermissionRequest { .. }
    )));
    assert!(captured.lock().expect("captured").is_empty());
}

#[test]
fn closed_broker_answers_an_elicitation_at_once() {
    let (broker, _, runtime, conn, mut reader) = question_harness();
    broker.close();
    reader.dispatch_value(elicitation_line("Allow the tool?"), &runtime);
    assert_eq!(broker.pending_len(), 0);
    assert!(!conn.pull_events().iter().any(|event| matches!(
        event.envelope.event,
        devboule_protocol::SessionEvent::PermissionRequest { .. }
    )));
}
