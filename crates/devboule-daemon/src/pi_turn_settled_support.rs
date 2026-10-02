//! The live pi run the settled-ending tests drive: its harness, the frames a
//! failed or answered attempt sends, and the readers of the rows it leaves.

use super::super::local_command_test_support::agent_start;
use super::test_support::{attached_journal, broker, deliver, drain, feed_line, harness};
use crate::journal::{new_session_record, Journal};
use crate::session::SessionRuntime;
use devboule_protocol::{SessionEvent, SessionKind};
use std::path::PathBuf;
use std::sync::Arc;

/// One live pi run on the attach seam, its prompt out and its watch armed,
/// plus the handles the run's replay afterwards needs.
pub(super) struct LiveRun {
    pub(super) harness: super::test_support::PiWatchHarness,
    pub(super) runtime: Arc<SessionRuntime>,
    pub(super) conn: Arc<crate::session::event_pull::ConnHandle>,
    dir: PathBuf,
    path: PathBuf,
    journal: Arc<Journal>,
    session_id: String,
}

impl LiveRun {
    /// The shared preamble: a prompt goes out and pi begins the turn.
    pub(super) fn start(session_id: &str) -> LiveRun {
        let (dir, path) = crate::journal::tmp_journal();
        let journal = Arc::new(Journal::open(&path).expect("open"));
        journal
            .create_session(new_session_record(
                session_id,
                "owner",
                None,
                SessionKind::Pi,
                "pi run",
            ))
            .expect("birth");
        let broker = broker();
        let mut harness = harness(&broker);
        let (runtime, conn) = attached_journal(&journal, session_id);
        runtime.begin_turn();
        deliver(&mut harness, "hello");
        feed_line(
            &mut harness,
            &runtime,
            serde_json::json!({"type":"response","id":"p-1","command":"prompt","success":true}),
        );
        feed_line(&mut harness, &runtime, agent_start());
        let _ = drain(&conn);
        LiveRun {
            harness,
            runtime,
            conn,
            dir,
            path,
            journal,
            session_id: session_id.to_string(),
        }
    }

    /// Close the run and hand back what reopening the journal needs.
    pub(super) fn shutdown(self) -> (PathBuf, String, PathBuf) {
        drop(self.harness);
        drop(self.runtime);
        drop(self.conn);
        self.journal.flush().expect("flush");
        self.journal.shutdown();
        (self.dir, self.session_id, self.path)
    }
}

/// The failed assistant message one attempt ends with.
fn failed_message() -> serde_json::Value {
    serde_json::json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "the answer never came"}],
        "provider": "openai-responses",
        "model": "gpt-5.6-terra",
        "errorMessage": "Request timed out.",
        "stopReason": "error",
    })
}

/// The attempt's own ending.
pub(super) fn failed_turn_end() -> serde_json::Value {
    serde_json::json!({
        "type": "turn_end",
        "message": failed_message(),
        "toolResults": [],
    })
}

/// The run's ending for that attempt: `willRetry` is pi's own stamp.
pub(super) fn failed_agent_end(will_retry: bool) -> serde_json::Value {
    serde_json::json!({
        "type": "agent_end",
        "messages": [failed_message()],
        "willRetry": will_retry,
    })
}

/// The plain answer a continuation produced: not a failure, so its ending
/// is what clears one.
fn answered_message() -> serde_json::Value {
    serde_json::json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "the answer"}],
        "stopReason": "stop",
    })
}

pub(super) fn answered_turn_end() -> serde_json::Value {
    serde_json::json!({
        "type": "turn_end",
        "message": answered_message(),
        "toolResults": [],
    })
}

pub(super) fn answered_agent_end() -> serde_json::Value {
    serde_json::json!({
        "type": "agent_end",
        "messages": [answered_message()],
        "willRetry": false,
    })
}

/// pi's own end of the run: bare, once per prompt, after every retry or
/// compaction continuation has run out.
pub(super) fn agent_settled() -> serde_json::Value {
    serde_json::json!({"type": "agent_settled"})
}

/// The failed attempt every continuation test starts from; the error row
/// is still unwritten when this returns.
pub(super) fn failed_attempt(flow: &mut LiveRun) {
    feed_line(&mut flow.harness, &flow.runtime, failed_turn_end());
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(false));
    let attempt = drain(&flow.conn);
    assert_eq!(
        errors_of(&attempt),
        Vec::<String>::new(),
        "the attempt's failure waits for the run's own ending: {attempt:?}"
    );
}

/// The error rows a pull carries, in order.
pub(super) fn errors_of(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentError { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

/// Close the run and read its journal back the way a restart does.
pub(super) fn replay_errors(flow: LiveRun) -> Vec<String> {
    let (dir, session_id, path) = flow.shutdown();
    let events = Journal::open(&path)
        .expect("reopen")
        .replay(&session_id)
        .expect("replay")
        .events;
    let _ = std::fs::remove_dir_all(&dir);
    errors_of(&events)
}
