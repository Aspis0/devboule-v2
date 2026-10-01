//! Shared harness for the Claude unread-steer-latch tests: the production
//! reader via `with_mode_gate`, the controls map and frame writer it would
//! share with the broker's sender at spawn, and an attached connection to
//! observe published cards. No child for the reader tests: the deny travels
//! through the sender closure, so a capturing writer takes its place. The
//! steerer test echoes through a node child, like the watchdog's
//! delivered-prompt tests.

use super::*;
use crate::session::ConnHandle;
use crate::session::PendingEvent;

/// The process and disk side of a harness, reaped on Drop: a panicking test
/// must not leak a node child that holds the inherited stdout (the signature
/// of a wedged cargo invocation) nor a temp journal directory.
pub(super) struct HarnessGuard {
    child: Option<std::process::Child>,
    journal: Arc<crate::journal::Journal>,
    dir: std::path::PathBuf,
}

impl Drop for HarnessGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.journal.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(super) struct LatchHarness {
    pub(super) reader: ClaudeReader,
    pub(super) latch: Arc<ClaudeSteerLatch>,
    pub(super) broker: Arc<PermissionBroker>,
    pub(super) runtime: Arc<SessionRuntime>,
    pub(super) conn: Arc<ConnHandle>,
    /// The reader's own stdin and gate: writes and inspections through the
    /// harness land on the objects the reader was built with.
    pub(super) stdin: Arc<Mutex<Option<ChildStdin>>>,
    pub(super) mode_gate: ClaudeModeGateRef,
    /// The reader's own control-response map: a switcher under test must
    /// register its waits here, or the reader drops its answers.
    pub(super) mode_responses: ClaudeModeResponses,
    /// What the broker answered each Claude card with, in answer order.
    pub(super) denied: Arc<Mutex<Vec<Value>>>,
    /// The frames written back to the CLI (captured, not piped).
    pub(super) written: Arc<Mutex<Vec<String>>>,
    /// The child, journal and temp dir, reaped on Drop.
    pub(super) guard: HarnessGuard,
}

impl LatchHarness {
    /// Whether the harness journal has marked itself degraded: a deny whose
    /// decision could not be recorded loses its sentence, and the assertion
    /// that reads the sentence should say so.
    pub(super) fn journal_is_degraded(&self) -> bool {
        self.guard.journal.is_session_degraded("s.claude.latch")
    }

    /// The echo child, for the tests that read what it echoes back.
    pub(super) fn child_as_mut(&mut self) -> &mut std::process::Child {
        self.guard.child.as_mut().expect("echo child")
    }

    /// The journal's replay of this session: what a restart would rebuild.
    pub(super) fn replay_events(&self) -> Vec<SessionEvent> {
        self.guard
            .journal
            .replay("s.claude.latch")
            .expect("replay")
            .events
    }
}

/// A throwaway journal so the broker's completion records like production,
/// with its session row born the way a create writes it: transcript appends
/// update that row, and the replay road reads through it.
fn test_journal() -> (Arc<crate::journal::Journal>, std::path::PathBuf) {
    let dir = crate::test_dirs::test_temp_dir("devboule-claude-latch");
    let journal =
        Arc::new(crate::journal::Journal::open(&dir.join("journal.db")).expect("journal"));
    journal
        .create_session(crate::journal::new_session_record(
            "s.claude.latch",
            "test",
            None,
            devboule_protocol::SessionKind::Claude,
            "latch",
        ))
        .expect("session row");
    (journal, dir)
}

/// The capture halves shared between the broker's sender and the tests'
/// assertions, plus the controls map the frame writer keys answers by.
struct CapturedParts {
    controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
    written: Arc<Mutex<Vec<String>>>,
    sender: Arc<PermissionSender>,
    denied: Arc<Mutex<Vec<Value>>>,
}

fn shared_parts() -> CapturedParts {
    let controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let written: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let writer: ClaudeFrameWriter = {
        let written = Arc::clone(&written);
        Arc::new(move |bytes: &[u8]| {
            written
                .lock()
                .map_err(|_| io::Error::other("capture lock poisoned"))?
                .push(String::from_utf8_lossy(bytes).into_owned());
            Ok(())
        })
    };
    let frame_sender = claude_permission_sender_with_writer(Arc::clone(&controls), writer);
    let denied: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let denied_for_sender = Arc::clone(&denied);
    let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
        if let Ok(mut denied) = denied_for_sender.lock() {
            denied.push(result.clone());
        }
        frame_sender(id, result)
    });
    CapturedParts {
        controls,
        written,
        sender,
        denied,
    }
}

fn attach(
    broker: &Arc<PermissionBroker>,
    journal: Arc<crate::journal::Journal>,
) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = SessionRuntime::for_acp(
        "s.claude.latch".to_string(),
        Some(journal),
        Arc::clone(broker),
    );
    let conn = crate::test_support::attach_and_track(&runtime, "s.claude.latch");
    (runtime, conn)
}

/// The harness pieces, unassembled: for the interleave tests that must move
/// the reader to another thread while holding on to the rest. `guard` still
/// reaps everything on Drop.
pub(super) struct LatchParts {
    pub(super) reader: ClaudeReader,
    pub(super) latch: Arc<ClaudeSteerLatch>,
    pub(super) broker: Arc<PermissionBroker>,
    pub(super) runtime: Arc<SessionRuntime>,
    pub(super) conn: Arc<ConnHandle>,
    pub(super) stdin: Arc<Mutex<Option<ChildStdin>>>,
    pub(super) mode_gate: ClaudeModeGateRef,
    pub(super) mode_responses: ClaudeModeResponses,
    pub(super) denied: Arc<Mutex<Vec<Value>>>,
    pub(super) written: Arc<Mutex<Vec<String>>>,
    pub(super) guard: HarnessGuard,
}

pub(super) fn latch_parts() -> LatchParts {
    let (journal, dir) = test_journal();
    let CapturedParts {
        controls,
        written,
        sender,
        denied,
    } = shared_parts();
    let broker = PermissionBroker::with_sender(sender);
    let latch = Arc::new(ClaudeSteerLatch::default());
    let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
    let gate = Arc::new(Mutex::new(ClaudeModeGate {
        state: ClaudeModeGateState::Ready,
        pending_frames: Vec::new(),
    }));
    let mode_responses: ClaudeModeResponses = Arc::new(Mutex::new(HashMap::new()));
    let (runtime, conn) = attach(&broker, Arc::clone(&journal));
    let reader = reader_with_latch(
        ClaudeView::new(None),
        Arc::clone(&broker),
        controls,
        Arc::clone(&mode_responses),
        &stdin,
        Arc::clone(&gate),
        Arc::clone(&latch),
        None,
    );
    LatchParts {
        reader,
        latch,
        broker,
        runtime,
        conn,
        stdin,
        mode_gate: gate,
        mode_responses,
        denied,
        written,
        guard: HarnessGuard {
            child: None,
            journal,
            dir,
        },
    }
}

pub(super) fn latch_harness() -> LatchHarness {
    let LatchParts {
        reader,
        latch,
        broker,
        runtime,
        conn,
        stdin,
        mode_gate,
        mode_responses,
        denied,
        written,
        guard,
    } = latch_parts();
    LatchHarness {
        reader,
        latch,
        broker,
        runtime,
        conn,
        stdin,
        mode_gate,
        mode_responses,
        denied,
        written,
        guard,
    }
}

/// The same construction against a live echo child, so a released gate
/// delivers a steer frame to a real stdin.
pub(super) fn latch_harness_echo() -> LatchHarness {
    let mut child = crate::test_support::spawn_node_echo();
    let (journal, dir) = test_journal();
    let CapturedParts {
        controls,
        written,
        sender,
        denied,
    } = shared_parts();
    let broker = PermissionBroker::with_sender(sender);
    let latch = Arc::new(ClaudeSteerLatch::default());
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let next_id = Arc::new(AtomicU64::new(1));
    let gate = start_initial_mode(&stdin, &next_id, "default".to_string()).expect("mode request");
    let mode_responses: ClaudeModeResponses = Arc::new(Mutex::new(HashMap::new()));
    let (runtime, conn) = attach(&broker, Arc::clone(&journal));
    let reader = reader_with_latch(
        ClaudeView::new(None),
        Arc::clone(&broker),
        controls,
        Arc::clone(&mode_responses),
        &stdin,
        gate.clone(),
        Arc::clone(&latch),
        None,
    );
    LatchHarness {
        reader,
        latch,
        broker,
        runtime,
        conn,
        stdin,
        mode_gate: gate,
        mode_responses,
        denied,
        written,
        guard: HarnessGuard {
            child: Some(child),
            journal,
            dir,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn reader_with_latch(
    view: ClaudeView,
    broker: Arc<PermissionBroker>,
    controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
    mode_responses: ClaudeModeResponses,
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    gate: ClaudeModeGateRef,
    latch: Arc<ClaudeSteerLatch>,
    next_id: Option<Arc<AtomicU64>>,
) -> ClaudeReader {
    let abort_gate: ClaudeAbortGateRef = Arc::new(crate::claude_abort::ClaudeAbortGate::default());
    let mut wiring = ClaudeModeGateWiring::new(Arc::clone(stdin), gate, abort_gate);
    wiring.steer_latch = latch;
    ClaudeReader::with_mode_gate(
        view,
        broker,
        controls,
        mode_responses,
        next_id.unwrap_or_else(|| Arc::new(AtomicU64::new(1))),
        wiring,
        None,
    )
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

pub(super) fn feed(reader: &mut ClaudeReader, runtime: &Arc<SessionRuntime>, value: Value) {
    reader
        .feed(format!("{value}\n").as_bytes(), runtime)
        .expect("feed");
}

pub(super) fn read_echoed_line(child: &mut std::process::Child) -> Value {
    let stdout = child.stdout.as_mut().expect("echoed stdout");
    crate::test_support::read_echoed_json(&mut std::io::BufReader::new(stdout))
}

/// Answer the echo harness's initial mode request the way the CLI does, so
/// the gate is Ready and steer frames deliver.
pub(super) fn release_gate(harness: &mut LatchHarness) {
    let request = read_echoed_line(harness.child_as_mut());
    feed(
        &mut harness.reader,
        &harness.runtime,
        crate::test_support::mode_control_response(&request),
    );
}

pub(super) fn can_use_tool(request_id: &str, tool: &str) -> Value {
    serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": tool,
            "display_name": tool,
            "input": {"command": "echo hi"},
            "tool_use_id": format!("toolu_{request_id}"),
        }
    })
}

pub(super) fn lifecycle(command_uuid: &str, state: &str) -> Value {
    serde_json::json!({
        "type": "command_lifecycle",
        "command_uuid": command_uuid,
        "state": state,
        "session_id": "sess",
    })
}

pub(super) fn published_cards(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn deny_answers(harness: &LatchHarness) -> Vec<Value> {
    harness
        .denied
        .lock()
        .expect("denied")
        .iter()
        .filter(|result| {
            result.pointer("/outcome/outcome").and_then(Value::as_str) == Some("cancelled")
        })
        .cloned()
        .collect()
}
