//! What a client attaching after a plan turn is sent, through the attach
//! reader: the plan's events before the turn's AgentFinished, and one row per
//! plan that carries both the text and the card's outcome.

use devboule_protocol::{PermissionOutcome, SessionEvent};

use super::plan_card_test_support::{
    attach_replay, feed_interrupted_plan_turn, feed_plan_turn, plan_reader, LIVE_PLAN, TURN,
};
use super::plan_rows_test_support::{is_plan_row_event, plan_rows};

fn captured_plan_text() -> String {
    crate::codex_view::fixture_frames(LIVE_PLAN)[1]["params"]["item"]["text"]
        .as_str()
        .expect("the captured plan item carries its text")
        .to_string()
}

fn finished_at(events: &[SessionEvent]) -> usize {
    events
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentFinished { .. }))
        .unwrap_or_else(|| panic!("the turn finishes: {events:?}"))
}

#[test]
fn an_attach_replays_the_plan_rows_before_agent_finished() {
    let (mut reader, runtime, _conn, _broker) = plan_reader();
    feed_plan_turn(&mut reader, &runtime);
    let events = attach_replay(&runtime);
    let last_plan = events
        .iter()
        .rposition(is_plan_row_event)
        .expect("the plan is replayed");
    assert!(
        last_plan < finished_at(&events),
        "the card's row comes before AgentFinished: {events:?}"
    );

    let (mut reader, runtime, _conn, _broker) = plan_reader();
    feed_interrupted_plan_turn(&mut reader, &runtime);
    let events = attach_replay(&runtime);
    let last_plan = events
        .iter()
        .rposition(is_plan_row_event)
        .expect("the folded row is replayed");
    assert!(
        last_plan < finished_at(&events),
        "the folded row comes before the finish: {events:?}"
    );
    let rows = plan_rows(&events);
    let row = rows.get(&format!("{TURN}-plan")).expect("the plan row");
    assert_eq!(
        row.text,
        captured_plan_text(),
        "the plan text is on the row once"
    );
}

#[test]
fn an_approved_plan_replays_as_one_row_with_its_text_and_outcome() {
    let (mut reader, runtime, _conn, broker) = plan_reader();
    feed_plan_turn(&mut reader, &runtime);
    let [card_id] = broker.pending_ids().try_into().expect("one card");
    broker
        .respond_with_option(
            &card_id,
            PermissionOutcome::AllowOnce,
            Some("implement".to_string()),
            None,
        )
        .expect("the card is approved");

    let events = attach_replay(&runtime);
    let rows = plan_rows(&events);
    assert_eq!(
        rows.keys().cloned().collect::<Vec<_>>(),
        vec![format!("{TURN}-plan")],
        "one row for the plan: {events:?}"
    );
    let row = &rows[&format!("{TURN}-plan")];
    assert_eq!(row.text, captured_plan_text(), "the row carries the plan");
    assert_eq!(
        (row.status.as_str(), row.title.as_str()),
        ("completed", "Approved"),
        "the row carries the approval: {events:?}"
    );
}

#[test]
fn a_refused_card_replays_as_the_plan_off_row() {
    let (mut reader, runtime, _conn, broker) = plan_reader();
    broker.close();
    feed_plan_turn(&mut reader, &runtime);

    let events = attach_replay(&runtime);
    let rows = plan_rows(&events);
    let row = rows.get(&format!("{TURN}-plan")).expect("the plan row");
    assert_eq!(
        row.text,
        captured_plan_text(),
        "the plan text is on the row once"
    );
    assert_eq!(row.status, "in_progress", "the plan-off status: {events:?}");
}
