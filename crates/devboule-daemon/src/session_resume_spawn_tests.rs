//! Characterisation tests for the spawn arms of the resume road
//! (`SessionRegistry::resume`), split by subject from
//! `session_resume_tests.rs`; the fixture and the ACP override harness both
//! live in `session_resume_fixture.rs`. Every test here calls the road and
//! asserts what it does, never a helper.
//!
//! What only a real spawn can prove lives here: the finish report — and the
//! unfinished runtime — a replaced child leaves behind, the detached end
//! marker of a failed respawn, the disown classification, and the arm of a
//! resume the far agent honours.

use super::session_resume_fixture::{
    acp_row, entry_present, insert_transcript, row_with_unreadable_overlay, take_bystander_slot,
    until_row, wait_for_child_finished, AcpEnv, ResumeFixture,
};
use super::tests::{insert_child, insert_live_agent};
use super::*;

/// Mutants: `child_ended_with` dropped (no finish record), `teardown_session`
/// in place of the resume teardown (the runtime is closed), the Err arm's
/// `session_finished` dropped (the count keeps the dead child's slot).
#[test]
fn evicting_a_dead_child_reports_its_end_and_leaves_its_runtime_open() {
    let fixture = ResumeFixture::new("report");
    let creator = fixture.id("creator");
    let child = fixture.id("child");
    fixture.write_row(acp_row(&child, &fixture.owner, "handle-child"));
    // The creator's own row: `replay` reads events by it.
    fixture.write_row(new_session_record(
        &creator,
        fixture.owner.user.clone(),
        None,
        SessionKind::Acp,
        "Creator",
    ));
    insert_live_agent(fixture.registry(), &creator, fixture.owner.clone());
    let child_runtime = insert_child(fixture.registry(), &child, fixture.owner.clone(), &creator);
    fixture
        .registry()
        .commit_agent_child_for_test(&creator, &child, true);
    child_runtime.mark_exited(Some(0));
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);

    let error = fixture
        .resume(&child, &fixture.conn())
        .expect_err("the respawn fails");
    assert!(
        error.message.starts_with("Could not start ACP agent "),
        "{error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        0,
        "the dead child's slot is paid off by the failed respawn"
    );
    assert!(
        !entry_present(fixture.registry(), &child),
        "the evicted entry is gone"
    );
    wait_for_child_finished(&fixture, &creator, &child);
    assert!(
        child_runtime.can_publish_agent_user_message(),
        "the resumed generation's runtime is not finished by the eviction"
    );
    fixture.finish();
}

/// Mutant: the `session_finished` on the lineage arm dropped — the slot the
/// gate took is never given back, and idle shutdown never re-arms.
#[test]
fn an_unreadable_overlay_gives_back_the_slot_the_resume_took() {
    let fixture = ResumeFixture::new("overlay");
    let id = fixture.id("overlay");
    fixture.write_row(row_with_unreadable_overlay(
        &id,
        &fixture.owner,
        &fixture.id("creator"),
        "handle-overlay",
    ));
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("the stored overlay is unreadable");
    assert_eq!(
        error.code,
        ErrorCode::Internal,
        "the read failure is internal: {error:?}"
    );
    assert_eq!(
        error.message,
        format!(
            "cannot resume session '{id}': its stored tool overlay is unreadable (sessions.overlay)"
        ),
        "{error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "the slot the gate took came back"
    );
    assert_eq!(
        fixture.row(&id).generation,
        1,
        "nothing was opened on the row"
    );
    fixture.finish();
}

/// The Err arm's whole tail: the slot comes back, the provider is measured
/// failed, and the detached thread writes the row's end marker (a phantom
/// live row would render a recovered session that does not exist).
/// Mutants: the arm's `session_finished` dropped (the count stays at the
/// bystander), `record_provider_health` dropped (the provider reads
/// `unknown`), the end-marker thread never spawned (the row stays live).
#[test]
fn a_failed_respawn_ends_the_generation_and_gives_back_its_slot() {
    let fixture = ResumeFixture::new("failed");
    let id = fixture.id("failed");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-failed"));
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("the respawn fails");
    assert!(
        error
            .message
            .starts_with("Could not start ACP agent devboule-no-such-agent"),
        "the spawn failure's own sentence: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "the slot the gate took came back"
    );
    let row = until_row(
        &fixture,
        &id,
        "the row was ended by the detached thread",
        |row| row.status != PersistStatus::Live,
    );
    assert_eq!(
        row.generation, 2,
        "the end marker names the generation the resume opened"
    );
    let health = fixture.state.provider_health("devboule-acp-stub");
    assert!(
        health.starts_with("failed: Could not start ACP agent devboule-no-such-agent"),
        "the failed respawn is measured against its provider: {health}"
    );
    fixture.finish();
}

/// P6's detached thread and P5's classification, on the one fact a failed
/// resume can carry: the far side's handle names a session it does not have.
/// The mark and the end marker are both written (the mark on the dispatch
/// thread *and* on the fallback thread — the two roads write the same
/// idempotent column), and a cached `Transcript` entry hydrated during the
/// spawn window is evicted so it cannot serve a stale `resumable`.
/// Mutants: the classification read off the error code (a dropped
/// `SessionNotFound` test leaves the family's own wire code and writes no
/// mark), the stale-transcript eviction dropped, the end-marker thread
/// dropped.
#[test]
fn a_disowned_handle_is_marked_and_the_stale_transcript_entry_is_evicted() {
    let fixture = ResumeFixture::new("disowned");
    let id = fixture.id("disowned");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-disowned"));
    let _env = AcpEnv::stub(&[
        ("DEVBOULE_STUB_REFUSE_LOAD", "1".to_string()),
        // A load slow enough that the entry below lands inside the spawn
        // window, which is the only place that eviction reaches.
        ("DEVBOULE_STUB_DELAY_LOAD_MS", "2000".to_string()),
    ]);
    take_bystander_slot(&fixture.state);

    let registry = fixture.registry().clone();
    let state = Arc::clone(&fixture.state);
    let owner = fixture.owner.clone();
    let resume_id = id.clone();
    let resuming = std::thread::spawn(move || {
        let conn = ConnHandle::new(7);
        state.sessions.resume(&state, &resume_id, &owner, &conn)
    });
    until_row(&fixture, &id, "the new generation was opened", |row| {
        row.status == PersistStatus::Live && row.generation == 2
    });
    insert_transcript(&registry, &id, fixture.owner.clone());
    assert!(
        entry_present(&registry, &id),
        "the cached entry is there for the spawn window to find"
    );
    let error = resuming
        .join()
        .expect("the resume thread")
        .expect_err("a disowned handle fails the resume");

    assert_eq!(
        error.code,
        ErrorCode::Io,
        "an ACP disown keeps the Io code its family always reported: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "the slot the gate took came back"
    );
    assert!(
        !entry_present(&registry, &id),
        "the stale transcript entry the window hydrated is evicted"
    );
    let row = until_row(&fixture, &id, "the disowned row was ended", |row| {
        row.status != PersistStatus::Live
    });
    assert_eq!(
        row.disowned_peer_session_id.as_deref(),
        Some("handle-disowned"),
        "the mark names the handle the resume tried to load"
    );
    fixture.finish();
}

/// The Ok arm: the slot stays taken (the session is live), and the provider
/// honouring this exact handle is the one fact that proves a mark wrong.
/// Mutants: a `session_finished` added to the Ok arm (count back to the
/// bystander), the gate's increment dropped (the live session never takes its
/// slot), the `clear_peer_session_disown` dropped (the stale mark stays).
#[test]
fn a_successful_resume_keeps_its_slot_and_clears_the_mark_it_honoured() {
    let fixture = ResumeFixture::new("resumed");
    let id = fixture.id("resumed");
    let mut row = acp_row(&id, &fixture.owner, "stub-session");
    row.disowned_peer_session_id = Some("stub-session".to_string());
    fixture.write_row(row);
    let _env = AcpEnv::stub(&[]);
    take_bystander_slot(&fixture.state);

    let session = fixture
        .resume(&id, &fixture.conn())
        .expect("the stub honours the load");
    assert_eq!(session.id, id, "the resumed session is the row's own");
    assert!(
        matches!(session.state, SessionState::Live { generation: 2 }),
        "the resumed generation is live: {:?}",
        session.state
    );
    assert_eq!(
        fixture.state.live_session_count(),
        2,
        "the resumed session holds the slot the gate took"
    );
    assert_eq!(
        fixture.state.provider_health("devboule-acp-stub"),
        "ok",
        "the honoured spawn is measured against its provider"
    );
    let row = until_row(&fixture, &id, "the mark was cleared", |row| {
        row.disowned_peer_session_id.is_none()
    });
    assert_eq!(row.status, PersistStatus::Live, "the row stays live");
    assert_eq!(row.generation, 2, "on the generation the resume opened");
    let _ = fixture.registry().close(&id, &fixture.owner, &None);
    fixture.finish();
}
