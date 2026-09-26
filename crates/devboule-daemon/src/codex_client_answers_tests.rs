//! Tests that every Codex input-request id gets an answer: pending cards at
//! session close, and requests that arrive when the broker already refuses
//! new cards. Each test reads the exact bytes off a fake child's stdout,
//! through the real sender — deleting the write fails the test on timeout
//! instead of passing silently.

use super::super::codex_elicitations::dispatch_elicitation;
use super::super::codex_questions::dispatch_question;
use super::input_test_support::{
    echo_harness, elicitation_line, single_question_params, user_input_line,
};

fn node_gated() -> bool {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return true;
    }
    false
}

#[test]
fn pending_question_answered_on_close() {
    if node_gated() {
        return;
    }
    let mut echo = echo_harness("s.codex.close", None);
    dispatch_question(
        &echo.deps(),
        &user_input_line("item/tool/requestUserInput", single_question_params()),
        &echo.runtime,
        None,
    );
    let _ = echo.conn.pull_events();
    assert_eq!(echo.broker.pending_len(), 1);
    echo.broker.close();
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({ "answers": {} })
    );
}

#[test]
fn closed_broker_answers_a_question_at_once() {
    if node_gated() {
        return;
    }
    let mut echo = echo_harness("s.codex.closed-q", None);
    echo.broker.close();
    dispatch_question(
        &echo.deps(),
        &user_input_line("item/tool/requestUserInput", single_question_params()),
        &echo.runtime,
        None,
    );
    // The card never exists: the id is answered at once and nothing waits.
    assert_eq!(echo.broker.pending_len(), 0);
    assert!(!echo.conn.pull_events().iter().any(|event| matches!(
        event.envelope.event,
        devboule_protocol::SessionEvent::PermissionRequest { .. }
    )));
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({ "answers": {} })
    );
}

#[test]
fn closed_broker_answers_an_elicitation_at_once() {
    if node_gated() {
        return;
    }
    let mut echo = echo_harness("s.codex.closed-e", None);
    echo.broker.close();
    dispatch_elicitation(
        &echo.deps(),
        &elicitation_line("Allow the tool?"),
        &echo.runtime,
        None,
    );
    assert_eq!(echo.broker.pending_len(), 0);
    assert!(!echo.conn.pull_events().iter().any(|event| matches!(
        event.envelope.event,
        devboule_protocol::SessionEvent::PermissionRequest { .. }
    )));
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({ "action": "cancel", "content": null, "_meta": null })
    );
}
