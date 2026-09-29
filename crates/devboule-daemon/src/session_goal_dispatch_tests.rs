//! What a `/goal` send does on each provider road: the stored value, the one
//! prompt it travels on, and the native road it never touches. One phrase for
//! the file — per-provider dispatch — beside the intercept
//! (`session_goal_tests.rs`), the durable half
//! (`session_goal_journal_tests.rs`), and the menu
//! (`session_goal_menu_tests.rs`).

use std::sync::{Arc, Mutex};

use devboule_protocol::SessionKind;

use super::super::tests::{
    attach_live_agent_for_test, insert_live_agent_with_out_of_band, test_owner,
    tmp_delete_registry, RecordingWriter,
};
use super::goal_test_support::{
    goal_text_of, live_agent, notices_of, pulled_events, user_messages, ClaimingOutOfBand,
};

#[test]
fn a_claude_set_sends_exactly_one_goal_prompt() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal", "process-goal");
    let received = Arc::new(Mutex::new(Vec::new()));
    let (runtime, conn) = live_agent(
        &registry,
        "s.goal.claude",
        &owner,
        SessionKind::Claude,
        &received,
    );

    registry
        .send_with_subscription(
            "s.goal.claude",
            41,
            "/goal ship it",
            &[],
            &[],
            &owner,
            &conn,
        )
        .expect("the set is accepted");

    assert_eq!(runtime.goal().as_deref(), Some("ship it"));
    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("prompt");
    assert_eq!(
        written.matches("Goal: ship it").count(),
        1,
        "one ordinary prompt carries the goal, never the slash text: {written:?}"
    );
    assert!(
        !written.contains("/goal"),
        "the slash form never reaches the provider: {written:?}"
    );
    let events = pulled_events(&conn);
    assert_eq!(
        goal_text_of(&events),
        [Some("ship it".to_string())],
        "the set stores and emits before sending"
    );
    assert!(
        user_messages(&events)
            .iter()
            .any(|text| text.contains("Goal: ship it")),
        "the prompt echoes as the transcript's user message"
    );
    assert!(
        notices_of(&events)
            .iter()
            .any(|notice| notice.0 == "Goal set: ship it"),
        "the set answers the way the Codex ack does"
    );
    let row_goal = journal
        .list()
        .expect("rows")
        .into_iter()
        .find(|row| row.id == "s.goal.claude")
        .expect("the row")
        .goal;
    assert_eq!(row_goal.as_deref(), Some("ship it"));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_clear_sends_nothing_and_journals_the_absence() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-clear", "process-goal-clear");
    let received = Arc::new(Mutex::new(Vec::new()));
    let (runtime, conn) = live_agent(
        &registry,
        "s.goal.clear",
        &owner,
        SessionKind::Pi,
        &received,
    );

    registry
        .send_with_subscription("s.goal.clear", 41, "/goal first", &[], &[], &owner, &conn)
        .expect("the set is accepted");
    let after_set = received.lock().expect("writer").len();
    registry
        .send_with_subscription("s.goal.clear", 41, "/goal clear", &[], &[], &owner, &conn)
        .expect("the clear is accepted");

    assert_eq!(runtime.goal(), None);
    assert_eq!(
        received.lock().expect("writer").len(),
        after_set,
        "a clear writes nothing to the provider"
    );
    let events = pulled_events(&conn);
    assert_eq!(
        goal_text_of(&events),
        [Some("first".to_string()), None],
        "the clear is journalled like a set, so replay never resurrects"
    );
    assert!(
        notices_of(&events)
            .iter()
            .any(|notice| notice.0 == "Goal cleared."),
        "the clear answers"
    );
    let row_goal = journal
        .list()
        .expect("rows")
        .into_iter()
        .find(|row| row.id == "s.goal.clear")
        .expect("the row")
        .goal;
    assert_eq!(row_goal, None, "the column lands NULL on a clear");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn later_prompts_carry_no_second_goal() {
    // Resume and recover never re-send: the goal travels once, on the set's
    // own prompt, and every later prompt — including the first after a
    // resume — is exactly what the human typed.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-once", "process-goal-once");
    let received = Arc::new(Mutex::new(Vec::new()));
    let (_runtime, conn) = live_agent(
        &registry,
        "s.goal.once",
        &owner,
        SessionKind::Acp,
        &received,
    );

    registry
        .send_with_subscription("s.goal.once", 41, "/goal alpha", &[], &[], &owner, &conn)
        .expect("the set is accepted");
    registry
        .send_with_subscription("s.goal.once", 41, "hello", &[], &[], &owner, &conn)
        .expect("the next prompt is accepted");

    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("prompt");
    assert_eq!(
        written.matches("Goal: alpha").count(),
        1,
        "the goal travels once: {written:?}"
    );
    assert_eq!(written.matches("hello").count(), 1);
    let events = pulled_events(&conn);
    assert_eq!(
        user_messages(&events)
            .iter()
            .filter(|text| text.contains("Goal: alpha"))
            .count(),
        1,
        "one user message carries the goal"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_claimed_goal_command_runs_natively_and_stores_nothing() {
    // The Codex shape without a Codex child: a provider road that claims
    // `/goal` keeps the text whole — no `Goal:` prompt, no stored goal —
    // and the native answer is what would move it.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-native", "process-goal-native");
    let received = Arc::new(Mutex::new(Vec::new()));
    registry
        .journal
        .as_ref()
        .expect("the test registry has a journal")
        .create_session(crate::journal::new_session_record(
            "s.goal.native",
            owner.user.clone(),
            None,
            SessionKind::Codex,
            "Agent",
        ))
        .expect("birth row");
    let runtime = insert_live_agent_with_out_of_band(
        &registry,
        "s.goal.native",
        owner.clone(),
        SessionKind::Codex,
        Box::new(RecordingWriter(Arc::clone(&received))),
        Some(Arc::new(ClaimingOutOfBand)),
    );
    let conn = attach_live_agent_for_test(&runtime, "s.goal.native", 45);
    registry
        .send_with_subscription(
            "s.goal.native",
            45,
            "/goal ship it",
            &[],
            &[],
            &owner,
            &conn,
        )
        .expect("the native road is accepted");
    assert_eq!(runtime.goal(), None, "the intercept stores nothing");
    assert!(
        received.lock().expect("writer").is_empty(),
        "no ordinary prompt is sent on the native road"
    );
    let events = pulled_events(&conn);
    assert!(
        goal_text_of(&events).is_empty(),
        "no goal change is emitted before the native answer"
    );
    assert!(
        user_messages(&events)
            .iter()
            .any(|text| text == "/goal ship it"),
        "the native road echoes the command text itself"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_uppercase_goal_reaches_the_provider_as_an_ordinary_prompt() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-case", "process-goal-case");
    let received = Arc::new(Mutex::new(Vec::new()));
    let (runtime, conn) = live_agent(
        &registry,
        "s.goal.case",
        &owner,
        SessionKind::Claude,
        &received,
    );
    registry
        .send_with_subscription("s.goal.case", 41, "/GOAL ship it", &[], &[], &owner, &conn)
        .expect("an ordinary prompt is accepted");
    assert_eq!(
        runtime.goal(),
        None,
        "no goal is stored for the uppercase text"
    );
    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("prompt");
    assert!(
        written.contains("/GOAL ship it"),
        "the text reaches the provider verbatim: {written:?}"
    );
    assert!(!written.contains("Goal: "), "no goal prompt is composed");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_live_snapshot_carries_the_goal_in_the_roster() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-roster", "process-goal-roster");
    let received = Arc::new(Mutex::new(Vec::new()));
    let (_runtime, conn) = live_agent(
        &registry,
        "s.goal.roster",
        &owner,
        SessionKind::Claude,
        &received,
    );
    registry
        .send_with_subscription(
            "s.goal.roster",
            41,
            "/goal ship it",
            &[],
            &[],
            &owner,
            &conn,
        )
        .expect("the set is accepted");
    let snapshots = registry.state_snapshots(&owner);
    let row = snapshots
        .iter()
        .find(|snapshot| snapshot.id == "s.goal.roster")
        .expect("the live row is listed");
    assert_eq!(row.goal.as_deref(), Some("ship it"));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
