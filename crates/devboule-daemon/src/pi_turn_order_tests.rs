//! The wire-order interleavings of one run's end: pi's line reader is
//! concurrent, so the aborted `turn_end`, the refusal and the `agent_end`
//! that close the interrupt-and-replace flow can arrive in either order.
//! Every order gives exactly one finish, live and replayed.

use super::super::local_command_test_support::agent_start;
use super::test_support::{
    abort_frame, attached_journal, broker, deliver, drain, feed_line, harness, rejects,
    turn_end_with,
};
use crate::journal::{new_session_record, Journal};
use crate::session::SessionKiller;
use crate::session::SessionRuntime;
use devboule_protocol::{AgentActivityState, SessionEvent, SessionKind};
use std::path::PathBuf;
use std::sync::Arc;

/// The shared setup of the two order tests: the /help-mid-turn flow up to
/// the interrupt, with the replacement delivered. The abort frame consumes
/// an id from the shared counter (a-2), so the replacement carries p-3.
struct OrderFlow {
    harness: super::test_support::PiWatchHarness,
    runtime: Arc<SessionRuntime>,
    conn: Arc<crate::session::event_pull::ConnHandle>,
    replacement_id: String,
    dir: PathBuf,
    path: PathBuf,
    journal: Arc<Journal>,
    session_id: String,
}

fn order_flow() -> OrderFlow {
    let (dir, path) = crate::journal::tmp_journal();
    let journal = Arc::new(Journal::open(&path).expect("open"));
    let session_id = "s.pi.order".to_string();
    journal
        .create_session(new_session_record(
            &session_id,
            "owner",
            None,
            SessionKind::Pi,
            "pi order",
        ))
        .expect("birth");
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached_journal(&journal, &session_id);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    feed_line(
        &mut harness,
        &runtime,
        serde_json::json!({"type":"response","id":"p-1","command":"prompt","success":true}),
    );
    feed_line(&mut harness, &runtime, agent_start());
    let _ = drain(&conn);
    // The steer road refused `/help`; the interrupt-and-replace went out.
    harness.killer.interrupt();
    let _ = abort_frame(&mut harness);
    deliver(&mut harness, "/help");
    OrderFlow {
        harness,
        runtime,
        conn,
        replacement_id: "p-3".to_string(),
        dir,
        path,
        journal,
        session_id,
    }
}

fn shutdown_order(flow: OrderFlow) -> (PathBuf, String, PathBuf) {
    drop(flow.harness);
    drop(flow.runtime);
    drop(flow.conn);
    flow.journal.flush().expect("flush");
    flow.journal.shutdown();
    (flow.dir, flow.session_id, flow.path)
}

/// The refusal can lose the race with pi's aborted end: the aborted
/// `turn_end` lands first, the refusal arrives while pi's run is still
/// open, and only `agent_end` closes that run. The deferred refusal end
/// publishes exactly once at the close, the watch rests, and a restart's
/// replay shows the same single finish.
#[test]
fn an_aborted_end_before_the_refusal_still_finishes_when_pi_s_run_closes() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = order_flow();
    // pi's aborted end for the interrupted turn arrives first: it finishes
    // nothing, because the replacement was still counted as delivered.
    feed_line(&mut flow.harness, &flow.runtime, turn_end_with("aborted"));
    let after_abort = drain(&flow.conn);
    assert_eq!(
        super::test_support::finishes_of(&after_abort),
        Vec::<String>::new(),
        "the stale end finishes nothing: {after_abort:?}"
    );
    // The refusal lands in the gap: its error shows, its finish waits for
    // pi's run to close.
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        rejects(
            &flow.replacement_id,
            "Agent is already processing. Specify streamingBehavior to queue it.",
        ),
    );
    let after_refusal = drain(&flow.conn);
    assert!(
        after_refusal.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message } if message.contains("Agent is already processing")
        )),
        "the refusal shows pi's own error text: {after_refusal:?}"
    );
    assert_eq!(
        super::test_support::finishes_of(&after_refusal),
        Vec::<String>::new(),
        "the refusal defers while pi's run is open: {after_refusal:?}"
    );
    // pi's run closes: the deferred refusal end publishes, exactly once.
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        serde_json::json!({"type":"agent_end"}),
    );
    let events = drain(&flow.conn);
    assert_eq!(
        super::test_support::finishes_of(&events),
        ["error"],
        "one error finish when pi's run closes: {events:?}"
    );
    assert!(
        matches!(flow.runtime.activity(), AgentActivityState::Idle),
        "the watch rests, got {:?}",
        flow.runtime.activity()
    );
    let (dir, session_id, path) = shutdown_order(flow);
    let replay = Journal::open(&path)
        .expect("reopen")
        .replay(&session_id)
        .expect("replay");
    assert_eq!(
        super::test_support::finishes_of(&replay.events),
        ["error"],
        "the restart's replay shows the same one finish: {:?}",
        super::test_support::finishes_of(&replay.events)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The other order — refusal first, then pi's aborted end, then the run's
/// close — gives the same single finish whatever closes pi's run.
#[test]
fn a_refusal_then_an_aborted_end_finishes_once_whatever_closes_pi_s_run() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = order_flow();
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        rejects(
            &flow.replacement_id,
            "Agent is already processing. Specify streamingBehavior to queue it.",
        ),
    );
    let after_refusal = drain(&flow.conn);
    assert!(
        after_refusal.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message } if message.contains("Agent is already processing")
        )),
        "the refusal shows pi's own error text: {after_refusal:?}"
    );
    assert_eq!(
        super::test_support::finishes_of(&after_refusal),
        Vec::<String>::new(),
        "the refusal defers while pi's run is open: {after_refusal:?}"
    );
    feed_line(&mut flow.harness, &flow.runtime, turn_end_with("aborted"));
    let after_abort = drain(&flow.conn);
    assert_eq!(
        super::test_support::finishes_of(&after_abort),
        ["aborted"],
        "pi's own end finishes the run once: {after_abort:?}"
    );
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        serde_json::json!({"type":"agent_end"}),
    );
    let events = drain(&flow.conn);
    assert_eq!(
        super::test_support::finishes_of(&events),
        Vec::<String>::new(),
        "the run already ended once: {events:?}"
    );
    assert!(
        matches!(flow.runtime.activity(), AgentActivityState::Idle),
        "the watch rests, got {:?}",
        flow.runtime.activity()
    );
    let (dir, session_id, path) = shutdown_order(flow);
    let replay = Journal::open(&path)
        .expect("reopen")
        .replay(&session_id)
        .expect("replay");
    assert_eq!(
        super::test_support::finishes_of(&replay.events),
        ["aborted"],
        "the restart's replay shows the same one finish: {:?}",
        super::test_support::finishes_of(&replay.events)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
