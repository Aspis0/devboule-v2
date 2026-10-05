//! The goal's durable half: the journal column, restart recovery, replay
//! equality, and the replacement a recovery builds. One phrase for the file
//! — the goal after a restart — beside the intercept (`session_goal_tests.rs`),
//! dispatch (`session_goal_dispatch_tests.rs`), and the menu
//! (`session_goal_menu_tests.rs`).

use std::sync::{Arc, Mutex};

use super::super::tests::test_epoch;
use devboule_protocol::{
    SessionEvent, SessionKind, SessionState, UserMessageAuthor, UserMessageKind,
};

use super::super::session_resume_fixture::{acp_row, take_bystander_slot, AcpEnv, ResumeFixture};
use super::super::tests::{
    attach_live_agent_for_test, insert_live_agent_with_kind_and_writer, test_owner,
    tmp_delete_registry, RecordingWriter,
};
use super::super::{ConnHandle, OwnerId, SessionRegistry};
use super::carry_goal_into_recovery;
use super::goal_test_support::{goal_text_of, notices_of, pulled_events};

fn live_claude(
    registry: &SessionRegistry,
    id: &str,
    owner: &OwnerId,
) -> (
    Arc<super::super::SessionRuntime>,
    Arc<ConnHandle>,
    Arc<Mutex<Vec<u8>>>,
) {
    // Production creates always write the journal row first; the goal road
    // records beside the live update, so the harness births the row too.
    registry
        .journal
        .as_ref()
        .expect("the test registry has a journal")
        .create_session(crate::journal::new_session_record(
            id,
            owner.user.clone(),
            None,
            SessionKind::Claude,
            "Agent",
        ))
        .expect("birth row");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_kind_and_writer(
        registry,
        id,
        owner.clone(),
        SessionKind::Claude,
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, id, 51);
    (runtime, conn, received)
}

fn send(registry: &SessionRegistry, id: &str, owner: &OwnerId, conn: &ConnHandle, text: &str) {
    registry
        .send_with_subscription(id, 51, text, &[], &[], owner, conn)
        .expect("the send is accepted");
}

fn goal_sequence(events: &[SessionEvent]) -> Vec<Option<String>> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::GoalChanged { goal } => Some(goal.clone()),
            _ => None,
        })
        .collect()
}

fn pulled(conn: &Arc<ConnHandle>) -> Vec<SessionEvent> {
    conn.pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect()
}

#[test]
fn set_restart_and_replay_restore_the_goal() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-j", "process-goal-j");
    let (_runtime, conn, _received) = live_claude(&registry, "s.goal.journal", &owner);
    send(&registry, "s.goal.journal", &owner, &conn, "/goal first");
    send(&registry, "s.goal.journal", &owner, &conn, "/goal second");

    let live = goal_sequence(&pulled(&conn));
    assert_eq!(
        live,
        [Some("first".to_string()), Some("second".to_string())],
        "live carries the whole value on every change"
    );

    // A daemon restart is a fresh open over the same file: the column, not
    // the runtime, is what survives it.
    let path = dir.join("journal.db");
    journal.shutdown();
    let reopened = Arc::new(crate::journal::Journal::open(&path).expect("reopen"));
    let row = reopened
        .list()
        .expect("rows")
        .into_iter()
        .find(|row| row.id == "s.goal.journal")
        .expect("the row survived the restart");
    assert_eq!(row.goal.as_deref(), Some("second"));

    // Replay equals live: the same `GoalChanged` sequence, in order.
    let replay = reopened.replay("s.goal.journal").expect("replay");
    assert_eq!(goal_sequence(&replay.events), live);
    reopened.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn set_then_clear_then_replay_gives_none() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-jc", "process-goal-jc");
    let (_runtime, conn, _received) = live_claude(&registry, "s.goal.jclear", &owner);
    send(&registry, "s.goal.jclear", &owner, &conn, "/goal doomed");
    send(&registry, "s.goal.jclear", &owner, &conn, "/goal clear");

    let live = goal_sequence(&pulled(&conn));
    assert_eq!(live, [Some("doomed".to_string()), None]);

    let path = dir.join("journal.db");
    journal.shutdown();
    let reopened = Arc::new(crate::journal::Journal::open(&path).expect("reopen"));
    let row = reopened
        .list()
        .expect("rows")
        .into_iter()
        .find(|row| row.id == "s.goal.jclear")
        .expect("the row survived the restart");
    assert_eq!(row.goal, None, "the clear landed NULL, not a stale text");
    let replay = reopened.replay("s.goal.jclear").expect("replay");
    assert_eq!(
        goal_sequence(&replay.events),
        live,
        "replay never resurrects the cleared text"
    );
    reopened.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_archived_session_keeps_its_last_goal_in_its_snapshot() {
    // Ended rows are roster rows too: the snapshot carries the last goal,
    // read-only — no live runtime exists to accept a `/goal` for it.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-arch", "process-goal-arch");
    let (_runtime, conn, _received) = live_claude(&registry, "s.goal.arch", &owner);
    send(&registry, "s.goal.arch", &owner, &conn, "/goal kept");
    journal
        .mark_ended_blocking("s.goal.arch", 1, Some(0))
        .expect("the session ends");
    journal.shutdown();

    let path = dir.join("journal.db");
    let reopened = Arc::new(crate::journal::Journal::open(&path).expect("reopen"));
    let registry2 = SessionRegistry::new(
        crate::paths::RuntimePaths::from_dir(&dir),
        Some(Arc::clone(&reopened)),
        test_epoch(),
    );
    let snapshots = registry2.state_snapshots(&owner);
    let row = snapshots
        .iter()
        .find(|snapshot| snapshot.id == "s.goal.arch")
        .expect("the archived row is listed");
    assert_eq!(row.goal.as_deref(), Some("kept"));
    reopened.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn restart_then_attach_shows_the_goal_in_the_roster() {
    // Mutant: the hydrate seed wiped (`set_goal(None)` in
    // `hydrate_transcript`) — the rebuilt roster must still list the goal,
    // read from the hydrated runtime rather than the journal column.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-goal-hydrate", "process-goal-hydrate");
    let (_runtime, conn, _received) = live_claude(&registry, "s.goal.hydrate", &owner);
    send(&registry, "s.goal.hydrate", &owner, &conn, "/goal ship it");
    let path = dir.join("journal.db");
    journal.shutdown();
    let reopened = Arc::new(crate::journal::Journal::open(&path).expect("reopen"));
    let registry2 = SessionRegistry::new(
        crate::paths::RuntimePaths::from_dir(&dir),
        Some(Arc::clone(&reopened)),
        test_epoch(),
    );
    let before = registry2.state_snapshots(&owner);
    let row = before
        .iter()
        .find(|snapshot| snapshot.id == "s.goal.hydrate")
        .expect("the journal row lists the goal before attach");
    assert_eq!(row.goal.as_deref(), Some("ship it"));
    let conn2 = ConnHandle::new(99);
    registry2
        .attach_with_subscription("s.goal.hydrate", 99, None, &conn2, &owner, false)
        .expect("attach hydrates the transcript");
    // The first read cached the journal-built roster; a client's next roster
    // request after the attach rebuilds it, so the pin drops the cache first
    // and asserts what the hydrated runtime reports.
    registry2.invalidate_state_roster_cache();
    let after = registry2.state_snapshots(&owner);
    let row = after
        .iter()
        .find(|snapshot| snapshot.id == "s.goal.hydrate")
        .expect("the hydrated row is listed");
    assert_eq!(
        row.goal.as_deref(),
        Some("ship it"),
        "the goal survives the restart and the attach"
    );
    reopened.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

fn recovered_user(text: &str) -> SessionEvent {
    SessionEvent::AgentUserMessage {
        message_id: None,
        text: text.to_string(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Composer,
        at_ms: None,
        images: Vec::new(),
    }
}

fn recovered_agent(text: &str) -> SessionEvent {
    SessionEvent::AgentMessage {
        message_id: None,
        text: text.to_string(),
        parent_tool_use_id: None,
        spawn_depth: None,

        images: Vec::new(),
    }
}

#[test]
fn a_recovered_session_carries_the_goal_from_the_row() {
    // Mutant: the recover carry dropped — the replacement answers "No goal
    // set." for a conversation that had one, with no column and no event.
    let fixture = ResumeFixture::new("goal-recover");
    let id = fixture.id("goal-recover");
    let prompts = fixture.dir.join("stub prompts.txt");
    let _env = AcpEnv::stub(&[(
        "DEVBOULE_ACP_STUB_PROMPT_FILE",
        prompts.to_string_lossy().into_owned(),
    )]);
    let _broker = fixture.state.mcp.start(&fixture.state).expect("MCP server");
    let mut row = acp_row(&id, &fixture.owner, "handle-goal-recover");
    row.cwd = Some(
        fixture
            .dir
            .join("removed-worktree")
            .to_string_lossy()
            .into_owned(),
    );
    fixture.write_row(row);
    fixture
        .journal()
        .set_session_goal(&id, Some("ship it"))
        .expect("the old row carries a goal");
    fixture.record_turn(&id, 1, &recovered_user("did you check the tests?"));
    fixture.record_turn(&id, 2, &recovered_agent("yes — and the gate too"));
    take_bystander_slot(&fixture.state);

    let session = fixture
        .resume(&id, &fixture.conn())
        .expect("a conversation is recovered into a new session");
    assert_ne!(
        session.id, id,
        "the old row is not reopened: the provider cannot reopen it"
    );
    assert!(
        matches!(session.state, SessionState::Live { .. }),
        "the replacement is a live session: {:?}",
        session.state
    );
    let runtime = fixture
        .registry()
        .agent_runtime_for(&session.id, &fixture.owner, &fixture.conn())
        .expect("the recovered runtime");
    assert_eq!(
        runtime.goal().as_deref(),
        Some("ship it"),
        "the replacement carries the old row's goal"
    );
    let snapshots = fixture.registry().state_snapshots(&fixture.owner);
    let listed = snapshots
        .iter()
        .find(|snapshot| snapshot.id == session.id)
        .expect("the replacement is listed");
    assert_eq!(listed.goal.as_deref(), Some("ship it"));
    let new_row = fixture.row(&session.id);
    assert_eq!(
        new_row.goal.as_deref(),
        Some("ship it"),
        "the carry writes the replacement's own row"
    );
    let conn2 = attach_live_agent_for_test(&runtime, &session.id, 52);
    let events = pulled_events(&conn2);
    assert_eq!(
        goal_text_of(&events),
        [Some("ship it".to_string())],
        "the carry emits the change the transcript replays"
    );
    let _ = fixture
        .state
        .sessions
        .close(&session.id, &fixture.owner, &None);
    fixture.finish();
}

#[test]
fn a_failed_journal_write_still_publishes_the_carried_goal_in_memory() {
    // Mutant: the in-memory publish after a failed write dropped — the
    // replacement keeps `None` and only the warning names the lost goal.
    let (dir, _registry, journal) = tmp_delete_registry();
    let runtime = Arc::new(super::super::SessionRuntime::with_journal(
        "s.goal.missing".to_string(),
        Some(Arc::clone(&journal)),
    ));
    let conn = attach_live_agent_for_test(&runtime, "s.goal.missing", 61);
    carry_goal_into_recovery(&runtime, "carried".to_string());
    assert_eq!(
        runtime.goal().as_deref(),
        Some("carried"),
        "the carried goal is published even though the write failed"
    );
    let events = pulled_events(&conn);
    assert_eq!(
        goal_text_of(&events),
        [Some("carried".to_string())],
        "the change is emitted beside the warning"
    );
    let notices = notices_of(&events);
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0].0.starts_with("The goal could not be recorded:"),
        "the failure leaves a warning naming itself: {}",
        notices[0].0
    );
    assert_eq!(notices[0].1, devboule_protocol::NoticeSeverity::Warning);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
