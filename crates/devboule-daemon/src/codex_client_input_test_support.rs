//! Test support for the Codex input-request cards: the capturing sender, the
//! reader harness, the echo-child harness, and the question/elicitation
//! fixtures the topic test files share.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use devboule_protocol::SessionEvent;

use super::super::codex_input_requests::{codex_permission_frame, CodexPendingResponse};
use super::super::event_pull::ConnHandle;
use super::super::permission_broker::{PermissionBroker, PermissionSender};
use super::super::session_runtime::SessionRuntime;
use super::{catalog_from_response, empty_commands, CodexReader, CodexRequests};
use crate::codex_view::CodexView;

/// Fixtures: question and elicitation params in the providers' shapes.
/// One parked Codex answer shaped for the wire through the same entry point
/// the live sender uses, captured instead of written.
fn capturing_codex_sender(
    captured: &Arc<Mutex<Vec<serde_json::Value>>>,
    response_ids: &Arc<Mutex<HashMap<u64, CodexPendingResponse>>>,
) -> Arc<PermissionSender> {
    let captured = Arc::clone(captured);
    let response_ids = Arc::clone(response_ids);
    Arc::new(move |broker_id, result| {
        let pending = response_ids
            .lock()
            .map_err(|_| std::io::Error::other("Codex permission map lock poisoned"))?
            .remove(&broker_id)
            .ok_or_else(|| {
                std::io::Error::other("Codex permission response had no matching request")
            })?;
        captured
            .lock()
            .map_err(|_| std::io::Error::other("captured lock poisoned"))?
            .push(codex_permission_frame(&pending, &result));
        Ok(())
    })
}

fn question_reader(
    broker: Arc<PermissionBroker>,
    response_ids: Arc<Mutex<HashMap<u64, CodexPendingResponse>>>,
) -> CodexReader {
    CodexReader {
        commands: empty_commands(),
        available_commands: None,
        buffer: Vec::new(),
        discarding_oversized_line: false,
        deferred: Vec::new(),
        manifest: None,
        state: Arc::new(crate::codex_view::CodexState::new(
            "thread".to_string(),
            catalog_from_response(&serde_json::json!({
                "data": [{ "id": "model", "isDefault": true }]
            }))
            .expect("catalog"),
            "auto",
        )),
        view: CodexView::new(None),
        permission_broker: Arc::clone(&broker),
        response_ids,
        stdin: Arc::new(Mutex::new(None)),
        next_id: Arc::new(AtomicU64::new(1)),
        requests: Arc::new(CodexRequests::new()),
        compactions: crate::codex_compaction::CodexCompactions::default(),
    }
}

pub(super) type QuestionHarness = (
    Arc<PermissionBroker>,
    Arc<Mutex<Vec<serde_json::Value>>>,
    Arc<SessionRuntime>,
    Arc<ConnHandle>,
    CodexReader,
);

pub(super) fn question_harness() -> QuestionHarness {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let response_ids = Arc::new(Mutex::new(HashMap::new()));
    let broker = PermissionBroker::for_test(capturing_codex_sender(&captured, &response_ids));
    let runtime =
        SessionRuntime::for_acp("s.codex.questions".to_string(), None, Arc::clone(&broker));
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.codex.questions",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let reader = question_reader(Arc::clone(&broker), Arc::clone(&response_ids));
    (broker, captured, runtime, conn, reader)
}

pub(super) fn user_input_line(method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": "server-1",
        "method": method,
        "params": params,
    })
}

pub(super) fn fence_question_params() -> serde_json::Value {
    serde_json::json!({
        "itemId": "item-1",
        "threadId": "thread-1",
        "turnId": "turn-1",
        "questions": [{
            "id": "q1",
            "header": "Fence colour",
            "question": "Which colour should I paint the fence?",
            "options": [
                {"label": "Forest green (Recommended)", "description": "Blends in."},
                {"label": "Barn red", "description": "Classic."}
            ]
        }, {
            "id": "q2",
            "header": "Toppings",
            "question": "Which toppings?",
            "multiSelect": true,
            "options": [
                {"label": "Cheese"},
                {"label": "Pepperoni"}
            ]
        }]
    })
}

pub(super) fn single_question_params() -> serde_json::Value {
    // One question, one pick, one answer: the shape a lone option pick and
    // a text answer travel. (A bare option id on a multi-question card is
    // not an answer to anything — the card always submits the whole map.)
    serde_json::json!({
        "itemId": "item-1",
        "threadId": "thread-1",
        "turnId": "turn-1",
        "questions": [{
            "id": "q1",
            "header": "Fence colour",
            "question": "Which colour should I paint the fence?",
            "options": [
                {"label": "Forest green (Recommended)", "description": "Blends in."},
                {"label": "Barn red", "description": "Classic."}
            ]
        }]
    })
}

pub(super) fn asked_question(events: &[crate::session::PendingEvent]) -> SessionEvent {
    events
        .iter()
        .find_map(|event| match &event.envelope.event {
            SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "item-1" => {
                Some(event.envelope.event.clone())
            }
            _ => None,
        })
        .expect("question card")
}

pub(super) fn elicitation_line(message: &str) -> serde_json::Value {
    user_input_line(
        "mcpServer/elicitation/request",
        serde_json::json!({
            "threadId": "01a0d638-b8ec-71e0-8fbe-dcd38ecb4657",
            "turnId": "01a0d638-f6a7-7030-bbb5-0bd8df1299cf",
            "serverName": "devboule",
            "mode": "form",
            "message": message,
            "requestedSchema": {"type": "object", "properties": {}}
        }),
    )
}

pub(super) fn asked_elicitation(
    events: &[crate::session::PendingEvent],
    tool_call_id: &str,
) -> SessionEvent {
    events
        .iter()
        .find_map(|event| match &event.envelope.event {
            SessionEvent::PermissionRequest {
                tool_call_id: id, ..
            } if id == tool_call_id => Some(event.envelope.event.clone()),
            _ => None,
        })
        .expect("elicitation card")
}

/// A fake Codex child that echoes stdin to stdout, so a test reads the exact
/// bytes the client wrote — the same arrangement the steer tests use. The
/// sender is the real one and the card ids come from the production
/// generator; the doubles are the child process, the `for_test` broker
/// (no journal half), and the harness-built deps.
pub(super) struct EchoHarness {
    pub broker: Arc<PermissionBroker>,
    pub runtime: Arc<SessionRuntime>,
    pub conn: Arc<ConnHandle>,
    pub next_id: Arc<AtomicU64>,
    stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
    response_ids: Arc<Mutex<HashMap<u64, CodexPendingResponse>>>,
    stdout: Option<std::io::BufReader<std::process::ChildStdout>>,
    child: Option<std::process::Child>,
}

impl Drop for EchoHarness {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl EchoHarness {
    pub(super) fn deps(&self) -> super::super::codex_input_requests::CodexInputDeps {
        super::super::codex_input_requests::CodexInputDeps {
            stdin: Arc::clone(&self.stdin),
            response_ids: Arc::clone(&self.response_ids),
            next_id: Arc::clone(&self.next_id),
            permission_broker: Arc::clone(&self.broker),
        }
    }

    /// One frame the child echoed. Blocks up to the timeout, so a dropped
    /// write fails the test instead of hanging it.
    pub(super) fn read_frame(&mut self) -> serde_json::Value {
        use std::io::BufRead;
        let mut stdout = self.stdout.take().expect("stdout taken");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let read = stdout.read_line(&mut line);
            let _ = tx.send((read, line, stdout));
        });
        let (read, line, stdout) = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("answer frame written");
        self.stdout = Some(stdout);
        read.expect("frame read");
        serde_json::from_str(&line).expect("frame json")
    }
}

/// Caller must have gated on node first
/// (`test_support::external_program_skip_reason`).
pub(super) fn echo_harness(
    session: &str,
    journal: Option<Arc<crate::journal::Journal>>,
) -> EchoHarness {
    let mut child = std::process::Command::new("node")
        .args([
            "-e",
            "process.stdin.on('data', data => process.stdout.write(data))",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node echo child");
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let response_ids = Arc::new(Mutex::new(HashMap::new()));
    let broker =
        PermissionBroker::for_test(super::super::codex_input_requests::codex_permission_sender(
            Arc::clone(&stdin),
            Arc::clone(&response_ids),
        ));
    let runtime = SessionRuntime::for_acp(session.to_string(), journal, Arc::clone(&broker));
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    EchoHarness {
        broker,
        runtime,
        conn,
        next_id: Arc::new(AtomicU64::new(1)),
        stdin,
        response_ids,
        stdout: Some(stdout),
        child: Some(child),
    }
}
