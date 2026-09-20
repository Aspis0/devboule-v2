//! The peer-id, session-access and roster-cache tests, moved whole out of
//! `session_tests.rs` lines 3011-3398 (at `5418a7d`): a learned peer session id
//! that is durable and restored on hydration, attach, close and stop admitting a
//! previous-run or dead-client session of the same user while refusing another
//! user's, the roster and history themselves user-scoped and carrying
//! previous-run rows, a live transition that neither requeries the journal roster
//! nor caches rows under a revision that changed after the list, a push-only row
//! carrying the child's name and creator, and a live transition that does not
//! rebuild a large roster. Every line below is byte-identical to its text there
//! apart from this header; every fixture it uses is already `pub(super)` in the
//! provider.

use super::tests::{ended_record, insert_live, insert_live_agent, test_owner, tmp_delete_registry};
use super::*;

#[test]
fn learned_peer_session_id_is_durable_and_restored_on_hydration() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-peer", "process-1111");
    let caller = test_owner("S-1-5-21-peer", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "peer01").expect("id");
    let mut record = ended_record(&session_id, &original.user);
    record.kind = SessionKind::Acp;
    journal.upsert_blocking(record).expect("row");

    let runtime = SessionRuntime::with_journal(session_id.clone(), Some(Arc::clone(&journal)));
    runtime.set_peer_session_id("peer-session-1".to_string());
    journal.flush().expect("peer id");
    let row = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|row| row.id == session_id)
        .expect("row");
    assert_eq!(row.peer_session_id.as_deref(), Some("peer-session-1"));

    let conn = ConnHandle::new(1);
    registry
        .attach(&session_id, None, &conn, &caller, true)
        .expect("same-user hydration");
    let hydrated = registry
        .inner
        .lock()
        .expect("registry")
        .get(&session_id)
        .expect("hydrated entry")
        .runtime()
        .peer_session_id();
    assert_eq!(hydrated.as_deref(), Some("peer-session-1"));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_attach_allows_a_previous_run_session_for_the_same_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-attach", "process-1111");
    let caller = test_owner("S-1-5-21-attach", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "attach01").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");
    registry
        .attach(&session_id, None, &ConnHandle::new(1), &caller, false)
        .expect("same user, different client must attach");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_attach_allows_a_live_registry_session_from_a_dead_client_same_user() {
    // The most common restart shape: the daemon survives, the app does
    // not. The registry still holds the LIVE entry under the old client
    // token; the new client (same user) must attach through
    // runtime_for_user, not through journal hydration.
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-attach-live", "process-1111");
    let caller = test_owner("S-1-5-21-attach-live", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "attach03").expect("id");
    insert_live(&registry, &session_id, original);
    registry
        .attach(&session_id, None, &ConnHandle::new(1), &caller, false)
        .expect("same user, different client must attach to the live entry");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_attach_rejects_a_live_registry_session_from_another_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-attach-live-owner", "process-1111");
    let stranger = test_owner("S-1-5-21-attach-live-stranger", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "attach04").expect("id");
    insert_live(&registry, &session_id, original);
    let error = registry
        .attach(&session_id, None, &ConnHandle::new(1), &stranger, false)
        .expect_err("different user must stay unauthorized on the live entry");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_attach_rejects_a_previous_run_session_from_another_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-attach-owner", "process-1111");
    let stranger = test_owner("S-1-5-21-attach-stranger", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "attach02").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");
    let error = registry
        .attach(&session_id, None, &ConnHandle::new(1), &stranger, false)
        .expect_err("different user must stay unauthorized");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_close_allows_a_previous_run_session_for_the_same_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-close", "process-1111");
    let caller = test_owner("S-1-5-21-close", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "close01").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");
    assert!(!registry
        .close(&session_id, &caller, &None)
        .expect("same user, different client must close"));
    assert!(journal
        .list()
        .expect("list")
        .iter()
        .all(|row| row.id != session_id));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_close_rejects_a_previous_run_session_from_another_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-close-owner", "process-1111");
    let stranger = test_owner("S-1-5-21-close-stranger", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "close02").expect("id");
    journal
        .upsert_blocking(ended_record(&session_id, &original.user))
        .expect("row");
    let error = registry
        .close(&session_id, &stranger, &None)
        .expect_err("different user must stay unauthorized");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert!(journal
        .list()
        .expect("list")
        .iter()
        .any(|row| row.id == session_id));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_stop_allows_a_previous_run_live_session_for_the_same_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-stop", "process-1111");
    let caller = test_owner("S-1-5-21-stop", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "stop01").expect("id");
    insert_live(&registry, &session_id, original);
    registry
        .stop(&session_id, &caller)
        .expect("same user, different client must stop");
    assert!(registry
        .inner
        .lock()
        .expect("registry")
        .get(&session_id)
        .and_then(RegistryEntry::as_peer_visible)
        .is_some_and(|session| session.preserve_on_exit.load(Ordering::Acquire)));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_stop_rejects_a_previous_run_live_session_from_another_user() {
    let (dir, registry, journal) = tmp_delete_registry();
    let original = test_owner("S-1-5-21-stop-owner", "process-1111");
    let stranger = test_owner("S-1-5-21-stop-stranger", "process-2222");
    let session_id = compose_session_id(&original.session_token(), "stop02").expect("id");
    insert_live(&registry, &session_id, original);
    let error = registry
        .stop(&session_id, &stranger)
        .expect_err("different user must stay unauthorized");
    assert_eq!(error.code, ErrorCode::Unauthorized);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn roster_and_history_are_user_scoped_and_include_previous_run_sessions() {
    let (dir, registry, journal) = tmp_delete_registry();
    let previous_run = test_owner("S-1-5-21-roster", "process-1111");
    let caller = test_owner("S-1-5-21-roster", "process-2222");
    let stranger = test_owner("S-1-5-21-other", "process-3333");
    let previous_id = compose_session_id(&previous_run.session_token(), "roster01").expect("id");
    let stranger_id = compose_session_id(&stranger.session_token(), "roster02").expect("id");
    journal
        .upsert_blocking(ended_record(&previous_id, &previous_run.user))
        .expect("previous row");
    journal
        .upsert_blocking(ended_record(&stranger_id, &stranger.user))
        .expect("stranger row");

    let roster = registry.state_snapshots(&caller);
    assert!(roster.iter().any(|session| session.id == previous_id));
    assert!(roster.iter().all(|session| session.id != stranger_id));
    let history = registry.list(&caller).expect("history");
    assert!(history.iter().any(|session| session.id == previous_id));
    assert!(history.iter().all(|session| session.id != stranger_id));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_transition_does_not_requery_the_journal_roster() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-roster-cache", "process-roster-cache");
    let runtime = insert_live_agent(&registry, "s.roster-cache.1", owner.clone());
    let journal_id =
        compose_session_id(&owner.session_token(), "roster-cache-history").expect("journal id");
    journal
        .upsert_blocking(ended_record(&journal_id, &owner.user))
        .expect("journal row");

    let _ = registry.state_snapshots(&owner);
    assert_eq!(registry.journal_list_call_count(), 1);

    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    let _ = registry.state_snapshots(&owner);

    assert_eq!(
        registry.journal_list_call_count(),
        1,
        "a live transition must reuse the cached journal roster"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn journal_roster_does_not_cache_rows_under_revision_that_changed_after_list() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-roster-race", "process-roster-race");
    let initial_id = compose_session_id(&owner.session_token(), "roster-race-initial")
        .expect("initial journal id");
    let added_after_list_id = compose_session_id(&owner.session_token(), "roster-race-after-list")
        .expect("post-list journal id");
    journal
        .upsert_blocking(ended_record(&initial_id, &owner.user))
        .expect("initial journal row");

    let hook_journal = Arc::clone(&journal);
    let hook_owner = owner.clone();
    let hook_id = added_after_list_id.clone();
    registry.set_journal_roster_after_list_hook(Arc::new(move || {
        hook_journal
            .upsert_blocking(ended_record(&hook_id, &hook_owner.user))
            .expect("post-list journal row");
    }));

    // The hook queues a real roster mutation after list() has returned,
    // deterministically reproducing the revision/data mismatch without
    // depending on sleeps or scheduler timing.
    let first = registry.state_snapshots(&owner);
    assert!(first
        .iter()
        .all(|session| session.id != added_after_list_id));

    let second = registry.state_snapshots(&owner);
    assert!(
        second
            .iter()
            .any(|session| session.id == added_after_list_id),
        "a row added after list() must not be hidden by a stale cache"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// S5-09 and S5-04 on the path that matters to a running app: a child
/// created while the client is already attached arrives as a *push*-only
/// row, so the snapshot that push carries must name the child and say which
/// session created it. The row the next full roster build produces must say
/// the same thing, or the two paths disagree about the same session.
#[test]
fn a_push_only_row_carries_the_childs_name_and_creator() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-snapshot-names", "process-snapshot-names");
    let creator = "s.snapshot-names.parent";
    let child = "s.snapshot-names.child";
    // The app is open and holds this roster already: the next state change
    // is served from the cache, which is what makes it a push.
    let _ = registry.state_snapshots(&owner);
    insert_live_agent(&registry, child, owner.clone());
    {
        let mut map = registry.inner.lock().expect("map");
        let live = map
            .get_mut(child)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("the live child");
        live.metadata.display_name = Some("worker".to_string());
        live.metadata.created_by = Some(creator.to_string());
    }

    registry.notify_session_transition(&owner, child);
    let pushed = registry.state_snapshots(&owner);
    assert_eq!(
        registry.full_roster_build_count(),
        1,
        "the row came from the push, not from a rebuild"
    );
    let row = pushed
        .iter()
        .find(|session| session.id == child)
        .expect("the pushed row");
    assert_eq!(
        row.display_name.as_deref(),
        Some("worker"),
        "the push names the child"
    );
    assert_eq!(
        row.created_by.as_deref(),
        Some(creator),
        "the push names the session that created it"
    );

    // The same session through a full build: the two paths agree.
    registry.state_roster_cache.lock().expect("cache").clear();
    let rebuilt = registry.state_snapshots(&owner);
    assert_eq!(registry.full_roster_build_count(), 2, "the cache was gone");
    let row = rebuilt
        .iter()
        .find(|session| session.id == child)
        .expect("the rebuilt row");
    assert_eq!(row.display_name.as_deref(), Some("worker"));
    assert_eq!(row.created_by.as_deref(), Some(creator));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn live_transition_does_not_rebuild_a_large_roster() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-large-roster", "process-large-roster");
    for index in 0..64 {
        let id = compose_session_id(
            &owner.session_token(),
            &format!("roster-history-{index:02}"),
        )
        .expect("journal id");
        journal
            .upsert_blocking(ended_record(&id, &owner.user))
            .expect("journal row");
    }
    let runtimes = (0..8)
        .map(|index| {
            insert_live_agent(&registry, &format!("s.large-roster-{index}"), owner.clone())
        })
        .collect::<Vec<_>>();

    let roster = registry.state_snapshots(&owner);
    assert_eq!(roster.len(), 72);
    assert_eq!(registry.full_roster_build_count(), 1);
    assert_eq!(registry.journal_list_call_count(), 1);

    runtimes[0].publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    let updated = registry.state_snapshots(&owner);

    assert_eq!(updated.len(), 72);
    assert_eq!(registry.full_roster_build_count(), 1);
    assert_eq!(
        registry.journal_list_call_count(),
        1,
        "the transition must not make work proportional to journal-only sessions"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
