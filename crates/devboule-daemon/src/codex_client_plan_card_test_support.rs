//! Shared harness for the plan-card reader tests: the captured plan-turn
//! frames, a reader whose session advertises plan mode, and the small readers a
//! test uses to assert on what was published.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use devboule_protocol::{SessionEvent, SessionKind};

use super::super::event_pull::ConnHandle;
use super::super::permission_broker::PermissionBroker;
use super::{empty_commands, CodexReader, CodexRequests};
use crate::journal::{new_session_record, Journal};
use crate::session::session_runtime::SessionRuntime;

pub(super) const LIVE_PLAN: &str =
    include_str!("../fixtures/wire/codex/codex-plan-items-live.jsonl");

pub(super) const SESSION: &str = "s.codex.plan-card";

/// The captured plan turn's own ids: the state's thread must match the frames'
/// `threadId`, or the reader's root-thread gate drops them.
pub(super) const THREAD: &str = "01a0e350-f461-7310-a1ef-f3f402825e31";
pub(super) const TURN: &str = "01a0e350-f530-73b2-9251-bb450adce3f7";

/// A reader whose session advertises plan mode, on a journal the way a live
/// session is: the resume reuses the same session row, so a card the journal
/// already closed is refused.
pub(super) fn plan_reader() -> (
    CodexReader,
    Arc<SessionRuntime>,
    Arc<ConnHandle>,
    Arc<PermissionBroker>,
) {
    plan_reader_observed_by(true)
}

/// `typed_permissions: false` is an observer that cannot receive a card, so
/// the broker refuses the registration as `capability_not_supported`.
pub(super) fn plan_reader_observed_by(
    typed_permissions: bool,
) -> (
    CodexReader,
    Arc<SessionRuntime>,
    Arc<ConnHandle>,
    Arc<PermissionBroker>,
) {
    let dir = crate::test_dirs::test_temp_dir("devboule-codex-plan-card");
    let path = dir.join("broker.sqlite");
    let journal = Arc::new(Journal::open(&path).expect("journal"));
    journal
        .upsert_blocking(new_session_record(
            SESSION.to_string(),
            "owner",
            None,
            SessionKind::Codex,
            "codex plan card test",
        ))
        .expect("session row");
    let runtime = Arc::new(SessionRuntime::with_journal(
        SESSION.to_string(),
        Some(journal),
    ));
    runtime.stream.lock().unwrap().screen = None;
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, typed_permissions)
        .expect("attach");
    conn.track_with_agent_replay(
        SESSION,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let broker = PermissionBroker::with_sender(Arc::new(|_, _| Ok(())));
    let state = Arc::new(crate::codex_view::CodexState::new(
        THREAD.to_string(),
        crate::codex_view::catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog"),
        "auto",
    ));
    state
        .set_collaboration_modes(&serde_json::json!({
            "data": [
                { "name": "Plan", "mode": "plan" },
                { "name": "Default", "mode": "default" }
            ]
        }))
        .expect("modes");
    let commands = empty_commands();
    let stdin = Arc::new(Mutex::new(None));
    let next_id = Arc::new(AtomicU64::new(1));
    let plan_prompt = Arc::new(super::CodexStaticPrompt::new(
        Arc::clone(&stdin),
        Arc::clone(&next_id),
        Arc::clone(&state),
        Arc::clone(&commands),
    ));
    let reader = CodexReader {
        images: None,
        commands,
        available_commands: None,
        buffer: Vec::new(),
        discarding_oversized_line: false,
        deferred: Vec::new(),
        manifest: None,
        state,
        view: crate::codex_view::CodexView::new(None),
        permission_broker: Arc::clone(&broker),
        response_ids: Arc::new(Mutex::new(HashMap::new())),
        stdin,
        next_id,
        requests: Arc::new(CodexRequests::new()),
        compactions: crate::codex_compaction::CodexCompactions::default(),
        plan_prompt,
    };
    (reader, runtime, conn, broker)
}

/// The session events the harness published, unwrapped from their envelopes.
pub(super) fn published(conn: &Arc<ConnHandle>) -> Vec<SessionEvent> {
    conn.pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect()
}

/// A stdin the prompt send can write to: a child that stays alive so the
/// pipe does not break under the write.
pub(super) fn working_stdin() -> Arc<Mutex<Option<std::process::ChildStdin>>> {
    let mut child = std::process::Command::new("node")
        .args(["-e", "process.stdin.resume()"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("node");
    let stdin = child.stdin.take().expect("stdin");
    std::mem::forget(child);
    Arc::new(Mutex::new(Some(stdin)))
}

/// A runtime with a conn attached, so a test can read what was published.
pub(super) fn capturing_runtime() -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.stream.lock().unwrap().screen = None;
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.codex.plan-card-author",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    (runtime, conn)
}

/// A conn on `runtime` that reads the live stream alone, in publication
/// order: no journal rebuild is put in front of it.
pub(super) fn live_conn(runtime: &Arc<SessionRuntime>) -> Arc<ConnHandle> {
    let conn = ConnHandle::new(3);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        SESSION,
        Arc::clone(runtime),
        false,
        None,
        outcome.generation,
        None,
    );
    conn
}

/// The captured plan-turn frames under one turn's own id: the thread, the
/// turn, and the plan item's `<turnId>-plan` id all follow it.
pub(super) fn frames_for_turn(turn_id: &str) -> Vec<serde_json::Value> {
    crate::codex_view::fixture_frames(LIVE_PLAN)
        .into_iter()
        .map(|mut frame| {
            for (path, value) in [
                ("/params/threadId", serde_json::json!(THREAD)),
                ("/params/turnId", serde_json::json!(turn_id)),
                ("/params/turn/id", serde_json::json!(turn_id)),
                (
                    "/params/item/id",
                    serde_json::json!(format!("{turn_id}-plan")),
                ),
            ] {
                if let Some(slot) = frame.pointer_mut(path) {
                    *slot = value;
                }
            }
            frame
        })
        .collect()
}

/// The captured plan-turn frames, preceded by the `turn/started` that carries
/// the turn's plan mode: the reader's own correlation, driven the way the
/// measured wire drives it.
pub(super) fn feed_plan_turn(reader: &mut CodexReader, runtime: &Arc<SessionRuntime>) {
    let frames = crate::codex_view::fixture_frames(LIVE_PLAN);
    reader.state.record_turn_start("d-1", true);
    reader.dispatch_value(
        serde_json::json!({
            "method": "turn/started",
            "params": {"threadId": THREAD, "turn": {"id": TURN}}
        }),
        runtime,
    );
    for frame in &frames {
        reader.dispatch_value(frame.clone(), runtime);
    }
}

/// The captured plan items, then a `turn/completed` whose status is
/// `interrupted`: the completion that folds the row in instead of raising a
/// card.
pub(super) fn feed_interrupted_plan_turn(reader: &mut CodexReader, runtime: &Arc<SessionRuntime>) {
    let frames = crate::codex_view::fixture_frames(LIVE_PLAN);
    reader.state.record_turn_start("d-1", true);
    reader.dispatch_value(
        serde_json::json!({
            "method": "turn/started",
            "params": {"threadId": THREAD, "turn": {"id": TURN}}
        }),
        runtime,
    );
    reader.dispatch_value(frames[0].clone(), runtime);
    reader.dispatch_value(frames[1].clone(), runtime);
    reader.dispatch_value(
        serde_json::json!({
            "method": "turn/completed",
            "params": {"threadId": THREAD, "turn": {"id": TURN, "status": "interrupted"}}
        }),
        runtime,
    );
}

/// What a client attaching now is sent: the journal read back through the
/// attach reader (`replay_agent_page`, re-derived by a fresh Codex view in
/// `event_pull`), not the recovery reader.
pub(super) fn attach_replay(runtime: &Arc<SessionRuntime>) -> Vec<SessionEvent> {
    // The spawn road sets the kind; it picks the Codex view on replay.
    runtime.set_agent_kind(SessionKind::Codex);
    runtime
        .journal
        .as_ref()
        .expect("journal")
        .flush()
        .expect("flush");
    let conn = ConnHandle::new(2);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    assert!(
        outcome.live_agent_replay.is_some(),
        "the attach replays the journal"
    );
    conn.track_with_agent_replay(
        SESSION,
        Arc::clone(runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let mut events = Vec::new();
    for _ in 0..64 {
        let page = published(&conn);
        if page.is_empty() {
            return events;
        }
        events.extend(page);
    }
    panic!("the attach replay never drained: {events:?}");
}
