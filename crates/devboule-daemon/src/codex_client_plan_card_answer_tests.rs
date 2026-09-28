//! A card answered the moment it becomes answerable, by a steer or by
//! Implement, leaves its row terminal: the row is out before the card, so
//! nothing after the answer can put it back to `in_progress`.

use devboule_protocol::{PermissionOutcome, SessionEvent};

use super::plan_card_test_support::{feed_plan_turn, live_conn, plan_reader, published, TURN};
use super::plan_rows_test_support::plan_rows;

/// The statuses the card's row is given, in order.
fn row_statuses(events: &[SessionEvent], card_id: &str) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentToolCall {
                tool_call_id,
                status,
                ..
            } if tool_call_id == card_id => Some(status.clone()),
            SessionEvent::AgentToolUpdate {
                tool_call_id,
                status: Some(status),
                ..
            } if tool_call_id == card_id => Some(status.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_card_answered_as_soon_as_it_is_answerable_leaves_its_row_terminal() {
    let card_id = format!("{TURN}-plan");
    for (answer, expected) in [("steer", "cancelled"), ("implement", "completed")] {
        let (mut reader, runtime, _rebuilt, broker) = plan_reader();
        let conn = live_conn(&runtime);
        feed_plan_turn(&mut reader, &runtime);

        // Everything the reader published up to the card: the card is the
        // last word on its row, so an answer given the instant it is
        // registered is the next one.
        let raised = published(&conn);
        let card = raised
            .iter()
            .position(|event| {
                matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. }
                    if tool_call_id == &card_id)
            })
            .unwrap_or_else(|| panic!("{answer}: the card is published: {raised:?}"));
        assert_eq!(
            row_statuses(&raised[card..], &card_id),
            Vec::<String>::new(),
            "{answer}: no row status follows the card: {raised:?}"
        );

        match answer {
            "steer" => broker.cancel_pending_for_steer(),
            _ => broker
                .respond_with_option(
                    &card_id,
                    PermissionOutcome::AllowOnce,
                    Some("implement".to_string()),
                    None,
                )
                .expect("the card is approved"),
        }
        let answered = published(&conn);
        assert_eq!(
            row_statuses(&answered, &card_id),
            vec![expected.to_string()],
            "{answer}: the answer is the row's last status: {answered:?}"
        );
        let all = [raised, answered].concat();
        assert_eq!(
            plan_rows(&all)[&card_id].status,
            expected,
            "{answer}: the row ends terminal"
        );
    }
}
