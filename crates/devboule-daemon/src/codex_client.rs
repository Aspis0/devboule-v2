//! Codex app-server stdio transport for live agent sessions.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_protocol::{ErrorCode, NoticeSeverity, SessionEvent, WireError};
use serde_json::Value;

use super::permission_broker::{PermissionBroker, PermissionSender};
use super::session_runtime::SessionRuntime;
use super::{
    write_child_stdin, ModelSwitcher, PtyCommand, ReaderDispatch, SessionKiller, SessionSteerer,
    SpawnedSession, StderrSource, StdioWaitableChild, TurnToken,
};
use crate::attachment_store::AttachmentStore;
use crate::codex_view::{
    catalog_from_response, mode_values, thread_mode_values, validate_mode, CodexCatalog,
    CodexState, CodexStdout, MAX_LINE_BYTES,
};
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::server::ServerState;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const KILL_GRACE: Duration = Duration::from_secs(2);
/// How long one `turn/steer` may wait for its response before the steer's fate
/// is reported as unknown. The wait happens outside the runtime's turn-hold (see
/// `CodexSteerer::steer_active_turn`), so this bound is what stops a silent
/// app-server from parking a steer forever.
const STEER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

/// The requests this client is waiting on answers for, keyed by the JSON-RPC id
/// the answer will name (A2-03).
///
/// Codex answers `turn/steer` with a response frame, and the response is the
/// only thing that can turn the steer into a truthful `Ok(true)`: writing the
/// request is not Codex taking it.
/// One request's answer channel, in the shape both failures share: `Ok(value)` is
/// the response, `Err(message)` is the transport ending before one came (S4-04).
/// Named, so no signature has to carry the `Result` in a `Result`.
type RequestAnswer = mpsc::Receiver<Result<Value, String>>;

/// The sender end of one [`RequestAnswer`].
type RequestAnswerSender = mpsc::Sender<Result<Value, String>>;

#[derive(Default)]
struct CodexRequests {
    pending: Mutex<HashMap<String, RequestAnswerSender>>,
}

impl CodexRequests {
    fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Register the channel one response id will be delivered on. `None` when
    /// the map cannot be locked — the caller answers `Err`, never `Ok(true)`.
    fn register(&self, id: &str) -> Option<RequestAnswer> {
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .ok()
            .map(|mut pending| pending.insert(id.to_string(), tx))?;
        Some(rx)
    }

    /// Drop one registration: the request failed to write, or its answer never
    /// came and the waiter is giving up on it.
    fn forget(&self, id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(id);
        }
    }

    /// Hand one response to the waiter that registered its id.
    ///
    /// A response whose id matches no waiter is *ignored*, deliberately: the id
    /// space carries Codex's answers to every request this client makes (the
    /// handshake's, the writer's `turn/start`, a `turn/steer` whose waiter
    /// already timed out), and a response for none of them is not an error and
    /// must not take anything down.
    fn deliver(&self, value: &Value) -> bool {
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            return false;
        };
        let sender = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(id));
        sender.is_some_and(|sender| sender.send(Ok(value.clone())).is_ok())
    }

    /// Wake every waiter still registered with the reason the control channel
    /// ended (S4-04).
    ///
    /// A response can no longer arrive for any of them — the app-server's output
    /// is what delivers responses, and it is over — so a waiter left registered
    /// would sit out its whole timeout for an answer that cannot come. The map is
    /// drained under its lock and each sender is answered outside it.
    fn fail_pending(&self, message: &str) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        let waiters: Vec<RequestAnswerSender> = pending.drain().map(|(_, sender)| sender).collect();
        drop(pending);
        for sender in waiters {
            let _ = sender.send(Err(message.to_string()));
        }
    }
}

pub(super) fn resolve_command(paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine Codex working directory: {error}"),
        )
    })?;
    let Some(agent) = crate::provider_catalog::find_available("codex") else {
        return Err(WireError::new(
            ErrorCode::Io,
            "Codex was not found on PATH.",
        ));
    };
    let _ = paths;
    let Some(mut argv) = agent.app_server_command else {
        return Err(WireError::new(
            ErrorCode::Io,
            "Codex is installed but has no app-server launch args.",
        ));
    };
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::Io,
            "Codex resolved to an empty command.",
        ));
    }
    let program = argv.remove(0);
    Ok(PtyCommand::new(program, argv, cwd, Vec::new()).with_provider_id("codex"))
}

pub(super) fn spawn_process(
    state: &Arc<ServerState>,
    command: PtyCommand,
    requested_mode: Option<String>,
) -> Result<SpawnedSession, WireError> {
    let mode_id = requested_mode.as_deref().unwrap_or("auto");
    validate_mode(mode_id)?;

    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &command.env {
        process.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        process.creation_flags(0x0800_0000);
    }
    let mut child = process.spawn().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not start Codex {}: {error}", command.program),
        )
    })?;

    #[cfg(windows)]
    let (process_job, os_handle) = {
        use std::os::windows::io::AsRawHandle;
        let process_job = JobObject::new().map_err(|error| {
            terminate_process(&mut child);
            WireError::new(
                ErrorCode::Io,
                format!("Could not create the Codex process job: {error}"),
            )
        })?;
        let handle = child.as_raw_handle();
        if let Err(error) = state
            .process_job
            .assign(handle)
            .and_then(|()| process_job.assign(handle))
        {
            terminate_process(&mut child);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not contain the Codex process: {error}"),
            ));
        }
        let os_handle = ProcessHandle::duplicate(handle).ok();
        (process_job, os_handle)
    };
    #[cfg(not(windows))]
    let process_job = JobObject::new().map_err(|error| {
        terminate_process(&mut child);
        WireError::new(
            ErrorCode::Io,
            format!("Could not create the Codex process job: {error}"),
        )
    })?;
    #[cfg(not(windows))]
    let os_handle = None;

    let stdin = child.stdin.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Codex did not provide stdin.")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Codex did not provide stdout.")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Codex did not provide stderr.")
    })?;
    let process = Arc::new(Mutex::new(child));
    let stdin = Arc::new(Mutex::new(Some(stdin)));
    let next_id = Arc::new(AtomicU64::new(1));
    let mut stdout = CodexStdout::spawn(stdout).map_err(|error| {
        terminate_shared_process(&process);
        WireError::new(
            ErrorCode::Io,
            format!("Could not read Codex stdout: {error}"),
        )
    })?;
    let handshake = perform_handshake(&mut stdout, &stdin, &next_id, &command.cwd, mode_id)
        .inspect_err(|_| terminate_shared_process(&process))?;

    let state = Arc::new(CodexState::new(
        handshake.thread_id,
        handshake.catalog,
        mode_id,
    ));
    let peer_session_id = state.thread_id();
    // One registration table for the requests this client awaits answers to
    // (A2-03), shared by the steerer that registers and the reader that
    // delivers.
    let requests = Arc::new(CodexRequests::new());
    let switcher = CodexSwitcher {
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        state: Arc::clone(&state),
        requests: Arc::clone(&requests),
    };
    let response_ids = Arc::new(Mutex::new(HashMap::new()));
    let permission_broker = PermissionBroker::with_sender(codex_permission_sender(
        Arc::clone(&stdin),
        Arc::clone(&response_ids),
    ));
    let writer = CodexWriter {
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        state: Arc::clone(&state),
        pending: Vec::new(),
    };
    // The static prompt route sends Codex's own `turn/start`: it shares the
    // stdin, the request-id counter and the thread state with the writer.
    let static_prompt = Arc::new(CodexStaticPrompt::new(
        Arc::clone(&stdin),
        Arc::clone(&next_id),
        Arc::clone(&state),
    ));
    let killer = CodexKiller {
        process: Arc::clone(&process),
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        state: Arc::clone(&state),
        permission_broker: Arc::clone(&permission_broker),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    let reader = CodexReader {
        buffer: Vec::new(),
        discarding_oversized_line: false,
        deferred: handshake.deferred,
        manifest: Some(state.manifest()),
        state,
        view: crate::codex_view::CodexView::new(Some(command.cwd)),
        permission_broker: Arc::clone(&permission_broker),
        response_ids,
        stdin: Arc::clone(&stdin),
        next_id,
        requests,
    };
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        switcher: Some(Box::new(switcher)),
        child: Box::new(StdioWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
        // Not an ACP session: no negotiated structured route. The static one
        // sends Codex's own `turn/start` frame.
        image_sink: None,
        static_image_sink: Some(static_prompt),
        reader: Box::new(stdout),
        reader_dispatch: Some(Box::new(reader)),
        stderr: Some(Box::new(CodexStderr::start(stderr))),
        permission_broker: Some(permission_broker),
        os_handle,
        peer_session_id: Some(peer_session_id),
        agent_version: None,
    })
}

fn terminate_process(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

fn terminate_shared_process(process: &Arc<Mutex<Child>>) {
    if let Ok(mut process) = process.lock() {
        terminate_process(&mut process);
    }
}

struct CodexSwitcher {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    requests: Arc<CodexRequests>,
}

struct CodexSteerer {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    requests: Arc<CodexRequests>,
}

impl SessionSteerer for CodexSteerer {
    fn steer_active_turn(
        &mut self,
        text: &str,
        turn: &mut TurnToken<'_>,
    ) -> Result<bool, WireError> {
        // The turn to steer is read here and checked again at the moment of
        // writing, inside `begin_steer`: a steer written for a turn Codex has
        // already left is not written at all. The daemon's admission token is
        // what keeps this side honest — it is held across this write, so the
        // runtime's turn cannot end between the caller's check and this frame
        // (S4-02) — while the id Codex compares against is its own turn id,
        // which is the only thing its protocol understands (S4-01 on this
        // provider).
        let Some(turn_id) = self.state.current_turn() else {
            return Ok(false);
        };
        // The write stays under the token's hold; the *answer* does not
        // (A2-03). A `turn/steer` is a request: writing it is not Codex taking
        // it, and the response is what says whether it did. That response is
        // delivered by the reader thread, which also publishes the events that
        // take this runtime's turn-hold — so waiting for it under the hold
        // would block the very thread that has to deliver it, exactly as it
        // would for Pi. The hold therefore ends with the write, and the wait
        // runs outside it: `Ok(true)` is only ever answered from a response
        // that names this turn.
        let request = turn.write_then_release(|| self.begin_steer(&turn_id, text))?;
        let Some((command_id, response)) = request else {
            return Ok(false);
        };
        self.await_steer_response(&command_id, &turn_id, response)
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(Self {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            requests: Arc::clone(&self.requests),
        })
    }
}

impl CodexSteerer {
    /// Register the answer channel and write one `turn/steer` for
    /// `expected_turn_id`, answering the id its response will name.
    ///
    /// `Ok(None)` with nothing written: Codex is no longer on the turn this
    /// steer was admitted for.
    fn begin_steer(
        &self,
        expected_turn_id: &str,
        text: &str,
    ) -> Result<Option<(String, RequestAnswer)>, WireError> {
        let Some(params) = steer_params_if_current(&self.state, expected_turn_id, text) else {
            return Ok(None);
        };
        let id = format!("d-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let Some(response) = self.requests.register(&id) else {
            return Err(WireError::new(
                ErrorCode::Io,
                "Codex request map is unavailable.",
            ));
        };
        if let Err(error) = send_frame(
            &self.stdin,
            &request_frame(&id, "turn/steer", params),
            "Codex",
        ) {
            self.requests.forget(&id);
            return Err(error);
        }
        Ok(Some((id, response)))
    }

    /// Wait for the response to one `turn/steer` and answer whether Codex took
    /// the steer for the turn it was written for.
    ///
    /// An error result, a response naming another turn, and a response with no
    /// turn at all are all `Ok(false)`: none of them is evidence that the text
    /// landed in this turn, so the caller's pre-existing fallback is the honest
    /// answer, never `Ok(true)`. A timeout is an `Err` — the steer's fate is
    /// then unknown, which is a different thing from knowing it did not land.
    fn await_steer_response(
        &self,
        command_id: &str,
        expected_turn_id: &str,
        response: RequestAnswer,
    ) -> Result<bool, WireError> {
        let value = response
            .recv_timeout(STEER_RESPONSE_TIMEOUT)
            .map_err(|error| {
                self.requests.forget(command_id);
                WireError::new(
                    ErrorCode::Io,
                    format!("Codex turn/steer response timed out: {error}"),
                )
            })?
            // The transport's own answer: the channel ended before a response
            // came (S4-04), so the steer's fate is unknown — an `Err`, not a
            // refusal, exactly as Pi answers one.
            .map_err(|message| WireError::new(ErrorCode::Io, message))?;
        Ok(steer_response_accepted(&value, expected_turn_id))
    }
}

/// Whether one `turn/steer` response says Codex took the steer for
/// `expected_turn_id` (A2-03).
///
/// The protocol's own confirmation is the turn the response reports: an error
/// result means the request was refused, and a response for a different turn (or
/// without one) is not evidence about *this* turn. Only a matching turn id is
/// `true`.
fn steer_response_accepted(value: &Value, expected_turn_id: &str) -> bool {
    if value.get("error").is_some() {
        return false;
    }
    turn_id_from_response(value).as_deref() == Some(expected_turn_id)
}

impl ModelSwitcher for CodexSwitcher {
    fn set_model(&self, model_id: Option<&str>, effort: Option<&str>) -> Result<(), WireError> {
        self.state.set_model(model_id, effort)
    }

    fn set_mode(&self, mode_id: &str) -> Result<(), WireError> {
        self.state.set_mode(mode_id)
    }

    fn manifest(&self) -> Option<SessionEvent> {
        Some(self.state.manifest())
    }

    fn clone_switcher(&self) -> Box<dyn ModelSwitcher> {
        Box::new(Self {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            requests: Arc::clone(&self.requests),
        })
    }

    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(CodexSteerer {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            requests: Arc::clone(&self.requests),
        })
    }
}

/// The static prompt route for Codex: it plans, then sends Codex's own
/// `turn/start` — the frame this provider's protocol defines for a prompt that
/// carries images.
pub(crate) struct CodexStaticPrompt {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
}

impl CodexStaticPrompt {
    fn new(
        stdin: Arc<Mutex<Option<ChildStdin>>>,
        next_id: Arc<AtomicU64>,
        state: Arc<CodexState>,
    ) -> Self {
        Self {
            stdin,
            next_id,
            state,
        }
    }
}

impl super::StaticImageSink for CodexStaticPrompt {
    fn plan_prompt(
        &self,
        store: &AttachmentStore,
        session_id: &str,
        text: &str,
        attachments: &[devboule_protocol::PromptAttachment],
    ) -> Result<Option<Box<dyn super::PlannedStaticPrompt>>, WireError> {
        let Some(plan) = plan_codex_prompt(store, session_id, text, attachments)? else {
            return Ok(None);
        };
        Ok(Some(Box::new(CodexPlannedPrompt {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            plan,
        })))
    }
}

/// One planned Codex prompt, ready to send. It carries the plan whole, so the
/// text on the wire and the text the caller journals cannot be two different
/// strings.
struct CodexPlannedPrompt {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    plan: CodexPromptPlan,
}

impl CodexPlannedPrompt {
    /// The `turn/start` params this prompt sends: the model, effort and policy
    /// override are read at send time, the way the writer reads them, so a
    /// model switched between prompt and send is not sent a stale name.
    fn params(&self) -> Value {
        turn_start_params_for_prompt(
            &self.state,
            &self.plan.fallback_text,
            &self.plan.image_paths,
        )
    }
}

impl super::PlannedStaticPrompt for CodexPlannedPrompt {
    fn text(&self) -> &str {
        &self.plan.fallback_text
    }

    fn send(&self) -> Result<(), WireError> {
        send_request(
            &self.stdin,
            &self.next_id,
            "turn/start",
            self.params(),
            "Codex",
        )
    }
}

struct CodexWriter {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    pending: Vec<u8>,
}

impl Write for CodexWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        // The text-only `turn/start`, unchanged. A prompt that carries
        // images is sent by the static route's own `turn/start` instead of by
        // this writer, so `turn_start_params_with_images` has one production
        // caller and this literal keeps the other: with no carried path the
        // two build the same params, which
        // `a_params_builder_without_images_is_the_text_only_one` pins.
        let (model, effort) = self.state.model_and_effort();
        let policy_mode = self.state.mode_override();
        send_request(
            &self.stdin,
            &self.next_id,
            "turn/start",
            turn_start_params(
                &self.state.thread_id(),
                &text,
                policy_mode.as_deref(),
                Some(&model),
                effort.as_deref(),
            ),
            "Codex",
        )
        .map_err(wire_to_io)
    }
}

struct CodexKiller {
    process: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    state: Arc<CodexState>,
    permission_broker: Arc<PermissionBroker>,
    cancelled: Arc<AtomicBool>,
}

impl SessionKiller for CodexKiller {
    fn interrupt(&mut self) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let _ = send_interrupt_request(&self.stdin, &self.next_id, &self.state);
        self.permission_broker.cancel_pending();
    }

    fn kill(&mut self) {
        if self.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        self.permission_broker.close();
        if let Ok(mut stdin) = self.stdin.lock() {
            *stdin = None;
        }
        let deadline = Instant::now() + KILL_GRACE;
        while Instant::now() < deadline {
            if self
                .process
                .lock()
                .ok()
                .and_then(|mut process| process.try_wait().ok())
                .flatten()
                .is_some()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Ok(mut process) = self.process.lock() {
            let _ = process.kill();
        }
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            process: Arc::clone(&self.process),
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            state: Arc::clone(&self.state),
            permission_broker: Arc::clone(&self.permission_broker),
            cancelled: Arc::clone(&self.cancelled),
        })
    }
}

struct Handshake {
    thread_id: String,
    catalog: CodexCatalog,
    deferred: Vec<Value>,
}

fn perform_handshake(
    stdout: &mut CodexStdout,
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    cwd: &Path,
    mode_id: &str,
) -> Result<Handshake, WireError> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut deferred = Vec::new();
    let _ = request_response(
        stdout,
        stdin,
        next_id,
        "initialize",
        initialize_params(),
        deadline,
        &mut deferred,
    )?;
    send_frame(
        stdin,
        &notification_frame("initialized", serde_json::json!({})),
        "Codex",
    )?;
    let models_response = request_response(
        stdout,
        stdin,
        next_id,
        "model/list",
        serde_json::json!({}),
        deadline,
        &mut deferred,
    )?;
    let mut catalog = catalog_from_response(&models_response)?;
    let thread_response = request_response(
        stdout,
        stdin,
        next_id,
        "thread/start",
        thread_start_params(cwd, mode_id),
        deadline,
        &mut deferred,
    )?;
    let thread_id = thread_response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| handshake_error("thread/start response had no thread.id"))?
        .to_string();
    catalog.apply_thread_response(&thread_response);
    Ok(Handshake {
        thread_id,
        catalog,
        deferred,
    })
}

fn initialize_params() -> Value {
    // Paseo's non-originating identity: Codex keeps its own CLI identity in
    // provider usage logs instead of showing the daemon as the originator.
    serde_json::json!({
        "clientInfo": {
            "name": "codex_app_server_daemon",
            "title": "Codex App Server Daemon",
            "version": "0.0.0",
        },
        "capabilities": {
            "experimentalApi": true,
            "mcpServerOpenaiFormElicitation": true,
        },
    })
}

fn request_response(
    stdout: &mut CodexStdout,
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    method: &str,
    params: Value,
    deadline: Instant,
    deferred: &mut Vec<Value>,
) -> Result<Value, WireError> {
    let id = format!("d-{}", next_id.fetch_add(1, Ordering::Relaxed));
    send_frame(stdin, &request_frame(&id, method, params), "Codex")?;
    loop {
        let line = stdout
            .next_line(deadline)
            .map_err(|error| {
                handshake_error(&format!("could not read {method} response: {error}"))
            })?
            .ok_or_else(|| handshake_error(&format!("Codex exited before {method} completed")))?;
        let value: Value = serde_json::from_str(&line)
            .map_err(|error| handshake_error(&format!("malformed {method} response: {error}")))?;
        if value.get("id").and_then(Value::as_str) == Some(id.as_str()) {
            if let Some(error) = value.get("error") {
                return Err(handshake_error(
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("request failed"),
                ));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
        deferred.push(value);
    }
}

fn send_request(
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    method: &str,
    params: Value,
    label: &'static str,
) -> Result<(), WireError> {
    let id = format!("d-{}", next_id.fetch_add(1, Ordering::Relaxed));
    send_frame(stdin, &request_frame(&id, method, params), label)
}

fn request_frame(id: &str, method: &str, params: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn method_not_supported_frame(id: &Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32601,
            "message": "method not supported",
        },
    })
}

fn send_interrupt_request(
    stdin: &Mutex<Option<ChildStdin>>,
    next_id: &AtomicU64,
    state: &CodexState,
) -> Result<bool, WireError> {
    let Some(turn_id) = state.current_turn() else {
        eprintln!("Codex interrupt ignored: no current turn.");
        return Ok(false);
    };
    send_request(
        stdin,
        next_id,
        "turn/interrupt",
        interrupt_params(&state.thread_id(), &turn_id),
        "Codex",
    )
    .map(|()| true)
}

fn turn_id_from_response(value: &Value) -> Option<String> {
    value
        .pointer("/result/turn/id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn notification_frame(method: &str, params: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

fn turn_start_params(
    thread_id: &str,
    text: &str,
    policy_mode: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    // The thread already carries the mode preset from `thread/start`; the
    // policy fields go back only after an explicit `set_mode`.
    let mut params = policy_mode.map(mode_values).unwrap_or_default();
    params.insert("threadId".to_string(), Value::String(thread_id.to_string()));
    params.insert(
        "input".to_string(),
        serde_json::json!([{ "type": "text", "text": text }]),
    );
    if let Some(model) = model {
        params.insert("model".to_string(), Value::String(model.to_string()));
    }
    if let Some(effort) = effort {
        params.insert("effort".to_string(), Value::String(effort.to_string()));
    }
    Value::Object(params)
}

fn turn_steer_params(thread_id: &str, expected_turn_id: &str, text: &str) -> Value {
    serde_json::json!({
        "threadId": thread_id,
        "expectedTurnId": expected_turn_id,
        "input": [{"type": "text", "text": text}],
    })
}

/// The `turn/steer` params for the turn the caller captured, or `None` when
/// Codex is no longer running that turn.
///
/// This is the write-time check: `current_turn()` is read again here, after the
/// caller's own capture, so a turn that ended (or a new one that started) in
/// between makes the steer unavailable instead of sending Codex a frame for a
/// turn that is over. `expectedTurnId` carries that same id, which is also the
/// precondition Codex itself checks.
fn steer_params_if_current(
    state: &CodexState,
    expected_turn_id: &str,
    text: &str,
) -> Option<Value> {
    (state.current_turn().as_deref() == Some(expected_turn_id))
        .then(|| turn_steer_params(&state.thread_id(), expected_turn_id, text))
}

/// One `turn/start` input entry for a materialized raster: the Paseo-measured
/// `{"type": "localImage", "path": ...}` shape. The path is the one
/// `materialize` returned, so the file at the other end is still the stripped
/// file — no bytes travel. `detail` is omitted deliberately: `auto` is the
/// server's own default and sending it would assert an unmeasured choice.
/// Stays on `localImage` even though the schema also lists `image`: Paseo
/// sends `localImage` and that is what was verified live, while `image` on
/// this surface has not been measured.
fn codex_local_image_entry(path: &std::path::Path) -> serde_json::Value {
    serde_json::json!({
        "type": "localImage",
        "path": path.to_string_lossy(),
    })
}

/// `turn/start` params with one trailing `localImage` entry per carried
/// raster, in attachment order, after the single text entry. The text entry
/// is the shared fallback text: the user's text plus the path lines for the
/// attachments that stay prose (SVG, which takes no inline shape).
///
/// With no carried path it builds exactly what `turn_start_params` builds —
/// `a_params_builder_without_images_is_the_text_only_one` pins that — which is
/// what lets the static route send every prompt it plans through this one
/// builder.
fn turn_start_params_with_images(
    thread_id: &str,
    text: &str,
    image_paths: &[std::path::PathBuf],
    policy_mode: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    // The thread already carries the mode preset from `thread/start`; the
    // policy fields go back only after an explicit `set_mode`.
    let mut params = policy_mode.map(mode_values).unwrap_or_default();
    params.insert("threadId".to_string(), Value::String(thread_id.to_string()));
    // The text entry first, then one `localImage` entry per carried raster,
    // in attachment order.
    let mut input = vec![serde_json::json!({ "type": "text", "text": text })];
    input.extend(image_paths.iter().map(|path| codex_local_image_entry(path)));
    params.insert("input".to_string(), Value::Array(input));
    if let Some(model) = model {
        params.insert("model".to_string(), Value::String(model.to_string()));
    }
    if let Some(effort) = effort {
        params.insert("effort".to_string(), Value::String(effort.to_string()));
    }
    Value::Object(params)
}

/// The `turn/start` params for one planned prompt: the model, effort and policy
/// override are read here, at send time, the way the writer reads them, so a
/// model switched between planning and sending is never sent a stale name.
fn turn_start_params_for_prompt(
    state: &CodexState,
    text: &str,
    image_paths: &[std::path::PathBuf],
) -> Value {
    let (model, effort) = state.model_and_effort();
    let policy_mode = state.mode_override();
    turn_start_params_with_images(
        &state.thread_id(),
        text,
        image_paths,
        policy_mode.as_deref(),
        Some(&model),
        effort.as_deref(),
    )
}

/// Prompt plan for one Codex send: the text plus one `localImage` path per
/// raster. `fallback_text` is the user's text with the path lines for the
/// non-raster attachments (SVG). `image_paths` are the `materialize` paths
/// for the rasters, in attachment order — every attachment is materialized
/// first, exactly the call the shared `with_attachment_paths` makes, so a
/// request that fails on its third attachment leaves nothing half-built.
///
/// It is handed to `CodexStaticPrompt`, which sends it as Codex's own
/// `turn/start`.
struct CodexPromptPlan {
    fallback_text: String,
    image_paths: Vec<std::path::PathBuf>,
}

/// The delivery Codex is authorised for: a fact about the protocol, not a
/// fact the peer agreed to — no negotiation, no capability probe. There is
/// deliberately no probe here: an unknown method on this surface answers
/// `-32600`, not `-32601`, so a method-not-found fallback would never fire and
/// the feature would fail silent.
fn codex_delivery() -> super::ImageDelivery {
    super::ImageDelivery::StaticImageBlock
}

/// Splits one request's attachments into `localImage` paths and path-line
/// fallbacks, each attachment materialized exactly once — the call the shared
/// `with_attachment_paths` makes.
///
/// `None` means the route did not run at all: no attachments, or a delivery
/// this sender is not authorised for. When it does run it answers with the
/// text as well, even if no raster became a `localImage` path, so that the
/// caller never has to walk the attachments a second time; with no carried
/// path the frame is the text-only `turn/start`, byte for byte.
fn plan_codex_prompt(
    store: &AttachmentStore,
    session_id: &str,
    text: &str,
    attachments: &[devboule_protocol::PromptAttachment],
) -> Result<Option<CodexPromptPlan>, devboule_protocol::WireError> {
    if attachments.is_empty() {
        return Ok(None);
    }
    // The static gate, read through the shared enum so a later change to
    // what "statically known" authorises cannot silently re-route this
    // sender: only the reserved variant frames paths.
    if codex_delivery() != super::ImageDelivery::StaticImageBlock {
        return Ok(None);
    }
    let session = store.session(session_id).ok_or_else(|| {
        devboule_protocol::WireError::new(
            devboule_protocol::ErrorCode::InvalidRequest,
            "Invalid session id.",
        )
    })?;
    let mut image_paths = Vec::new();
    let mut fallback_paths = Vec::new();
    for attachment in attachments {
        let path = session.materialize(attachment)?;
        if crate::raster_metadata::RasterMime::from_mime_type(&attachment.mime_type).is_some() {
            image_paths.push(path);
        } else {
            fallback_paths.push(path);
        }
    }
    Ok(Some(CodexPromptPlan {
        fallback_text: super::prompt_text_with_fallback_paths(text, &fallback_paths),
        image_paths,
    }))
}

/// The read-side twin of the routing rule above: what the daemon kept on
/// disk for a carried entry. Used by tests to pin the rule without spawning
/// a child.
#[cfg(test)]
fn carried_image_paths(plan: Option<&CodexPromptPlan>) -> Vec<&std::path::Path> {
    plan.map(|plan| {
        plan.image_paths
            .iter()
            .map(std::path::PathBuf::as_path)
            .collect()
    })
    .unwrap_or_default()
}

fn interrupt_params(thread_id: &str, turn_id: &str) -> Value {
    serde_json::json!({ "threadId": thread_id, "turnId": turn_id })
}

fn thread_start_params(cwd: &Path, mode_id: &str) -> Value {
    let mut params = thread_mode_values(mode_id);
    params.insert("model".to_string(), Value::Null);
    params.insert(
        "cwd".to_string(),
        Value::String(cwd.to_string_lossy().into_owned()),
    );
    Value::Object(params)
}

fn send_frame(
    stdin: &Mutex<Option<ChildStdin>>,
    frame: &Value,
    label: &'static str,
) -> Result<(), WireError> {
    let mut bytes = serde_json::to_vec(frame).map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not encode {label} frame: {error}"),
        )
    })?;
    bytes.push(b'\n');
    write_child_stdin(stdin, &bytes, label).map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not write {label} frame: {error}"),
        )
    })
}

fn wire_to_io(error: WireError) -> io::Error {
    io::Error::other(error.message)
}

fn handshake_error(message: &str) -> WireError {
    WireError::new(
        ErrorCode::Io,
        format!("Could not start Codex session: {message}"),
    )
}

fn codex_permission_sender(
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    response_ids: Arc<Mutex<HashMap<u64, Value>>>,
) -> Arc<PermissionSender> {
    Arc::new(move |broker_id, result| {
        let request_id = response_ids
            .lock()
            .map_err(|_| io::Error::other("Codex permission map lock poisoned"))?
            .get(&broker_id)
            .cloned()
            .ok_or_else(|| io::Error::other("Codex permission response had no matching request"))?;
        let frame = permission_decision_frame(&request_id, permission_decision(&result));
        let result =
            send_frame(&stdin, &frame, "Codex").map_err(|error| io::Error::other(error.message));
        if result.is_ok() {
            let _ = response_ids.lock().map(|mut ids| ids.remove(&broker_id));
        }
        result
    })
}

fn permission_decision(result: &Value) -> &'static str {
    if result.pointer("/outcome/outcome").and_then(Value::as_str) != Some("selected") {
        return "cancel";
    }
    match result.pointer("/outcome/optionId").and_then(Value::as_str) {
        Some("allow") => "accept",
        Some("deny") => "decline",
        _ => "cancel",
    }
}

struct CodexReader {
    buffer: Vec<u8>,
    discarding_oversized_line: bool,
    deferred: Vec<Value>,
    manifest: Option<SessionEvent>,
    state: Arc<CodexState>,
    view: crate::codex_view::CodexView,
    permission_broker: Arc<PermissionBroker>,
    response_ids: Arc<Mutex<HashMap<u64, Value>>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    requests: Arc<CodexRequests>,
}

impl CodexReader {
    fn publish(&self, runtime: &SessionRuntime, event: SessionEvent, seq: Option<u64>) {
        let _ = runtime.publish_agent_event_with_seq(event, None, seq);
    }

    fn dispatch_value(&mut self, value: Value, runtime: &Arc<SessionRuntime>) {
        let event_seq = runtime.journal_agent_envelope(&value);
        if value.get("method").and_then(Value::as_str)
            == Some("item/commandExecution/requestApproval")
            || value.get("method").and_then(Value::as_str)
                == Some("item/fileChange/requestApproval")
        {
            self.dispatch_permission(&value, runtime, event_seq);
            return;
        }
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            if method == "item/tool/requestUserInput" || method == "mcpServer/elicitation/request" {
                self.decline_input_request(&value, runtime);
                return;
            }
            if method == "turn/started" {
                self.state.set_turn(
                    value
                        .pointer("/params/turn/id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                );
            } else if let Some(id) = value.get("id") {
                let _ = send_frame(&self.stdin, &method_not_supported_frame(id), "Codex");
            }
        } else if value.get("id").is_some() {
            // A response to one of this client's own requests: hand it to the
            // waiter that registered that id before anything else looks at it
            // (A2-03). A response whose id matches no waiter is ignored.
            self.requests.deliver(&value);
            if let Some(turn_id) = turn_id_from_response(&value) {
                self.state.set_turn(Some(turn_id));
            }
            if let Some(error) = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
            {
                let _ = runtime.publish_session_notice(error.to_string(), NoticeSeverity::Warning);
            }
        }
        let mut seq = event_seq;
        for event in self.view.ingest(&value) {
            if matches!(
                event,
                SessionEvent::AgentFinished { .. }
                    | SessionEvent::AgentError { .. }
                    | SessionEvent::SessionNotice { .. }
            ) && value.get("method").and_then(Value::as_str) == Some("turn/completed")
            {
                self.state.set_turn(None);
            }
            self.publish(runtime, event, seq.take());
        }
        if let Some(context_window) = self.view.take_context_window_update() {
            self.state.set_context_window(context_window);
            let manifest = runtime.store_session_manifest(self.state.manifest());
            self.publish(runtime, manifest, seq.take().or(event_seq));
        }
    }

    fn dispatch_permission(
        &mut self,
        value: &Value,
        runtime: &Arc<SessionRuntime>,
        seq: Option<u64>,
    ) {
        let params = value.get("params").unwrap_or(&Value::Null);
        if params.get("itemId").and_then(Value::as_str).is_none() {
            return;
        }
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let file_change = method == "item/fileChange/requestApproval";
        let Some(event) = crate::codex_view::permission_request_event(params, file_change) else {
            return;
        };
        let broker_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut ids) = self.response_ids.lock() {
            if let Some(id) = value.get("id") {
                ids.insert(broker_id, id.clone());
            }
        }
        if let Err(error) = self
            .permission_broker
            .register(broker_id, event.clone(), runtime)
        {
            self.response_ids
                .lock()
                .ok()
                .map(|mut ids| ids.remove(&broker_id));
            let _ = send_permission_decision(&self.stdin, value.get("id"), "cancel");
            let _ = runtime.publish_session_notice(
                format!("Could not queue Codex permission request: {error}"),
                NoticeSeverity::Warning,
            );
            return;
        }
        self.publish(runtime, event, seq);
    }

    fn decline_input_request(&self, value: &Value, runtime: &Arc<SessionRuntime>) {
        let result = decline_input_result(value);
        if let Some(id) = value.get("id") {
            let _ = send_result(&self.stdin, id, result);
        }
        let _ = runtime.publish_session_notice(
            "Codex requested user input; Devboule declined it.".to_string(),
            NoticeSeverity::Info,
        );
    }
}

fn decline_input_result(value: &Value) -> Value {
    if value.get("method").and_then(Value::as_str) == Some("mcpServer/elicitation/request") {
        serde_json::json!({ "action": "decline" })
    } else {
        serde_json::json!({ "answers": {} })
    }
}

impl ReaderDispatch for CodexReader {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String> {
        if let Some(manifest) = self.manifest.take() {
            let manifest = runtime.store_session_manifest(manifest);
            self.publish(runtime, manifest, None);
            for value in std::mem::take(&mut self.deferred) {
                self.dispatch_value(value, runtime);
            }
        }
        let mut bytes = bytes;
        if self.discarding_oversized_line {
            let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
                return Ok(());
            };
            self.discarding_oversized_line = false;
            bytes = &bytes[newline + 1..];
        }
        self.buffer.extend_from_slice(bytes);
        loop {
            let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') else {
                if self.buffer.len() > MAX_LINE_BYTES {
                    self.buffer.clear();
                    self.discarding_oversized_line = true;
                    let _ = runtime.publish_session_notice(
                        format!("Codex input line exceeded {MAX_LINE_BYTES} bytes."),
                        NoticeSeverity::Warning,
                    );
                }
                break;
            };
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let value = serde_json::from_slice::<Value>(line)
                .map_err(|error| format!("Malformed Codex output: {error}"))?;
            self.dispatch_value(value, runtime);
        }
        Ok(())
    }

    fn finish(&mut self, runtime: &Arc<SessionRuntime>) {
        self.permission_broker.close();
        // (S4-04) The transport is over, so no answer can arrive for a request
        // still waiting: failing them here is what stops a steer from waiting out
        // the whole `STEER_RESPONSE_TIMEOUT` after the provider is gone. The same
        // shape as Pi's `PiControl::fail_pending`.
        self.requests
            .fail_pending("Codex control channel closed before the response arrived.");
        if let Ok(mut ids) = self.response_ids.lock() {
            ids.clear();
        }
        if !self.buffer.is_empty() {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "Codex ended with an unterminated output line.".to_string(),
                },
                None,
            );
        }
    }
}

fn send_permission_decision(
    stdin: &Mutex<Option<ChildStdin>>,
    id: Option<&Value>,
    decision: &str,
) -> io::Result<()> {
    let Some(id) = id else {
        return Ok(());
    };
    send_frame(stdin, &permission_decision_frame(id, decision), "Codex").map_err(wire_to_io)
}

fn send_result(stdin: &Mutex<Option<ChildStdin>>, id: &Value, result: Value) -> io::Result<()> {
    let frame = response_frame(id, result);
    let mut bytes = serde_json::to_vec(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    write_child_stdin(stdin, &bytes, "Codex")
}

fn response_frame(id: &Value, result: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn permission_decision_frame(id: &Value, decision: &str) -> Value {
    response_frame(id, serde_json::json!({ "decision": decision }))
}

struct CodexStderr {
    stderr: Option<ChildStderr>,
}

impl CodexStderr {
    fn start(stderr: ChildStderr) -> Self {
        Self {
            stderr: Some(stderr),
        }
    }
}

impl StderrSource for CodexStderr {
    fn spawn(mut self: Box<Self>, runtime: Arc<SessionRuntime>) -> io::Result<JoinHandle<()>> {
        let mut stderr = self
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("Codex stderr drain was already consumed"))?;
        std::thread::Builder::new()
            .name("session-codex-stderr".to_string())
            .spawn(move || {
                let mut buffer = [0u8; 4096];
                loop {
                    match stderr.read(&mut buffer) {
                        Ok(0) => return,
                        Ok(length) => {
                            let data = String::from_utf8_lossy(&buffer[..length]).into_owned();
                            let _ = runtime
                                .publish_agent_event(SessionEvent::AgentStderr { data }, None);
                        }
                        Err(_) => return,
                    }
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::event_pull::ConnHandle;
    use super::super::permission_broker::PermissionBroker;
    use super::super::session_runtime::SessionRuntime;
    use super::{
        carried_image_paths, codex_delivery, codex_local_image_entry, decline_input_result,
        initialize_params, interrupt_params, mode_values, notification_frame, permission_decision,
        permission_decision_frame, plan_codex_prompt, request_frame, send_interrupt_request,
        steer_params_if_current, thread_start_params, turn_id_from_response, turn_start_params,
        turn_start_params_for_prompt, turn_start_params_with_images, turn_steer_params,
        validate_mode, CodexReader, CodexRequests, CodexSteerer,
    };
    use crate::attachment_store::AttachmentStore;
    use crate::codex_view::{catalog_from_response, fixture_frames, CodexState, CodexView};
    use crate::raster_metadata::{clean_png, png_with_text_chunk, vector_input, vector_output};
    use crate::session::ReaderDispatch;
    use devboule_protocol::PromptAttachment;
    use devboule_protocol::SessionEvent;
    use devboule_protocol::WireError;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn method_frame(source: &str, method: &str) -> serde_json::Value {
        fixture_frames(source)
            .into_iter()
            .find(|frame| frame.get("method").and_then(serde_json::Value::as_str) == Some(method))
            .expect("measured request")
    }
    #[test]
    fn codex_modes_use_the_measured_policy_shapes() {
        assert_eq!(
            mode_values("auto"),
            serde_json::json!({
                "approvalPolicy": "on-request",
                "sandboxPolicy": {
                    "type": "workspaceWrite",
                    "networkAccess": false,
                    "writableRoots": []
                }
            })
            .as_object()
            .expect("object")
            .clone()
        );
        assert_eq!(
            mode_values("auto-review").get("approvalsReviewer"),
            Some(&serde_json::json!("auto_review"))
        );
        assert_eq!(
            mode_values("full-access"),
            serde_json::json!({
                "approvalPolicy": "never",
                "sandboxPolicy": { "type": "dangerFullAccess" }
            })
            .as_object()
            .expect("object")
            .clone()
        );
        assert!(validate_mode("bypass").is_err());
    }

    #[test]
    fn measured_control_frames_and_turn_modes_keep_the_wire_shapes() {
        let thread = method_frame(
            include_str!("../fixtures/wire/codex/E1-step3-thread.jsonl"),
            "thread/start",
        );
        let cwd = thread["params"]["cwd"].as_str().expect("cwd");
        assert_eq!(
            thread["params"],
            thread_start_params(Path::new(cwd), "auto")
        );

        let initialized = method_frame(
            include_str!("../fixtures/wire/codex/E1-step1-handshake.jsonl"),
            "initialized",
        );
        assert_eq!(
            initialized,
            notification_frame("initialized", serde_json::json!({}))
        );

        let changed = method_frame(
            include_str!("../fixtures/wire/codex/E1-step6-modechange.jsonl"),
            "turn/start",
        );
        let params = &changed["params"];
        assert_eq!(
            *params,
            turn_start_params(
                params["threadId"].as_str().expect("thread id"),
                params["input"][0]["text"].as_str().expect("prompt"),
                Some("full-access"),
                None,
                None,
            )
        );
        let interrupt = method_frame(
            include_str!("../fixtures/wire/codex/E1-step6b-interrupt.jsonl"),
            "turn/interrupt",
        );
        assert_eq!(
            interrupt["params"],
            interrupt_params(
                interrupt["params"]["threadId"].as_str().expect("thread id"),
                interrupt["params"]["turnId"].as_str().expect("turn id"),
            )
        );

        for (outcome, decision) in [
            (
                serde_json::json!({"outcome":{"outcome":"selected","optionId":"allow"}}),
                "accept",
            ),
            (
                serde_json::json!({"outcome":{"outcome":"selected","optionId":"deny"}}),
                "decline",
            ),
            (
                serde_json::json!({"outcome":{"outcome":"cancelled"}}),
                "cancel",
            ),
        ] {
            assert_eq!(permission_decision(&outcome), decision);
            let mut bytes =
                serde_json::to_vec(&permission_decision_frame(&serde_json::json!(5), decision))
                    .expect("response json");
            bytes.push(b'\n');
            let response: serde_json::Value = serde_json::from_slice(&bytes).expect("response");
            assert_eq!(response["result"]["decision"], decision);
        }
    }

    #[test]
    fn turn_start_carries_the_policy_only_after_a_mode_change() {
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let state = CodexState::new("thread".to_string(), catalog, "auto");
        assert_eq!(state.mode_override(), None);

        let first = turn_start_params(
            &state.thread_id(),
            "first",
            state.mode_override().as_deref(),
            None,
            None,
        );
        assert!(first.get("approvalPolicy").is_none());
        assert!(first.get("sandboxPolicy").is_none());

        state.set_mode("read-only").expect("set mode");
        let changed = turn_start_params(
            &state.thread_id(),
            "changed",
            state.mode_override().as_deref(),
            None,
            None,
        );
        assert_eq!(changed["approvalPolicy"], "on-request");
        assert_eq!(changed["sandboxPolicy"]["type"], "readOnly");

        // Paseo keeps `hasWorkflowModeOverride` set, so every later turn
        // re-sends the policy too.
        let later = turn_start_params(
            &state.thread_id(),
            "later",
            state.mode_override().as_deref(),
            None,
            None,
        );
        assert_eq!(later["sandboxPolicy"]["type"], "readOnly");
    }

    #[test]
    fn read_only_thread_start_sends_the_read_only_sandbox() {
        let params = thread_start_params(Path::new("C:\\work"), "read-only");
        assert_eq!(params["approvalPolicy"], "on-request");
        assert_eq!(params["sandbox"], "read-only");
    }

    #[test]
    fn initialize_request_includes_paseo_capabilities_on_the_wire() {
        let frame = super::request_frame("d-1", "initialize", initialize_params());
        let mut bytes = serde_json::to_vec(&frame).expect("initialize request");
        bytes.push(b'\n');
        let wire = std::str::from_utf8(&bytes).expect("initialize bytes");
        assert!(wire.contains(r#""experimentalApi":true"#));
        assert!(wire.contains(r#""mcpServerOpenaiFormElicitation":true"#));
        assert_eq!(
            frame["params"]["clientInfo"],
            serde_json::json!({
                "name": "codex_app_server_daemon",
                "title": "Codex App Server Daemon",
                "version": "0.0.0"
            })
        );
    }

    #[test]
    fn unknown_server_request_gets_a_method_not_supported_error() {
        use std::io::{BufRead, BufReader};

        // S4-06: the fake app-server is a `node` script, so this skips where
        // there is no node rather than failing there, like every other
        // node-backed test in this file.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        let mut child = std::process::Command::new("node")
            .args([
                "-e",
                "process.stdin.on('data', data => process.stdout.write(data))",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("node is required for the Codex request test");
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let mut reader = CodexReader {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            deferred: Vec::new(),
            manifest: None,
            state: Arc::new(CodexState::new("thread".to_string(), catalog, "auto")),
            view: CodexView::new(None),
            permission_broker: Arc::clone(&broker),
            response_ids: Arc::new(Mutex::new(HashMap::new())),
            stdin,
            next_id: Arc::new(AtomicU64::new(1)),
            requests: Arc::new(CodexRequests::new()),
        };
        let runtime = Arc::new(SessionRuntime::new());
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "server-17",
            "method": "codex/futureRequest",
            "params": {}
        });
        reader.dispatch_value(request, &runtime);
        let mut line = String::new();
        stdout.read_line(&mut line).expect("response");
        let response: serde_json::Value = serde_json::from_str(&line).expect("response json");
        assert_eq!(
            response,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "server-17",
                "error": { "code": -32601, "message": "method not supported" }
            })
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn interrupt_without_a_current_turn_is_an_ok_noop() {
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let state = CodexState::new("thread".to_string(), catalog, "auto");
        let stdin = Mutex::new(None);
        let next_id = AtomicU64::new(1);
        assert!(matches!(
            send_interrupt_request(&stdin, &next_id, &state),
            Ok(false)
        ));
        assert_eq!(next_id.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn turn_start_response_records_the_turn_before_started_notification() {
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let state = CodexState::new("thread".to_string(), catalog, "auto");
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "d-7",
            "result": { "turn": { "id": "turn-7" } }
        });
        state.set_turn(turn_id_from_response(&response));
        assert_eq!(state.current_turn().as_deref(), Some("turn-7"));
    }

    #[test]
    fn declined_input_is_a_session_notice_and_keeps_the_decline_shapes() {
        assert_eq!(
            decline_input_result(&serde_json::json!({
                "method": "item/tool/requestUserInput"
            })),
            serde_json::json!({ "answers": {} })
        );
        assert_eq!(
            decline_input_result(&serde_json::json!({
                "method": "mcpServer/elicitation/request"
            })),
            serde_json::json!({ "action": "decline" })
        );

        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let runtime = Arc::new(SessionRuntime::new());
        runtime.stream.lock().unwrap().screen = None;
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach");
        conn.track_with_agent_replay(
            "s.codex.notice",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let mut reader = CodexReader {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            deferred: Vec::new(),
            manifest: None,
            state: Arc::new(CodexState::new(
                "thread".to_string(),
                catalog_from_response(&serde_json::json!({
                    "data": [{ "id": "model", "isDefault": true }]
                }))
                .expect("catalog"),
                "auto",
            )),
            view: CodexView::new(None),
            permission_broker: Arc::clone(&broker),
            response_ids: Arc::new(Mutex::new(HashMap::new())),
            stdin: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(1)),
            requests: Arc::new(CodexRequests::new()),
        };
        reader.dispatch_value(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "server-1",
                "method": "item/tool/requestUserInput",
                "params": {}
            }),
            &runtime,
        );
        let events = conn.pull_events();
        assert!(events.iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::SessionNotice { ref text, severity }
                if text == "Codex requested user input; Devboule declined it."
                    && severity == devboule_protocol::NoticeSeverity::Info
        )));
        assert!(!events
            .iter()
            .any(|event| matches!(event.envelope.event, SessionEvent::AgentMessage { .. })));
    }

    // --- image delivery (the static route) --------------------------------
    //
    // The routing decision lives in `plan_codex_prompt`, tested here
    // against the attachment store directly, without spawning a child — the
    // same arrangement the ACP sibling seam's tests use. The wire shape of
    // one entry is pinned against the live-verified `localImage` input:
    // `{"type":"localImage","path":...}` with no `detail`.

    struct PlanTempDir(std::path::PathBuf);

    impl PlanTempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let dir = std::env::temp_dir().join(format!(
                "devboule-codex-plan-{}-{}-{}",
                std::process::id(),
                tag,
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for PlanTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn plan_attachment(name: &str, mime_type: &str, bytes: &[u8]) -> PromptAttachment {
        use base64::Engine;
        PromptAttachment {
            name: name.to_string(),
            mime_type: mime_type.to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    #[test]
    fn codex_delivery_is_the_static_variant() {
        // No handshake to negotiate with and deliberately no probe: an
        // unknown method on this surface answers `-32600`, not `-32601`, so
        // a method-not-found fallback would never fire. The format accepts
        // images, so the delivery is the static one the route reads.
        assert_eq!(
            codex_delivery(),
            super::super::ImageDelivery::StaticImageBlock
        );
    }

    #[test]
    fn a_capable_codex_prompt_plans_local_image_paths_and_no_path_line() {
        // Each raster becomes one `localImage` path; the text is the bare
        // user text, with no path line.
        let temp = PlanTempDir::new("capable");
        let store = AttachmentStore::new(&temp.0);
        let session_id = "codex-plan-capable";
        // A container the walk accepts but changes: what sits at the planned
        // path must be the stripped bytes, never the wire bytes.
        let sent = png_with_text_chunk();
        let kept = clean_png(0x01);
        assert_ne!(
            sent, kept,
            "the fixture must actually carry something that leaves"
        );
        let plan = plan_codex_prompt(
            &store,
            session_id,
            "describe this",
            &[plan_attachment("photo.png", "image/png", &sent)],
        )
        .expect("materialized")
        .expect("a raster plans paths");
        assert_eq!(plan.fallback_text, "describe this", "no path line");
        assert_eq!(carried_image_paths(Some(&plan)).len(), 1);
        let planned = carried_image_paths(Some(&plan))[0];
        assert_eq!(
            std::fs::read(planned).expect("read"),
            kept,
            "the path names the stripped file"
        );
        let params = turn_start_params_with_images(
            "thread-1",
            &plan.fallback_text,
            &plan.image_paths,
            None,
            None,
            None,
        );
        assert_eq!(params["threadId"], "thread-1");
        let input = params["input"].as_array().expect("input array");
        assert_eq!(input.len(), 2);
        assert_eq!(
            input[0],
            serde_json::json!({"type": "text", "text": "describe this"})
        );
        // The exact entry shape, pinned literally: `type` plus `path`, no
        // `detail`, and `localImage` — not the unmeasured `image` the schema
        // also lists.
        assert_eq!(input[1]["type"], "localImage");
        assert_eq!(
            input[1]["path"].as_str().expect("path"),
            planned.to_string_lossy(),
            "the entry names the materialized file"
        );
        assert!(input[1].get("detail").is_none(), "no unmeasured detail");
        let entry = codex_local_image_entry(planned);
        assert_eq!(entry["type"], "localImage");
        assert!(entry.get("detail").is_none());
    }

    #[test]
    fn an_svg_only_codex_prompt_plans_no_paths_and_still_builds_the_legacy_text() {
        // SVG takes no inline shape on this surface. The plan still answers
        // with the text, and that text is exactly what the legacy write would
        // have produced, which is why a prompt with no carried path can take
        // the route without moving a byte on the wire. (This test used to
        // assert `plan.is_none()`: the route answers with the text now, so
        // that the send path never walks the attachments twice.)
        let temp = PlanTempDir::new("svg-only");
        let store = AttachmentStore::new(&temp.0);
        let session_id = "codex-plan-svg-only";
        let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let attachment = plan_attachment("drawing.svg", "image/svg+xml", source);
        let plan = plan_codex_prompt(
            &store,
            session_id,
            "logo",
            std::slice::from_ref(&attachment),
        )
        .expect("materialized")
        .expect("an SVG plans no path, but the plan still carries the text");
        assert!(plan.image_paths.is_empty(), "an SVG plans no path");
        let stored = store
            .session(session_id)
            .expect("session")
            .materialize(&attachment)
            .expect("stored");
        assert_eq!(
            plan.fallback_text,
            format!("logo\n\n[Image available at: {}]", stored.to_string_lossy()),
            "the plan's text is the legacy path line, byte for byte"
        );
    }

    #[test]
    fn the_static_route_builds_the_turn_the_plan_decided() {
        // The route's frame is the measured `turn/start`: one text entry, then
        // one `localImage` per carried path, and nothing else moved — the
        // thread id and the model still come from the live state.
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let state = CodexState::new("thread".to_string(), catalog, "auto");
        let temp = PlanTempDir::new("route");
        let store = AttachmentStore::new(&temp.0);
        let plan = plan_codex_prompt(
            &store,
            "codex-route",
            "describe this",
            &[plan_attachment("photo.png", "image/png", &clean_png(0x51))],
        )
        .expect("materialized")
        .expect("a raster plans a path");
        assert_eq!(plan.image_paths.len(), 1);
        let carried = turn_start_params_for_prompt(&state, &plan.fallback_text, &plan.image_paths);
        assert_eq!(carried["threadId"], "thread");
        let input = carried["input"].as_array().expect("input array");
        assert_eq!(input.len(), 2, "the text entry, then the path");
        assert_eq!(
            input[0],
            serde_json::json!({ "type": "text", "text": "describe this" })
        );
        assert_eq!(input[1]["type"], "localImage");
        // No carried path: the text-only turn, one entry.
        let bare = turn_start_params_for_prompt(&state, "describe this", &[]);
        assert_eq!(bare["input"].as_array().expect("input array").len(), 1);
    }

    #[test]
    fn a_params_builder_without_images_is_the_text_only_one() {
        // The static route sends every planned prompt through the images
        // builder, including one whose paths are all path lines. With no
        // carried path it has to be the text-only `turn/start` this surface
        // has always sent: same keys, same values, same order.
        for (policy_mode, model, effort) in [
            (None, None, None),
            (Some("workspace-write"), Some("gpt-5-codex"), Some("high")),
        ] {
            assert_eq!(
                turn_start_params_with_images(
                    "thread-1",
                    "describe this",
                    &[],
                    policy_mode,
                    model,
                    effort,
                ),
                turn_start_params("thread-1", "describe this", policy_mode, model, effort),
                "no carried path must not move a key"
            );
        }
    }

    #[test]
    fn codex_turn_steer_frame_is_byte_exact() {
        let frame = request_frame(
            "d-7",
            "turn/steer",
            turn_steer_params("thread-1", "turn-2", "hello"),
        );
        assert_eq!(
            serde_json::to_vec(&frame).expect("frame"),
            br#"{"jsonrpc":"2.0","id":"d-7","method":"turn/steer","params":{"threadId":"thread-1","expectedTurnId":"turn-2","input":[{"type":"text","text":"hello"}]}}"#
        );
    }

    /// A `CodexState` on `thread-1` whose running turn is `turn_id`.
    fn state_on_turn(turn_id: &str) -> Arc<CodexState> {
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [{ "id": "model", "isDefault": true }]
        }))
        .expect("catalog");
        let state = CodexState::new("thread-1".to_string(), catalog, "auto");
        state.set_turn(Some(turn_id.to_string()));
        Arc::new(state)
    }

    #[test]
    fn a_codex_steer_carries_the_turn_it_was_checked_for() {
        // The frame names the turn the caller captured as its precondition, so
        // Codex itself refuses a steer aimed at a turn that is over.
        let state = state_on_turn("turn-3");
        let params = steer_params_if_current(&state, "turn-3", "turn left")
            .expect("the captured turn is the current one");
        assert_eq!(params["threadId"], "thread-1");
        assert_eq!(params["expectedTurnId"], "turn-3");
        assert_eq!(params["input"][0]["text"], "turn left");
    }

    #[test]
    fn a_codex_steer_for_a_turn_that_has_moved_on_is_never_written() {
        // The write-time check, not the caller's earlier one: the state says
        // Codex is on `turn-4` while the steer was admitted for `turn-3`, and
        // nothing is written at all. The stdin here is absent, so a write
        // attempt would answer `Err` instead of `Ok(false)` — what this pins is
        // that the frame is never built, and no answer is ever waited for.
        let state = state_on_turn("turn-4");
        assert!(steer_params_if_current(&state, "turn-3", "turn left").is_none());
        let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
        let steerer = CodexSteerer {
            stdin,
            next_id: Arc::new(AtomicU64::new(1)),
            state,
            requests: Arc::new(CodexRequests::new()),
        };
        assert!(matches!(
            steerer.begin_steer("turn-3", "turn left"),
            Ok(None)
        ));
    }

    /// S4-04: the app-server's output ending fails every waiter still registered,
    /// so a steer answers `Err` — the fate is unknown — instead of waiting out
    /// the whole fifteen-second timeout for an answer that cannot come.
    #[test]
    fn a_codex_steer_is_not_left_waiting_when_the_app_server_ends() {
        use std::io::BufReader;
        use std::process::Stdio;

        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        // A fake Codex that reads one request and exits without answering it.
        let mut child = std::process::Command::new("node")
            .args(["-e", "process.stdin.once('data', () => process.exit(0))"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("node is required for the Codex end-of-transport test");
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let stdout = child.stdout.take().expect("stdout");
        let requests = Arc::new(CodexRequests::new());
        let mut steerer = CodexSteerer {
            stdin,
            next_id: Arc::new(AtomicU64::new(7)),
            state: state_on_turn("turn-3"),
            requests: Arc::clone(&requests),
        };
        let mut reader = CodexReader {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            deferred: Vec::new(),
            manifest: None,
            state: state_on_turn("turn-3"),
            view: CodexView::new(None),
            permission_broker: PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
            response_ids: Arc::new(Mutex::new(HashMap::new())),
            stdin: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(1)),
            requests: Arc::clone(&requests),
        };
        // The reader runs the real end-of-transport path: read to EOF, then
        // `finish`, which is where the waiters are failed.
        let runtime = Arc::new(SessionRuntime::new());
        let reader_runtime = Arc::clone(&runtime);
        let reader_thread = std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            let mut bytes = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stdout, &mut bytes);
            reader.finish(&reader_runtime);
        });

        let started = Instant::now();
        let answer = crate::test_support::steer_through_the_turn(&mut steerer, "turn left");
        let error = match answer {
            Some(Err(error)) => error,
            other => panic!("the transport is over: expected an error, got {other:?}"),
        };
        assert!(
            error.message.contains("control channel closed"),
            "the failure is the transport ending, not a timeout: {}",
            error.message
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the waiter was failed by the EOF, not by its own fifteen-second timeout"
        );
        let _ = child.wait();
        reader_thread.join().expect("the reader thread");
    }

    /// Spawn a fake Codex that echoes each line it reads back, so a test can
    /// read the frame the client wrote. The caller must have checked `node`
    /// first (`external_program_skip_reason`).
    fn spawn_codex_echoing() -> std::process::Child {
        std::process::Command::new("node")
            .args([
                "-e",
                "process.stdin.on('data', data => process.stdout.write(data))",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("node is required for the Codex steer tests")
    }

    /// Spawn a fake Codex that answers each `turn/steer` with `body`, whose `id`
    /// it fills with the request's own — the app-server's shape, so the test
    /// exercises the real id-correlated delivery. The caller must have checked
    /// `node` first (`external_program_skip_reason`).
    fn spawn_codex_answering(body: &str) -> std::process::Child {
        let script = r#"
let buf = '';
process.stdin.on('data', data => {
  buf += data;
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i);
    buf = buf.slice(i + 1);
    let request;
    try { request = JSON.parse(line); } catch (error) { continue; }
    if (request.method === 'turn/steer') {
      const answer = __ANSWER__;
      answer.id = request.id;
      process.stdout.write(JSON.stringify(answer) + '\n');
    }
  }
});
"#
        .replace("__ANSWER__", body);
        std::process::Command::new("node")
            .args(["-e", &script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("node is required for the Codex steer tests")
    }

    /// One steer against a fake Codex that answers `body`: the child, the real
    /// delivery (`CodexRequests::deliver`, which is what the reader calls) and
    /// the steer all run, so the answer the steerer returns is the one the
    /// response produced.
    fn codex_steer_against(body: &str) -> Result<bool, WireError> {
        let requests = Arc::new(CodexRequests::new());
        let mut child = spawn_codex_answering(body);
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let stdout = child.stdout.take().expect("stdout");
        let delivered = Arc::clone(&requests);
        let reader = std::thread::spawn(move || {
            use std::io::BufRead;
            let mut stdout = std::io::BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match stdout.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let value: serde_json::Value =
                    serde_json::from_str(line.trim()).expect("the answer the fake Codex wrote");
                assert!(
                    delivered.deliver(&value),
                    "the answer named an id no waiter registered"
                );
            }
        });
        let mut steerer = CodexSteerer {
            stdin,
            next_id: Arc::new(AtomicU64::new(7)),
            state: state_on_turn("turn-3"),
            requests,
        };
        let answer = crate::test_support::steer_through_the_turn(&mut steerer, "turn left");
        let _ = child.kill();
        let _ = child.wait();
        reader.join().expect("the fake Codex reader");
        answer.expect("the turn was running at admission")
    }

    #[test]
    fn a_codex_steer_is_accepted_only_by_a_response_for_the_steered_turn() {
        // A2-03: the write is a request, not a decision. The acceptance is the
        // app-server's own response, and only one that names the turn the steer
        // was written into counts as one.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        assert!(
            matches!(
                codex_steer_against(r#"{"jsonrpc":"2.0","result":{"turn":{"id":"turn-3"}}}"#),
                Ok(true)
            ),
            "the response naming the steered turn is the acceptance"
        );
    }

    #[test]
    fn a_codex_steer_response_for_another_turn_is_a_refusal_not_an_acceptance() {
        // The response says the app-server took a steer — for a *different*
        // turn. Answering `Ok(true)` here would tell the caller its text landed
        // in the turn it was admitted for when the provider said otherwise.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        assert!(matches!(
            codex_steer_against(r#"{"jsonrpc":"2.0","result":{"turn":{"id":"turn-9"}}}"#),
            Ok(false)
        ));
    }

    #[test]
    fn a_codex_steer_answered_by_an_error_or_no_turn_is_a_refusal() {
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        assert!(
            matches!(
                codex_steer_against(
                    r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"steer refused"}}"#
                ),
                Ok(false)
            ),
            "an error result is not a steer"
        );
        assert!(
            matches!(
                codex_steer_against(r#"{"jsonrpc":"2.0","result":{}}"#),
                Ok(false)
            ),
            "a response that names no turn is not a steer for this one"
        );
    }

    #[test]
    fn a_codex_steer_that_is_still_current_is_written_with_its_precondition() {
        // The frame itself, read back from the child: the request names the turn
        // the caller captured as its precondition, so Codex refuses a steer
        // aimed at a turn that is over. The answer is not read here — this pins
        // the bytes, and `CodexRequests` is what turns them into a decision.
        if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
            eprintln!("{reason}");
            return;
        }
        use std::io::{BufRead, BufReader};

        let mut child = spawn_codex_echoing();
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let steerer = CodexSteerer {
            stdin,
            next_id: Arc::new(AtomicU64::new(7)),
            state: state_on_turn("turn-3"),
            requests: Arc::new(CodexRequests::new()),
        };
        let request = steerer
            .begin_steer("turn-3", "turn left")
            .expect("the frame was written");
        assert!(
            request.is_some(),
            "the current turn is the one the steer is written for"
        );
        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .expect("the frame the child read");
        let frame: serde_json::Value = serde_json::from_str(&line).expect("frame json");
        assert_eq!(frame["jsonrpc"], "2.0");
        assert_eq!(frame["method"], "turn/steer");
        assert_eq!(frame["params"]["expectedTurnId"], "turn-3");
        assert_eq!(frame["params"]["input"][0]["text"], "turn left");
        assert_eq!(
            frame["id"], "d-7",
            "the request id is the one its answer will name"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn an_svg_keeps_its_path_line_beside_codex_image_paths() {
        // A mixed prompt carries both: the raster as a `localImage` path,
        // the SVG as a path line in the text.
        let temp = PlanTempDir::new("mixed");
        let store = AttachmentStore::new(&temp.0);
        let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let plan = plan_codex_prompt(
            &store,
            "codex-plan-mixed",
            "logo and photo",
            &[
                plan_attachment("photo.png", "image/png", &clean_png(0x13)),
                plan_attachment("drawing.svg", "image/svg+xml", source),
            ],
        )
        .expect("materialized")
        .expect("the raster plans a path");
        assert_eq!(carried_image_paths(Some(&plan)).len(), 1);
        assert!(
            plan.fallback_text
                .starts_with("logo and photo\n\n[Image available at: "),
            "{}",
            plan.fallback_text
        );
        assert!(
            plan.fallback_text.ends_with(".svg]"),
            "{}",
            plan.fallback_text
        );
        assert!(
            !plan.fallback_text.contains(".png]"),
            "the raster left no path line: {}",
            plan.fallback_text
        );
        let params = turn_start_params_with_images(
            "thread-1",
            &plan.fallback_text,
            &plan.image_paths,
            None,
            None,
            None,
        );
        let input = params["input"].as_array().expect("array");
        assert_eq!(input.len(), 2);
        assert!(input[0]["text"].as_str().expect("text").ends_with(".svg]"));
        assert_eq!(input[1]["type"], "localImage");
    }

    #[test]
    fn a_jpeg_plans_its_stripped_path_on_codex() {
        const EXIF_JPEG_VECTOR: &str =
            "a jpeg whose APP1 holds EXIF, between a kept JFIF APP0 and a kept ICC APP2";
        let temp = PlanTempDir::new("jpeg");
        let store = AttachmentStore::new(&temp.0);
        let sent = vector_input(EXIF_JPEG_VECTOR);
        let kept = vector_output(EXIF_JPEG_VECTOR);
        assert_ne!(sent, kept, "the vector must actually strip something");
        let plan = plan_codex_prompt(
            &store,
            "codex-plan-jpeg",
            "describe this",
            &[plan_attachment("photo.jpg", "image/jpeg", &sent)],
        )
        .expect("materialized")
        .expect("a JPEG plans a path");
        assert_eq!(plan.fallback_text, "describe this");
        let planned = carried_image_paths(Some(&plan))[0];
        assert_eq!(
            std::fs::read(planned).expect("read"),
            kept,
            "stripped JPEG bytes at the planned path"
        );
    }
}
