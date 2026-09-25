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
fn unfinished_root_compaction_is_closed_at_turn_end_and_late_completion_is_ignored() {
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
        1
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| *line == "Context compaction did not complete.")
            .count(),
        1
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
