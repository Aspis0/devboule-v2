//! Tests that every grok question id gets an answer: pending cards at
//! session close, and requests that arrive when the broker already refuses
//! new cards. Each test reads the exact frame off a fake child's stdout,
//! through the production sender — deleting the write fails the test on
//! timeout instead of passing silently.

use devboule_protocol::SessionEvent;

use super::question_support::{echo_harness, enveloped, has_notice, live_turn, SESSION};

#[test]
fn pending_question_answered_on_close() {
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    echo.dispatch(&enveloped(0));
    let _ = echo.conn.pull_events();
    assert_eq!(echo.broker.pending_len(), 1);
    echo.broker.close();
    assert_eq!(
        echo.read_frame(),
        serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "result": { "outcome": "skip_interview" },
        })
    );
}

#[test]
fn closed_broker_answers_at_once() {
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    echo.broker.close();
    echo.dispatch(&enveloped(0));
    assert_eq!(echo.broker.pending_len(), 0);
    assert!(!echo
        .conn
        .pull_events()
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })));
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({ "outcome": "skip_interview" }),
        "the id is answered, not dropped"
    );
}

#[test]
fn duplicate_tool_call_id_answers_at_once() {
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    echo.dispatch(&enveloped(0));
    let _ = echo.conn.pull_events();
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
    echo.dispatch(&second);
    assert_eq!(echo.broker.pending_len(), 1, "no second card");
    let cards = echo
        .conn
        .pull_events()
        .into_iter()
        .filter(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. }))
        .count();
    assert_eq!(cards, 0, "the repeat never becomes a card");
    assert_eq!(
        echo.read_frame(),
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": { "outcome": "skip_interview" },
        })
    );
}

#[test]
fn reused_wire_id_is_dropped_and_answered_once() {
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    echo.dispatch(&enveloped(0));
    let events = echo.conn.pull_events();
    assert!(events
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })));
    // The same wire id, still waiting, for another question: the duplicate
    // is dropped with a notice — never answered — so the id keeps exactly
    // one response.
    echo.dispatch(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "_x.ai/ask_user_question",
        "params": {
            "sessionId": SESSION,
            "toolCallId": "call-other-0",
            "questions": [{"question": "Another?"}],
            "mode": "default",
        },
    }));
    assert_eq!(echo.broker.pending_len(), 1, "no second card");
    let events = echo.conn.pull_events();
    assert!(!events
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })));
    assert!(has_notice(&events));
    assert!(
        echo.poll_frame(std::time::Duration::from_millis(500))
            .is_none(),
        "the duplicate gets no response of its own"
    );
    // And the parked card still answers, once, against its own questions.
    echo.broker
        .respond_with_option(
            "call-fence-0",
            devboule_protocol::PermissionOutcome::AllowOnce,
            Some("q0o1".to_string()),
            None,
        )
        .expect("option pick");
    assert_eq!(
        echo.read_frame(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "result": {
                "outcome": "accepted",
                "answers": { super::question_support::FENCE: ["Barn red"] },
            },
        })
    );
    assert!(
        echo.poll_frame(std::time::Duration::from_millis(500))
            .is_none(),
        "exactly one response for the id"
    );
}

#[test]
fn unparseable_frame_is_said_out_loud_and_answered() {
    // Nothing the person could answer: the id is answered at once, and the
    // transcript says so instead of dropping the frame in silence.
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    echo.dispatch(&serde_json::json!({
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
    assert_eq!(echo.broker.pending_len(), 0);
    let events = echo.conn.pull_events();
    assert!(!events
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })));
    assert!(has_notice(&events));
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({ "outcome": "skip_interview" })
    );
}

#[test]
fn foreign_session_question_answers_at_once() {
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    let mut frame = enveloped(0);
    frame["params"]["sessionId"] = serde_json::json!("another-session");
    echo.dispatch(&frame);
    assert_eq!(echo.broker.pending_len(), 0);
    assert!(has_notice(&echo.conn.pull_events()));
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({ "outcome": "skip_interview" })
    );
}

#[test]
fn missing_session_question_answers_at_once() {
    // Every shape grok sends carries the session: an absent one is refused
    // like a wrong one.
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    let mut frame = enveloped(0);
    frame["params"]
        .as_object_mut()
        .expect("params object")
        .remove("sessionId");
    echo.dispatch(&frame);
    assert_eq!(echo.broker.pending_len(), 0);
    assert!(has_notice(&echo.conn.pull_events()));
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({ "outcome": "skip_interview" })
    );
}

#[test]
fn question_after_turn_end_answers_at_once() {
    let mut echo = echo_harness();
    echo.dispatch(&enveloped(0));
    assert_eq!(echo.broker.pending_len(), 0);
    assert!(has_notice(&echo.conn.pull_events()));
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({ "outcome": "skip_interview" })
    );
}

#[test]
fn oversize_question_is_refused_and_answered() {
    // Past the broker's field bound: no card, and the id still gets the
    // decline frame.
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    let mut frame = enveloped(0);
    frame["params"]["questions"][0]["question"] = serde_json::json!("Q".repeat(9 * 1024));
    echo.dispatch(&frame);
    assert_eq!(echo.broker.pending_len(), 0);
    assert!(!echo
        .conn
        .pull_events()
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })));
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({ "outcome": "skip_interview" })
    );
}
