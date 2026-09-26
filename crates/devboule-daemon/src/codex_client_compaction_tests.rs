//! The events a Codex command or compaction turns into: the list published at
//! session start, the answer a command is owed, and the two channels Codex
//! reports one compaction through.
//!
//! No child process here — the reader is driven with the frames the app-server
//! is measured to send, and the session's own event queue is what is read back.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use devboule_protocol::SessionEvent;

use super::super::event_pull::ConnHandle;
use super::super::permission_broker::PermissionBroker;
use super::super::session_runtime::SessionRuntime;
use super::command_test_support::{thread_state, Fixture};
use super::{CodexCommands, CodexReader, CodexRequests};
use crate::codex_view::CodexView;
use crate::session::ReaderDispatch;

/// One compaction item, as the app-server sends it: `item/started` or
/// `item/completed`, naming a `contextCompaction` item.
fn compaction_item(method: &str) -> serde_json::Value {
    serde_json::json!({
        "method": method,
        "params": {"threadId": "thread-fake", "item": {"id": "i-1", "type": "contextCompaction"}}
    })
}

fn thread_compacted(thread_id: &str) -> serde_json::Value {
    serde_json::json!({"method": "thread/compacted", "params": {"threadId": thread_id}})
}

/// A reader whose session has just started: the manifest and the command list
/// are both waiting to go out, on the first frame the child sends.
fn started_reader(
    commands: Arc<CodexCommands>,
) -> (CodexReader, Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.stream.lock().unwrap().screen = None;
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.codex.commands",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let state = thread_state();
    let reader = CodexReader {
        available_commands: Some(SessionEvent::AvailableCommands {
            commands: commands.views(),
        }),
        commands,
        buffer: Vec::new(),
        discarding_oversized_line: false,
        deferred: Vec::new(),
        manifest: Some(state.manifest()),
        state,
        view: CodexView::new(None),
        permission_broker: PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
        response_ids: Arc::new(Mutex::new(HashMap::new())),
        stdin: Arc::new(Mutex::new(None)),
        next_id: Arc::new(AtomicU64::new(1)),
        spawn_nonce: "test-spawn".to_string(),
        requests: Arc::new(CodexRequests::new()),
        compactions: crate::codex_compaction::CodexCompactions::default(),
    };
    (reader, runtime, conn)
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

fn published(conn: &Arc<ConnHandle>) -> Vec<SessionEvent> {
    conn.pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect()
}

#[test]
fn the_command_list_is_published_once_at_session_start() {
    let fixture = Fixture::new("publish");
    let commands = fixture.commands(true, true);
    let (mut reader, runtime, conn) = started_reader(commands);
    reader
        .feed(b"{}\n", &runtime)
        .expect("the first frame published the session's standing state");
    let events = published(&conn);
    let listed: Vec<&SessionEvent> = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::AvailableCommands { .. }))
        .collect();
    assert_eq!(
        listed.len(),
        1,
        "published once, beside the manifest: {events:?}"
    );
    match listed[0] {
        SessionEvent::AvailableCommands { commands } => {
            let names: Vec<&str> = commands.iter().map(|view| view.name.as_str()).collect();
            assert_eq!(
                names,
                ["compact", "goal", "plotting", "prompts:commit"],
                "the built-ins and the two files, in name order"
            );
            let goal = commands
                .iter()
                .find(|view| view.name == "goal")
                .expect("the gated goal entry is published");
            assert_eq!(
                goal.hint.as_deref(),
                Some("[<objective>|pause|resume|clear]"),
                "the menu carries the arguments the command accepts"
            );
        }
        other => panic!("filtered above, got {other:?}"),
    }
    // A second frame says nothing more about commands: the list is read at
    // session start, not re-walked per frame.
    reader
        .feed(b"{}\n", &runtime)
        .expect("the second feed runs");
    assert!(
        published(&conn)
            .iter()
            .all(|event| !matches!(event, SessionEvent::AvailableCommands { .. })),
        "the list is published once per session"
    );
}

/// A temp dir removed on drop, even when an assertion panics midway — a
/// manual `remove_dir_all` at the end leaks the directory on failure.
struct CleanupDir(std::path::PathBuf);

impl Drop for CleanupDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_command_list_written_by_codex_reader_is_in_session_replay() {
    let fixture = Fixture::new("published-replay");
    let commands = fixture.commands(true, true);
    let (mut reader, _, _) = started_reader(Arc::clone(&commands));
    let session_id = "s.codex.command-list-replay";
    let dir = CleanupDir(crate::test_dirs::test_temp_dir(
        "devboule-codex-command-replay",
    ));
    let journal = Arc::new(crate::journal::Journal::open(&dir.0.join("journal.db")).unwrap());
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            devboule_protocol::SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.stream.lock().unwrap().screen = None;
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach before Codex publishes its list");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );

    reader
        .feed(b"{}\n", &runtime)
        .expect("Codex reader publishes the startup list");
    let replay = journal.replay(session_id).expect("replay the produced row");
    assert!(replay.events.iter().any(|event| matches!(
        event,
        SessionEvent::AvailableCommands { commands }
            if commands.iter().any(|command| command.name == "prompts:commit")
    )));
    drop(conn);
    drop(runtime);
    drop(journal);
    drop(dir);
}

#[test]
fn a_command_answer_is_told_in_the_commands_own_words_and_only_once() {
    let fixture = Fixture::new("answer");
    let commands = fixture.commands(false, true);
    let command = commands
        .command("/goal clear")
        .expect("goals are enabled on this surface");
    assert!(commands.owe("d-1", &command));
    let (mut reader, runtime, conn) = started_reader(commands);
    reader.dispatch_value(
        serde_json::json!({
            "id": "d-1",
            "error": { "code": -32600, "message": "no active goal" }
        }),
        &runtime,
    );
    assert_eq!(
        notices(&conn),
        vec!["Failed to update goal: no active goal".to_string()],
        "Paseo's own sentence for a failed goal (:5081-5085), told once — the \
         generic error notice does not also go out"
    );
}

#[test]
fn an_accepted_compaction_answer_publishes_nothing() {
    let fixture = Fixture::new("compact-answer");
    let commands = fixture.commands(false, true);
    let command = commands.command("/compact").expect("compact is a command");
    commands.owe("d-2", &command);
    let (mut reader, runtime, conn) = started_reader(commands);
    reader.dispatch_value(serde_json::json!({ "id": "d-2", "result": {} }), &runtime);
    assert!(
        notices(&conn).is_empty(),
        "Paseo's compact success emits nothing; `thread/compacted` is the report"
    );
}

#[test]
fn a_null_error_is_a_successful_compact_response() {
    let fixture = Fixture::new("null-error");
    let commands = fixture.commands(false, true);
    let command = commands.command("/compact").expect("compact is a command");
    assert!(commands.owe("d-null", &command));
    let (mut reader, runtime, conn) = started_reader(commands);
    reader.dispatch_value(
        serde_json::json!({ "id": "d-null", "result": {}, "error": null }),
        &runtime,
    );
    assert!(
        notices(&conn).is_empty(),
        "JSON null is falsy, like Paseo raw.error"
    );
}

#[test]
fn a_response_that_is_not_a_command_keeps_the_plain_error_notice() {
    let fixture = Fixture::new("plain-error");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, true));
    reader.dispatch_value(
        serde_json::json!({
            "id": "d-99",
            "error": { "code": -32600, "message": "turn gone" }
        }),
        &runtime,
    );
    assert_eq!(
        notices(&conn),
        vec!["turn gone".to_string()],
        "the handling every other response already had is unchanged"
    );
}

#[test]
fn an_error_without_a_message_never_becomes_a_success_line() {
    let fixture = Fixture::new("error-without-message");
    let commands = fixture.commands(false, true);
    let command = commands.command("/goal clear").expect("goal is enabled");
    assert!(commands.owe("d-4", &command));
    let (mut reader, runtime, conn) = started_reader(commands);
    reader.dispatch_value(
        serde_json::json!({
            "id": "d-4", "error": {"code": -32601}
        }),
        &runtime,
    );
    assert_eq!(
        notices(&conn),
        vec!["Failed to update goal: Codex returned an error without a message".to_string()]
    );
}

#[test]
fn one_compaction_is_one_notice_whichever_channel_arrives_first() {
    // Codex can report one finished compaction twice — as a completed
    // `contextCompaction` item and as `thread/compacted`. Paseo pairs the two
    // with counters (:5612-5621, :6211-6216); either order, the session hears
    // it once.
    for (first, second) in [
        (
            compaction_item("item/completed"),
            thread_compacted("thread-fake"),
        ),
        (
            thread_compacted("thread-fake"),
            compaction_item("item/completed"),
        ),
    ] {
        let fixture = Fixture::new("compaction");
        let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
        reader.dispatch_value(first, &runtime);
        reader.dispatch_value(second, &runtime);
        assert_eq!(
            notices(&conn),
            vec!["Context compacted.".to_string()],
            "the pair reports one compaction, once"
        );
    }
}

#[test]
fn a_finished_turn_forgets_an_unpaired_compaction_count() {
    // A compaction that reported on one channel only leaves the pairing
    // counter standing. Paseo clears it at the turn boundary
    // (`resetTurnTrackingState` :6046-6053), and so must this reader: without
    // that, the NEXT turn's compaction would be swallowed as the pair of a
    // compaction that already finished.
    let fixture = Fixture::new("turn-boundary");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    reader.dispatch_value(compaction_item("item/completed"), &runtime);
    reader.dispatch_value(
        serde_json::json!({"method": "turn/completed",
            "params": {"turn": {"id": "t-1", "status": "completed"}}}),
        &runtime,
    );
    reader.dispatch_value(thread_compacted("thread-fake"), &runtime);
    assert_eq!(
        notices(&conn),
        vec![
            "Context compacted.".to_string(),
            "Context compacted.".to_string()
        ],
        "the turn boundary separates the two compactions, so both are told"
    );
}

#[test]
fn a_compaction_under_way_says_so() {
    let fixture = Fixture::new("compacting");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    reader.dispatch_value(compaction_item("item/started"), &runtime);
    assert_eq!(
        notices(&conn),
        vec!["Compacting the context.".to_string()],
        "Paseo's loading row (:6583-6588) is this notice"
    );
}

#[test]
fn a_root_compaction_without_thread_id_is_kept() {
    let fixture = Fixture::new("missing-thread-id");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    reader.dispatch_value(
        serde_json::json!({
            "method": "item/started",
            "params": { "item": { "id": "i-root", "type": "contextCompaction" } }
        }),
        &runtime,
    );
    assert_eq!(notices(&conn), ["Compacting the context."]);
}

#[test]
fn a_child_turn_end_does_not_close_root_compaction_state() {
    let fixture = Fixture::new("child-turn-end");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    reader.dispatch_value(compaction_item("item/started"), &runtime);
    reader.dispatch_value(
        serde_json::json!({ "method": "turn/completed", "params": { "threadId": "other-thread" } }),
        &runtime,
    );
    reader.dispatch_value(compaction_item("item/completed"), &runtime);
    assert_eq!(
        notices(&conn),
        ["Compacting the context.", "Context compacted."],
        "the child turn end leaves root pairing state intact"
    );
}

#[test]
fn a_compaction_from_another_thread_is_silent_on_both_channels() {
    // Codex runs sub-agent threads over the same stream; Paseo drops a
    // `thread/compacted` naming a thread other than the session's (:6205-6207).
    let fixture = Fixture::new("other-thread");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    reader.dispatch_value(thread_compacted("other-thread"), &runtime);
    reader.dispatch_value(serde_json::json!({
        "method": "item/started",
        "params": {"threadId": "other-thread", "item": {"id": "child-1", "type": "contextCompaction"}}
    }), &runtime);
    assert!(
        notices(&conn).is_empty(),
        "not this session's compaction, so nothing is said"
    );
}

#[test]
fn a_late_completion_after_turn_end_reports_again_like_paseo() {
    // Paseo keeps no stale set: its turn reset clears the pending ids
    // (`resetTurnTrackingState` :6037-6059), so a completion that lands after
    // the boundary opens a new count instead of being swallowed. The late
    // frame below is the third report, not a duplicate of the second.
    let fixture = Fixture::new("unfinished-compaction");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    reader.dispatch_value(compaction_item("item/started"), &runtime);
    reader.dispatch_value(compaction_item("item/completed"), &runtime);
    reader.dispatch_value(compaction_item("item/started"), &runtime);
    reader.dispatch_value(
        serde_json::json!({
            "method": "turn/completed",
            "params": {"turn": {"id": "t-1", "status": "completed"}}
        }),
        &runtime,
    );
    reader.dispatch_value(compaction_item("item/completed"), &runtime);
    let lines = notices(&conn);
    assert_eq!(
        lines
            .iter()
            .filter(|line| *line == "Compacting the context.")
            .count(),
        2
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| *line == "Context compacted.")
            .count(),
        3,
        "paired, turn-end close, then the late frame as a new count: {lines:?}"
    );
}

#[test]
fn a_notification_before_turn_end_reports_one_compaction() {
    // The double-emit P2: `thread/compacted` used to leave the pending item
    // in place, so the turn end reported the same compaction again. Paseo
    // consumes the pending item in its notification handler
    // (`consumePendingRootCompaction` :6211), and so does this one.
    let fixture = Fixture::new("notify-then-end");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    reader.dispatch_value(compaction_item("item/started"), &runtime);
    reader.dispatch_value(thread_compacted("thread-fake"), &runtime);
    reader.dispatch_value(
        serde_json::json!({"method": "turn/completed",
            "params": {"threadId": "thread-fake", "turn": {"id": "t-1", "status": "completed"}}}),
        &runtime,
    );
    assert_eq!(
        notices(&conn),
        ["Compacting the context.", "Context compacted."],
        "loading, then the one completion — the turn end finds nothing pending"
    );
}

#[test]
fn a_compacted_notification_without_a_thread_id_is_dropped() {
    // D1, Paseo-exact: `ContextCompactedNotificationSchema` requires
    // `threadId`, so a frame without one is an invalid payload Paseo warns
    // and drops — never a root event. An empty string compares strictly
    // against the current thread (:6205-6207), so it drops too.
    for params in [serde_json::json!({}), serde_json::json!({"threadId": ""})] {
        let fixture = Fixture::new("compacted-no-thread");
        let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
        reader.dispatch_value(
            serde_json::json!({"method": "thread/compacted", "params": params}),
            &runtime,
        );
        assert!(
            notices(&conn).is_empty(),
            "no root notice for a thread-less compacted frame: {params}"
        );
    }
}

#[test]
fn an_empty_thread_id_is_the_root_thread_on_the_optional_channels() {
    // D1, Paseo-exact: `getSubAgentCallIdForThread` treats a missing or empty
    // id as the root thread (:5477), for the channels whose schema leaves it
    // optional (`item/*`, `turn/completed`).
    let fixture = Fixture::new("empty-thread-id");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    reader.dispatch_value(
        serde_json::json!({
            "method": "item/started",
            "params": {"threadId": "", "item": {"id": "i-empty", "type": "contextCompaction"}}
        }),
        &runtime,
    );
    reader.dispatch_value(
        serde_json::json!({"method": "turn/completed",
            "params": {"threadId": "", "turn": {"id": "t-1", "status": "completed"}}}),
        &runtime,
    );
    assert_eq!(
        notices(&conn),
        ["Compacting the context.", "Context compacted."],
        "the empty id walks the root path: loading, then the turn-end close"
    );
}

#[test]
fn a_new_turn_starts_with_clear_pairing_counts() {
    // Paseo resets its turn tracking on the root `turn/started`
    // (`resetTurnTrackingState` :5973-5991): a completion that landed between
    // turns must not swallow the next turn's notification through a stale
    // count — while a child thread's turn leaves the root counts alone.
    let fixture = Fixture::new("turn-start-reset");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    // A completion with nothing pending opens a count...
    reader.dispatch_value(compaction_item("item/completed"), &runtime);
    // ...that the next root turn's start clears before its own notification.
    reader.dispatch_value(
        serde_json::json!({"method": "turn/started",
            "params": {"threadId": "thread-fake", "turn": {"id": "t-2"}}}),
        &runtime,
    );
    reader.dispatch_value(thread_compacted("thread-fake"), &runtime);
    assert_eq!(
        notices(&conn),
        ["Context compacted.", "Context compacted."],
        "the between-turns completion reports, and so does the new turn's notification"
    );

    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    reader.dispatch_value(compaction_item("item/completed"), &runtime);
    reader.dispatch_value(
        serde_json::json!({"method": "turn/started",
            "params": {"threadId": "other-thread", "turn": {"id": "t-child"}}}),
        &runtime,
    );
    reader.dispatch_value(thread_compacted("thread-fake"), &runtime);
    assert_eq!(
        notices(&conn),
        ["Context compacted."],
        "a child turn start is not the root boundary: the pending count still pairs"
    );
}

#[test]
fn an_ordinary_item_still_reaches_the_view_untouched() {
    // The compaction branch must not swallow the frames the view already maps:
    // a completed command item still becomes the tool update it was.
    let fixture = Fixture::new("other-item");
    let (mut reader, runtime, conn) = started_reader(fixture.commands(false, false));
    reader.dispatch_value(
        serde_json::json!({
            "method": "item/completed",
            "params": {"item": {"id": "i-2", "type": "commandExecution", "status": "completed",
                                 "command": "cargo test", "aggregatedOutput": "ok"}}
        }),
        &runtime,
    );
    let events = published(&conn);
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, SessionEvent::SessionNotice { .. })),
        "a command item is not a compaction"
    );
    let tools: Vec<&SessionEvent> = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::AgentToolUpdate { .. }))
        .collect();
    assert_eq!(tools.len(), 1, "the view still maps it: {events:?}");
}
