//! Shared harness for the pi turn-watchdog tests: the watch is built the
//! way spawn builds it and reaches the reader only through the production
//! wiring; a turn is armed by writing a prompt through `PiWriter::flush`,
//! and time is faked with the watch's deterministic hooks, never slept out.

use std::collections::HashMap;
use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::{SessionEvent, SessionKind};
use serde_json::Value;

use super::super::local_commands::SlashPromptFate;
use super::super::{PiControl, PiKiller, PiReader, PiWriter};
use crate::session::acp_client::read_line_bounded;
use crate::session::event_pull::ConnHandle;
use crate::session::permission_broker::PermissionBroker;
use crate::session::turn_watch::TurnWatch;
use crate::session::{ReaderDispatch, SessionRuntime};

/// The echo fake's share of the stdin protocol: every framed line back on
/// stdout. The framing loop is the shared preamble; this only handles
/// whole lines, newline kept.
const ECHO_PI_FRAMED: &str = r#"
function onFramedLine(line) {
  process.stdout.write(line);
}
"#;

pub(super) struct PiWatchHarness {
    child: Arc<Mutex<std::process::Child>>,
    pub(super) stdout: std::io::BufReader<std::process::ChildStdout>,
    pub(super) watch: Arc<TurnWatch>,
    pub(super) reader: PiReader,
    pub(super) writer: PiWriter,
    /// The killer the interrupt road runs through, wired to the same stdin
    /// and broker the watch holds.
    pub(super) killer: PiKiller,
}

impl Drop for PiWatchHarness {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The production construction, shrunk to one harness: the watch gets the
/// same four handles spawn gives it — the control stdin its abort rides,
/// the id counter, the broker whose cards hold the clock — and reaches the
/// reader only through the same builder spawn calls.
pub(super) fn harness(broker: &Arc<PermissionBroker>) -> PiWatchHarness {
    let script = format!(
        "{}{}",
        crate::test_support::NODE_FRAMED_STDIN,
        ECHO_PI_FRAMED
    );
    let child = Command::new("node")
        .arg("-e")
        .arg(script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node echo pi");
    let child = Arc::new(Mutex::new(child));
    let stdin = Arc::new(Mutex::new(Some(
        child.lock().expect("child").stdin.take().expect("stdin"),
    )));
    let stdout =
        std::io::BufReader::new(child.lock().expect("child").stdout.take().expect("stdout"));
    let next_id = Arc::new(AtomicU64::new(1));
    let cancelled = Arc::new(AtomicBool::new(false));
    let owed_late_end = Arc::new(super::OwedTurnEnd::default());
    // A failure-only bound: long enough that the live tick thread can
    // never fire mid-test (every expiry is driven by the watch's own
    // deterministic hooks), short enough that a wedged watch is a failed
    // test, not a hung one.
    let watch = super::pi_turn_watch(
        Duration::from_secs(30),
        Arc::clone(&stdin),
        Arc::clone(&next_id),
        Arc::clone(broker),
        Arc::clone(&cancelled),
        Arc::clone(&owed_late_end),
        None,
    );
    let arbiter = Arc::new(super::super::pi_turn_arbiter::TurnArbiter::new(
        Some(Arc::clone(&watch)),
        owed_late_end,
    ));
    let fate = Arc::new(SlashPromptFate::new());
    let control = Arc::new(PiControl::new(Arc::clone(&stdin), Arc::clone(&next_id)));
    let reader = PiReader::new(
        Vec::new(),
        SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: None,
            current_model_provider_id: None,
        },
        Arc::clone(broker),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::clone(&next_id),
        control,
        Arc::clone(&stdin),
        Arc::new(AtomicBool::new(true)),
    )
    .with_prompt_fate(Arc::clone(&fate))
    .with_turn_arbiter(Arc::clone(&arbiter));
    let writer = PiWriter {
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        pending: Vec::new(),
        fate,
        arbiter: Arc::clone(&arbiter),
    };
    let killer = PiKiller {
        process: Arc::clone(&child),
        stdin,
        next_id,
        permission_broker: Arc::clone(broker),
        arbiter,
        cancelled,
        extension_path: crate::test_dirs::test_temp_dir("devboule-pi-watch-ext")
            .join("extension.ts"),
        bridge_path: None,
    };
    PiWatchHarness {
        child,
        stdout,
        watch,
        reader,
        writer,
        killer,
    }
}

/// A live session mid-turn on the attach seam, the way production is when a
/// prompt goes out.
pub(super) fn attached(broker: &Arc<PermissionBroker>) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = SessionRuntime::for_acp("s.pi.watch".to_string(), None, Arc::clone(broker));
    runtime.set_agent_kind(SessionKind::Pi);
    let conn = crate::test_support::attach_and_track(&runtime, "s.pi.watch");
    (runtime, conn)
}

/// A prompt the way the send path writes one: the text, then the flush that
/// puts the frame on the wire. The echo is consumed, so the next echo a
/// test reads is the next frame the daemon wrote.
pub(super) fn deliver(harness: &mut PiWatchHarness, text: &str) {
    harness
        .writer
        .write_all(text.as_bytes())
        .expect("buffer the prompt");
    harness.writer.flush().expect("the prompt goes to the wire");
    next_echo(harness);
}

pub(super) fn feed_line(harness: &mut PiWatchHarness, runtime: &Arc<SessionRuntime>, value: Value) {
    let line = format!("{value}\n");
    harness
        .reader
        .feed(line.as_bytes(), runtime)
        .expect("feed the frame");
}

/// The next frame on the wire, read off the echo with a bound instead of a
/// hang. `None` — nothing more was written inside the bound.
pub(super) fn try_next_echo(harness: &mut PiWatchHarness, bound: Duration) -> Option<Value> {
    let line = read_line_bounded(
        &mut harness.stdout,
        Instant::now() + bound,
        Duration::from_secs(5),
    )
    .ok()?;
    Some(serde_json::from_str(&line).expect("echoed frame"))
}

fn next_echo(harness: &mut PiWatchHarness) -> Value {
    try_next_echo(harness, Duration::from_secs(5))
        .expect("the echo pi answers every frame it is sent")
}

/// One benign inbound frame: binds the runtime to the watch (as `feed` in
/// production does on the priming read) and resets activity.
pub(super) fn touch() -> Value {
    serde_json::json!({"type": "message_update"})
}

/// The confirm card one tool call waits on: a person's decision.
pub(super) fn confirm_card(request_id: &str) -> Value {
    serde_json::json!({
        "type": "extension_ui_request",
        "id": request_id,
        "method": "confirm",
        "title": {"title": "bash", "message": "Allow the command?"},
    })
}

pub(super) fn toolcall_start(id: &str) -> Value {
    serde_json::json!({
        "type": "message_update",
        "assistantMessageEvent": {"type": "toolcall_start", "id": id, "toolName": "bash"},
    })
}

pub(super) fn tool_execution_end(id: &str) -> Value {
    serde_json::json!({"type": "tool_execution_end", "toolCallId": id, "isError": false})
}

/// The `PermissionBroker::for_test` every harness test pairs with its
/// runtime.
pub(in crate::session::pi_client) fn broker() -> Arc<PermissionBroker> {
    PermissionBroker::for_test(Arc::new(|_, _| Ok(())))
}

/// A `turn_end` carrying the stop reason the test names — pi's aborted
/// answer to an abort reads `"aborted"`; a turn that ran reads `"stop"`.
pub(super) fn turn_end_with(stop_reason: &str) -> Value {
    serde_json::from_str(&format!(
        r#"{{"type":"turn_end","message":{{"role":"assistant","content":[],"model":"m","usage":{{"totalTokens":1}},"stopReason":"{stop_reason}"}},"toolResults":[]}}"#
    ))
    .expect("turn_end frame")
}

/// A prompt refusal on the wire: pi's preflight `success:false`.
pub(super) fn rejects(id: &str, error: &str) -> Value {
    serde_json::json!({
        "type": "response",
        "id": id,
        "command": "prompt",
        "success": false,
        "error": error,
    })
}

/// The `AgentFinished` stop reasons a pull carries, in order.
pub(super) fn finishes_of(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentFinished { stop_reason, .. } => Some(stop_reason.clone()),
            _ => None,
        })
        .collect()
}

/// A live journal runtime on the attach seam, so a test can assert the
/// transcript live AND as a restart replays it.
pub(super) fn attached_journal(
    journal: &Arc<crate::journal::Journal>,
    session_id: &str,
) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(journal)),
    ));
    runtime.set_agent_kind(SessionKind::Pi);
    let conn = crate::test_support::attach_and_track(&runtime, session_id);
    (runtime, conn)
}

/// The abort frame the expiry writes, read back from the echo.
pub(super) fn abort_frame(harness: &mut PiWatchHarness) -> Value {
    next_echo(harness)
}

pub(super) fn drain(conn: &ConnHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            return events;
        }
        for event in &batch {
            conn.event_sent(event);
        }
        events.extend(batch.into_iter().map(|pending| pending.envelope.event));
    }
}

pub(super) fn is_eof_error(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::AgentError { message } if message.contains("output ended while a turn")
    )
}

/// Close the harness down the way the reader loop does: `finish` once for
/// the road under test, then the harness drop kills the child.
pub(super) fn finish(harness: &mut PiWatchHarness, runtime: &Arc<SessionRuntime>) {
    harness.reader.finish(runtime);
}
