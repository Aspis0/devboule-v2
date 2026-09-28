//! The plan card's reader wiring: the captured frames reach the registered
//! card, and the card or folded row is published before the turn's
//! AgentFinished.

use devboule_protocol::SessionEvent;

use super::plan_card_test_support::{
    feed_interrupted_plan_turn, feed_plan_turn, plan_reader, published, SESSION, TURN,
};
use super::plan_rows_test_support::is_plan_row_event;

#[test]
fn a_captured_plan_turn_registers_its_card_through_the_reader() {
    let (mut reader, runtime, conn, broker) = plan_reader();
    feed_plan_turn(&mut reader, &runtime);

    let events = published(&conn);
    let row = events.iter().find(|event| {
        matches!(event, SessionEvent::AgentToolCall { title, kind: Some(kind), .. }
            if title == "Plan" && kind == "plan")
    });
    assert!(row.is_some(), "the Plan row is published: {events:?}");
    let card = events.iter().find_map(|event| match event {
        SessionEvent::PermissionRequest {
            tool_call_id,
            plan: Some(plan),
            ..
        } => Some((tool_call_id.clone(), plan.clone())),
        _ => None,
    });
    let (card_id, plan) = card.expect("the plan card is registered");
    assert_eq!(
        card_id,
        format!("{TURN}-plan"),
        "the card is keyed on the plan item's own id"
    );
    assert!(
        plan.contains("Create `hello.txt`"),
        "the card carries the captured plan text"
    );
    assert_eq!(broker.pending_len(), 1);
}

#[test]
fn the_plan_row_and_card_come_before_the_turns_agent_finished() {
    // The daemon's own event order for a plan turn: the card (or the folded
    // row) is published before the turn's AgentFinished, so the app never
    // opens a phantom turn over a finished one.
    let (mut reader, runtime, conn, _broker) = plan_reader();
    feed_plan_turn(&mut reader, &runtime);

    let events = published(&conn);
    let card = events
        .iter()
        .position(|event| matches!(event, SessionEvent::PermissionRequest { .. }))
        .expect("the card is published");
    let finished = events
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentFinished { .. }))
        .expect("the turn finishes");
    assert!(
        card < finished,
        "the card is published before AgentFinished: {events:?}"
    );
    // The unclean completion folds the row in before AgentFinished too.
    let (mut reader, runtime, conn, _broker) = plan_reader();
    feed_interrupted_plan_turn(&mut reader, &runtime);
    let events = published(&conn);
    let row = events
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentToolCall { kind: Some(kind), .. } if kind == "plan"))
        .expect("the folded row is published");
    let finished = events
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentFinished { .. }))
        .expect("the turn finishes");
    assert!(
        row < finished,
        "the folded row is published before AgentFinished: {events:?}"
    );
}

#[test]
fn replay_puts_the_plan_events_before_agent_finished() {
    // The recovery reader's order (`journal.replay`); the attach reader's is
    // in the attach tests. The plan row must reach the transcript before the
    // turn's AgentFinished.
    let (mut reader, runtime, _conn, _broker) = plan_reader();
    feed_plan_turn(&mut reader, &runtime);
    let journal = runtime.journal.as_ref().expect("journal");
    let replay = journal.replay(SESSION).expect("replay");
    let events = &replay.events;
    let plan = events
        .iter()
        .rposition(is_plan_row_event)
        .expect("the plan row is journalled");
    let finished = events
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentFinished { .. }))
        .expect("the turn finishes");
    assert!(
        plan < finished,
        "every plan row comes before AgentFinished on replay: {events:?}"
    );
    // The interrupted turn folds the row in before its finish too.
    let (mut reader, runtime, _conn, _broker) = plan_reader();
    feed_interrupted_plan_turn(&mut reader, &runtime);
    let journal = runtime.journal.as_ref().expect("journal");
    let replay = journal.replay(SESSION).expect("replay");
    let events = &replay.events;
    let row = events
        .iter()
        .rposition(is_plan_row_event)
        .expect("the folded row is replayed");
    let finished = events
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentFinished { .. }))
        .expect("the interrupted turn finishes");
    assert!(
        row < finished,
        "the folded row comes before the finish on replay: {events:?}"
    );
}
