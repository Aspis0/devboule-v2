//! Which `turn/completed` ends a Codex run, and whose output reaches the
//! transcript: only the root thread's completion for the turn the daemon is
//! tracking settles the run, and a child thread's frames are the child's own
//! work, never the root agent's reply.
//!
//! No child process here — the reader is driven with the frames the
//! app-server is measured to send, and the session's event queue is what is
//! read back.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use devboule_protocol::SessionEvent;

use super::super::event_pull::ConnHandle;
use super::super::permission_broker::PermissionBroker;
use super::super::session_runtime::SessionRuntime;
use super::command_test_support::thread_state;
use super::{empty_commands, CodexReader, CodexRequests, CodexStaticPrompt};
use crate::codex_view::{CodexState, CodexView};

const ROOT: &str = "thread-fake";
const CHILD: &str = "thread-child";

fn started_reader() -> (
    CodexReader,
    Arc<CodexState>,
    Arc<SessionRuntime>,
    Arc<ConnHandle>,
) {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.stream.lock().unwrap().screen = None;
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.codex.turns",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let state = thread_state();
    let commands = empty_commands();
    let stdin = Arc::new(Mutex::new(None));
    let next_id = Arc::new(AtomicU64::new(1));
    let plan_prompt = Arc::new(CodexStaticPrompt::new(
        Arc::clone(&stdin),
        Arc::clone(&next_id),
        Arc::clone(&state),
        Arc::clone(&commands),
    ));
    let reader = CodexReader {
        images: None,
        available_commands: None,
        commands,
        buffer: Vec::new(),
        discarding_oversized_line: false,
        deferred: Vec::new(),
        manifest: None,
        state: Arc::clone(&state),
        view: CodexView::new(None),
        permission_broker: PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
        response_ids: Arc::new(Mutex::new(HashMap::new())),
        stdin,
        next_id,
        requests: Arc::new(CodexRequests::new()),
        compactions: crate::codex_compaction::CodexCompactions::default(),
        plan_prompt,
    };
    (reader, state, runtime, conn)
}

fn published(conn: &Arc<ConnHandle>) -> Vec<SessionEvent> {
    conn.pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect()
}

fn finished(events: &[SessionEvent]) -> Vec<&SessionEvent> {
    events
        .iter()
        .filter(|event| matches!(event, SessionEvent::AgentFinished { .. }))
        .collect()
}

fn turn_started(turn_id: &str) -> serde_json::Value {
    turn_started_on(ROOT, turn_id)
}

fn turn_started_on(thread_id: &str, turn_id: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "turn/started",
        "params": {"threadId": thread_id, "turn": {"id": turn_id}}
    })
}

fn turn_completed(thread_id: &str, turn_id: &str, status: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "turn/completed",
        "params": {"threadId": thread_id, "turn": {"id": turn_id, "status": status}}
    })
}

fn failed_turn_completed(thread_id: &str, turn_id: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "turn/completed",
        "params": {
            "threadId": thread_id,
            "turn": {"id": turn_id, "status": "failed", "error": {"message": message}}
        }
    })
}

/// The response to a `turn/start` this daemon wrote: the id is the one
/// `record_turn_start` registered for it.
fn turn_start_response(id: &str, turn_id: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {"turn": {"id": turn_id}}
    })
}

/// A response whose id matches no request this test registered — the shape
/// `codex_client_tests.rs` pins as a steer refusal.
fn foreign_turn_response(turn_id: &str) -> serde_json::Value {
    turn_start_response("d-97", turn_id)
}

fn current_context_tokens(state: &CodexState) -> Option<u64> {
    match state.manifest() {
        SessionEvent::SessionManifest {
            current_model_id,
            models,
            ..
        } => models
            .iter()
            .find(|model| Some(&model.model_id) == current_model_id.as_ref())
            .and_then(|model| model.context_tokens),
        other => panic!("the manifest is a SessionManifest, got {other:?}"),
    }
}

fn token_usage(thread_id: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "thread/tokenUsage/updated",
        "params": {
            "threadId": thread_id,
            "tokenUsage": {"last": {"inputTokens": 10, "outputTokens": 2, "totalTokens": 12}}
        }
    })
}

fn agent_delta(thread_id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "item/agentMessage/delta",
        "params": {"threadId": thread_id, "itemId": "m-1", "delta": text}
    })
}

#[test]
fn a_child_threads_completion_leaves_the_run_active_and_its_usage_latched() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_started("turn-B"), &runtime);
    reader.dispatch_value(token_usage(ROOT), &runtime);

    reader.dispatch_value(turn_completed(CHILD, "turn-A", "completed"), &runtime);
    let events = published(&conn);
    assert!(
        finished(&events).is_empty(),
        "a child thread's completion is not the run's end: {events:?}"
    );
    assert_eq!(
        state.current_turn().as_deref(),
        Some("turn-B"),
        "the run's turn survives the child's completion"
    );

    reader.dispatch_value(turn_completed(ROOT, "turn-B", "completed"), &runtime);
    let events = published(&conn);
    let ended = finished(&events);
    assert_eq!(
        ended.len(),
        1,
        "the root completion for turn-B ends the run"
    );
    match ended[0] {
        SessionEvent::AgentFinished {
            stop_reason, usage, ..
        } => {
            assert_eq!(stop_reason, "completed");
            assert!(
                usage.is_some(),
                "the child's completion must not consume the run's usage"
            );
        }
        other => panic!("filtered above, got {other:?}"),
    }
    assert_eq!(state.current_turn(), None);
}

#[test]
fn a_stale_root_completion_for_the_previous_turn_leaves_the_run_active() {
    let (mut reader, state, runtime, conn) = started_reader();
    // A ran first; B is the daemon's replace, adopted through its own
    // `turn/start` response — the measured wire answers that before the
    // turn's `turn/started`, so the response is what may displace A.
    reader.dispatch_value(turn_started("turn-A"), &runtime);
    state.record_turn_start("d-1", false);
    reader.dispatch_value(turn_start_response("d-1", "turn-B"), &runtime);
    reader.dispatch_value(token_usage(ROOT), &runtime);

    reader.dispatch_value(turn_completed(ROOT, "turn-A", "completed"), &runtime);
    let events = published(&conn);
    assert!(
        finished(&events).is_empty(),
        "a completion for a turn that was replaced does not end the live run: {events:?}"
    );
    assert_eq!(
        state.current_turn().as_deref(),
        Some("turn-B"),
        "the live turn survives the stale completion"
    );

    reader.dispatch_value(turn_completed(ROOT, "turn-B", "completed"), &runtime);
    let events = published(&conn);
    let ended = finished(&events);
    assert_eq!(ended.len(), 1, "turn-B's own completion still ends the run");
    match ended[0] {
        SessionEvent::AgentFinished { usage, .. } => {
            assert!(
                usage.is_some(),
                "the stale completion must not take the usage"
            );
        }
        other => panic!("filtered above, got {other:?}"),
    }
}

#[test]
fn the_current_turns_own_completion_ends_the_run() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_started("turn-B"), &runtime);
    reader.dispatch_value(turn_completed(ROOT, "turn-B", "completed"), &runtime);
    let events = published(&conn);
    let ended = finished(&events);
    assert_eq!(
        ended.len(),
        1,
        "the positive control ends the run exactly as before"
    );
    match ended[0] {
        SessionEvent::AgentFinished { stop_reason, .. } => {
            assert_eq!(stop_reason, "completed");
        }
        other => panic!("filtered above, got {other:?}"),
    }
    assert_eq!(state.current_turn(), None);
}

#[test]
fn an_interrupted_completion_for_turn_a_does_not_settle_turn_b() {
    let (mut reader, state, runtime, conn) = started_reader();
    // A ran first and was replaced by the daemon's turn/start for B; its
    // interrupted acknowledgement arrives after B was adopted.
    reader.dispatch_value(turn_started("turn-A"), &runtime);
    state.record_turn_start("d-1", false);
    reader.dispatch_value(turn_start_response("d-1", "turn-B"), &runtime);

    reader.dispatch_value(turn_completed(ROOT, "turn-A", "interrupted"), &runtime);
    let events = published(&conn);
    assert!(
        finished(&events).is_empty(),
        "turn A's interrupt acknowledgement must not settle turn B: {events:?}"
    );
    assert_eq!(state.current_turn().as_deref(), Some("turn-B"));

    reader.dispatch_value(turn_completed(ROOT, "turn-B", "interrupted"), &runtime);
    let events = published(&conn);
    let ended = finished(&events);
    assert_eq!(ended.len(), 1, "turn B's own interrupt still ends the run");
    match ended[0] {
        SessionEvent::AgentFinished { stop_reason, .. } => {
            assert_eq!(stop_reason, "interrupted");
        }
        other => panic!("filtered above, got {other:?}"),
    }
}

#[test]
fn a_root_completion_with_no_tracked_turn_still_settles() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_completed(ROOT, "turn-x", "completed"), &runtime);
    let events = published(&conn);
    let ended = finished(&events);
    assert_eq!(
        ended.len(),
        1,
        "with no tracked turn there is nothing to correlate against; dropping \
         would orphan a run whose completion never comes again"
    );
    assert_eq!(state.current_turn(), None);
}

#[test]
fn a_child_threads_message_delta_never_reaches_the_main_channel() {
    let (mut reader, _, runtime, conn) = started_reader();

    reader.dispatch_value(agent_delta(CHILD, "child narration"), &runtime);
    let events = published(&conn);
    assert!(
        events.is_empty(),
        "a child thread's message is dropped whole, not merely unnarrated: {events:?}"
    );

    reader.dispatch_value(agent_delta(ROOT, "root reply"), &runtime);
    let events = published(&conn);
    match events.as_slice() {
        [SessionEvent::AgentMessage { text, .. }] => {
            assert_eq!(text, "root reply");
        }
        other => panic!("a root delta reaches the main channel unchanged: {other:?}"),
    }
}

#[test]
fn replay_drops_child_thread_output_and_keeps_the_roots() {
    let plans = HashSet::new();
    let mut view = CodexView::new(None);

    let child = agent_delta(CHILD, "child narration");
    assert!(crate::codex_view::drive_replay(&mut view, Some(ROOT), &plans, &child).is_empty());
    let child_completion = turn_completed(CHILD, "turn-A", "completed");
    assert!(
        crate::codex_view::drive_replay(&mut view, Some(ROOT), &plans, &child_completion)
            .is_empty()
    );

    let root = agent_delta(ROOT, "root reply");
    let events = crate::codex_view::drive_replay(&mut view, Some(ROOT), &plans, &root);
    assert!(
        matches!(events.as_slice(), [SessionEvent::AgentMessage { .. }]),
        "the root's own output replays as before: {events:?}"
    );
}

#[test]
fn a_response_for_another_turn_never_displaces_the_tracked_turn() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_started("turn-B"), &runtime);

    // The shape codex_client_tests pins as a steer refusal: a response that
    // names a turn this daemon never started from this request.
    reader.dispatch_value(foreign_turn_response("turn-9"), &runtime);
    assert_eq!(
        state.current_turn().as_deref(),
        Some("turn-B"),
        "only a turn/start response matched to its own request writes the slot"
    );

    reader.dispatch_value(turn_completed(ROOT, "turn-B", "completed"), &runtime);
    let events = published(&conn);
    let ended = finished(&events);
    assert_eq!(
        ended.len(),
        1,
        "the live turn's completion still ends the run"
    );
    assert_eq!(state.current_turn(), None);
}

#[test]
fn a_child_threads_turn_started_does_not_displace_the_tracked_turn() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_started("turn-B"), &runtime);

    reader.dispatch_value(turn_started_on(CHILD, "turn-C"), &runtime);
    assert_eq!(
        state.current_turn().as_deref(),
        Some("turn-B"),
        "a child thread's turn leaves the root slot alone"
    );

    reader.dispatch_value(turn_completed(ROOT, "turn-B", "completed"), &runtime);
    let events = published(&conn);
    let ended = finished(&events);
    assert_eq!(ended.len(), 1, "the root turn's completion still settles");
}

#[test]
fn a_failed_turn_ends_the_run_and_keeps_the_error_notice() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_started("turn-B"), &runtime);

    reader.dispatch_value(
        failed_turn_completed(ROOT, "turn-B", "model refused"),
        &runtime,
    );
    let events = published(&conn);
    assert!(
        events.iter().any(
            |event| matches!(event, SessionEvent::SessionNotice { text, .. }
                if text == "model refused")
        ),
        "the failure is still told: {events:?}"
    );
    let ended = finished(&events);
    assert_eq!(ended.len(), 1, "a failed turn ends the run like any other");
    match ended[0] {
        SessionEvent::AgentFinished { stop_reason, .. } => {
            assert_eq!(stop_reason, "failed");
        }
        other => panic!("filtered above, got {other:?}"),
    }
    assert_eq!(state.current_turn(), None);
}

#[test]
fn a_child_threads_token_usage_never_touches_the_roots() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_started("turn-B"), &runtime);

    reader.dispatch_value(token_usage(CHILD), &runtime);
    let events = published(&conn);
    assert!(
        events.is_empty(),
        "a child thread's usage report publishes no meter: {events:?}"
    );
    assert_eq!(
        current_context_tokens(state.as_ref()),
        None,
        "the child's window must not become the root model's context_tokens"
    );

    reader.dispatch_value(turn_completed(ROOT, "turn-B", "completed"), &runtime);
    let events = published(&conn);
    let ended = finished(&events);
    match ended[0] {
        SessionEvent::AgentFinished { usage, .. } => {
            assert!(usage.is_none(), "the child's counts were never latched");
        }
        other => panic!("filtered above, got {other:?}"),
    }
}

#[test]
fn a_duplicate_root_completion_is_harmless() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_started("turn-B"), &runtime);
    runtime.begin_turn();
    assert!(
        runtime.is_running_turn(),
        "the setup has a live roster turn"
    );
    reader.dispatch_value(turn_completed(ROOT, "turn-B", "completed"), &runtime);
    assert_eq!(finished(&published(&conn)).len(), 1);
    assert!(
        !runtime.is_running_turn(),
        "the run ended at the first settle"
    );
    // A retried completion for an already-settled turn finds no tracked
    // turn and settles again: a duplicate row with an empty usage latch.
    // The roster transition is the no-op; the row is the accepted cost.
    reader.dispatch_value(turn_completed(ROOT, "turn-B", "completed"), &runtime);
    assert!(!runtime.is_running_turn(), "the duplicate starts nothing");
    assert_eq!(state.current_turn(), None);
    let events = published(&conn);
    let ended = finished(&events);
    assert_eq!(ended.len(), 1, "the duplicate publishes its own row");
    match ended[0] {
        SessionEvent::AgentFinished { usage, .. } => {
            assert!(usage.is_none(), "the latch was spent on the first settle");
        }
        other => panic!("filtered above, got {other:?}"),
    }
}

#[test]
fn a_provider_initiated_root_turn_never_displaces_the_tracked_turn() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_started("turn-A"), &runtime);

    // A root turn this daemon never started — a compaction, a review —
    // announces itself while A runs. It fills an empty slot only: it may
    // complete through `thread/compacted` alone, and adopting it would let
    // A's own completion be dropped as provably late, stranding the run.
    reader.dispatch_value(turn_started_on(ROOT, "turn-C"), &runtime);
    assert_eq!(
        state.current_turn().as_deref(),
        Some("turn-A"),
        "a provider-initiated root turn leaves the tracked turn alone"
    );

    reader.dispatch_value(turn_completed(ROOT, "turn-A", "completed"), &runtime);
    let events = published(&conn);
    let ended = finished(&events);
    assert_eq!(ended.len(), 1, "the run's own completion ends the run");
    assert_eq!(state.current_turn(), None);
}

#[test]
fn a_completion_the_daemon_cannot_prove_late_still_settles() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_started("turn-B"), &runtime);

    // A root completion for a turn this session never adopted: nothing
    // proves it predates turn-B, so the run settles rather than strands.
    reader.dispatch_value(turn_completed(ROOT, "turn-unknown", "completed"), &runtime);
    let events = published(&conn);
    let ended = finished(&events);
    assert_eq!(ended.len(), 1, "an unprovable completion is base behaviour");
    assert_eq!(state.current_turn(), None);
}

#[test]
fn a_stale_completion_does_not_close_the_compaction_pairing() {
    let (mut reader, state, runtime, conn) = started_reader();
    reader.dispatch_value(turn_started("turn-A"), &runtime);
    state.record_turn_start("d-1", false);
    reader.dispatch_value(turn_start_response("d-1", "turn-B"), &runtime);
    reader.dispatch_value(
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "item/started",
            "params": {"threadId": ROOT, "item": {"id": "c-1", "type": "contextCompaction"}}
        }),
        &runtime,
    );
    assert_eq!(
        notices(&conn),
        vec!["Compacting the context.".to_string()],
        "the compaction opens under the live turn"
    );

    reader.dispatch_value(turn_completed(ROOT, "turn-A", "completed"), &runtime);
    assert!(
        !notices(&conn)
            .iter()
            .any(|text| text == "Context compacted."),
        "the replaced turn's completion must not spend the live turn's pairing"
    );

    reader.dispatch_value(turn_completed(ROOT, "turn-B", "completed"), &runtime);
    assert!(
        notices(&conn)
            .iter()
            .any(|text| text == "Context compacted."),
        "the live turn's own boundary closes it"
    );
}

fn notices(conn: &Arc<ConnHandle>) -> Vec<String> {
    published(conn)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::SessionNotice { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}
