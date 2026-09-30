//! Shared harness for the Claude turn-watchdog tests: construction goes
//! through the production `with_mode_gate` (which binds the broker), turns
//! start in `ClaudeWriter::flush`, the runtime binds on `feed`, and each
//! test ends through `finish`. Time is faked with the watch's deterministic
//! hooks, never slept out.

use super::*;
use crate::session::{ConnHandle, PendingEvent};
use std::io::{BufReader, Write};
use std::process::ChildStdout;
use std::time::Duration;

pub(super) struct WatchHarness {
    pub(super) child: Option<std::process::Child>,
    pub(super) stdout: Option<BufReader<ChildStdout>>,
    pub(super) watch: Arc<TurnWatch>,
    pub(super) reader: ClaudeReader,
    pub(super) writer: ClaudeWriter,
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
        events.extend(
            batch
                .into_iter()
                .map(|pending: PendingEvent| pending.envelope.event),
        );
    }
}

pub(super) fn attached(broker: &Arc<PermissionBroker>) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = SessionRuntime::for_acp("s.claude.watch".to_string(), None, Arc::clone(broker));
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.claude.watch",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    (runtime, conn)
}

fn build(
    broker: &Arc<PermissionBroker>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    stdout: Option<BufReader<ChildStdout>>,
    child: Option<std::process::Child>,
    gate: ClaudeModeGateRef,
    next_id: Arc<AtomicU64>,
    cancelled: bool,
) -> WatchHarness {
    let abort_gate: ClaudeAbortGateRef = Arc::new(crate::claude_abort::ClaudeAbortGate::default());
    let watch = claude_turn_watch(
        Duration::from_millis(300),
        Arc::clone(&stdin),
        Arc::clone(&next_id),
        Arc::clone(broker),
        Arc::clone(&abort_gate),
        Arc::new(AtomicBool::new(cancelled)),
    );
    let wiring = ClaudeModeGateWiring::new(
        Arc::clone(&stdin),
        Arc::clone(&gate),
        Arc::clone(&abort_gate),
    );
    let reader = ClaudeReader::with_mode_gate(
        ClaudeView::new(None),
        Arc::clone(broker),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::clone(&next_id),
        wiring,
        Some(Arc::clone(&watch)),
    );
    let writer = ClaudeWriter {
        stdin: Arc::clone(&stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&gate)),
        abort_gate: Arc::clone(&abort_gate),
        watch: Some(Arc::clone(&watch)),
    };
    WatchHarness {
        child,
        stdout,
        watch,
        reader,
        writer,
    }
}

/// The production construction path with no child: stdin is gone, so the
/// mode gate never releases and every flush queues — which still arms the
/// watch through the same `flush` lines the S1 mutation deletes. Tests that
/// need a delivered prompt use `harness_echo` instead.
pub(super) fn harness(broker: &Arc<PermissionBroker>, cancelled: bool) -> WatchHarness {
    let stdin = Arc::new(Mutex::new(None));
    let gate = Arc::new(Mutex::new(ClaudeModeGate {
        state: ClaudeModeGateState::AwaitingResponse {
            request_id: "u7a-watch".to_string(),
        },
        pending_frames: Vec::new(),
    }));
    build(
        broker,
        stdin,
        None,
        None,
        gate,
        Arc::new(AtomicU64::new(1)),
        cancelled,
    )
}

/// The same construction against a live echo child, so a released gate
/// delivers prompts the abort gate counts. Callers skip cleanly without
/// node before calling this.
pub(super) fn harness_echo(broker: &Arc<PermissionBroker>) -> WatchHarness {
    let mut child = std::process::Command::new("node")
        .args([
            "-e",
            "process.stdin.on('data', data => process.stdout.write(data))",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node is required for the delivered-prompt tests");
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let next_id = Arc::new(AtomicU64::new(1));
    let gate = start_initial_mode(&stdin, &next_id, "default".to_string()).expect("mode request");
    build(
        broker,
        stdin,
        Some(stdout),
        Some(child),
        gate,
        next_id,
        false,
    )
}

fn read_line(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut buf = Vec::new();
    use std::io::BufRead;
    stdout.read_until(b'\n', &mut buf).expect("echoed line");
    serde_json::from_str::<Value>(String::from_utf8_lossy(&buf).trim_end()).expect("echoed json")
}

pub(super) fn feed_line(reader: &mut ClaudeReader, runtime: &Arc<SessionRuntime>, value: Value) {
    reader
        .feed(format!("{value}\n").as_bytes(), runtime)
        .expect("feed");
}

/// One benign inbound line through `feed`: binds the runtime to the watch
/// and resets activity, so tests that expire without any other traffic do
/// not trip the unbound-watch path. A convenience for tests, not a mirror
/// of production: the live reader loop binds on its priming feed before the
/// first read.
pub(super) fn touch(harness: &mut WatchHarness, runtime: &Arc<SessionRuntime>) {
    feed_line(
        &mut harness.reader,
        runtime,
        serde_json::json!({
            "type": "assistant",
            "message": {
                "id": "touch-message-1",
                "model": "stub-model",
                "role": "assistant",
                "content": [{"type": "text", "text": "working"}],
            },
        }),
    );
}

/// Answer the echo harness gate the way the CLI does, so prompts deliver
/// instead of queueing: the replacement a stale result is measured against
/// must have reached the child.
pub(super) fn release_gate(harness: &mut WatchHarness, runtime: &Arc<SessionRuntime>) {
    let stdout = harness.stdout.as_mut().expect("echo harness");
    let request = read_line(stdout);
    feed_line(
        &mut harness.reader,
        runtime,
        serde_json::json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request["request_id"],
                "response": {"mode": "default"},
            },
        }),
    );
}

pub(super) fn deliver(harness: &mut WatchHarness, text: &str) {
    harness.writer.write_all(text.as_bytes()).expect("buffer");
    harness.writer.flush().expect("queue the prompt");
    if let Some(stdout) = harness.stdout.as_mut() {
        let _ = read_line(stdout);
    }
}

pub(super) fn finish(harness: &mut WatchHarness, runtime: &Arc<SessionRuntime>) {
    harness.reader.finish(runtime);
    if let Some(child) = harness.child.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

pub(super) fn tool_frame(parent: Option<&str>, id: &str) -> Value {
    let mut frame = serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "watch-message-1",
            "model": "stub-model",
            "role": "assistant",
            "content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {}}],
        },
    });
    if let Some(parent) = parent {
        frame["parent_tool_use_id"] = serde_json::Value::String(parent.to_string());
    }
    frame
}

pub(super) fn is_watchdog_error(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::AgentError { message }
            if message.contains("produced no output") && message.contains("the run was ended")
    )
}

pub(super) fn is_error_finish(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "error"
    )
}

pub(super) fn is_any_finish(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::AgentFinished { .. })
}
