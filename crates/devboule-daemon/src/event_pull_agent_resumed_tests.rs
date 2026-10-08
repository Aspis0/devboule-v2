//! The resumed-child event reaches only the readers that agreed its name, live
//! and replayed alike; a reader without the name never sees a trace of it.

use super::super::*;
use super::*;

use super::test_support::{attach_live_agent_replay, drain};

const SESSION_ID: &str = "s.agent.resumed";
const CHILD_ID: &str = "s.child.1";

fn created_row(seq: u64) -> crate::journal::EventRecord {
    let created = SessionEvent::AgentCreated {
        message_id: None,
        child_session_id: CHILD_ID.to_string(),
        display_name: "worker".to_string(),
        provider: "claude".to_string(),
        profile: "worker".to_string(),
    };
    crate::journal::agent_report_record(SESSION_ID, 1, seq, &created).expect("created row")
}

fn resumed_row(seq: u64) -> crate::journal::EventRecord {
    let resumed = SessionEvent::AgentResumed {
        child_session_id: CHILD_ID.to_string(),
        display_name: "worker".to_string(),
    };
    crate::journal::agent_report_record(SESSION_ID, 1, seq, &resumed).expect("resumed row")
}

fn is_resumed(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::AgentResumed { .. })
}

#[test]
fn a_reader_without_the_name_gets_no_resumed_row_on_replay() {
    let dir = crate::test_dirs::test_temp_dir("devboule-agent-resumed-replay-quiet");
    let session = new_session_record(SESSION_ID, "S-1-5-21-1", None, SessionKind::Claude, "Agent");
    let (_journal, _runtime, conn) = attach_live_agent_replay(
        &dir,
        session,
        Some(SessionKind::Claude),
        vec![created_row(1), resumed_row(2)],
    );
    let events = drain(&conn);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentCreated { .. })),
        "the replay itself ran: {events:?}"
    );
    assert!(
        !events.iter().any(is_resumed),
        "a reader that never agreed the name is not sent it: {events:?}"
    );
}

#[test]
fn a_reader_with_the_name_gets_the_resumed_row_on_replay() {
    let dir = crate::test_dirs::test_temp_dir("devboule-agent-resumed-replay-agreed");
    let session = new_session_record(SESSION_ID, "S-1-5-21-1", None, SessionKind::Claude, "Agent");
    let (_journal, _runtime, conn) = attach_live_agent_replay(
        &dir,
        session,
        Some(SessionKind::Claude),
        vec![created_row(1), resumed_row(2)],
    );
    conn.set_agent_resumed_negotiated(true);
    let events = drain(&conn);
    assert!(
        events.iter().any(is_resumed),
        "a reader that agreed the name is replayed the row: {events:?}"
    );
}

#[test]
fn a_live_resumed_row_reaches_only_the_readers_that_agreed_the_name() {
    let dir = crate::test_dirs::test_temp_dir("devboule-agent-resumed-live");
    let session = new_session_record(SESSION_ID, "S-1-5-21-1", None, SessionKind::Claude, "Agent");
    let (_journal, runtime, quiet) =
        attach_live_agent_replay(&dir, session, Some(SessionKind::Claude), Vec::new());
    let agreed = ConnHandle::new(2);
    agreed.set_agent_resumed_negotiated(true);
    let outcome = runtime
        .try_attach_with_replay(None, &agreed, true)
        .expect("attach the agreeing reader");
    agreed.track_with_agent_replay(
        SESSION_ID,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );

    runtime
        .publish_child_resumed(CHILD_ID, "worker")
        .expect("the resumed row is journaled");

    assert!(
        !drain(&quiet).iter().any(is_resumed),
        "a live reader that never agreed the name is not sent it"
    );
    assert!(
        drain(&agreed).iter().any(is_resumed),
        "a live reader that agreed the name is sent it"
    );
}
