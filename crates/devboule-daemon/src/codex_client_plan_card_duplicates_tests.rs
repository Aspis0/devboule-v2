//! Duplicate completions and the row a refused registration leaves: a second
//! `turn/completed` for a turn whose card is already raised changes nothing, and
//! a refused registration folds the plan text into the row.

use devboule_protocol::{PermissionOutcome, SessionEvent};

use super::plan_card_test_support::{
    feed_plan_turn, plan_reader, plan_reader_observed_by, published, TURN,
};
use super::plan_rows_test_support::plan_rows;

#[test]
fn a_duplicate_completion_whose_card_was_answered_is_a_noop() {
    let (mut reader, runtime, conn, broker) = plan_reader();
    feed_plan_turn(&mut reader, &runtime);
    let card_id = format!("{TURN}-plan");
    broker
        .respond_with_option(
            &card_id,
            PermissionOutcome::Deny,
            Some("deny".to_string()),
            None,
        )
        .expect("the first card answers");
    let _ = conn.pull_events();

    // The same turn completes again: its card id is already in the journal, so
    // the second completion is a no-op — no new row, no failed, no notice.
    feed_plan_turn(&mut reader, &runtime);

    let events = published(&conn);
    assert!(
        !events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentToolCall { .. } | SessionEvent::AgentToolUpdate { .. }
        )),
        "a duplicate completion raises no row: {events:?}"
    );
    assert_eq!(broker.pending_len(), 0, "no card is re-registered");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::SessionNotice { .. })),
        "a no-op raises no notice: {events:?}"
    );
}

#[test]
fn a_duplicate_completion_whose_card_is_still_pending_is_a_noop() {
    let (mut reader, runtime, conn, broker) = plan_reader();
    feed_plan_turn(&mut reader, &runtime);
    assert_eq!(broker.pending_len(), 1, "the first card is pending");
    let _ = conn.pull_events();

    // The turn completes again before the person answers: the card is still
    // pending, so the second completion changes nothing.
    feed_plan_turn(&mut reader, &runtime);

    let events = published(&conn);
    assert!(
        !events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentToolCall { .. } | SessionEvent::AgentToolUpdate { .. }
        )),
        "a duplicate completion raises no row: {events:?}"
    );
    assert_eq!(broker.pending_len(), 1, "the pending card is left alone");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::SessionNotice { .. })),
        "a no-op raises no notice: {events:?}"
    );
}

#[test]
fn a_refused_registration_folds_the_plan_text_into_the_row() {
    // The broker is closed, so the registration is refused before the card
    // exists. The row published before the registration takes the plan text
    // and keeps its in_progress, as a plan-off plan row does: the person sees
    // the running Plan row with its text, "Interrupted — session ended" once
    // the session ends, and the notice beside it.
    let card_id = format!("{TURN}-plan");
    let (mut reader, runtime, conn, broker) = plan_reader();
    broker.close();
    feed_plan_turn(&mut reader, &runtime);

    let events = published(&conn);
    let calls = events
        .iter()
        .filter(|event| {
            matches!(event, SessionEvent::AgentToolCall { tool_call_id, .. } if tool_call_id == &card_id)
        })
        .count();
    assert_eq!(calls, 1, "one call row for the plan: {events:?}");
    let row = &plan_rows(&events)[&card_id];
    assert_eq!(row.status, "in_progress", "the plan-off status: {events:?}");
    assert!(
        row.text.contains("hello.txt"),
        "the row carries the plan text"
    );
    assert_eq!(broker.pending_len(), 0, "no card is left pending");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::SessionNotice { .. })),
        "the refusal leaves a notice: {events:?}"
    );
}

#[test]
fn a_card_no_observer_can_receive_keeps_the_brokers_outcome_and_the_text() {
    // The only observer cannot receive cards: the broker's cancel is the
    // card's one completion, and the text still reaches the row.
    let card_id = format!("{TURN}-plan");
    let (mut reader, runtime, conn, broker) = plan_reader_observed_by(false);
    feed_plan_turn(&mut reader, &runtime);

    let events = published(&conn);
    let terminal = events
        .iter()
        .filter(|event| {
            matches!(event, SessionEvent::AgentToolUpdate { tool_call_id, status: Some(_), .. }
                if tool_call_id == &card_id)
        })
        .count();
    assert_eq!(terminal, 1, "one terminal status for the card: {events:?}");
    let row = &plan_rows(&events)[&card_id];
    assert_eq!(
        row.status, "cancelled",
        "the broker's outcome stands: {events:?}"
    );
    assert!(
        row.text.contains("hello.txt"),
        "the row carries the plan text"
    );
    assert_eq!(broker.pending_len(), 0, "no card is left pending");
}
