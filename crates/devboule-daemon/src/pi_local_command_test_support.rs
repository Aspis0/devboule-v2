//! The fake-pi harness the local-slash-command cases share: a `node` child
//! that echoes every frame it is sent (so a case can assert on the bytes
//! written) and answers `get_state` with the state one case configures, a
//! reader on real pipes, and the frames measured on a live `pi --mode rpc`.

use super::{local_commands::SlashPromptFate, PermissionBroker, PiControl, PiReader, PiWriter};
use crate::session::event_pull::ConnHandle;
use crate::session::{acp_client::read_line_bounded, ReaderDispatch, SessionRuntime};
use devboule_protocol::{SessionEvent, SessionKind};
use std::collections::HashMap;
use std::io::Write;
use std::process::ChildStdin;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FAKE_PI_LOCAL: &str = r#"
const state = JSON.parse(process.env.PI_FAKE_STATE);
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    process.stdout.write(JSON.stringify({ received: line }) + "\n");
    let frame;
    try { frame = JSON.parse(line); } catch { continue; }
    if (frame.type === "get_state") {
      process.stdout.write(JSON.stringify({
        id: frame.id,
        type: "response",
        command: "get_state",
        success: true,
        data: state,
      }) + "\n");
    }
  }
});
"#;

pub(super) struct LocalPi {
    child: std::process::Child,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    fate: Arc<SlashPromptFate>,
    next_id: Arc<AtomicU64>,
    /// Every raw line the fake was sent, in order, from its echoes.
    sent: Vec<String>,
}

impl LocalPi {
    pub(super) fn spawn(state: &str) -> Self {
        let mut child = std::process::Command::new("node")
            .args(["-e", FAKE_PI_LOCAL])
            .env("PI_FAKE_STATE", state)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("node fake pi");
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
            fate: Arc::new(SlashPromptFate::new()),
            next_id: Arc::new(AtomicU64::new(1)),
            sent: Vec::new(),
        }
    }

    /// The prompt writer the send path uses, against this fake's stdin.
    pub(super) fn writer(&self) -> PiWriter {
        PiWriter {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            pending: Vec::new(),
            fate: Arc::clone(&self.fate),
            arbiter: super::pi_turn_arbiter::TurnArbiter::bare(),
        }
    }

    pub(super) fn reader(&self) -> PiReader {
        PiReader::new(
            Vec::new(),
            SessionEvent::SessionManifest {
                provider_id: Some("pi".to_string()),
                current_model_id: None,
                models: Vec::new(),
                modes: None,
            },
            PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::clone(&self.next_id),
            Arc::new(PiControl::new(
                Arc::clone(&self.stdin),
                Arc::clone(&self.next_id),
            )),
            Arc::clone(&self.stdin),
            Arc::new(AtomicBool::new(true)),
        )
        .with_prompt_fate(Arc::clone(&self.fate))
    }

    /// Read exactly `lines` lines off the fake: its echoes are recorded,
    /// its protocol responses are fed to the reader, which is what acts on
    /// them. A missing line fails the case on a bound instead of hanging;
    /// five seconds is a hundredfold what the echo needs and keeps a red
    /// from costing half a minute.
    pub(super) fn drain(
        &mut self,
        reader: &mut PiReader,
        runtime: &Arc<SessionRuntime>,
        lines: usize,
    ) {
        for _ in 0..lines {
            let line = read_line_bounded(
                &mut self.stdout,
                Instant::now() + Duration::from_secs(5),
                Duration::from_secs(5),
            )
            .expect("the fake answers every frame it is sent");
            let frame: serde_json::Value = serde_json::from_str(&line).expect("fake frame");
            if let Some(received) = frame.get("received").and_then(serde_json::Value::as_str) {
                self.sent.push(received.to_string());
            }
            if frame.get("type").and_then(serde_json::Value::as_str) == Some("response") {
                reader
                    .feed(line.as_bytes(), runtime)
                    .expect("the fake's response dispatches");
            }
        }
    }

    pub(super) fn sent_frames(&self) -> Vec<serde_json::Value> {
        self.sent
            .iter()
            .map(|line| serde_json::from_str(line).expect("frame the fake received"))
            .collect()
    }
}

impl Drop for LocalPi {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A prompt the way the send path writes one: text, then the flush that
/// puts the frame on the wire.
pub(super) fn write_prompt(pi: &LocalPi, text: &str) {
    let mut writer = pi.writer();
    writer
        .write_all(text.as_bytes())
        .and_then(|_| writer.flush())
        .expect("prompt write");
}

/// A live session mid-turn, the way production is when the write goes out:
/// the roster row says `working`, and the attach seam lets a case pull what
/// the reader published.
pub(super) fn runtime_mid_turn() -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.set_agent_kind(SessionKind::Pi);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.pi.local",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    runtime.begin_turn();
    (runtime, conn)
}

/// The extension notify of the measured `/goal-list` run.
pub(super) fn goal_list_notify() -> serde_json::Value {
    notify_of("No open goals. Use /goal <objective> or /sisyphus <objective> to start immediately.")
}

/// Any extension notify: the frame shape the measurement carries.
pub(super) fn notify_of(message: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "extension_ui_request",
        "id": "f90a0c15-1dc6-4bba-a4d9-f756a90cccf4",
        "method": "notify",
        "message": message,
        "notifyType": "info",
    })
}

/// The measured prompt acknowledgement.
pub(super) fn prompt_response(id: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "type": "response", "command": "prompt", "success": true })
}

pub(super) fn agent_start() -> serde_json::Value {
    serde_json::json!({ "type": "agent_start" })
}

/// The recorded `turn_end` of `pi_view.rs`'s own fixture.
pub(super) fn recorded_turn_end() -> serde_json::Value {
    serde_json::from_str(
        r#"{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"OK"}],"api":"openai-completions","provider":"openrouter","model":"z-ai/glm-5.3-flash","usage":{"input":25848,"output":3,"cacheRead":0,"cacheWrite":0,"reasoning":0,"totalTokens":25851},"stopReason":"stop"},"toolResults":[]}"#,
    )
    .expect("recorded turn_end frame")
}

pub(super) fn feed(reader: &mut PiReader, runtime: &Arc<SessionRuntime>, frame: serde_json::Value) {
    let line = format!("{frame}\n");
    reader
        .feed(line.as_bytes(), runtime)
        .expect("dispatch frame");
}

/// The SessionNotices a pull carries, texts in order.
pub(super) fn notices(conn: &ConnHandle) -> Vec<String> {
    conn.pull_events()
        .into_iter()
        .filter_map(|pending| match pending.envelope.event {
            SessionEvent::SessionNotice { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

/// The AgentFinished stop reasons a pull carries, in order.
pub(super) fn finishes(conn: &ConnHandle) -> Vec<String> {
    conn.pull_events()
        .into_iter()
        .filter_map(|pending| match pending.envelope.event {
            SessionEvent::AgentFinished { stop_reason, .. } => Some(stop_reason),
            _ => None,
        })
        .collect()
}

/// What pi answers when nothing streams and nothing is queued: the state
/// the measured `get_state` for a locally handled command reported.
pub(super) fn idle_state() -> &'static str {
    r#"{"isStreaming":false,"isCompacting":false,"pendingMessageCount":0,"messageCount":0}"#
}

/// What pi answers while a turn streams: the race state.
pub(super) fn busy_state() -> &'static str {
    r#"{"isStreaming":true,"isCompacting":false,"pendingMessageCount":0,"messageCount":0}"#
}
