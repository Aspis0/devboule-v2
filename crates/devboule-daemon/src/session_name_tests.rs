//! Agent titles: the rename frame's daemon half and the first-prompt
//! auto-title.
//!
//! A rename lands on the live session and its journal row together, so a
//! restart keeps it; a paired device renames only what it may already write
//! (the ownership check every other session write goes through). The
//! auto-title fires on the first person-authored prompt of an untitled agent
//! session — the person's text, never the daemon-composed first prompt — and
//! an explicit name is never overwritten. A resumed session that is still
//! untitled derives from the first user message its journal holds.

use super::tests::{
    attach_live_agent_for_test, insert_live_agent, insert_live_agent_with_writer, test_owner,
    tmp_delete_registry, RecordingWriter,
};
use super::*;
use std::sync::Mutex;

/// One roster push, as the transition sink delivers it: whose roster, and
/// the rows it carried.
type PushedRoster = (String, Vec<SessionStateSnapshot>);

fn birth_row(journal: &Arc<Journal>, id: &str, owner: &OwnerId) {
    let record = crate::journal::new_session_record(
        id.to_string(),
        owner.user.clone(),
        None,
        SessionKind::Acp,
        "Agent",
    );
    journal.upsert_blocking(record).expect("birth row");
}

fn live_display_name(registry: &SessionRegistry, id: &str) -> Option<String> {
    registry
        .inner
        .lock()
        .expect("registry")
        .get(id)
        .and_then(RegistryEntry::as_peer_visible)
        .expect("live entry")
        .metadata
        .display_name
        .clone()
}

fn journal_display_name(journal: &Arc<Journal>, id: &str) -> Option<String> {
    journal
        .list()
        .expect("journal rows")
        .into_iter()
        .find(|record| record.id == id)
        .expect("the row")
        .display_name
        .clone()
}

#[test]
fn a_rename_lands_on_the_live_session_and_its_journal_row() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-ok", "c1");
    let id = "s.name.1";
    insert_live_agent(&registry, id, owner.clone());
    birth_row(&journal, id, &owner);
    let conn = ConnHandle::new(1);

    registry
        .set_display_name(id, &owner, "  worker one  ", &conn)
        .expect("the rename lands");

    assert_eq!(
        live_display_name(&registry, id).as_deref(),
        Some("worker one"),
        "the live session carries the trimmed name"
    );
    assert_eq!(
        journal_display_name(&journal, id).as_deref(),
        Some("worker one"),
        "the journal row carries it too, so a restart keeps it"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rename_survives_a_journal_reload() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-reload", "c1");
    let id = "s.name.2";
    insert_live_agent(&registry, id, owner.clone());
    birth_row(&journal, id, &owner);
    let conn = ConnHandle::new(1);
    registry
        .set_display_name(id, &owner, "worker one", &conn)
        .expect("the rename lands");

    journal.shutdown();
    let reopened = Arc::new(Journal::open(&dir.join("journal.db")).expect("reopen"));
    let row = reopened
        .list()
        .expect("journal rows")
        .into_iter()
        .find(|record| record.id == id)
        .expect("the row");
    assert_eq!(
        row.display_name.as_deref(),
        Some("worker one"),
        "the name is on disk, not only in memory"
    );
    reopened.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rename_by_another_owner_is_refused_with_the_existing_sentence() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-mine", "c1");
    let other = test_owner("name-theirs", "c9");
    let id = "s.name.3";
    insert_live_agent(&registry, id, owner.clone());
    birth_row(&journal, id, &owner);
    let conn = ConnHandle::new(1);

    let error = registry
        .set_display_name(id, &other, "taken", &conn)
        .expect_err("another owner's session cannot be renamed");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert_eq!(
        error.message, "This client is not authorized to use that session.",
        "the same sentence every other session write answers with: {error:?}"
    );
    assert_eq!(
        live_display_name(&registry, id),
        None,
        "a refused rename changes nothing live"
    );
    assert_eq!(
        journal_display_name(&journal, id),
        None,
        "and nothing on the row"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rename_refuses_names_the_protocol_refuses() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-bad", "c1");
    let id = "s.name.4";
    insert_live_agent(&registry, id, owner.clone());
    birth_row(&journal, id, &owner);
    let conn = ConnHandle::new(1);

    for bad in ["", "   \t ", &"x".repeat(61), "line one\nline two"] {
        let error = registry
            .set_display_name(id, &owner, bad, &conn)
            .expect_err("an invalid name is refused");
        assert_eq!(
            error.code,
            ErrorCode::InvalidRequest,
            "a bad name is a bad request, not a refusal: {bad:?}"
        );
    }
    assert_eq!(
        live_display_name(&registry, id),
        None,
        "refused renames leave the session untitled"
    );
    assert_eq!(journal_display_name(&journal, id), None);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_first_person_prompt_titles_an_untitled_agent_session() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-auto", "c1");
    let id = "s.name.5";
    let runtime = insert_live_agent_with_writer(
        &registry,
        id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    birth_row(&journal, id, &owner);
    let conn = attach_live_agent_for_test(&runtime, id, 51);

    registry
        .send_with_subscription(
            id,
            conn.id,
            "Fix the login redirect\nsome more detail",
            &[],
            &[],
            &owner,
            &conn,
        )
        .expect("the prompt is accepted");

    assert_eq!(
        live_display_name(&registry, id).as_deref(),
        Some("Fix the login redirect"),
        "the title is the first prompt's first line"
    );
    assert_eq!(
        journal_display_name(&journal, id).as_deref(),
        Some("Fix the login redirect"),
        "peers read the row, so the title is journalled with the prompt"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_daemon_composed_first_prompt_does_not_become_the_title() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-creation", "c1");
    let id = "s.name.6";
    let runtime = insert_live_agent_with_writer(
        &registry,
        id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    birth_row(&journal, id, &owner);
    let conn = attach_live_agent_for_test(&runtime, id, 52);

    // The creation's send: the creator agent's task beside the profile's
    // spawn prompt and the preset preamble, authored by the daemon's own
    // composition rather than by a person.
    registry
        .send_with_subscription_timeout(&SendRequest {
            session_id: id,
            subscription_id: conn.id,
            text: "do the task",
            attachments: &[],
            attachment_references: &[],
            owner: &owner,
            conn: &conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: true,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: Some("the agent preamble"),
            spawn_prompt: Some("the profile spawn prompt"),
            author: UserMessageAuthor::Creation,
            message_kind: UserMessageKind::Creation,
        })
        .expect("the creation's prompt is accepted");

    assert_eq!(
        live_display_name(&registry, id),
        None,
        "standing instructions, spawn prompt and preamble never name a session"
    );
    assert_eq!(journal_display_name(&journal, id), None);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_explicit_name_is_never_overwritten_by_a_later_prompt() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-kept", "c1");
    let id = "s.name.7";
    let runtime = insert_live_agent_with_writer(
        &registry,
        id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    birth_row(&journal, id, &owner);
    let conn = attach_live_agent_for_test(&runtime, id, 53);
    registry
        .set_display_name(id, &owner, "worker one", &conn)
        .expect("the explicit name lands");

    registry
        .send_with_subscription(
            id,
            conn.id,
            "Fix the login redirect",
            &[],
            &[],
            &owner,
            &conn,
        )
        .expect("the prompt is accepted");

    assert_eq!(
        live_display_name(&registry, id).as_deref(),
        Some("worker one"),
        "a derived title never overwrites a name that is already set"
    );
    assert_eq!(
        journal_display_name(&journal, id).as_deref(),
        Some("worker one")
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_untitled_session_derives_its_title_from_its_journal() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-import", "c1");
    let id = "s.name.8";
    birth_row(&journal, id, &owner);
    let event = SessionEvent::AgentUserMessage {
        message_id: Some("devboule-user-1-2".to_string()),
        text: "Fix the login redirect\nsome more detail".to_string(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Composer,
    };
    journal
        .append_blocking(crate::journal::agent_report_record(id, 1, 2, &event).expect("record"))
        .expect("the first user message is journalled");
    insert_live_agent(&registry, id, owner.clone());
    let conn = ConnHandle::new(1);

    assert!(
        registry.title_untitled_from_journal(id, &owner, &journal, &conn.conn_peer),
        "an untitled session with a journalled prompt is titled"
    );
    assert_eq!(
        live_display_name(&registry, id).as_deref(),
        Some("Fix the login redirect")
    );
    assert_eq!(
        journal_display_name(&journal, id).as_deref(),
        Some("Fix the login redirect")
    );

    assert!(
        !registry.title_untitled_from_journal(id, &owner, &journal, &conn.conn_peer),
        "a second pass changes nothing: the name is set now"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rename_pushes_the_roster_with_the_new_name() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-push", "c1");
    let id = "s.name.9";
    insert_live_agent(&registry, id, owner.clone());
    birth_row(&journal, id, &owner);
    let pushed: Arc<Mutex<Vec<PushedRoster>>> = Arc::new(Mutex::new(Vec::new()));
    let fired = Arc::clone(&pushed);
    registry.set_transition_sink(Arc::new(move |owner, snapshots| {
        fired
            .lock()
            .expect("push log")
            .push((owner.user.clone(), snapshots.unwrap_or_default()));
    }));
    let conn = ConnHandle::new(1);

    registry
        .set_display_name(id, &owner, "worker one", &conn)
        .expect("the rename lands");

    let pushes = pushed.lock().expect("push log");
    assert!(
        !pushes.is_empty(),
        "the rename pushes the roster so every client sees it"
    );
    let (user, snapshots) = pushes.last().expect("a push");
    assert_eq!(user, "name-push");
    let row = snapshots
        .iter()
        .find(|row| row.id == id)
        .expect("the renamed row is in the push");
    assert_eq!(
        row.display_name.as_deref(),
        Some("worker one"),
        "the push carries the name: strip, header, History and toasts read it"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
