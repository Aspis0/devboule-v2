//! The pass a resumed reader runs: the one `get_messages` read, the one
//! `AgentTasks` it publishes, and the row it leaves behind.

use devboule_protocol::{AgentTaskStatus, SessionEvent, SessionKind};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::super::PiControl;
use super::test_support::{
    attached_runtime, fake_pi, feed_reader, item, pull_until, reader_for, restored_tasks,
    rows_after_the_snapshot,
};
use crate::journal::{EventKind, Journal};
use crate::session::write_child_stdin;

/// The prompt's one text frame is what later proves the session still
/// dispatches after the read.
const PI_RESTORES_THE_CHECKLIST: &str = r#"
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    if (frame.type === "get_messages") {
      process.stdout.write(JSON.stringify({
        id: frame.id, type: "response", command: "get_messages", success: true,
        data: { messages: [
          { role: "assistant", content: [
            { type: "toolCall", id: "call-1", name: "set_goal_tasks", arguments: {} }
          ] },
          { role: "toolResult", toolCallId: "call-1", toolName: "set_goal_tasks", isError: false,
            content: [{ type: "text", text: "Task list set." }],
            details: { version: 3, goal: { taskList: { tasks: [
              { id: "task-1", title: "First", status: "pending" }
            ] } } } },
          { role: "assistant", content: [
            { type: "toolCall", id: "call-2", name: "set_goal_tasks", arguments: {} }
          ] },
          { role: "toolResult", toolCallId: "call-2", toolName: "set_goal_tasks", isError: false,
            content: [{ type: "text", text: "Task list set." }],
            details: { version: 3, goal: { taskList: { tasks: [
              { id: "task-1", title: "First", status: "complete" },
              { id: "task-2", title: "Second", status: "pending" }
            ] } } } }
        ] }
      }) + "\n");
      continue;
    }
    if (frame.type === "prompt") {
      process.stdout.write(JSON.stringify({
        type: "message_update",
        assistantMessageEvent: { type: "text_delta", delta: "still here" }
      }) + "\n");
    }
  }
});
"#;

/// The prompt's answer waits for the refusal, so a pass that never asked
/// cannot leave this test green.
const PI_REFUSES_THE_HISTORY: &str = r#"
let buffered = "";
let refused = false;
let promptHeld = false;
const answerPrompt = () => {
  process.stdout.write(JSON.stringify({
    type: "message_update",
    assistantMessageEvent: { type: "text_delta", delta: "still here" }
  }) + "\n");
};
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    if (frame.type === "get_messages") {
      process.stdout.write(JSON.stringify({
        id: frame.id, type: "response", command: "get_messages",
        success: false, error: "Unknown command: get_messages"
      }) + "\n");
      refused = true;
      if (promptHeld) { promptHeld = false; answerPrompt(); }
      continue;
    }
    if (frame.type === "prompt") {
      if (refused) { answerPrompt(); } else { promptHeld = true; }
    }
  }
});
"#;

/// The reply the test releases: the fake writes the live checklist, then
/// the held reply, then the marker — the ordering the gate must survive.
const PI_HOLDS_THE_HISTORY: &str = r#"
let buffered = "";
let held = null;
const write = (value) => process.stdout.write(JSON.stringify(value) + "\n");
const delta = (text) => write({ type: "message_update", assistantMessageEvent: { type: "text_delta", delta: text } });
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    if (frame.type === "get_messages") {
      held = {
        id: frame.id, type: "response", command: "get_messages", success: true,
        data: { messages: [
          { role: "assistant", content: [
            { type: "toolCall", id: "call-1", name: "set_goal_tasks", arguments: {} }
          ] },
          { role: "toolResult", toolCallId: "call-1", toolName: "set_goal_tasks", isError: false,
            content: [{ type: "text", text: "Task list set." }],
            details: { version: 3, goal: { taskList: { tasks: [
              { id: "task-1", title: "Restored", status: "pending" }
            ] } } } }
        ] }
      };
      delta("history-requested");
      continue;
    }
    if (frame.type === "release_after_live") {
      write({ type: "tool_execution_end", toolCallId: "call-live", toolName: "set_goal_tasks",
        isError: false, result: { details: { version: 3, goal: { taskList: { tasks: [
          { id: "task-live", title: "From the live turn", status: "pending" }
        ] } } } } });
      write(held);
      delta("history-released");
    }
  }
});
"#;

/// The pass end to end: the last restored snapshot publishes once, the
/// reply leaves no transcript row, and the session keeps working.
#[test]
fn a_resumed_pi_publishes_the_last_restored_checklist_once() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let dir = crate::test_dirs::test_temp_dir("devboule-pi-history-tasks");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let session_id = "pi-history-tasks";
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            "owner",
            None,
            SessionKind::Pi,
            "Pi",
        ))
        .expect("session row");
    let (runtime, conn) = attached_runtime(session_id, Some(Arc::clone(&journal)));
    let (mut child, stdin, stdout) = fake_pi(PI_RESTORES_THE_CHECKLIST);
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let reader = reader_for(Arc::clone(&control), &stdin).with_restored_history();
    let feeder = feed_reader(reader, stdout, Arc::clone(&runtime));

    let events = pull_until(
        &conn,
        |event| matches!(event, SessionEvent::AgentTasks { .. }),
        Duration::from_secs(10),
    );
    assert_eq!(
        restored_tasks(&events),
        vec![vec![
            item("task-1", "First", AgentTaskStatus::Completed),
            item("task-2", "Second", AgentTaskStatus::Pending),
        ]],
        "the last snapshot of the restored history, published once"
    );

    // The daemon's own read leaves no envelope row for the conversation.
    let records = rows_after_the_snapshot(&journal, session_id);
    assert!(
        records
            .iter()
            .all(|record| record.kind == EventKind::AgentReport),
        "the restored reply must leave no transcript row: {:?}",
        records.iter().map(|record| record.kind).collect::<Vec<_>>()
    );

    write_child_stdin(&stdin, b"{\"type\":\"prompt\"}\n", "Pi").expect("prompt");
    let mut all = events;
    all.extend(pull_until(
        &conn,
        |event| matches!(event, SessionEvent::AgentMessage { text, .. } if text == "still here"),
        Duration::from_secs(10),
    ));
    assert_eq!(
        restored_tasks(&all).len(),
        1,
        "the restored snapshot is published exactly once: {all:?}"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = feeder.join();
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A refused read publishes nothing and the session dispatches on.
#[test]
fn a_refused_history_read_publishes_nothing_and_the_session_continues() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let (mut child, stdin, stdout) = fake_pi(PI_REFUSES_THE_HISTORY);
    let (runtime, conn) = attached_runtime("pi-history-refused", None);
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let reader = reader_for(Arc::clone(&control), &stdin).with_restored_history();
    let feeder = feed_reader(reader, stdout, Arc::clone(&runtime));

    write_child_stdin(&stdin, b"{\"type\":\"prompt\"}\n", "Pi").expect("prompt");
    let events = pull_until(
        &conn,
        |event| matches!(event, SessionEvent::AgentMessage { text, .. } if text == "still here"),
        Duration::from_secs(10),
    );
    assert!(
        restored_tasks(&events).is_empty(),
        "a refused read publishes no checklist: {events:?}"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = feeder.join();
}

/// The ordering gate: a reply released after the live checklist published
/// finds an AgentTasks and must publish nothing over it.
#[test]
fn a_history_reply_released_after_a_live_checklist_publishes_nothing() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let (mut child, stdin, stdout) = fake_pi(PI_HOLDS_THE_HISTORY);
    let (runtime, conn) = attached_runtime("pi-history-late", None);
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let reader = reader_for(Arc::clone(&control), &stdin).with_restored_history();
    let feeder = feed_reader(reader, stdout, Arc::clone(&runtime));

    // The pass has asked: the fake holds the reply until the release.
    pull_until(
        &conn,
        |event| matches!(event, SessionEvent::AgentMessage { text, .. } if text == "history-requested"),
        Duration::from_secs(10),
    );
    write_child_stdin(&stdin, b"{\"type\":\"release_after_live\"}\n", "Pi").expect("release");

    // The marker is written after the reply: the reader has dispatched the
    // reply by the time it lands, so the awakened worker is on the clock.
    let mut all = pull_until(
        &conn,
        |event| matches!(event, SessionEvent::AgentMessage { text, .. } if text == "history-released"),
        Duration::from_secs(10),
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        for pending in conn.pull_events() {
            conn.event_sent(&pending);
            all.push(pending.envelope.event);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        restored_tasks(&all),
        vec![vec![item(
            "task-live",
            "From the live turn",
            AgentTaskStatus::Pending
        )]],
        "a history reply released after the live checklist must not replace it: {all:?}"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = feeder.join();
}
