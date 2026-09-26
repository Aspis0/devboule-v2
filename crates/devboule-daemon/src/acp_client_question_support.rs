//! Test support for the grok question cards: the echo harness that answers
//! through the production sender, and the enveloped/unenveloped fixtures
//! the topic test files share.

use std::collections::HashSet;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use devboule_protocol::SessionEvent;

use super::super::event_pull::ConnHandle;
use super::super::permission_broker::{permission_path, PermissionBroker};
use super::super::session_runtime::SessionRuntime;
use super::AcpReader;
use crate::journal::Journal;

pub(super) const SESSION: &str = "s.acp.grok";
pub(super) const FENCE: &str = "Which colour should I paint the fence?";
pub(super) const TOPPINGS: &str = "Which toppings?";

static ECHO_SEQ: AtomicU64 = AtomicU64::new(1);

pub(super) fn node_gated() -> bool {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return true;
    }
    false
}

/// A fake ACP child that echoes stdin to stdout, so a test reads the exact
/// bytes the production sender wrote. The broker is the transport's own —
/// require-journal and all — with a throwaway journal behind it, so answers
/// take the recorded road exactly as in production.
pub(super) struct EchoHarness {
    pub(super) broker: Arc<PermissionBroker>,
    pub(super) runtime: Arc<SessionRuntime>,
    pub(super) conn: Arc<ConnHandle>,
    pub(super) reader: AcpReader,
    stdout: Option<std::io::BufReader<std::process::ChildStdout>>,
    child: Option<std::process::Child>,
    journal: Arc<Journal>,
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
    pub(super) fn dispatch(&self, frame: &serde_json::Value) {
        self.reader.dispatch_value(frame, &self.runtime);
        // The conn replays durable rows before it serves the live queue,
        // and the envelope row lands on a background thread: flush so one
        // pull deterministically sees replay and live together.
        self.journal.flush().expect("journal flush");
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

/// Caller must have gated on node first (`node_gated`).
pub(super) fn echo_harness() -> EchoHarness {
    let mut child = std::process::Command::new("node")
        .args([
            "-e",
            "process.stdin.on('data', data => process.stdout.write(data))",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node echo child");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let (transport, broker, _) = super::AcpReader::test_transport(stdin);
    let seq = ECHO_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let journal = Arc::new(
        Journal::open(&permission_path(&format!("acp-grok-echo-{seq}"))).expect("journal"),
    );
    journal
        .upsert_blocking(crate::journal::SessionRecord {
            id: SESSION.to_string(),
            owner: "owner".to_string(),
            workspace_id: None,
            cwd: None,
            kind: devboule_protocol::SessionKind::Acp,
            provider: None,
            title: "grok echo".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
            generation: 1,
            status: crate::journal::PersistStatus::Live,
            exit_code: None,
            closed: false,
            last_seq: 0,
            degraded: false,
            dropped_frames: 0,
            dropped_bytes: 0,
            payload_bytes: 0,
            trimmed_bytes: 0,
            reaped: false,
            peer_session_id: None,
            disowned_peer_session_id: None,
            origin: devboule_protocol::SessionOrigin::local(),
            display_name: None,
            created_by: None,
            profile_id: None,
            context_id: None,
            unattended_state: devboule_protocol::UnattendedState::Unknown,
            labels: Default::default(),
            overlay: None,
            depth: None,
        })
        .expect("session row");
    let runtime = SessionRuntime::for_acp(
        SESSION.to_string(),
        Some(Arc::clone(&journal)),
        Arc::clone(&broker),
    );
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        SESSION,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let dir = crate::test_dirs::test_temp_dir("devboule-acp-echo-host");
    let reader = AcpReader::for_test_with_transport(
        Arc::new(Mutex::new(HashSet::new())),
        SESSION.to_string(),
        Arc::clone(&broker),
        super::super::acp_host::AcpHost::new(dir.clone(), dir),
        transport,
    );
    EchoHarness {
        broker,
        runtime,
        conn,
        reader,
        stdout: Some(stdout),
        child: Some(child),
        journal,
    }
}

pub(super) fn live_turn(reader: &AcpReader) {
    reader.turn.start_prompt(7);
}

pub(super) fn fence_params() -> serde_json::Value {
    serde_json::json!({
        "sessionId": SESSION,
        "toolCallId": "call-fence-0",
        "questions": [{
            "question": FENCE,
            "options": [
                {"label": "Forest green (Recommended)", "description": "Blends in."},
                {"label": "Barn red", "description": "Classic red."},
                {"label": "Weathered grey", "description": "Aged look."}
            ],
            "multiSelect": null
        }],
        "mode": "default"
    })
}

/// The live frame shape from our own journal, as grok 1.0.25/26 sent it.
pub(super) fn enveloped(id: u64) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "_x.ai/ask_user_question",
        "params": fence_params(),
    })
}

/// The 1.0.40 shape: the same fields directly under the method.
pub(super) fn unenveloped(id: u64) -> serde_json::Value {
    let mut frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "_x.ai/ask_user_question",
    });
    for (key, value) in fence_params().as_object().expect("params object").iter() {
        if key.as_str() != "toolCallId" {
            frame[key.as_str()] = value.clone();
        }
    }
    frame["toolCallId"] = serde_json::json!("call-bare-5");
    frame
}

pub(super) fn asked(events: &[crate::session::PendingEvent], tool_call_id: &str) -> SessionEvent {
    events
        .iter()
        .find_map(|event| match &event.envelope.event {
            SessionEvent::PermissionRequest {
                tool_call_id: id, ..
            } if id == tool_call_id => Some(event.envelope.event.clone()),
            _ => None,
        })
        .expect("question card")
}

pub(super) fn has_notice(events: &[crate::session::PendingEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event.envelope.event,
            SessionEvent::SessionNotice { .. } | SessionEvent::AgentError { .. }
        )
    })
}
