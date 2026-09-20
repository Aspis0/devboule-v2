//! The creation-budget tests, moved whole out of `session_tests.rs` lines
//! 1894-2506 (at `141bdfa`): one live child per creator and eight daemon-wide,
//! depth judged on the child's own depth, the creation card owed once per
//! creator session, the ten-an-hour window and the sweep that rolls it, a
//! reservation released once by its identity, a commit registering the child
//! the spawn named, the in-flight creation key, an end that arrives before its
//! commit, a child resumed twice counted once, a report surviving a steerer
//! that errors, a child's end claiming its report once, and a pending card
//! refusing a concurrent creation. Every line below is byte-identical to its
//! text there apart from this header; `insert_live_agent_with_turn_control` and
//! `NoopKiller` are promoted to `pub(super)` for this move, and the other
//! fixtures come from the provider's own imports.

use super::tests::{
    insert_live_agent_with_kind_and_writer, insert_live_agent_with_turn_control, test_owner,
    tmp_delete_registry, FailingWriter, NoopKiller, RecordingWriter,
};
use super::*;

/// The creation budget (`S5` decision 5): one slot per creator, ten
/// creations an hour, depth two, eight daemon-wide. Nothing here spawns
/// anything — the caps are decided before the first spawn, which is why
/// they hold under a race and why they are testable without a provider.
#[test]
fn the_live_child_cap_is_per_creator() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let alex = "session-alex";
    let blair = "session-blair";
    for index in 0..MAX_LIVE_CHILDREN_PER_CREATOR {
        let ticket = registry
            .test_ticket(alex, 1)
            .expect("within the creator's cap");
        assert_eq!(ticket.caps().live_children, index as u32 + 1);
        assert_eq!(ticket.caps().max_live_children, 3);
        // The same sequence the handler walks (audit S5-06): the first
        // reservation leaves the card with the human, and the allow is what
        // opens the gate for the ones after it.
        if ticket.card_owed {
            registry.accept_agent_creation(alex);
        }
        registry.commit_agent_child_for_test(alex, &format!("child-{index}"), true);
    }
    let refused = registry
        .test_ticket(alex, 1)
        .expect_err("a fourth live child is over the cap");
    assert_eq!(refused.message, "creation limit exceeded; do not retry");
    // Another creator's budget is untouched by the first one's spending.
    let other = registry
        .test_ticket(blair, 1)
        .expect("every creator has its own three");
    assert_eq!(other.caps().live_children, 1);
    registry.abandon_agent_creation_for_test(blair);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

#[test]
fn the_daemon_wide_cap_counts_live_agent_sessions() {
    let (_dir, registry, journal) = tmp_delete_registry();
    for index in 0..MAX_LIVE_AGENT_SESSIONS {
        let creator = format!("session-{index}");
        registry
            .test_ticket(&creator, 1)
            .expect("within the daemon-wide cap");
        registry.commit_agent_child_for_test(&creator, &format!("child-{index}"), true);
    }
    let refused = registry
        .test_ticket("session-late", 1)
        .expect_err("ninth live agent is over the cap");
    assert_eq!(refused.message, "creation limit exceeded; do not retry");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

#[test]
fn the_depth_cap_is_judged_on_the_childs_own_depth() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let ticket = registry
        .test_ticket("session-deep", MAX_AGENT_DEPTH)
        .expect("depth two may create");
    assert_eq!(ticket.caps().depth, MAX_AGENT_DEPTH);
    assert_eq!(ticket.caps().max_depth, MAX_AGENT_DEPTH);
    registry.abandon_agent_creation_for_test("session-deep");
    let refused = registry
        .test_ticket("session-deeper", MAX_AGENT_DEPTH + 1)
        .expect_err("a grandchild may not create");
    assert_eq!(refused.message, "depth limit; do not retry");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

#[test]
fn the_creation_card_is_owed_once_per_creator_session_and_a_refusal_keeps_it_shut() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let alex = "session-alex";
    let first = registry.test_ticket(alex, 1).expect("first");
    assert!(first.card_owed(), "the first creation asks the human");
    // Refused: the slot goes back and the gate is still shut.
    registry.abandon_agent_creation_for_test(alex);
    let again = registry
        .test_ticket(alex, 1)
        .expect("a refusal does not spend the slot");
    assert!(again.card_owed(), "a refusal must not open the gate");
    registry.accept_agent_creation(alex);
    registry.commit_agent_child_for_test(alex, "child-1", true);
    registry.accept_agent_creation(alex);
    let second = registry.test_ticket(alex, 1).expect("second");
    assert!(!second.card_owed(), "one card per creator session");
    registry.abandon_agent_creation_for_test(alex);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

#[test]
fn the_hourly_cap_is_ten_and_a_sweep_rolls_the_window() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let alex = "session-alex";
    // A refused card spends nothing, so the hour is spent by real
    // creations: create, finish, close, ten times.
    for index in 0..MAX_CREATIONS_PER_WINDOW {
        // The ticket is held across the commit: it *is* the reservation,
        // and a dropped one takes its hour-slot with it.
        let ticket = registry
            .test_ticket(alex, 1)
            .unwrap_or_else(|_| panic!("creation {index} is within the hour"));
        registry.accept_agent_creation(alex);
        registry.commit_agent_child_for_test(alex, &format!("child-{index}"), true);
        registry.release_agent_child(&format!("child-{index}"));
        drop(ticket);
    }
    let refused = registry
        .test_ticket(alex, 1)
        .expect_err("eleventh creation in the hour");
    assert_eq!(refused.message, "creation limit exceeded; do not retry");
    // An hour later the window has rolled: the same creator has its ten back.
    {
        let mut table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.sweep(Instant::now() + CREATION_WINDOW + Duration::from_secs(1));
    }
    let allowed = registry.test_ticket(alex, 1).expect("a new hour");
    assert_eq!(allowed.caps().creations_this_hour, 1);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// Audit S5B-02: the reservation is an identity, so a release handled on
/// two paths cannot un-count a neighbour's creation.
///
/// The audit found exactly that shape: the spawn-error path and the
/// handler's error arm both gave the same slot back, and a second decrement
/// lands on whoever reserved next.
#[test]
fn a_reservation_is_released_once_by_its_identity_and_a_second_release_is_a_no_op() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let alex = "session-alex";
    let first = registry.test_ticket(alex, 1).expect("first");
    let id = first.reservation();
    registry.accept_agent_creation(alex);
    let second = registry.test_ticket(alex, 1).expect("second");
    let blair = "session-blair";
    let neighbour = registry.test_ticket(blair, 1).expect("a neighbour");

    assert!(
        registry.release_agent_creation(alex, id),
        "the first release"
    );
    // The same id again — the double abandon the audit found — is a no-op.
    assert!(
        !registry.release_agent_creation(alex, id),
        "the second release of one id does nothing"
    );
    // And it did not take the neighbour's reservation with it.
    let held = |creator: &str| {
        let table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table
            .creators
            .get(creator)
            .map(AgentCreatorCaps::held)
            .unwrap_or(0)
    };
    assert_eq!(held(alex), 1, "the creator's own second reservation stands");
    assert_eq!(
        held(blair),
        1,
        "the neighbour's reservation was not un-counted"
    );

    drop(first);
    drop(second);
    drop(neighbour);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// Audit S5B-04, as far as this pass got: the *reservation* carries no
/// child id, so the caller's commit names the child the spawn produced and
/// the end routine finds it. Registering the link before the spawn (the
/// ruling's stronger form) is deferred: the pre-composed id closed the
/// child's own transport before its handshake — see the report.
#[test]
fn a_commit_registers_the_child_the_spawn_named_and_an_end_releases_it() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let alex = "session-alex";
    let ticket = registry.test_ticket(alex, 1).expect("the reservation");
    let reservation = ticket.reservation();
    let live = |registry: &SessionRegistry| {
        let table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.live_agent_sessions()
    };
    assert_eq!(
        live(&registry),
        1,
        "the reservation is a session the daemon promised (S5-02)"
    );
    assert!(
        registry
            .commit_agent_creation(alex, reservation, "s.alex.child", true)
            .0,
        "the commit of a live creator"
    );
    ticket.commit();
    assert_eq!(
        live(&registry),
        1,
        "the reservation became the child: still exactly one session"
    );
    // The end routine finds that link, releases the slot and leaves the
    // creator able to create again.
    registry.child_ended_with("s.alex.child", None, None, None);
    assert_eq!(live(&registry), 0, "the slot came back");
    registry.accept_agent_creation(alex);
    registry
        .test_ticket(alex, 1)
        .unwrap_or_else(|error| panic!("the creator may create again: {}", error.message));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// Audit S5-02: the daemon-wide cap counts what the daemon has *promised*,
/// not only what it has finished creating.
///
/// The reservation is the promise — it is taken before the card and before
/// any spawn — so two creators racing can no longer each pass the global
/// check and then commit past it.
#[test]
fn the_daemon_wide_cap_counts_in_flight_reservations_too() {
    let (_dir, registry, journal) = tmp_delete_registry();
    // Every reservation is left in flight: nothing is committed.
    // The tickets are held: a reservation lives exactly as long as its
    // ticket, and dropping one here would give the slot back between
    // iterations (audit S5B-02 made that explicit).
    let mut held = Vec::new();
    for index in 0..MAX_LIVE_AGENT_SESSIONS {
        let creator = format!("session-{index}");
        let ticket = registry
            .test_ticket(&creator, 1)
            .expect("within the daemon-wide cap");
        assert_eq!(
            ticket.caps().live_agent_sessions as usize,
            index + 1,
            "the reservation being asked about is counted"
        );
        held.push(ticket);
    }
    let refused = registry
        .test_ticket("session-late", 1)
        .expect_err("the cap counts what is in flight, not only what committed");
    assert_eq!(refused.message, "creation limit exceeded; do not retry");
    // Abandoning one gives the slot back, which is the other half of the
    // same rule: a refused card must not hold the daemon-wide budget.
    registry.abandon_agent_creation_for_test("session-0");
    let allowed = registry
        .test_ticket("session-late", 1)
        .expect("a released reservation is room again");
    assert_eq!(
        allowed.caps.live_agent_sessions as usize,
        MAX_LIVE_AGENT_SESSIONS
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// Audit S5-03: one retry identity, held from before the first check until
/// the call answers.
///
/// The refusal costs nothing (`S5-03`'s "spends nothing" is the point: a
/// re-sent frame must not spend a slot, a card or a session), and the key
/// is free again as soon as the call that held it returns — including when
/// that call failed.
#[test]
fn an_in_flight_creation_key_refuses_a_second_call_and_is_released_by_its_hold() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let key = "mcp-create:session-alex:7";
    let hold = registry
        .hold_creation_key(key)
        .expect("the first call holds the key");
    let refused = match registry.hold_creation_key(key) {
        Ok(_second) => panic!("a second call while the first is in flight"),
        Err(error) => error,
    };
    assert_eq!(refused.message, "creation in progress; retry");
    // The refusal spent nothing: the creator has no reservation and no
    // card outstanding, so its next reservation is a first reservation.
    let first = registry
        .test_ticket("session-alex", 1)
        .expect("nothing was spent");
    assert!(first.card_owed(), "the card is still owed");
    registry.abandon_agent_creation_for_test("session-alex");
    drop(hold);
    let after = registry
        .hold_creation_key(key)
        .expect("the key is free once the call that held it ended");
    drop(after);
    // A hold that was committed releases the key the same way, and the
    // result it remembered is what a later retry reads.
    let mut committed = registry.hold_creation_key(key).expect("free again");
    committed.commit();
    assert!(
        registry.hold_creation_key(key).is_ok(),
        "a committed hold is not still in flight"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// Audit S5-01: one routine ends a child, and every path that can end one
/// goes through it.
///
/// The two things an end owes the creator are claimed *once*: the finish
/// report and the slot. Calling it twice — which is what a race between a
/// process exit and a close looks like from here — releases once and
/// reports once, because the second call finds the link already gone.
/// Audit-2 §2, unit level: a child that ends before its creation committed
/// is parked, and the commit is what reports and releases it. Drop the check
/// in `commit_agent_creation` and the end never runs: the slot stays held.
#[test]
fn an_end_that_arrives_before_the_commit_waits_for_it_and_then_releases_the_slot() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-mayfly", "process-mayfly");
    let end_runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "child-mayfly-view",
        owner.clone(),
        SessionKind::Acp,
        Box::new(FailingWriter),
    );
    let end_view = registry
        .inner
        .lock()
        .expect("map")
        .get("child-mayfly-view")
        .map(|entry| entry.to_session())
        .expect("a view for the parked end");
    let creator = "session-mayfly-creator";
    // The first caller asks (the gate goes Pending) and the human answers;
    // after that the gate is Open and each further child is a plain reserve.
    let mut ticket = registry.test_ticket(creator, 1).expect("reserve");
    registry.accept_agent_creation(creator);
    for round in 0..MAX_LIVE_CHILDREN_PER_CREATOR {
        let child = format!("session-mayfly-creator.child{round}");
        // The spawn noted the child; its provider exited at once, so the
        // end arrived before anything committed the link.
        registry.note_pending_child(&child, ticket.reservation());
        assert!(
            registry.defer_child_end_if_pending(&child, &end_view, &end_runtime, &owner),
            "the end is parked for the commit"
        );
        let reservation = ticket.reservation();
        let (committed, deferred) =
            registry.commit_agent_creation(creator, reservation, &child, true);
        assert!(committed, "the creation commits");
        let parked = deferred.expect("the end that arrived first waits for the commit");
        registry.child_ended_with(
            &child,
            parked.0.as_ref(),
            parked.1.as_deref(),
            parked.2.as_ref(),
        );
        ticket.commit();
        // One reserve per round, unconditionally: the one after the last
        // end is the proof that every parked end gave its slot back. With
        // a leaked slot it is "creation limit exceeded; do not retry"
        // instead of a ticket.
        ticket = registry
            .test_ticket(creator, 1)
            .expect("the parked ends gave their slots back");
    }
    drop(ticket);
    drop(journal);
}

/// One creator's live children, read where the caps keep them.
fn live_children_of(registry: &SessionRegistry, creator: &str) -> usize {
    registry
        .creations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .creators
        .get(creator)
        .map_or(0, |caps| caps.live_children)
}

/// Audit S5B-05, unit level: the resume path re-admits a child through
/// `readmit_agent_child` (`session.rs:3470`), and a child resumed twice is
/// still **one** child for its creator's caps. Drop the readmit and the
/// resumed child is invisible (the count every later creation is admitted
/// against undercounts by exactly the children that came back).
#[test]
fn a_child_resumed_twice_is_counted_once_by_its_creator() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s5b05", "process-s5b05");
    // `readmit_agent_child` re-admits only to a creator whose runtime the
    // registry still holds, which is what the resume path has in hand.
    insert_live_agent_with_kind_and_writer(
        &registry,
        "creator-s5b05",
        owner.clone(),
        SessionKind::Acp,
        Box::new(FailingWriter),
    );
    registry.test_ticket("creator-s5b05", 1).expect("reserve");
    registry.accept_agent_creation("creator-s5b05");
    registry.commit_agent_child_for_test("creator-s5b05", "child-s5b05", true);
    // The close that preceded the resumes gave the slot back.
    registry.release_agent_child("child-s5b05");
    assert_eq!(
        live_children_of(&registry, "creator-s5b05"),
        0,
        "the close gave the slot back"
    );
    for round in 0..2 {
        registry.readmit_agent_child("child-s5b05", Some("creator-s5b05"), &owner);
        assert_eq!(
            live_children_of(&registry, "creator-s5b05"),
            1,
            "the same child, resumed {} time(s), is one child",
            round + 1
        );
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The steerer `S5B-06` needs: it answers the trait's error, which is what
/// puts the report on the prompt fallback. A steerer that answers
/// `Ok(false)` is **not** this case: for a local creator the daemon turns a
/// refusal into an interrupt (`interrupt_on_steer_refusal`), so the steer
/// delivery succeeds and no fallback happens.
struct ErroringSteerer;

impl SessionSteerer for ErroringSteerer {
    fn steer_active_turn(
        &mut self,
        _text: &str,
        _turn: &mut TurnToken<'_>,
    ) -> Result<bool, WireError> {
        Err(WireError::new(
            ErrorCode::InvalidRequest,
            "the provider refused the steer",
        ))
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(Self)
    }
}

/// Audit S5B-06, unit level: a steer that comes back as an **error** must
/// not take the report with it. The same envelope goes out once more as a
/// plain prompt, and the structured finish record carries the id that
/// delivery got — not `None` and not "finish report not delivered".
#[test]
fn a_report_survives_a_steerer_that_errors_and_lands_as_a_prompt() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s5b06", "process-s5b06");
    let received = Arc::new(Mutex::new(Vec::new()));
    let creator = insert_live_agent_with_turn_control(
        &registry,
        "creator-s5b06",
        owner.clone(),
        SessionKind::Acp,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        None,
        Box::new(NoopKiller),
        Box::new(ErroringSteerer),
    );
    // The creator is mid-turn: the report is a steer, and this one fails.
    creator.begin_turn();
    registry.test_ticket("creator-s5b06", 1).expect("reserve");
    registry.accept_agent_creation("creator-s5b06");
    registry.commit_agent_child_for_test("creator-s5b06", "child-s5b06", true);
    let child_runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "child-s5b06",
        owner.clone(),
        SessionKind::Acp,
        Box::new(FailingWriter),
    );
    let child_view = registry
        .inner
        .lock()
        .expect("registry map")
        .get("child-s5b06")
        .map(|entry| entry.to_session())
        .expect("the child's own view");
    registry.child_ended_with(
        "child-s5b06",
        Some(&child_view),
        Some(&child_runtime),
        Some(&owner),
    );

    // The envelope went out as a plain prompt after the steer failed.
    let written = String::from_utf8_lossy(&received.lock().expect("written")).into_owned();
    assert!(
        written.contains("agent_finished"),
        "the fallback prompt carried the report: {written}"
    );
    // And the delivery answers the id of the message it left on the
    // creator's transcript (`S5-04`) — the value `ChildFinished.message_id`
    // carries. The creator is mid-turn again, so this second delivery takes
    // the same steer-then-fallback route; with the fallback removed it is an
    // `Err`, and that is exactly what turns the record into
    // `message_id: None` plus the note "finish report not delivered".
    creator.begin_turn();
    let delivered = registry
        .deliver_to_creator("creator-s5b06", &owner, "finish report for child-s5b06")
        .expect("a refused steer must not take the report with it");
    assert!(
        delivered.is_some(),
        "the id of the delivered prompt, not None"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_childs_end_releases_its_slot_and_claims_its_report_once_whatever_path_calls_it() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let creator = "session-alex";
    registry.test_ticket(creator, 1).expect("first");
    registry.accept_agent_creation(creator);
    registry.commit_agent_child_for_test(creator, "child-1", true);
    assert!(
        registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .children
            .contains_key("child-1"),
        "the slot is held for the child by the creator's entry"
    );
    registry.child_ended_with("child-1", None, None, None);
    assert!(
        !registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .children
            .contains_key("child-1"),
        "the end gives the slot back"
    );
    // A second end of a child that is already gone is a no-op: the link it
    // would count against is not there, so the creator's count cannot go
    // below zero and the slot is not released twice.
    registry.child_ended_with("child-1", None, None, None);
    {
        let table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            table
                .creators
                .get(creator)
                .map(|caps| caps.live_children)
                .unwrap_or(0),
            0,
            "a double end must not underflow the creator's count"
        );
    }
    // The creator has room again, which is what the release was for.
    assert!(registry.test_ticket(creator, 1).is_ok());
    registry.abandon_agent_creation_for_test(creator);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// Audit S5-06: a card that is with the human blocks *every* other creation
/// from that session, and the refusal spends nothing.
#[test]
fn a_pending_creation_card_refuses_a_concurrent_creation_and_spends_nothing() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let alex = "session-alex";
    let first = registry.test_ticket(alex, 1).expect("first");
    assert!(first.card_owed(), "the first creation asks the human");
    // The card has not been answered yet.
    let refused = registry
        .test_ticket(alex, 1)
        .expect_err("a second creation while the first card is pending");
    assert_eq!(refused.message, "creation permission pending; retry");
    // The refusal took no slot: the creator is at one held creation, the
    // one whose card is out, and the hour is charged once.
    {
        let table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let caps = table.creators.get(alex).expect("the creator's entry");
        assert_eq!(caps.in_flight.len(), 1, "the refused caller spent no slot");
        assert_eq!(caps.creations_in_window, 1, "and no hour quota");
    }
    // The human allows: the gate opens, and the next creation is not asked.
    registry.accept_agent_creation(alex);
    let second = registry.test_ticket(alex, 1).expect("allowed");
    assert!(!second.card_owed(), "one card per creator session");
    assert_eq!(second.caps.creations_this_hour, 2);
    registry.abandon_agent_creation_for_test(alex);
    registry.abandon_agent_creation_for_test(alex);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}
