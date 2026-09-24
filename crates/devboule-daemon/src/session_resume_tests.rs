//! Characterisation tests for the resume road (`SessionRegistry::resume`),
//! split by subject from `session_resume_spawn_tests.rs` and
//! `session_resume_phase_tests.rs`: every test here calls the road and asserts
//! what it does, never a helper, because a test written after an extraction
//! characterises the extraction. The fixture they share is
//! `session_resume_fixture.rs`.
//!
//! This file holds the paths that end before a spawn: the running-process
//! guard, the entry owner's guard, the shutdown gate, the slot a replaced
//! child hands over, and the unpin an evicted transcript owes.
//!
//! Four `session_finished` call sites keep the lifecycle slot count honest,
//! and a missing one leaks a slot in silence — nothing else in the suite
//! notices, the daemon just never re-arms idle shutdown. Every failure path
//! in all three files therefore asserts the count after the call, not the
//! return value. One slot is seeded before each call, so a stray increment and
//! a missing decrement both show up as a wrong number.

use super::session_resume_fixture::{
    acp_row, entry_present, insert_transcript, row_with_unreadable_overlay, take_bystander_slot,
    AcpEnv, ResumeFixture,
};
use super::tests::insert_live_agent;
use super::*;

/// Mutant: the running-process guard dropped — the resume evicts a live
/// child, mints a generation on its row, and takes its slot. The narrower
/// question (which accessor the guard asks) is the next test's claim.
#[test]
fn a_running_child_is_refused_before_anything_is_evicted() {
    let fixture = ResumeFixture::new("running");
    let id = fixture.id("running");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-running"));
    insert_live_agent(fixture.registry(), &id, fixture.owner.clone());
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("a running process cannot be resumed over");
    assert_eq!(
        error.code,
        ErrorCode::InvalidRequest,
        "the guard's own code: {error:?}"
    );
    assert_eq!(
        error.message, "This session cannot be resumed while its process is running.",
        "the guard's own sentence: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "a refusal before the gate takes no slot"
    );
    assert!(
        entry_present(fixture.registry(), &id),
        "the refusal is not a teardown: the live entry stays"
    );
    assert_eq!(
        fixture.row(&id).generation,
        1,
        "no generation was started on the row"
    );
    fixture.finish();
}

/// Mutant: the guard asked over a peer-visible accessor instead of
/// `as_child_process` — a `Configuring` child is a running child (the
/// re-audit's P2-1), and a resume would replace it out from under its
/// in-flight create.
#[test]
fn a_configuring_child_is_a_running_child_for_the_resume_guard() {
    let fixture = ResumeFixture::new("configuring");
    let id = fixture.id("configuring");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-configuring"));
    insert_live_agent(fixture.registry(), &id, fixture.owner.clone());
    {
        let mut map = fixture.registry().inner.lock().expect("registry");
        let entry = map.remove(&id).expect("the entry");
        match entry {
            RegistryEntry::Live(session) => {
                map.insert(id.clone(), RegistryEntry::Configuring(session));
            }
            _ => panic!("the inserted entry was live"),
        }
    }
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("a configuring child holds a running process");
    assert_eq!(
        error.message, "This session cannot be resumed while its process is running.",
        "the window is not a hole in the guard: {error:?}"
    );
    assert_eq!(fixture.state.live_session_count(), 1, "no slot taken");
    assert!(
        matches!(
            fixture.registry().inner.lock().expect("registry").get(&id),
            Some(RegistryEntry::Configuring(_))
        ),
        "the windowed child is still there, still configuring"
    );
    fixture.finish();
}

/// Mutant: the shutdown gate removed — a resume during shutdown mints a
/// generation and takes a slot the daemon is waiting to be free.
#[test]
fn a_resume_during_shutdown_is_refused_and_takes_no_slot() {
    let fixture = ResumeFixture::new("shutdown");
    let id = fixture.id("shutdown");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-shutdown"));
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);
    fixture.state.request_shutdown();

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("a shutting-down daemon refuses the resume");
    assert_eq!(
        error.code,
        ErrorCode::ShuttingDown,
        "the gate's own code: {error:?}"
    );
    assert_eq!(
        error.message, "daemon is shutting down",
        "the gate's own sentence: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "the gate refuses and takes nothing"
    );
    assert_eq!(
        fixture.row(&id).generation,
        1,
        "the row was never opened for a new generation"
    );
    fixture.finish();
}

/// The record's owner and the entry's owner are two objects, and the record
/// gate is not the entry guard: a row this caller owns can still be
/// registered under somebody else's entry. Mutant: the `check_user_owner`
/// guard dropped from the eviction — the resume evicts a stranger's entry,
/// reports its end to their creator, and registers this caller's session
/// over it.
#[test]
fn a_resume_refuses_an_entry_another_user_owns() {
    let fixture = ResumeFixture::new("entry-owner");
    let id = fixture.id("entry-owner");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-entry"));
    let stranger = OwnerId::new("s-1-5-21-resume-stranger", "stranger-client").expect("owner");
    insert_live_agent(fixture.registry(), &id, stranger);
    {
        let map = fixture.registry().inner.lock().expect("registry");
        let session = map
            .get(&id)
            .and_then(RegistryEntry::as_child_process)
            .expect("the entry");
        // Dead, so the only guard left between the resume and the eviction is
        // the owner's.
        Arc::clone(&session.runtime).mark_exited(None);
    }
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("another user's entry is not this caller's to replace");
    assert_eq!(
        error.code,
        ErrorCode::Unauthorized,
        "the entry guard's own code: {error:?}"
    );
    assert_eq!(
        error.message, "This client is not authorized to use that session.",
        "the entry guard's own sentence: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "the refusal happens before the gate takes a slot"
    );
    assert!(
        entry_present(fixture.registry(), &id),
        "the stranger's entry is still there"
    );
    assert_eq!(fixture.row(&id).generation, 1, "the row was not opened");
    fixture.finish();
}

/// The gate's exact shape: `had_live_slot` is asked first, so the gate does
/// not even call `session_started()` for a resume that replaces a dead child
/// — the slot it inherits is the one the child already held.
/// Mutant: the two operands swapped — the swapped gate calls
/// `session_started()` on this path too, increments, and the failed
/// respawn's `session_finished` gives back only one of the two slots.
#[test]
fn a_resume_that_replaces_a_dead_child_takes_no_second_slot() {
    let fixture = ResumeFixture::new("handoff");
    let id = fixture.id("handoff");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-handoff"));
    insert_live_agent(fixture.registry(), &id, fixture.owner.clone());
    let evicted = fixture
        .registry()
        .inner
        .lock()
        .expect("registry")
        .get(&id)
        .and_then(RegistryEntry::as_child_process)
        .map(|session| Arc::clone(&session.runtime))
        .expect("the live runtime");
    evicted.mark_exited(None);
    let _env = AcpEnv::missing_agent();
    // The one slot is the dead child's own: the resumed session inherits it.
    take_bystander_slot(&fixture.state);

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("the respawn still fails");
    assert!(
        error.message.starts_with("Could not start ACP agent "),
        "the resume ran past the gate and died at the spawn: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        0,
        "the replaced child's slot is released by the failed respawn, \
         and no second one was ever taken"
    );
    assert!(
        !entry_present(fixture.registry(), &id),
        "the dead entry was evicted and nothing re-registered it"
    );
    fixture.finish();
}

/// The same short-circuit from the refusal's side: a shutting-down daemon
/// still serves a resume that replaces a dead child, because that resume is
/// not a new session — the slot it spends is one the daemon already counts.
/// Mutant: the gate asking `session_started()` unconditionally (the
/// `had_live_slot` exemption dropped) — this resume would be refused
/// `ShuttingDown` although it takes nothing from the daemon.
#[test]
fn a_shutdown_daemon_still_resumes_the_slot_its_dead_child_held() {
    let fixture = ResumeFixture::new("handoff-shutdown");
    let id = fixture.id("handoff-shutdown");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-shutdown"));
    insert_live_agent(fixture.registry(), &id, fixture.owner.clone());
    {
        let map = fixture.registry().inner.lock().expect("registry");
        let session = map
            .get(&id)
            .and_then(RegistryEntry::as_child_process)
            .expect("the live child");
        Arc::clone(&session.runtime).mark_exited(None);
    }
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);
    fixture.state.request_shutdown();

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("the respawn still fails");
    assert!(
        error.message.starts_with("Could not start ACP agent "),
        "the shutdown gate let the slot-owning resume through: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        0,
        "the slot came back with the failed respawn"
    );
    fixture.finish();
}

/// Mutant: `journal.unpin` dropped from the Transcript arm — the row stays
/// pinned forever and retention can never reclaim it, with nothing failing.
#[test]
fn a_resume_over_a_transcript_row_unpins_the_row_it_replaces() {
    let fixture = ResumeFixture::new("unpin");
    let id = fixture.id("transcript");
    let mut row = row_with_unreadable_overlay(
        &id,
        &fixture.owner,
        &fixture.id("creator"),
        "handle-transcript",
    );
    // The same row fails the resume at the lineage read, before any
    // `start_generation` could make it live again.
    row.status = PersistStatus::Ended;
    fixture.write_row(row);
    let mut live = new_session_record(
        fixture.id("bystander"),
        fixture.owner.user.clone(),
        None,
        SessionKind::Acp,
        "Bystander",
    );
    live.status = PersistStatus::Live;
    fixture.write_row(live);
    fixture.journal().pin(&id).expect("pin");
    insert_transcript(fixture.registry(), &id, fixture.owner.clone());
    fixture
        .journal()
        .retention_set(RetentionPatch {
            max_age_ms: None,
            max_bytes: None,
            max_sessions: Some(1),
            session_max_bytes: None,
        })
        .expect("session cap");
    // Two rows over a cap of one, and the only non-live one is pinned: the
    // overage is unreclaimable exactly until the pin goes.
    assert_eq!(
        fixture
            .journal()
            .usage()
            .expect("usage")
            .unreclaimable
            .sessions_over,
        1,
        "the pinned transcript row is the unreclaimable overage"
    );
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("the stored overlay is unreadable");
    assert_eq!(
        error.message,
        format!(
            "cannot resume session '{id}': its stored tool overlay is unreadable (sessions.overlay)"
        ),
        "the refusal names the row it read: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "the refused resume gave its slot back"
    );
    assert!(
        !entry_present(fixture.registry(), &id),
        "the transcript entry was evicted"
    );
    assert_eq!(
        fixture
            .journal()
            .usage()
            .expect("usage")
            .unreclaimable
            .sessions_over,
        0,
        "the evicted transcript row was unpinned"
    );
    fixture.finish();
}

/// The register arm's own error: the broker is stopped here, which is a
/// production state (`McpServerHandle::drop` sets the flag), so the arm needs
/// no seam — only a handle that was started and dropped.
/// Mutant: the register arm's `session_finished` dropped — the slot the gate
/// took is never given back and idle shutdown never re-arms.
#[test]
fn a_stopped_broker_gives_back_the_slot_the_resume_took() {
    let fixture = ResumeFixture::new("mcp-stopped");
    let id = fixture.id("stopped");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-stopped"));
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);
    let server = fixture.state.mcp.start(&fixture.state).expect("MCP server");
    drop(server);

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("a stopped broker refuses the registration");
    assert_eq!(error.code, ErrorCode::Io, "the arm's own code: {error:?}");
    assert_eq!(
        error.message, "The MCP broker is stopped.",
        "the refusal comes from the stop flag, not another register failure: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "the slot the gate took came back"
    );
    assert_eq!(
        fixture.row(&id).generation,
        1,
        "the arm is before the generation is opened"
    );
    fixture.finish();
}

/// The generation arm's own error, with the refusal installed by the test:
/// the production trigger — the row vanishing between `list()` and the UPDATE
/// — is a race no single thread can force, so a `BEFORE UPDATE` trigger on the
/// journal file stands in for it. The arm cannot tell the two apart; its whole
/// content is "give the slot back, revoke the registration, return".
/// Mutant: the generation arm's `session_finished` dropped — the slot the gate
/// took is never given back.
#[test]
fn a_refused_generation_start_gives_back_the_slot_the_resume_took() {
    let fixture = ResumeFixture::new("generation");
    let id = fixture.id("generation");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-generation"));
    let _env = AcpEnv::missing_agent();
    take_bystander_slot(&fixture.state);
    let db = rusqlite::Connection::open(fixture.dir.join("journal.db")).expect("journal file");
    db.execute_batch(&format!(
        "CREATE TRIGGER refuse_generation BEFORE UPDATE ON sessions \
         WHEN OLD.id = '{id}' BEGIN SELECT RAISE(ABORT, 'refused'); END;"
    ))
    .expect("the refusal trigger");

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("the generation cannot be opened");
    assert_eq!(
        error.code,
        ErrorCode::Journal,
        "the trigger's refusal, not the missing-row race: {error:?}"
    );
    assert!(
        error.message.contains("refused"),
        "the arm surfaced the refused write: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "the slot the gate took came back"
    );
    assert_eq!(
        fixture.row(&id).generation,
        1,
        "the refused UPDATE opened no generation"
    );
    assert!(
        !fixture.state.mcp.is_registered(&id),
        "the registration the arm already minted was revoked"
    );
    drop(db);
    fixture.finish();
}

/// The pre-flight at the road's own level: a session whose recorded directory
/// is gone, and which has **no conversation to recover**, is refused in words
/// and nothing is launched for it. The stub's pid file is the evidence — it is
/// written by the stub's `main`, so a file that never appears is a provider
/// that never ran (the brief's "not only the text").
///
/// Mutants: the pre-flight dropped (the spawn goes ahead, the pid file appears
/// and the provider answers for us); the refusal turned into a replacement
/// session that has nothing to carry; the row evicted anyway.
#[test]
fn a_resume_for_a_gone_directory_and_an_empty_transcript_refuses_without_spawning() {
    let fixture = ResumeFixture::new("gone-dir");
    let id = fixture.id("gone-dir");
    let gone = fixture.dir.join("removed-worktree");
    let pids = fixture.dir.join("stub pids.txt");
    let _env = AcpEnv::stub(&[(
        "DEVBOULE_ACP_STUB_PIDS_FILE",
        pids.to_string_lossy().into_owned(),
    )]);
    let mut row = acp_row(&id, &fixture.owner, "handle-gone");
    row.cwd = Some(gone.to_string_lossy().into_owned());
    fixture.write_row(row);
    take_bystander_slot(&fixture.state);

    let outcome = fixture.resume(&id, &fixture.conn());
    // The stub writes its pid file in `main`, before it reads a single frame, so
    // the check is the file and not a timing window — and it is made **first**,
    // because the claim it carries is the one a road that refused *and* spawned
    // anyway would otherwise pass while the sentence looked right.
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !pids.exists(),
        "no provider was launched for a folder that does not exist"
    );
    let error = outcome.expect_err("nothing can be launched in a directory that is gone");
    assert_eq!(
        error.code,
        ErrorCode::WorkspaceUnavailable,
        "the pre-flight's own code: {error:?}"
    );
    assert_eq!(
        error.message,
        format!(
            "the folder this session worked in no longer exists: {}",
            crate::workspace::plain_path(&gone.to_string_lossy())
        ),
        "the sentence names the path it looked for: {error:?}"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        1,
        "the refusal takes no slot"
    );
    assert!(
        !entry_present(fixture.registry(), &id),
        "no entry was registered for the refused resume"
    );
    assert_eq!(
        fixture.row(&id).generation,
        1,
        "the row was never opened for a new generation"
    );
    fixture.finish();
}
