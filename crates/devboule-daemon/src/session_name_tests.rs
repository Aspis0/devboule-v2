//! Agent titles: the rename frame's daemon half and the first-prompt
//! auto-title.
//!
//! A rename lands on the live session and its journal row together, so a
//! restart keeps it; a paired device renames only what it may already write
//! (the ownership check every other session write goes through). The
//! auto-title fires on the first qualifying prompt of an untitled agent
//! session — decided by message kind, never by author: the person's
//! composer text and the creator's task qualify, envelopes, relays and
//! notices never do — and an explicit name is never overwritten. A resumed
//! session that is still untitled derives from the first composer message
//! its journal holds.

use super::tests::{
    attach_live_agent_for_test, insert_live, insert_live_agent, insert_live_agent_with_writer,
    remote_conn, set_entry_origin, test_owner, tmp_delete_registry, RecordingWriter,
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
fn the_creation_send_titles_from_the_task_not_the_composition() {
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
    // spawn prompt and the preset preamble. The kind admits the task — it
    // is the creator's own words — while the hook derives from the raw
    // task, never from the composed prompt around it.
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
        live_display_name(&registry, id).as_deref(),
        Some("do the task"),
        "the title is the task, not the composition around it"
    );
    for marker in ["agent preamble", "spawn prompt"] {
        assert!(
            !live_display_name(&registry, id)
                .expect("titled")
                .contains(marker),
            "no composed part leaks into the title: {marker}"
        );
    }
    assert_eq!(
        journal_display_name(&journal, id).as_deref(),
        Some("do the task")
    );
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
    // Notices and relays precede the person's message in the journal: none
    // of them may name the session, and none of them may spend the slot.
    for (seq, kind, text) in [
        (
            2,
            UserMessageKind::Creation,
            "standing instructions\n\nspawn prompt\n\npreamble\n\ndo the task",
        ),
        (
            3,
            UserMessageKind::SystemNotice,
            "<devboule-system>\nkind: agent_quiet\nsummary: still working\n</devboule-system>",
        ),
        (
            4,
            UserMessageKind::IncomingA2a,
            "<devboule-system>\nkind: agent_message\nbody: delegate this\n</devboule-system>",
        ),
    ] {
        let event = SessionEvent::AgentUserMessage {
            message_id: Some(format!("devboule-user-1-{seq}")),
            text: text.to_string(),
            author: UserMessageAuthor::Agent,
            message_kind: kind,
        };
        journal
            .append_blocking(
                crate::journal::agent_report_record(id, 1, seq, &event).expect("record"),
            )
            .expect("history is journalled");
    }
    let event = SessionEvent::AgentUserMessage {
        message_id: Some("devboule-user-1-5".to_string()),
        text: "Fix the login redirect\nsome more detail".to_string(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Composer,
    };
    journal
        .append_blocking(crate::journal::agent_report_record(id, 1, 5, &event).expect("record"))
        .expect("the first user message is journalled");
    insert_live_agent(&registry, id, owner.clone());
    let conn = ConnHandle::new(1);
    let record = journal
        .list()
        .expect("rows")
        .into_iter()
        .find(|record| record.id == id)
        .expect("the row");

    assert!(
        registry.title_untitled_from_journal(id, &owner, &record, &journal, &conn.conn_peer),
        "an untitled session with a journalled prompt is titled"
    );
    assert_eq!(
        live_display_name(&registry, id).as_deref(),
        Some("Fix the login redirect"),
        "the person's words name it — not the composed echo, the notice or the relay"
    );
    assert_eq!(
        journal_display_name(&journal, id).as_deref(),
        Some("Fix the login redirect")
    );

    let record = journal
        .list()
        .expect("rows")
        .into_iter()
        .find(|record| record.id == id)
        .expect("the row");
    assert!(
        !registry.title_untitled_from_journal(id, &owner, &record, &journal, &conn.conn_peer),
        "a second pass changes nothing: the name is set now"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_terminal_never_reads_its_journal_for_a_title() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-terminal-row", "c1");
    let id = "s.name.10";
    let mut record = crate::journal::new_session_record(
        id.to_string(),
        owner.user.clone(),
        None,
        SessionKind::Terminal,
        "Terminal",
    );
    record.display_name = None;
    journal.upsert_blocking(record.clone()).expect("birth row");
    let event = SessionEvent::AgentUserMessage {
        message_id: Some("devboule-user-1-2".to_string()),
        text: "Fix the login redirect".to_string(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Composer,
    };
    journal
        .append_blocking(crate::journal::agent_report_record(id, 1, 2, &event).expect("record"))
        .expect("history is journalled");
    // The journal is shut down before the call: any event decode would fail
    // or hang, so a clean `false` proves the kind gate runs first.
    journal.shutdown();
    let conn = ConnHandle::new(1);
    assert!(
        !registry.title_untitled_from_journal(id, &owner, &record, &journal, &conn.conn_peer),
        "a terminal has no title to derive, however rich its journal"
    );
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

#[test]
fn an_a2a_envelope_never_becomes_the_title() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-envelope", "c1");
    let id = "s.name.20";
    let runtime = insert_live_agent_with_writer(
        &registry,
        id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    birth_row(&journal, id, &owner);
    let conn = attach_live_agent_for_test(&runtime, id, 60);

    // The delegation delivery, verbatim: `Agent`-authored, but an envelope —
    // `IncomingA2a` — rather than anyone's words.
    registry
        .send_with_subscription_timeout(&SendRequest {
            session_id: id,
            subscription_id: conn.id,
            text: "<devboule-system>\norigin: local\nrole: daemon\nfrom_agent: s.a.9\nkind: agent_message\ntimestamp: 1\nbody: delegate this\n</devboule-system>",
            attachments: &[],
            attachment_references: &[],
            owner: &owner,
            conn: &conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: true,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: None,
            spawn_prompt: None,
            author: UserMessageAuthor::Agent,
            message_kind: UserMessageKind::IncomingA2a,
        })
        .expect("the relay is accepted");

    assert_eq!(
        live_display_name(&registry, id),
        None,
        "a relay must leave the slot unspent for the person's first prompt"
    );
    assert_eq!(journal_display_name(&journal, id), None);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_daemon_notice_never_becomes_the_title() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-notice", "c1");
    let id = "s.name.21";
    let runtime = insert_live_agent_with_writer(
        &registry,
        id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    birth_row(&journal, id, &owner);
    let conn = attach_live_agent_for_test(&runtime, id, 61);

    registry
        .send_with_subscription_timeout(&SendRequest {
            session_id: id,
            subscription_id: conn.id,
            text: "<devboule-system>\norigin: local\nrole: daemon\nfrom_agent: s.a.9\nkind: agent_quiet\ntimestamp: 1\nsummary: still working\n</devboule-system>",
            attachments: &[],
            attachment_references: &[],
            owner: &owner,
            conn: &conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: true,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: None,
            spawn_prompt: None,
            author: UserMessageAuthor::Agent,
            message_kind: UserMessageKind::SystemNotice,
        })
        .expect("the notice is accepted");

    assert_eq!(
        live_display_name(&registry, id),
        None,
        "a notice must leave the slot unspent for the person's first prompt"
    );
    assert_eq!(journal_display_name(&journal, id), None);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_composed_first_prompt_titles_from_the_persons_words() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-composed", "c1");
    let id = "s.name.22";
    let runtime = insert_live_agent_with_writer(
        &registry,
        id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    birth_row(&journal, id, &owner);
    let conn = attach_live_agent_for_test(&runtime, id, 62);
    // A notice arrives before the person types: it must not spend the slot.
    registry
        .send_with_subscription_timeout(&SendRequest {
            session_id: id,
            subscription_id: conn.id,
            text:
                "<devboule-system>\nkind: agent_quiet\nsummary: still working\n</devboule-system>",
            attachments: &[],
            attachment_references: &[],
            owner: &owner,
            conn: &conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: true,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: None,
            spawn_prompt: None,
            author: UserMessageAuthor::Agent,
            message_kind: UserMessageKind::SystemNotice,
        })
        .expect("the notice is accepted");
    assert_eq!(
        live_display_name(&registry, id),
        None,
        "the notice spends nothing"
    );

    // The person's first prompt, composed with standing instructions, spawn
    // prompt and preamble by the send path (the standing half rides the same
    // glue, pinned by `standing_instructions_come_before_the_preset_preamble`).
    registry
        .send_with_subscription_timeout(&SendRequest {
            session_id: id,
            subscription_id: conn.id,
            text: "Fix the login redirect\nsome more detail",
            attachments: &[],
            attachment_references: &[],
            owner: &owner,
            conn: &conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: true,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: Some("PREAMBLE-MARKER"),
            spawn_prompt: Some("SPAWN-MARKER"),
            author: UserMessageAuthor::Human,
            message_kind: UserMessageKind::Composer,
        })
        .expect("the prompt is accepted");

    let title = live_display_name(&registry, id).expect("titled");
    assert_eq!(title, "Fix the login redirect");
    for marker in ["PREAMBLE-MARKER", "SPAWN-MARKER", "<devboule-system>"] {
        assert!(
            !title.contains(marker),
            "no composed or envelope part leaks into the title: {marker}"
        );
    }
    assert_eq!(
        journal_display_name(&journal, id).as_deref(),
        Some("Fix the login redirect")
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_journal_write_restores_the_record() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-rollback", "c1");
    let id = "s.name.23";
    insert_live_agent(&registry, id, owner.clone());
    birth_row(&journal, id, &owner);
    journal.shutdown();
    let conn = ConnHandle::new(1);

    let error = registry
        .set_display_name(id, &owner, "worker one", &conn)
        .expect_err("a rename the journal does not hold is refused");
    assert_eq!(
        error.code,
        ErrorCode::Journal,
        "a stopped journal is a journal error, not a refusal: {error:?}"
    );
    assert_eq!(
        live_display_name(&registry, id),
        None,
        "the record is restored: memory and disk agree again"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rollback_never_restores_over_a_newer_write() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-epoch", "c1");
    let id = "s.name.24";
    insert_live_agent(&registry, id, owner.clone());
    birth_row(&journal, id, &owner);
    let conn = ConnHandle::new(1);
    registry
        .set_display_name(id, &owner, "first", &conn)
        .expect("the first rename lands");
    registry
        .set_display_name(id, &owner, "worker", &conn)
        .expect("the second rename lands");

    // A stale writer — epoch 1, from before the second rename — rolling back
    // must not wipe the landed name, even for the identical string the old
    // name-comparison guard would have restored over.
    registry.rollback_display_name(id, &owner, &conn.conn_peer, Some("first".to_string()), 1);
    assert_eq!(
        live_display_name(&registry, id).as_deref(),
        Some("worker"),
        "a superseded rollback restores nothing"
    );
    // The owning writer still restores its own write.
    registry.rollback_display_name(id, &owner, &conn.conn_peer, Some("first".to_string()), 2);
    assert_eq!(
        live_display_name(&registry, id).as_deref(),
        Some("first"),
        "the current writer's rollback still lands"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_noop_rename_converges_the_row_without_a_broadcast() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-noop", "c1");
    let id = "s.name.25";
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
    let pushes = pushed.lock().expect("push log").len();

    registry
        .set_display_name(id, &owner, "worker one", &conn)
        .expect("renaming to the same name succeeds");

    assert_eq!(
        pushed.lock().expect("push log").len(),
        pushes,
        "a no-op rename broadcasts nothing"
    );
    assert_eq!(
        journal_display_name(&journal, id).as_deref(),
        Some("worker one")
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_paired_client_renames_only_its_paired_users_sessions() {
    let (dir, registry, journal) = tmp_delete_registry();
    let mine = test_owner("S-1-5-21-mine", "process-1");
    let theirs = test_owner("S-1-5-21-theirs", "process-2");
    let mine_id = "s.name.26";
    let theirs_id = "s.name.27";
    insert_live_agent(&registry, mine_id, mine.clone());
    insert_live_agent(&registry, theirs_id, theirs.clone());
    birth_row(&journal, mine_id, &mine);
    birth_row(&journal, theirs_id, &theirs);
    let conn = remote_conn(PeerRole::Client, Some("S-1-5-21-mine"));

    registry
        .set_display_name(mine_id, &mine, "worker one", &conn)
        .expect("a paired device renames its paired user's session");
    assert_eq!(
        live_display_name(&registry, mine_id).as_deref(),
        Some("worker one")
    );
    assert_eq!(
        journal_display_name(&journal, mine_id).as_deref(),
        Some("worker one")
    );

    let error = registry
        .set_display_name(theirs_id, &mine, "taken", &conn)
        .expect_err("another account's session is refused");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert_eq!(
        error.message, "This client is not authorized to use that session.",
        "the same sentence every other session write answers with: {error:?}"
    );
    assert_eq!(live_display_name(&registry, theirs_id), None);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_daemon_peer_renames_only_the_sessions_it_created() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("peer_dev-phone", "daemon");
    let own_id = "s.name.28";
    let local_id = "s.name.29";
    insert_live_agent(&registry, own_id, owner.clone());
    insert_live_agent(&registry, local_id, owner.clone());
    birth_row(&journal, own_id, &owner);
    birth_row(&journal, local_id, &owner);
    set_entry_origin(
        &registry,
        own_id,
        SessionOrigin::peer("dev-phone", PeerRole::Daemon),
    );
    let conn = remote_conn(PeerRole::Daemon, Some("peer_dev-phone"));

    registry
        .set_display_name(own_id, &owner, "worker one", &conn)
        .expect("a daemon peer renames the session its device created");
    assert_eq!(
        live_display_name(&registry, own_id).as_deref(),
        Some("worker one")
    );

    let error = registry
        .set_display_name(local_id, &owner, "taken", &conn)
        .expect_err("a session this device did not create is refused");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert_eq!(
        live_display_name(&registry, local_id),
        None,
        "a refused rename changes nothing"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_terminal_is_renamed_but_never_auto_titled() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-terminal", "c1");
    let id = "s.name.30";
    insert_live(&registry, id, owner.clone());
    birth_row(&journal, id, &owner);
    let conn = ConnHandle::new(1);

    registry
        .set_display_name(id, &owner, "shell one", &conn)
        .expect("a terminal is renameable like any session");
    assert_eq!(
        live_display_name(&registry, id).as_deref(),
        Some("shell one")
    );
    assert_eq!(
        journal_display_name(&journal, id).as_deref(),
        Some("shell one")
    );
    assert!(
        !registry.title_if_unset(id, &owner, "derived", &conn.conn_peer),
        "the auto-title is agents-only: a terminal's name is set, never derived"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_first_prompts_title_exactly_once() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-race", "c1");
    let id = "s.name.31";
    insert_live_agent(&registry, id, owner.clone());
    birth_row(&journal, id, &owner);

    // Eight prompts race for one untitled session: the check-and-set is one
    // locked step, so exactly one wins and the losers change nothing.
    let barrier = Arc::new(std::sync::Barrier::new(8));
    std::thread::scope(|scope| {
        for worker in 0..8 {
            let barrier = Arc::clone(&barrier);
            let registry = &registry;
            let owner = &owner;
            scope.spawn(move || {
                barrier.wait();
                registry.title_if_unset(id, owner, &format!("worker {worker}"), &None)
            });
        }
    });

    let live = live_display_name(&registry, id).expect("titled exactly once");
    assert!(
        live.starts_with("worker "),
        "the winner's name stands: {live:?}"
    );
    assert_eq!(
        journal_display_name(&journal, id).as_deref(),
        Some(live.as_str()),
        "the row carries the winner's name, not a loser's"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_title_scan_stops_at_the_window() {
    let (dir, _registry, journal) = tmp_delete_registry();
    let owner = test_owner("name-window", "c1");
    let notice = SessionEvent::AgentUserMessage {
        message_id: None,
        text: "<devboule-system>\nkind: agent_quiet\nsummary: still working\n</devboule-system>"
            .to_string(),
        author: UserMessageAuthor::Agent,
        message_kind: UserMessageKind::SystemNotice,
    };
    let late = SessionEvent::AgentUserMessage {
        message_id: None,
        text: "Fix the login redirect".to_string(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Composer,
    };
    // Past the window the session stays untitled until its next prompt: the
    // scan stays bounded rather than decoding the transcript to reach it.
    let far_id = "s.name.32";
    birth_row(&journal, far_id, &owner);
    for seq in 2..=300u64 {
        journal
            .append_blocking(
                crate::journal::agent_report_record(far_id, 1, seq, &notice).expect("record"),
            )
            .expect("history is journalled");
    }
    journal
        .append_blocking(
            crate::journal::agent_report_record(far_id, 1, 301, &late).expect("record"),
        )
        .expect("history is journalled");
    assert_eq!(
        journal.first_composer_title(far_id).expect("the scan runs"),
        None,
        "past the window the session stays untitled until its next prompt"
    );
    // Inside the window a composer message is found, skipping the notices
    // ahead of it.
    let near_id = "s.name.33";
    birth_row(&journal, near_id, &owner);
    for seq in 2..=100u64 {
        journal
            .append_blocking(
                crate::journal::agent_report_record(near_id, 1, seq, &notice).expect("record"),
            )
            .expect("history is journalled");
    }
    journal
        .append_blocking(
            crate::journal::agent_report_record(near_id, 1, 101, &late).expect("record"),
        )
        .expect("history is journalled");
    assert_eq!(
        journal
            .first_composer_title(near_id)
            .expect("the scan runs"),
        Some("Fix the login redirect".to_string()),
        "a composer message inside the window is found"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
