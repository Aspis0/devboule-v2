//! Tests for one topic: what the Claude turn watchdog's finish suppresses —
//! the stale-result races, the journaled replay, and the killer contract.
//! Shares the harness with `claude_turn_watch_tests.rs`; time is faked with
//! the watch's deterministic hooks, never slept out.

use super::turn_watch_test_support::{
    attached, deliver, drain, feed_line, finish, harness, harness_echo, is_any_finish,
    is_error_finish, is_watchdog_error, release_gate, touch,
};
use super::*;
use crate::journal::Journal;
use crate::session::ConnHandle;
use devboule_protocol::SessionKind;
use std::time::Duration;

#[test]
fn an_expiry_interrupts_so_the_old_result_cannot_finish_the_next_turn() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut harness = harness_echo(&broker);
    let (runtime, conn) = attached(&broker);
    release_gate(&mut harness, &runtime);
    runtime.begin_turn();
    deliver(&mut harness, "first");
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    let expired = drain(&conn);
    assert!(
        expired.iter().any(is_watchdog_error) && expired.iter().any(is_error_finish),
        "the abandoned turn ends first: {expired:?}"
    );
    // The replacement, through the same production flush: delivered (so the
    // abort gate counts it) and re-armed.
    let next_turn = runtime.turn_counter();
    runtime.begin_turn();
    deliver(&mut harness, "second");
    // The abandoned turn's answer, abort-marked the way a CLI honouring the
    // expiry's interrupt sends it.
    feed_line(
        &mut harness.reader,
        &runtime,
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "terminal_reason": "aborted by interrupt",
            "stop_reason": "end_turn",
        }),
    );
    let late = drain(&conn);
    assert!(
        !late.iter().any(is_any_finish),
        "the stale result finishes nothing: {late:?}"
    );
    assert!(
        runtime.is_turn_active(next_turn),
        "the new turn survives its predecessor's answer"
    );
    // The new turn's watchdog is still armed: silence past the bound ends it.
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(1));
    harness.watch.tick_for_test();
    let rearmed = drain(&conn);
    assert!(
        rearmed.iter().any(is_watchdog_error) && rearmed.iter().any(is_error_finish),
        "the replacement's own silence still expires: {rearmed:?}"
    );
    finish(&mut harness, &runtime);
}

/// A deaf CLI answers the expiry's interrupt with a genuine result, which
/// U5b's epoch cannot tell from the replacement's own: it consumes the armed
/// expectation and settles. What the watchdog still guarantees: exactly one
/// finish for the ended turn, and none for a turn with nothing running.
#[test]
fn a_deaf_cli_genuine_answer_ends_one_finish_not_two() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut harness = harness(&broker, false);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "first");
    touch(&mut harness, &runtime);
    let _ = drain(&conn);
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    let expired = drain(&conn);
    assert!(
        expired.iter().any(is_watchdog_error) && expired.iter().any(is_error_finish),
        "the abandoned turn ends first: {expired:?}"
    );
    runtime.begin_turn();
    deliver(&mut harness, "second");
    feed_line(
        &mut harness.reader,
        &runtime,
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "stop_reason": "end_turn",
        }),
    );
    let early = drain(&conn);
    assert_eq!(
        early.iter().filter(|event| is_any_finish(event)).count(),
        1,
        "the stale genuine answer ends the running turn exactly once: {early:?}"
    );
    feed_line(
        &mut harness.reader,
        &runtime,
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "stop_reason": "end_turn",
        }),
    );
    let late = drain(&conn);
    assert!(
        !late.iter().any(is_any_finish),
        "one turn, one finish: {late:?}"
    );
    finish(&mut harness, &runtime);
}

/// Past a kill the expiry body stays quiet: the interrupt road is already a
/// no-op there, and settling or publishing on a gone child would write an
/// expiry for a turn the kill already ended.
#[test]
fn no_expiry_rows_past_a_kill() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut harness = harness(&broker, true);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "doomed");
    touch(&mut harness, &runtime);
    let _ = drain(&conn);
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    let events = drain(&conn);
    assert!(
        events.is_empty(),
        "a killed session expires silently: {events:?}"
    );
    finish(&mut harness, &runtime);
}

/// The killer's interrupt records the request even past a kill: the gate's
/// contract is the moment of request, not the fate of the frame. Asserted
/// through the shared road directly — no child process exists on this path,
/// so no process handle is needed and none is spawned.
#[test]
fn a_killed_killer_interrupt_still_arms_the_gate() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let stdin = Arc::new(Mutex::new(None));
    let next_id = Arc::new(AtomicU64::new(1));
    let abort_gate: ClaudeAbortGateRef = Arc::new(crate::claude_abort::ClaudeAbortGate::default());
    interrupt_claude_turn(
        &stdin,
        &next_id,
        &broker,
        &abort_gate,
        &Arc::new(AtomicBool::new(true)),
    );
    abort_gate.note_prompt_delivered();
    assert!(
        abort_gate.settle_result(true),
        "the request outranks the replacement even though the frame never went out"
    );
}

/// The pair append journals both rows adjacently and reports the envelope's
/// seq, so a close between them cannot strand the marker.
#[test]
fn journal_pair_appends_marker_with_its_envelope() {
    let dir = crate::test_dirs::test_temp_dir("devboule-marker-pair");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    journal
        .upsert_blocking(crate::journal::new_session_record(
            "s.marker.pair",
            "owner",
            None,
            SessionKind::Claude,
            "Pair",
        ))
        .expect("session row");
    let runtime = Arc::new(SessionRuntime::with_journal(
        "s.marker.pair".to_string(),
        Some(Arc::clone(&journal)),
    ));
    let marker = crate::claude_view::withheld_finish_marker();
    let envelope = serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "pair-message-1",
            "model": "stub-model",
            "role": "assistant",
            "content": [{"type": "text", "text": "held"}],
        },
    });
    let seq = runtime
        .journal_agent_envelope_pair(&marker, &envelope)
        .expect("pair journals");
    let after = runtime
        .journal_agent_envelope(&envelope)
        .expect("single journals");
    assert_eq!(
        after,
        seq + 1,
        "the pair consumed exactly two consecutive seqs"
    );
    let replay = journal.replay("s.marker.pair").expect("replay");
    let kinds: Vec<&str> = replay.events.iter().map(|event| event.kind()).collect();
    assert_eq!(
        &kinds[..2],
        &["agent_message", "agent_message"],
        "marker row then envelope row, in order: {kinds:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Expiry plus the late answer must replay to one finish: the watchdog's
/// rows are journaled, and the late envelope carries the suppression marker
/// the live pass used, so replay derives the same single finish it showed.
#[test]
fn an_expiry_and_its_late_answer_replay_to_one_finish() {
    let dir = crate::test_dirs::test_temp_dir("devboule-watchdog-replay");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    journal
        .upsert_blocking(crate::journal::new_session_record(
            "s.watchdog.replay",
            "owner",
            None,
            SessionKind::Claude,
            "Watchdog",
        ))
        .expect("session row");
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let runtime = Arc::new(SessionRuntime::with_journal(
        "s.watchdog.replay".to_string(),
        Some(Arc::clone(&journal)),
    ));
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.watchdog.replay",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let mut harness = harness(&broker, false);
    runtime.begin_turn();
    deliver(&mut harness, "first");
    touch(&mut harness, &runtime);
    let _ = drain(&conn);
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    let expired = drain(&conn);
    assert!(
        expired.iter().any(is_watchdog_error) && expired.iter().any(is_error_finish),
        "the abandoned turn ends first: {expired:?}"
    );
    // The late answer with no replacement: the turn is already over, so the
    // live pass publishes nothing for it — and journals the marker, so
    // replay publishes nothing either.
    feed_line(
        &mut harness.reader,
        &runtime,
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "terminal_reason": "aborted by interrupt",
            "stop_reason": "end_turn",
        }),
    );
    let late = drain(&conn);
    assert!(late.is_empty(), "the stale answer is live-silent: {late:?}");
    let replay = journal.replay("s.watchdog.replay").expect("replay");
    assert_eq!(
        replay
            .events
            .iter()
            .filter(|event| is_any_finish(event))
            .count(),
        1,
        "one finish across the rebuilt transcript: {:?}",
        replay
            .events
            .iter()
            .map(|event| event.kind())
            .collect::<Vec<_>>()
    );
    finish(&mut harness, &runtime);
    let _ = std::fs::remove_dir_all(&dir);
}
