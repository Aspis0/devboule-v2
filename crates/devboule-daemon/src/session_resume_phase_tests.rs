//! Direct unit tests of the phases in `session_resume.rs`: each phase is
//! called on its own, so a phase that is wrong fails here even before the
//! through-the-road characterisation in `session_resume_tests.rs` notices.
//! They characterise the extraction, not the behaviour it preserved — the
//! road tests are the ones that predate the helpers.

use super::session_resume_fixture::{
    acp_row, entry_present, insert_transcript, until_row, AcpEnv, ResumeFixture,
};
use super::tests::insert_live_agent;
use super::*;

/// Mutants: the lookup by position instead of by id (a row that is not first
/// is missed), the id validated *after* the lookup (an invalid id answers
/// "no such session" instead of its own refusal), a missing journal answered
/// as a missing row.
#[test]
fn resume_locate_record_finds_the_row_by_id_and_keeps_its_own_refusals() {
    let fixture = ResumeFixture::new("locate");
    let first = fixture.id("first");
    let second = fixture.id("second");
    fixture.write_row(acp_row(&first, &fixture.owner, "handle-first"));
    fixture.write_row(acp_row(&second, &fixture.owner, "handle-second"));
    let _env = AcpEnv::missing_agent();

    let (journal, record) = fixture
        .registry()
        .resume_locate_record(&second)
        .expect("the row is found by id");
    assert_eq!(record.id, second, "not the first row the journal yielded");
    assert_eq!(
        record.peer_session_id.as_deref(),
        Some("handle-second"),
        "the row's own handle"
    );
    assert!(
        Arc::ptr_eq(&journal, fixture.journal()),
        "the journal the caller will write through is the registry's own"
    );

    let error = match fixture.registry().resume_locate_record("not a session id") {
        Ok(_) => panic!("the id alphabet is checked first"),
        Err(error) => error,
    };
    assert_eq!(
        error.message, "Invalid session id.",
        "the validation refusal: {error:?}"
    );

    let ghost = fixture.id("ghost");
    let error = match fixture.registry().resume_locate_record(&ghost) {
        Ok(_) => panic!("no row, no resume"),
        Err(error) => error,
    };
    assert_eq!(
        error.code,
        ErrorCode::SessionNotFound,
        "the lookup's own code: {error:?}"
    );
    assert_eq!(
        error.message, "No session with that id.",
        "the lookup's own sentence: {error:?}"
    );

    let bare_dir = fixture.dir.join("no-journal");
    std::fs::create_dir(&bare_dir).expect("tmp dir");
    let bare = SessionRegistry::new(RuntimePaths::from_dir(&bare_dir), None);
    let error = match bare.resume_locate_record(&second) {
        Ok(_) => panic!("without a journal nothing is readable"),
        Err(error) => error,
    };
    assert_eq!(
        error.code,
        ErrorCode::Journal,
        "the unreadable journal is its own refusal: {error:?}"
    );
    assert_eq!(
        error.message, "The conversation journal is unavailable.",
        "{error:?}"
    );
    fixture.finish();
}

/// Mutants: the generation not bumped (the row keeps its own) or bumped by
/// wrapping (a row at the end of the range opens generation 0).
#[test]
fn resume_stage_command_stages_the_records_family_and_the_next_generation() {
    let fixture = ResumeFixture::new("stage");
    let id = fixture.id("stage");
    let _env = AcpEnv::missing_agent();

    let record = acp_row(&id, &fixture.owner, "handle-stage");
    let (command, generation) = fixture
        .registry()
        .resume_stage_command(&record, "devboule-acp-stub")
        .expect("the override resolves");
    assert_eq!(generation, 2, "the row's generation plus one");
    assert_eq!(
        command.program, "devboule-no-such-agent",
        "the command is the one the record's family resolved"
    );
    assert_eq!(
        command.provider_id.as_deref(),
        Some("devboule-acp-stub"),
        "the persisted provider travels with the command"
    );

    let mut last = record;
    last.generation = u64::MAX;
    let (_command, generation) = fixture
        .registry()
        .resume_stage_command(&last, "devboule-acp-stub")
        .expect("a saturated row still stages");
    assert_eq!(
        generation,
        u64::MAX,
        "the bump saturates: a row at the end of the range opens no generation 0"
    );
    fixture.finish();
}

/// Mutant: `had_live_slot` narrowed to `Live` only — a `Configuring` child
/// answers "no slot", and the resumed session takes a second one.
#[test]
fn resume_evict_previous_answers_the_slot_the_entry_held() {
    let fixture = ResumeFixture::new("evict");
    let owner = fixture.owner.clone();
    let conn = fixture.conn();

    let absent = fixture.id("absent");
    assert!(
        !fixture
            .registry()
            .resume_evict_previous(fixture.journal(), &absent, &owner, &conn)
            .expect("no entry evicts nothing"),
        "an id nothing registered holds no slot"
    );

    let dead = fixture.id("dead");
    let runtime = insert_live_agent(fixture.registry(), &dead, owner.clone());
    runtime.mark_exited(Some(0));
    assert!(
        fixture
            .registry()
            .resume_evict_previous(fixture.journal(), &dead, &owner, &conn)
            .expect("a stopped entry is replaced"),
        "a removed live entry hands its slot to the resume"
    );
    assert!(!entry_present(fixture.registry(), &dead), "and is gone");

    let windowed = fixture.id("windowed");
    insert_live_agent(fixture.registry(), &windowed, owner.clone());
    {
        let mut map = fixture.registry().inner.lock().expect("registry");
        let entry = map.remove(&windowed).expect("the entry");
        match entry {
            RegistryEntry::Live(session) => {
                map.insert(windowed.clone(), RegistryEntry::Configuring(session));
            }
            _ => panic!("the inserted entry was live"),
        }
    }
    let mut map = fixture.registry().inner.lock().expect("registry");
    if let Some(RegistryEntry::Configuring(session)) = map.get_mut(&windowed) {
        Arc::clone(&session.runtime).mark_exited(None);
    }
    drop(map);
    assert!(
        fixture
            .registry()
            .resume_evict_previous(fixture.journal(), &windowed, &owner, &conn)
            .expect("a stopped windowed entry is replaced"),
        "a configuring child held a slot too"
    );

    let transcript = fixture.id("transcript");
    let mut row = acp_row(&transcript, &owner, "handle-transcript");
    row.status = PersistStatus::Ended;
    fixture.write_row(row);
    fixture.journal().pin(&transcript).expect("pin");
    insert_transcript(fixture.registry(), &transcript, owner.clone());
    assert!(
        !fixture
            .registry()
            .resume_evict_previous(fixture.journal(), &transcript, &owner, &conn)
            .expect("a transcript is replaced"),
        "a transcript holds no slot"
    );
    assert!(
        !entry_present(fixture.registry(), &transcript),
        "and is gone"
    );
    fixture.finish();
}

/// Mutant: the guard dropped from the phase, so an entry whose child still
/// runs is evicted and its slot handed on as if the process were gone.
#[test]
fn resume_evict_previous_refuses_a_still_running_child() {
    let fixture = ResumeFixture::new("evict-running");
    let id = fixture.id("running");
    insert_live_agent(fixture.registry(), &id, fixture.owner.clone());
    let conn = fixture.conn();

    let error = fixture
        .registry()
        .resume_evict_previous(fixture.journal(), &id, &fixture.owner, &conn)
        .expect_err("a running child is not evicted");
    assert_eq!(
        error.message, "This session cannot be resumed while its process is running.",
        "{error:?}"
    );
    assert!(
        entry_present(fixture.registry(), &id),
        "the refusal leaves the entry where it was"
    );
    fixture.finish();
}

/// Mutant: the internal sentence changed — the daemon's own invariant
/// failure is what a caller prints when a resume reports success over
/// nothing.
#[test]
fn resume_read_registered_answers_only_a_registered_entry() {
    let fixture = ResumeFixture::new("read");
    let absent = fixture.id("absent");
    let error = fixture
        .registry()
        .resume_read_registered(&absent)
        .expect_err("nothing was registered");
    assert_eq!(
        error.code,
        ErrorCode::Internal,
        "the daemon's own invariant failed: {error:?}"
    );
    assert_eq!(
        error.message, "resumed session was not registered",
        "{error:?}"
    );

    let live = fixture.id("live");
    insert_live_agent(fixture.registry(), &live, fixture.owner.clone());
    let session = fixture
        .registry()
        .resume_read_registered(&live)
        .expect("the registered entry reads back");
    assert_eq!(session.id, live);
    assert!(
        matches!(session.state, SessionState::Live { generation: 1 }),
        "the entry's own state: {:?}",
        session.state
    );
    fixture.finish();
}

/// The generation the marker was given, read where it lands: `mark_ended`
/// writes it into the exit *event*, and the sessions row's own `generation`
/// column belongs to `start_generation` — so the event table is the only
/// place the two can disagree.
fn exit_event_generation(fixture: &ResumeFixture, id: &str) -> u64 {
    let conn = rusqlite::Connection::open(fixture.dir.join("journal.db")).expect("journal file");
    let generation: i64 = conn
        .query_row(
            "SELECT generation FROM events WHERE session_id = ?1 AND kind = 'exit'",
            [id],
            |row| row.get(0),
        )
        .expect("the exit row the marker wrote");
    generation.max(0) as u64
}

/// Mutants: the end marker dropped (the row stays live and the roster
/// renders a phantom recovered session), the disown fallback dropped, or the
/// marker given the wrong generation. The fallback's *order* — before the
/// unbounded end-marker wait — is not observable in the final state: the two
/// writes are separate columns, and either order leaves both set.
#[test]
fn resume_end_generation_detached_writes_the_end_and_the_disown_fallback() {
    let fixture = ResumeFixture::new("end-marker");
    let id = fixture.id("end-marker");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-end"));

    resume_end_generation_detached(fixture.journal(), &id, 7, false, "handle-end".to_string());
    let row = until_row(&fixture, &id, "the end marker landed", |row| {
        row.status != PersistStatus::Live
    });
    assert_eq!(
        exit_event_generation(&fixture, &id),
        7,
        "the marker ends the generation it was handed"
    );
    assert_eq!(
        row.disowned_peer_session_id, None,
        "nothing disowned this handle"
    );

    let second = fixture.id("disowned-end");
    fixture.write_row(acp_row(&second, &fixture.owner, "handle-disowned"));
    resume_end_generation_detached(
        fixture.journal(),
        &second,
        3,
        true,
        "handle-disowned".to_string(),
    );
    let row = until_row(&fixture, &second, "both writes landed", |row| {
        row.status != PersistStatus::Live && row.disowned_peer_session_id.is_some()
    });
    assert_eq!(
        exit_event_generation(&fixture, &second),
        3,
        "the fallback thread ends its own generation"
    );
    assert_eq!(
        row.disowned_peer_session_id.as_deref(),
        Some("handle-disowned"),
        "the fallback marked the handle it was given"
    );
    fixture.finish();
}
