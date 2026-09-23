//! The terminal-ownership tests, moved whole out of `session_tests.rs` lines
//! 3625-4268 (at `5c22b34`): a terminal send never publishes an agent user
//! message, the same user's attached client may send, resize and answer a
//! permission card while an unattached client or a different user may not, a
//! failed or poisoned writer still records the error the client sees, and
//! `delete_session` admits a journal-only or dead entry for the same user while
//! refusing another user's row and a live session. Every line below is
//! byte-identical to its text there apart from this header; `insert_transcript`
//! is promoted to `pub(super)` for this move, and the other fixtures come from
//! the provider's own imports. The two tests at the end (`delete_session_
//! allows_archived_terminal…`, `finishing_a_preserved_terminal…`) are later
//! additions for the archive/conhost release, not part of the moved block.

use super::session_spawn::{finish_reader_session, release_preserved_pty_after_drain};
use super::tests::{
    attach_live_agent_for_test, ended_record, insert_live, insert_live_agent,
    insert_live_agent_with_writer, insert_transcript, test_owner, tmp_delete_registry,
    RecordingWriter,
};
use super::*;

#[test]
fn terminal_send_does_not_publish_an_agent_user_message() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-terminal", "process-terminal");
    insert_live(&registry, "terminal-send", owner.clone());
    let conn = ConnHandle::new(108);
    registry
        .attach("terminal-send", None, &conn, &owner, false)
        .expect("terminal attaches");
    registry
        .send("terminal-send", "typed terminal input", &owner, &conn)
        .expect("terminal send");
    let runtime = registry.runtime("terminal-send").expect("runtime");
    assert_eq!(runtime.current_agent_seq(), 0);
    assert!(!runtime
        .stream
        .lock()
        .expect("stream")
        .observers
        .values()
        .flat_map(|attachment| attachment.pending.iter())
        .any(|item| matches!(
            item,
            PendingItem::Agent {
                event: SessionEvent::AgentUserMessage { .. },
                ..
            }
        )));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn same_user_attached_restarted_client_can_send() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-reconnect-send", "process-1111");
    let restarted = test_owner("S-1-5-21-reconnect-send", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "send01").expect("id");
    insert_live(&registry, &session_id, original);
    let conn = ConnHandle::new(101);
    registry
        .attach(&session_id, None, &conn, &restarted, false)
        .expect("restarted same-user client attaches");
    registry
        .send(&session_id, "restart input", &restarted, &conn)
        .expect("attached restarted client can send");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn same_user_unattached_client_cannot_send() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-unattached-send", "process-1111");
    let caller = test_owner("S-1-5-21-unattached-send", "process-2222");
    let session_id = compose_session_id(&owner.session_token(), "send02").expect("id");
    insert_live(&registry, &session_id, owner);
    let attached = ConnHandle::new(113);
    registry
        .attach(&session_id, None, &attached, &caller, false)
        .expect("a same-user connection attaches");
    let conn = ConnHandle::new(111);
    let error = registry
        .send(&session_id, "unattached input", &caller, &conn)
        .expect_err("unattached client must not send");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("not attached"));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn same_user_attached_restarted_client_can_resize_terminal() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-reconnect-resize", "process-1111");
    let restarted = test_owner("S-1-5-21-reconnect-resize", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "resize01").expect("id");
    insert_live(&registry, &session_id, original);
    let conn = ConnHandle::new(102);
    registry
        .attach(&session_id, None, &conn, &restarted, false)
        .expect("restarted same-user client attaches");
    registry
        .resize(&session_id, 100, 30, &restarted, &conn)
        .expect("attached restarted client can resize");
    let runtime = registry.runtime(&session_id).expect("runtime");
    assert_eq!(
        runtime
            .stream
            .lock()
            .expect("stream")
            .screen
            .as_ref()
            .expect("terminal screen")
            .dimensions(),
        (100, 30)
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn same_user_unattached_client_cannot_resize_terminal() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-unattached-resize", "process-1111");
    let caller = test_owner("S-1-5-21-unattached-resize", "process-2222");
    let session_id = compose_session_id(&owner.session_token(), "resize02").expect("id");
    insert_live(&registry, &session_id, owner);
    let attached = ConnHandle::new(114);
    registry
        .attach(&session_id, None, &attached, &caller, false)
        .expect("a same-user connection attaches");
    let conn = ConnHandle::new(112);
    let error = registry
        .resize(&session_id, 100, 30, &caller, &conn)
        .expect_err("unattached client must not resize");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("not attached"));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn same_user_attached_restarted_client_can_respond_to_permission() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-reconnect-permission", "process-1111");
    let restarted = test_owner("S-1-5-21-reconnect-permission", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "perm01").expect("id");
    let runtime = insert_live_agent(&registry, &session_id, original);
    let conn = ConnHandle::new(103);
    registry
        .attach(&session_id, None, &conn, &restarted, false)
        .expect("restarted same-user client attaches");
    runtime
        .permission_broker()
        .expect("permission broker")
        .register(
            7,
            permission_broker::permission("restart-permission"),
            &runtime,
        )
        .expect("permission request");
    registry
        .permission_respond(
            &session_id,
            "restart-permission",
            PermissionOutcome::AllowOnce,
            &conn,
            &restarted,
        )
        .expect("attached restarted client can respond");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn same_user_unattached_client_cannot_respond_to_permission() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-unattached-permission", "process-1111");
    let caller = test_owner("S-1-5-21-unattached-permission", "process-2222");
    let session_id = compose_session_id(&owner.session_token(), "perm02").expect("id");
    let runtime = insert_live_agent(&registry, &session_id, owner.clone());
    let attached = ConnHandle::new(104);
    registry
        .attach(&session_id, None, &attached, &owner, false)
        .expect("owner attaches");
    runtime
        .permission_broker()
        .expect("permission broker")
        .register(
            8,
            permission_broker::permission("unattached-permission"),
            &runtime,
        )
        .expect("permission request");
    let unattached = ConnHandle::new(105);
    let error = registry
        .permission_respond(
            &session_id,
            "unattached-permission",
            PermissionOutcome::AllowOnce,
            &unattached,
            &caller,
        )
        .expect_err("unattached client must not respond");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("not attached"));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn different_user_cannot_send_or_resize() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-security-terminal", "process-1111");
    let stranger = test_owner("S-1-5-21-security-stranger", "process-2222");
    let session_id = compose_session_id(&owner.session_token(), "secure01").expect("id");
    insert_live(&registry, &session_id, owner.clone());
    let conn = ConnHandle::new(106);
    registry
        .attach(&session_id, None, &conn, &owner, false)
        .expect("owner attaches");
    assert_eq!(
        registry
            .send(&session_id, "hostile input", &stranger, &conn)
            .expect_err("different user must not send")
            .code,
        ErrorCode::Unauthorized
    );
    assert_eq!(
        registry
            .resize(&session_id, 100, 30, &stranger, &conn)
            .expect_err("different user must not resize")
            .code,
        ErrorCode::Unauthorized
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn different_user_cannot_respond_to_permission() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-security-permission", "process-1111");
    let stranger = test_owner("S-1-5-21-security-stranger-2", "process-2222");
    let session_id = compose_session_id(&owner.session_token(), "secure02").expect("id");
    let runtime = insert_live_agent(&registry, &session_id, owner.clone());
    let conn = ConnHandle::new(107);
    registry
        .attach(&session_id, None, &conn, &owner, false)
        .expect("owner attaches");
    runtime
        .permission_broker()
        .expect("permission broker")
        .register(
            9,
            permission_broker::permission("foreign-permission"),
            &runtime,
        )
        .expect("permission request");
    let error = registry
        .permission_respond(
            &session_id,
            "foreign-permission",
            PermissionOutcome::AllowOnce,
            &conn,
            &stranger,
        )
        .expect_err("different user must not respond");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn failed_agent_send_replays_error_without_prompt() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-agent", "process-agent");
    let runtime = insert_live_agent(&registry, "agent-send-failure", owner.clone());
    journal
        .upsert_blocking(new_session_record(
            "agent-send-failure",
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("agent session row");
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "agent-send-failure",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );

    let error = registry
        .send(
            "agent-send-failure",
            "prompt that cannot be sent",
            &owner,
            &conn,
        )
        .expect_err("writer must fail");
    assert_eq!(error.code, ErrorCode::Io);
    journal.flush().expect("flush prompt and error");

    let live = conn
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect::<Vec<_>>();
    assert!(!live.iter().any(|event| {
        matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "prompt that cannot be sent")
    }));
    let error_index = live
        .iter()
        .position(|event| {
            matches!(event, SessionEvent::AgentError { message } if message.contains("forced writer failure"))
        })
        .expect("failed send error must reach the live client");
    assert!(
        error_index < live.len(),
        "live failed send events: {live:?}"
    );

    runtime.detach_if_conn(conn.id);
    conn.untrack("agent-send-failure");
    let reattached = ConnHandle::new(2);
    let outcome = runtime
        .try_attach_with_replay(None, &reattached, true)
        .expect("reattach");
    reattached.track_with_agent_replay(
        "agent-send-failure",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let replayed = reattached
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect::<Vec<_>>();
    assert!(!replayed.iter().any(|event| {
        matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "prompt that cannot be sent")
    }));
    let error_index = replayed
        .iter()
        .position(|event| {
            matches!(event, SessionEvent::AgentError { message } if message.contains("forced writer failure"))
        })
        .expect("failed send error must replay");
    assert!(
        error_index < replayed.len(),
        "replayed failed send events: {replayed:?}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn poisoned_agent_writer_publishes_error_without_prompt() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-poisoned-writer", "process-agent");
    let runtime = insert_live_agent(&registry, "agent-poisoned-writer", owner.clone());
    journal
        .upsert_blocking(new_session_record(
            "agent-poisoned-writer",
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("agent session row");
    let conn = attach_live_agent_for_test(&runtime, "agent-poisoned-writer", 3);
    let writer = {
        let map = registry.inner.lock().expect("registry");
        match map.get("agent-poisoned-writer").expect("session") {
            RegistryEntry::Live(session) => Arc::clone(&session.writer),
            // A test-fixture entry is inserted as Live, never as the
            // delivery-window state; the arm only closes the match.
            RegistryEntry::Configuring(_) | RegistryEntry::Transcript(_) => {
                panic!("expected live session")
            }
        }
    };
    std::thread::spawn(move || {
        let _guard = writer.lock().expect("writer lock");
        panic!("poison writer for test");
    })
    .join()
    .expect_err("writer lock must be poisoned");

    let error = registry
        .send(
            "agent-poisoned-writer",
            "prompt with poisoned writer",
            &owner,
            &conn,
        )
        .expect_err("poisoned writer must reject the send");
    assert_eq!(error.code, ErrorCode::Internal);
    journal.flush().expect("flush prompt and writer error");

    let live = conn
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect::<Vec<_>>();
    assert!(!live.iter().any(|event| {
        matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "prompt with poisoned writer")
    }));
    let error_index = live
        .iter()
        .position(|event| {
            matches!(event, SessionEvent::AgentError { message } if message == "Session state is unavailable.")
        })
        .expect("poisoned writer error must reach the client");
    assert!(
        error_index < live.len(),
        "live poisoned writer events: {live:?}"
    );

    let replay = journal
        .replay("agent-poisoned-writer")
        .expect("replay poisoned writer");
    let replayed = replay.events;
    assert!(!replayed.iter().any(|event| {
        matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "prompt with poisoned writer")
    }));
    let error_index = replayed
        .iter()
        .position(|event| {
            matches!(event, SessionEvent::AgentError { message } if message == "Session state is unavailable.")
        })
        .expect("poisoned writer error must replay");
    assert!(
        error_index < replayed.len(),
        "replayed poisoned writer: {replayed:?}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn closed_agent_output_refuses_unrecordable_prompt() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-closed-agent", "process-agent");
    let written = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        "agent-closed-output",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&written))),
    );
    journal
        .upsert_blocking(new_session_record(
            "agent-closed-output",
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("agent session row");
    let conn = attach_live_agent_for_test(&runtime, "agent-closed-output", 109);
    runtime.close_output();

    let error = registry
        .send(
            "agent-closed-output",
            "prompt after output closed",
            &owner,
            &conn,
        )
        .expect_err("closed output must reject an unrecordable prompt");
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(error.message, "Agent input could not be recorded.");
    assert!(written.lock().expect("written lock").is_empty());
    assert_eq!(runtime.current_agent_seq(), 0);
    journal.flush().expect("flush closed-output journal");
    let replayed = journal
        .replay("agent-closed-output")
        .expect("replay closed output")
        .events;
    assert!(!replayed.iter().any(|event| matches!(
        event,
        SessionEvent::AgentUserMessage { text, .. } if text == "prompt after output closed"
    )));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn poisoned_agent_stream_refuses_unrecordable_prompt() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-poisoned-stream", "process-agent");
    let written = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        "agent-poisoned-stream",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&written))),
    );
    journal
        .upsert_blocking(new_session_record(
            "agent-poisoned-stream",
            &owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("agent session row");
    let conn = attach_live_agent_for_test(&runtime, "agent-poisoned-stream", 110);
    let poisoned_runtime = Arc::clone(&runtime);
    std::thread::spawn(move || {
        let _guard = poisoned_runtime.stream.lock().expect("stream lock");
        panic!("poison stream for test");
    })
    .join()
    .expect_err("stream lock must be poisoned");

    let error = registry
        .send(
            "agent-poisoned-stream",
            "prompt after stream poison",
            &owner,
            &conn,
        )
        .expect_err("poisoned stream must reject an unrecordable prompt");
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(error.message, "Session state is unavailable.");
    assert!(written.lock().expect("written lock").is_empty());
    journal.flush().expect("flush poisoned-stream journal");
    let replayed = journal
        .replay("agent-poisoned-stream")
        .expect("replay poisoned stream")
        .events;
    assert!(!replayed.iter().any(|event| matches!(
        event,
        SessionEvent::AgentUserMessage { text, .. } if text == "prompt after stream poison"
    )));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn delete_session_allows_journal_only_record_from_another_client_of_the_same_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-1", "process-1111");
    let caller = test_owner("S-1-5-21-1", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "dead01").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");

    let result = registry.delete_session(&session_id, &caller);
    assert!(
        result.is_ok(),
        "same user, different client must be able to delete a journal-only history row: {result:?}"
    );
    assert!(
        journal
            .list()
            .expect("list")
            .iter()
            .all(|row| row.id != session_id),
        "journal-only delete must remove the row"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_session_rejects_journal_only_record_owned_by_another_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("user-alice", "process-1111");
    let stranger = test_owner("user-bob", "process-1111");
    let session_id = compose_session_id(&owner.session_token(), "dead02").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &owner.user))
        .expect("row");

    let error = registry
        .delete_session(&session_id, &stranger)
        .expect_err("different user must stay unauthorized");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert!(
        journal
            .list()
            .expect("list")
            .iter()
            .any(|row| row.id == session_id),
        "unauthorized delete must leave the row"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_session_allows_dead_registry_entry_from_another_client_of_the_same_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-1", "process-1111");
    let caller = test_owner("S-1-5-21-1", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "dead03").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");
    insert_transcript(&registry, &session_id, original);

    let result = registry.delete_session(&session_id, &caller);
    assert!(
        result.is_ok(),
        "same user, different client must delete a dead registry entry: {result:?}"
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .get(&session_id)
            .is_none(),
        "dead registry entry must be removed"
    );
    assert!(journal
        .list()
        .expect("list")
        .iter()
        .all(|row| row.id != session_id));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_session_refuses_live_registry_entry_until_closed() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-1", "process-1111");
    let caller = test_owner("S-1-5-21-1", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "live01").expect("id");
    insert_live(&registry, &session_id, original);

    let error = registry
        .delete_session(&session_id, &caller)
        .expect_err("live session must refuse delete");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.message, "Close the session before deleting it.");
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .get(&session_id)
            .is_some(),
        "live registry entry must stay"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_session_allows_archived_terminal_once_its_child_has_ended() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-1", "process-1111");
    let caller = test_owner("S-1-5-21-1", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "archived01").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");
    insert_live(&registry, &session_id, original);
    // The registry state a completed archive (stop, then the preserved
    // entry's child end) leaves: the entry is still `Live`, and the
    // child has ended.
    {
        let mut map = registry.inner.lock().expect("registry");
        let session = map
            .get_mut(&session_id)
            .and_then(RegistryEntry::as_child_process_mut)
            .expect("live entry");
        session.preserve_on_exit.store(true, Ordering::SeqCst);
        session.exited.store(true, Ordering::SeqCst);
    }

    let result = registry.delete_session(&session_id, &caller);
    assert!(
        result.is_ok(),
        "an ended child holds nothing the delete could strand, and the \
         History panel deletes without a close: {result:?}"
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .get(&session_id)
            .is_none(),
        "archived registry entry must be removed"
    );
    assert!(journal
        .list()
        .expect("list")
        .iter()
        .all(|row| row.id != session_id));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn finishing_a_preserved_terminal_closes_its_writer() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-release", "process-release");
    let session_id = "archived-release";
    journal
        .upsert_blocking(ended_record(session_id, &owner.user))
        .expect("row");
    insert_live(&registry, session_id, owner);
    let (writer, runtime) = {
        let mut map = registry.inner.lock().expect("registry");
        let session = map
            .get_mut(session_id)
            .and_then(RegistryEntry::as_child_process_mut)
            .expect("live entry");
        session.preserve_on_exit.store(true, Ordering::SeqCst);
        (Arc::clone(&session.writer), Arc::clone(&session.runtime))
    };

    finish_reader_session(&registry, session_id, &runtime);

    let still_listed = registry
        .inner
        .lock()
        .expect("registry")
        .get(session_id)
        .and_then(RegistryEntry::as_child_process)
        .is_some();
    assert!(
        still_listed,
        "a preserved entry stays in the map for History"
    );
    let error = writer
        .lock()
        .expect("writer")
        .write_all(b"late input")
        .expect_err("the writer slot must be closed with the pipe");
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preserved_terminal_release_leaves_a_running_child_alone() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-release", "process-release");
    let session_id = "running-release";
    insert_live(&registry, session_id, owner);
    let writer = {
        let mut map = registry.inner.lock().expect("registry");
        let session = map
            .get_mut(session_id)
            .and_then(RegistryEntry::as_child_process_mut)
            .expect("live entry");
        Arc::clone(&session.writer)
    };

    release_preserved_pty_after_drain(&registry, session_id, Duration::ZERO);

    writer
        .lock()
        .expect("writer")
        .write_all(b"typed input")
        .expect("a child that is still running keeps its pipe");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preserved_terminal_release_closes_the_writer_after_the_child_is_reaped() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-release", "process-release");
    let session_id = "reaped-release";
    journal
        .upsert_blocking(ended_record(session_id, &owner.user))
        .expect("row");
    insert_live(&registry, session_id, owner);
    let writer = {
        let mut map = registry.inner.lock().expect("registry");
        let session = map
            .get_mut(session_id)
            .and_then(RegistryEntry::as_child_process_mut)
            .expect("live entry");
        let writer = Arc::clone(&session.writer);
        session.preserve_on_exit.store(true, Ordering::SeqCst);
        session.exited.store(true, Ordering::SeqCst);
        writer
    };

    release_preserved_pty_after_drain(&registry, session_id, Duration::ZERO);

    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .get(session_id)
            .and_then(RegistryEntry::as_child_process)
            .is_some(),
        "the preserved entry stays in the map for History"
    );
    let error = writer
        .lock()
        .expect("writer")
        .write_all(b"late input")
        .expect_err("the writer slot must be closed with the pipe");
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
