//! The creation-race tests, moved whole out of `session_tests.rs` lines
//! 7236-7856 (at `a9b31af`), the two fixtures at their head included: the
//! structural check that the creation check and its park are one call, a commit
//! racing an end releasing the child once, a creation whose creator is gone
//! leaving nothing parked, a creation slower than the slot expiry keeping its
//! marker, the creation record published before a parked end runs, a readmitted
//! child keeping what it already spent, standing instructions before the preset
//! preamble, and the session-id mint — its per-process nonce, its advancing
//! counter, its width, and its over-budget refusal. Every line below is
//! byte-identical to its text there apart from this header; `creator_children`
//! and `creator_replay` sit above the tests and travel with them, so nothing was
//! promoted for this move.

use super::tests::{
    insert_live_agent_with_kind_and_writer, test_owner, tmp_delete_registry, FailingWriter,
    RecordingWriter,
};
use super::*;

/// One creator's live children, read where the caps keep them.
fn creator_children(registry: &SessionRegistry, creator: &str) -> usize {
    registry
        .creations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .creators
        .get(creator)
        .map_or(0, |caps| caps.live_children)
}

/// The creator's own journal, once both records are in it.
fn creator_replay(journal: &Arc<Journal>, session_id: &str) -> Vec<SessionEvent> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        // The appends are the journal thread's work: flush before reading,
        // or a loaded machine reads a transcript that is still in flight.
        let _ = journal.flush();
        let events = journal.replay(session_id).expect("replay").events;
        let created = events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentCreated { .. }));
        let finished = events
            .iter()
            .any(|event| matches!(event, SessionEvent::ChildFinished { .. }));
        if created && finished {
            return events;
        }
        assert!(
            Instant::now() < deadline,
            "the creator's journal never held both records: {events:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Audit-3 §1, the structural half: the check and the park are one call, and
/// the two helpers that let a commit land between them are gone.
#[test]
fn the_check_and_the_park_are_one_call() {
    // The pair moved out of session.rs into its sibling: read it where it is.
    let source = include_str!("session_children.rs");
    // Built at runtime: the needles must not appear in the source this test
    // is compiled from, or the assertion would match itself.
    let split_check = ["fn child_end_is", "pending("].concat();
    assert!(
        !source.contains(&split_check),
        "the split check must not come back (audit-3 §1)"
    );
    let bare_park = ["fn defer_child_end", "("].concat();
    assert!(
        !source.contains(&bare_park),
        "a park without its check must not come back (audit-3 §1)"
    );
    let merged = ["fn defer_child_end_if", "_pending("].concat();
    assert!(source.contains(&merged));
}

/// Audit-3 §1, the racing half: a commit on one thread and an end on the
/// other, both admitted by the same marker. Whoever loses finds the marker
/// taken and runs the routine itself; the slot is released exactly once.
#[test]
fn a_commit_racing_an_end_releases_the_child_once() {
    use std::sync::Barrier;

    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s5c01", "process-s5c01");
    insert_live_agent_with_kind_and_writer(
        &registry,
        "creator-s5c01",
        owner.clone(),
        SessionKind::Acp,
        Box::new(FailingWriter),
    );
    let child_runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "child-s5c01",
        owner.clone(),
        SessionKind::Acp,
        Box::new(FailingWriter),
    );
    let child_session = registry
        .inner
        .lock()
        .expect("map")
        .get("child-s5c01")
        .map(|entry| entry.to_session())
        .expect("the child's view");
    let ticket = registry.test_ticket("creator-s5c01", 1).expect("reserve");
    registry.accept_agent_creation("creator-s5c01");
    registry.note_pending_child("child-s5c01", ticket.reservation());
    let barrier = Arc::new(Barrier::new(2));
    {
        let first = Arc::clone(&barrier);
        let second = Arc::clone(&barrier);
        let registry = &registry;
        let child_session = &child_session;
        let child_runtime = &child_runtime;
        let owner = &owner;
        std::thread::scope(|scope| {
            scope.spawn(move || {
                first.wait();
                if !registry.defer_child_end_if_pending(
                    "child-s5c01",
                    child_session,
                    child_runtime,
                    owner,
                ) {
                    registry.child_ended_with(
                        "child-s5c01",
                        Some(child_session),
                        Some(child_runtime),
                        Some(owner),
                    );
                }
            });
            scope.spawn(move || {
                second.wait();
                let (committed, deferred) = registry.commit_agent_creation(
                    "creator-s5c01",
                    ticket.reservation(),
                    "child-s5c01",
                    true,
                );
                assert!(
                    committed,
                    "the commit wins or loses the race, but it commits"
                );
                if let Some((session, runtime, owner)) = deferred {
                    registry.child_ended_with(
                        "child-s5c01",
                        session.as_ref(),
                        runtime.as_deref(),
                        owner.as_ref(),
                    );
                }
            });
        });
    }
    assert_eq!(
        creator_children(&registry, "creator-s5c01"),
        0,
        "the end ran exactly once: the child's slot came back"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// Audit-3 §2: a creation that did not commit because its creator is gone
/// leaves nothing behind. The clear precedes the close, so the end the close
/// produces is not parked for a commit that will never run; the reservation,
/// and the marker behind it, are the backstop rather than the sweep
/// (audit-3 S5D-01).
#[test]
fn a_creation_whose_creator_is_gone_leaves_nothing_parked() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s5c02", "process-s5c02");
    insert_live_agent_with_kind_and_writer(
        &registry,
        "child-s5c02",
        owner.clone(),
        SessionKind::Acp,
        Box::new(FailingWriter),
    );
    let ticket = registry.test_ticket("creator-s5c02", 1).expect("reserve");
    registry.accept_agent_creation("creator-s5c02");
    registry.note_pending_child("child-s5c02", ticket.reservation());
    // The creator closes between the spawn and the commit: the commit finds
    // no caps row for it, which is the gone-creator shape (S5B-09).
    registry
        .creations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .creators
        .remove("creator-s5c02");
    let (committed, deferred) =
        registry.commit_agent_creation("creator-s5c02", ticket.reservation(), "child-s5c02", true);
    assert!(
        !committed && deferred.is_none(),
        "an unowned creation does not commit"
    );
    {
        let table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(
            table.pending_children.contains_key("child-s5c02"),
            "the start's marker is still there for the caller to clear"
        );
    }
    registry.abandon_uncommitted_child("child-s5c02", &owner);
    {
        let table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(
            !table.pending_children.contains_key("child-s5c02"),
            "the abandoned creation left no marker"
        );
        assert!(
            !table.deferred_child_ends.contains_key("child-s5c02"),
            "the abandoned creation left no parked end"
        );
    }
    assert!(
        registry
            .inner
            .lock()
            .expect("map")
            .get("child-s5c02")
            .is_none_or(|entry| !matches!(entry, RegistryEntry::Live(_))),
        "the abandoned child is not left live"
    );
    // The marker is not the sweep's to take (audit-3 S5D-01): its lifetime
    // is its reservation's, so an aged one outlives the sweep...
    let aged = registry
        .test_ticket("creator-s5c02-aged", 1)
        .expect("reserve");
    registry.note_pending_child("child-s5c02-aged", aged.reservation());
    {
        let mut table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.sweep(Instant::now() + DEFERRED_SLOT_EXPIRY + Duration::from_secs(1));
        assert!(
            table.pending_children.contains_key("child-s5c02-aged"),
            "the sweep does not age a marker: its reservation is still live"
        );
    }
    // ...and the reservation's release is what carries it away.
    drop(aged);
    {
        let table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(
            !table.pending_children.contains_key("child-s5c02-aged"),
            "the released reservation took its marker with it"
        );
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// Audit S5D-01: a creation whose spawn outlives the slot expiry keeps its
/// marker, and the commit that follows still finds the end that arrived
/// while the spawn was running.
///
/// The sweep must not age `pending_children`: an ACP handshake can take
/// longer than any expiry the sweep could pick, and the marker it would drop
/// is the only thing tying an early end to a creation that has not committed
/// yet, so dropping it strands the reservation, the slot and the finish
/// report. The age is expressed the way the neighbouring tests express it,
/// by moving the sweep's clock rather than sleeping for a minute.
#[test]
fn a_creation_slower_than_the_slot_expiry_keeps_its_marker() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s5d01", "process-s5d01");
    let child_runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "child-s5d01",
        owner.clone(),
        SessionKind::Acp,
        Box::new(FailingWriter),
    );
    let child_session = registry
        .inner
        .lock()
        .expect("map")
        .get("child-s5d01")
        .map(|entry| entry.to_session())
        .expect("the child's view");
    let creator = "creator-s5d01";
    let ticket = registry.test_ticket(creator, 1).expect("reserve");
    registry.accept_agent_creation(creator);
    // The spawn noted the child before its handshake, which is still running.
    registry.note_pending_child("child-s5d01", ticket.reservation());
    {
        let mut table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.sweep(Instant::now() + DEFERRED_SLOT_EXPIRY + Duration::from_secs(1));
        assert!(
            table.pending_children.contains_key("child-s5d01"),
            "the handshake is still running: the marker is not the sweep's"
        );
        // ...and the sweep the next admission runs is this same sweep with
        // the real clock: the marker is older than the expiry either way.
        table.last_sweep = None;
    }
    // Another creation request arrives while the first handshake is running,
    // and its admission sweeps first.
    let second = registry
        .test_ticket(creator, 1)
        .expect("the caps still admit a creation");
    {
        let table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(
            table.pending_children.contains_key("child-s5d01"),
            "the marker survived the sweep the admission ran"
        );
    }
    // The child's provider exits while the spawn is still running: the end
    // arrives before the link exists, and parks.
    assert!(
        registry.defer_child_end_if_pending("child-s5d01", &child_session, &child_runtime, &owner),
        "the marker was there to park the end against"
    );
    // The handshake returns: the commit still finds the marker, and with it
    // the end that arrived first.
    let (committed, deferred) =
        registry.commit_agent_creation(creator, ticket.reservation(), "child-s5d01", true);
    assert!(committed, "the slow creation still commits");
    let parked = deferred.expect("the marker survived, so the parked end came back");
    registry.child_ended_with(
        "child-s5d01",
        parked.0.as_ref(),
        parked.1.as_deref(),
        parked.2.as_ref(),
    );
    ticket.commit();
    drop(second);
    assert_eq!(
        creator_children(&registry, creator),
        0,
        "the end ran: the child's slot came back"
    );
    {
        let table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(
            !table.pending_children.contains_key("child-s5d01"),
            "the commit took the marker with the link"
        );
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// Audit-3 §3: the creator can name the child before it is told the child
/// finished. Read from the creator's own journal sequence.
#[test]
fn the_creation_record_is_published_before_a_parked_end_runs() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s5c03", "process-s5c03");
    let creator_runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "creator-s5c03",
        owner.clone(),
        SessionKind::Acp,
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let child_runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "child-s5c03",
        owner.clone(),
        SessionKind::Acp,
        Box::new(FailingWriter),
    );
    let child_session = registry
        .inner
        .lock()
        .expect("map")
        .get("child-s5c03")
        .map(|entry| entry.to_session())
        .expect("the child's view");
    // The link the commit would have registered, so the parked end's
    // release has something to remove.
    registry.test_ticket("creator-s5c03", 1).expect("reserve");
    registry.accept_agent_creation("creator-s5c03");
    registry.commit_agent_child_for_test("creator-s5c03", "child-s5c03", true);
    // A journal row is what `replay` reads by.
    journal
        .upsert_blocking(new_session_record(
            "creator-s5c03",
            owner.user.clone(),
            None,
            SessionKind::Acp,
            "creator",
        ))
        .expect("the creator's row");
    registry.publish_child_created_then_end(
        Some(&creator_runtime),
        "child-s5c03",
        "worker",
        "devboule-acp-stub",
        "worker",
        Some((Some(child_session), Some(child_runtime), Some(owner))),
    );
    // Both records reach the creator's journal — the finish is journaled
    // like the creation (brief §1) — and in that order.
    let events = creator_replay(&journal, "creator-s5c03");
    let created = events
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentCreated { .. }))
        .expect("AgentCreated in the creator's journal");
    let finished = events
        .iter()
        .position(|event| matches!(event, SessionEvent::ChildFinished { .. }))
        .expect("ChildFinished in the creator's journal");
    assert!(
        created < finished,
        "the creation record precedes the finish: {events:?}"
    );
    let journaled = events
        .iter()
        .find(|event| matches!(event, SessionEvent::ChildFinished { .. }))
        .cloned()
        .expect("the finish record");
    let message_id = match &journaled {
        SessionEvent::ChildFinished { message_id, .. } => message_id.clone(),
        _ => None,
    };
    assert!(
        message_id.is_some(),
        "the finish record carries the id of the text record beside it: {journaled:?}"
    );
    assert!(
        !registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .children
            .contains_key("child-s5c03"),
        "the parked end ran after the publish: the link is released"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// Audit-3 §5: a child re-admitted after its report was claimed does not get
/// a second one — the early return comes before the link is overwritten.
#[test]
fn a_readmitted_child_keeps_what_it_already_spent() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-s5c05", "process-s5c05");
    insert_live_agent_with_kind_and_writer(
        &registry,
        "creator-s5c05",
        owner.clone(),
        SessionKind::Acp,
        Box::new(FailingWriter),
    );
    registry.test_ticket("creator-s5c05", 1).expect("reserve");
    registry.accept_agent_creation("creator-s5c05");
    registry.commit_agent_child_for_test("creator-s5c05", "child-s5c05", true);
    {
        let mut table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let link = table.children.get_mut("child-s5c05").expect("the link");
        link.report_owed = false;
        link.notice_owed = false;
    }
    registry.readmit_agent_child("child-s5c05", Some("creator-s5c05"), &owner);
    registry.readmit_agent_child("child-s5c05", Some("creator-s5c05"), &owner);
    {
        let table = registry
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let link = table.children.get("child-s5c05").expect("the link");
        assert!(
            !link.report_owed && !link.notice_owed,
            "what the first admission spent stays spent"
        );
        assert_eq!(
            table
                .creators
                .get("creator-s5c05")
                .map_or(0, |caps| caps.live_children),
            1,
            "the same child, admitted twice, is one child"
        );
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
/// The order `create-from-profile` fixes: the human's standing instructions,
/// then the preset preamble where the caller has one, then the prompt.
#[test]
fn standing_instructions_come_before_the_preset_preamble() {
    assert_eq!(
        super::compose_first_prompt("standing", None, Some("preamble"), "prompt"),
        "standing\n\npreamble\n\nprompt"
    );
    assert_eq!(
        super::compose_first_prompt("standing", None, None, "prompt"),
        "standing\n\nprompt"
    );
    assert_eq!(
        super::compose_first_prompt("", None, Some("preamble"), "prompt"),
        "preamble\n\nprompt"
    );
    // The position that decides the property: the instructions are in front
    // of the preamble, and the preamble in front of the prompt.
    let composed = super::compose_first_prompt("standing", None, Some("preamble"), "prompt");
    let standing = composed.find("standing").expect("the instructions");
    let preamble = composed.find("preamble").expect("the preamble");
    let prompt = composed.find("prompt").expect("the prompt");
    assert!(standing < preamble && preamble < prompt, "{composed}");
}

/// The spawn prompt's place, fixed by `create-from-profile`: the human's
/// standing instructions, then the **profile's spawn prompt**, then the preset
/// preamble, then the prompt. The spawn text is the profile the child was
/// created from speaking before its creator does, so it sits behind the
/// device-wide rules and in front of the creation's own preamble.
#[test]
fn the_spawn_prompt_sits_between_the_standing_instructions_and_the_preamble() {
    assert_eq!(
        super::compose_first_prompt("standing", Some("spawn"), Some("preamble"), "prompt"),
        "standing\n\nspawn\n\npreamble\n\nprompt"
    );
    // The spawn text keeps its place without the standing instructions too:
    // an absent half removes its separator, never reorders the rest.
    assert_eq!(
        super::compose_first_prompt("", Some("spawn"), Some("preamble"), "prompt"),
        "spawn\n\npreamble\n\nprompt"
    );
    assert_eq!(
        super::compose_first_prompt("standing", Some("spawn"), None, "prompt"),
        "standing\n\nspawn\n\nprompt"
    );
    assert_eq!(
        super::compose_first_prompt("", Some("spawn"), None, "prompt"),
        "spawn\n\nprompt"
    );
    // The position that decides the property, spelled out on the full shape.
    let composed =
        super::compose_first_prompt("standing", Some("spawn"), Some("preamble"), "prompt");
    let standing = composed.find("standing").expect("the instructions");
    let spawn = composed.find("spawn").expect("the spawn prompt");
    let preamble = composed.find("preamble").expect("the preamble");
    let prompt = composed.find("prompt").expect("the prompt");
    assert!(
        standing < spawn && spawn < preamble && preamble < prompt,
        "{composed}"
    );
}

/// A profile without a spawn prompt composes exactly what the profileless
/// rule composed: the absent field is no blank line and no separator, and
/// every other half keeps the place it already had.
#[test]
fn an_absent_spawn_prompt_changes_no_composition_at_all() {
    assert_eq!(
        super::compose_first_prompt("standing", None, Some("preamble"), "prompt"),
        "standing\n\npreamble\n\nprompt"
    );
    assert_eq!(
        super::compose_first_prompt("", None, None, "the prompt"),
        "the prompt"
    );
}

/// An **empty** spawn prompt is the absent one: the store trims the field to
/// exactly this value, so a profile saved with nothing to say composes
/// nothing at all — the preamble follows the standing instructions directly.
#[test]
fn an_empty_spawn_prompt_is_the_absent_one() {
    assert_eq!(
        super::compose_first_prompt("standing", Some(""), Some("preamble"), "prompt"),
        "standing\n\npreamble\n\nprompt"
    );
    assert_eq!(
        super::compose_first_prompt("", Some(""), None, "the prompt"),
        "the prompt"
    );
}

/// An empty standing-instructions text leaves the prompt **byte for byte**
/// what it was: no separator, no trailing newline, nothing to see.
#[test]
fn empty_standing_instructions_change_no_prompt_at_all() {
    assert_eq!(
        super::compose_first_prompt("", None, None, "the prompt"),
        "the prompt"
    );
    let today = format!("{}\n\n{}", "the preamble", "the prompt");
    assert_eq!(
        super::compose_first_prompt("", None, Some("the preamble"), "the prompt"),
        today,
        "an empty text adds nothing to the glue the preamble already had"
    );
}

/// An id minted by one daemon process must be impossible for a later daemon
/// process to mint again. A restart hands the new process a fresh nonce while
/// the counter starts over; the property is that the same counter position in
/// two lives of the daemon cannot produce the same id. The id's shape is
/// asserted nowhere here.
#[test]
fn session_ids_from_two_daemon_lives_differ_at_the_same_counter_position() {
    let first = session_unique(0x9f2c_1a7b_3e5d_6048, 1);
    let second = session_unique(0x1b4d_9e2f_7a6c_05d3, 1);
    assert_ne!(
        first, second,
        "two daemon lives minted the same id at counter 1"
    );
}

/// The unique component stays inside `compose_session_id`'s budget — at most
/// 32 characters of its closed alphabet — and the daemon's own mint hands it
/// exactly that: 8 hex of counter, one dash, 16 hex of nonce.
#[test]
fn the_minted_unique_stays_within_the_session_id_budget() {
    let unique = session_unique(u64::MAX, 1);
    assert!(
        unique.len() <= 32,
        "{unique} exceeds the 32-character budget"
    );
    let composed = compose_session_id("process-1234", &unique).expect("within the id rules");
    assert!(composed.starts_with("s.process-1234."));

    let minted = mint_session_unique();
    assert_eq!(minted.len(), 25, "{minted}");
    assert!(
        minted
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "{minted} left the lower-case hex-and-dash alphabet"
    );
}

/// M2 (nonce, pinnable in-process): the mint uses one stable per-process
/// value and advances the counter. What this cannot prove from inside one
/// process is that a fresh process draws a different nonce — that needs two
/// processes or control of the OS RNG, so the cross-process half rests on
/// inspection of `draw_session_nonce` (OS entropy, pid+time fallback) behind
/// `OnceLock`, not on a test.
#[test]
fn mint_uses_a_stable_process_nonce_and_an_advancing_counter() {
    let first = mint_session_unique();
    let second = mint_session_unique();
    let (first_counter, first_nonce) = first.split_once('-').expect("mint shape");
    let (second_counter, second_nonce) = second.split_once('-').expect("mint shape");
    assert_eq!(
        first_nonce, second_nonce,
        "two mints in one process share the nonce"
    );
    assert_ne!(
        first_counter, second_counter,
        "two mints advance the counter"
    );
    assert_eq!(
        first_nonce,
        format!("{:016x}", session_nonce()),
        "the mint uses the process nonce"
    );
    assert_eq!(session_nonce(), session_nonce(), "the nonce is stable");
}

/// M2 (boundary): the 8-wide counter is a minimum width, not a maximum —
/// past it the unique grows gracefully and still composes. The terminal
/// refusal at the 32-char budget is pinned with a literal, not via the
/// minter (which debug-asserts first — see below).
#[test]
fn counter_past_its_width_grows_gracefully_within_budget() {
    let unique = session_unique(0x9f2c_1a7b_3e5d_6048, 0x1_0000_0000);
    assert_eq!(unique.len(), 26, "{unique}");
    compose_session_id("process-1234", &unique).expect("still within budget");
    let overlong = "f".repeat(33);
    assert!(
        compose_session_id("process-1234", &overlong).is_err(),
        "33 chars must refuse at the 32-char budget"
    );
}

/// M2 (boundary guard): an over-budget counter must fail loudly in the mint,
/// not as a confusing compose surprise. `u64::MAX` needs 16 hex digits — 33
/// chars with nonce+dash — so the mint must refuse it here in test builds.
/// Before the guard this does not panic (red).
#[test]
#[should_panic(expected = "session id budget")]
fn mint_refuses_a_counter_past_the_id_budget() {
    let _ = session_unique(0x9f2c_1a7b_3e5d_6048, u64::MAX);
}
