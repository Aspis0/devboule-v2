//! Claude stream-json stdio adapter for live agent sessions.
//!
//! This module owns only the process and protocol adapters. The parent
//! session module still owns the runtime, attachment queue, journal,
//! liveness monitor, registry and teardown order. Permissions go through
//! the existing [`super::permission_broker::PermissionBroker`]; this file does not
//! fork it.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use devboule_protocol::{ErrorCode, PermissionOption, SessionEvent, WireError};
use serde_json::Value;

use super::permission_broker::{PermissionBroker, PermissionSender};
use super::PtyCommand;
use super::{
    write_child_stdin, ModelSwitcher, ReaderDispatch, SessionKiller, SessionRuntime,
    SpawnedSession, StderrSource, StdioWaitableChild,
};
use crate::claude_view::ClaudeView;
use crate::mcp_broker::McpLaunchConfig;
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::server::ServerState;

const COMMAND_ENV: &str = "DEVBOULE_CLAUDE_COMMAND";
const MAX_LINE_BYTES: usize = 10 * 1024 * 1024;
const CONTROL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

type ClaudeModeResponses = Arc<Mutex<HashMap<String, Sender<Result<(), String>>>>>;

struct ClaudePendingControl {
    request_id: String,
    input: Value,
}

enum ClaudeModeGateState {
    AwaitingResponse {
        request_id: String,
        requested_mode: String,
    },
    Ready,
    Failed(String),
}

struct ClaudeModeGate {
    state: ClaudeModeGateState,
    pending_frames: Vec<Vec<u8>>,
}

type ClaudeModeGateRef = Arc<Mutex<ClaudeModeGate>>;

/// Launch-time mode gate wiring: the stdin the gate writes through, the gate
/// itself, and the response deadline.
struct ClaudeModeGateWiring {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    gate: ClaudeModeGateRef,
    timeout: Duration,
}

impl ClaudeModeGateWiring {
    fn new(stdin: Arc<Mutex<Option<ChildStdin>>>, gate: ClaudeModeGateRef) -> Self {
        Self {
            stdin,
            gate,
            timeout: CONTROL_RESPONSE_TIMEOUT,
        }
    }
}

/// Resolve `claude` plus the measured stream-json launch args. Honors
/// `DEVBOULE_CLAUDE_COMMAND` as a JSON string array, matching the ACP override.
pub(super) fn resolve_command(_paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine agent working directory: {error}"),
        )
    })?;
    let mut argv: Vec<String> = match std::env::var(COMMAND_ENV) {
        Ok(argv) => serde_json::from_str(&argv).map_err(|error| {
            WireError::new(
                ErrorCode::InvalidRequest,
                format!("{COMMAND_ENV} must be a non-empty JSON string array: {error}"),
            )
        })?,
        Err(_) => {
            let Some(agent) = crate::provider_catalog::find_available("claude") else {
                return Err(WireError::new(
                    ErrorCode::Io,
                    format!(
                        "Claude was not found on PATH. Set {COMMAND_ENV} to a non-empty JSON string array to choose a command explicitly."
                    ),
                ));
            };
            let Some(stream_json) = agent.stream_json_command else {
                return Err(WireError::new(
                    ErrorCode::Io,
                    "Claude is installed but has no stream-json launch args.",
                ));
            };
            stream_json
        }
    };
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("{COMMAND_ENV} must contain an executable."),
        ));
    }
    let program = argv.remove(0);
    Ok(PtyCommand::new(program, argv, cwd, Vec::new()).with_provider_id("claude"))
}

fn launch_in_bypass_mode(argv: Vec<String>) -> Vec<String> {
    let mut args = strip_flag(argv, "--permission-mode");
    args.extend([
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
    ]);
    args
}

/// The CLI runs its own default model when no `--model` is passed, so the tab
/// would name a model the process is not running. Pin the catalog's current
/// model at launch; a later set_model control request still overrides it.
fn launch_with_model(argv: Vec<String>, model_id: Option<&str>) -> Vec<String> {
    let mut args = strip_flag(argv, "--model");
    if let Some(model_id) = model_id {
        args.extend(["--model".to_string(), model_id.to_string()]);
    }
    args
}

fn strip_flag(argv: Vec<String>, flag: &str) -> Vec<String> {
    let with_equals = format!("{flag}=");
    let mut args = Vec::with_capacity(argv.len());
    let mut skip_value = false;
    for arg in argv {
        if skip_value {
            skip_value = false;
            continue;
        }
        if arg == flag {
            skip_value = true;
            continue;
        }
        if arg.starts_with(&with_equals) {
            continue;
        }
        args.push(arg);
    }
    args
}

pub(super) fn spawn_process(
    state: &Arc<ServerState>,
    command: PtyCommand,
    mcp: Option<McpLaunchConfig>,
    requested_mode: Option<String>,
) -> Result<SpawnedSession, WireError> {
    let requested_mode = requested_mode.unwrap_or_else(|| "default".to_string());
    if !crate::claude_view::mode_state(&requested_mode)
        .available_modes
        .iter()
        .any(|mode| mode.id == requested_mode)
    {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("Claude session mode '{requested_mode}' is not available."),
        ));
    }
    let mut args = command.args.clone();
    if let Some(path) = mcp
        .as_ref()
        .and_then(|config| config.claude_config_path.as_ref())
    {
        args.push("--mcp-config".to_string());
        args.push(path.to_string_lossy().into_owned());
        if !args.iter().any(|arg| arg == "--strict-mcp-config") {
            args.push("--strict-mcp-config".to_string());
        }
    }
    let args = launch_with_model(
        launch_in_bypass_mode(args),
        crate::claude_catalog::default_model_id(&state.claude_models().models).as_deref(),
    );
    let mut process = Command::new(&command.program);
    process
        .args(&args)
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
            format!("Could not start Claude {}: {error}", command.program),
        )
    })?;

    #[cfg(windows)]
    let (process_job, os_handle) = {
        use std::os::windows::io::AsRawHandle;
        let process_job = JobObject::new().map_err(|error| {
            terminate_process(&mut child);
            WireError::new(
                ErrorCode::Io,
                format!("Could not create the Claude process job: {error}"),
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
                format!("Could not contain the Claude process: {error}"),
            ));
        }
        let os_handle = match ProcessHandle::duplicate(handle) {
            Ok(duplicated) => Some(duplicated),
            Err(error) => {
                eprintln!("could not duplicate Claude process handle for OS liveness: {error}");
                None
            }
        };
        (process_job, os_handle)
    };

    #[cfg(not(windows))]
    let process_job = JobObject::new().map_err(|error| {
        terminate_process(&mut child);
        WireError::new(
            ErrorCode::Io,
            format!("Could not create the Claude process job: {error}"),
        )
    })?;
    #[cfg(not(windows))]
    let os_handle = None;

    let stdin = child.stdin.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Claude did not provide stdin.")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Claude did not provide stdout.")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "Claude did not provide stderr.")
    })?;

    let process = Arc::new(Mutex::new(child));
    let stdin = Arc::new(Mutex::new(Some(stdin)));
    let controls = Arc::new(Mutex::new(HashMap::new()));
    let mode_responses = Arc::new(Mutex::new(HashMap::new()));
    let next_id = Arc::new(AtomicU64::new(1));
    let mode_gate = match start_initial_mode(&stdin, &next_id, requested_mode.clone()) {
        Ok(mode_gate) => mode_gate,
        Err(error) => {
            if let Ok(mut process) = process.lock() {
                terminate_process(&mut process);
            }
            drop(process_job);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not send initial Claude mode request: {error}"),
            ));
        }
    };
    let sender = claude_permission_sender(Arc::clone(&stdin), Arc::clone(&controls));
    let permission_broker = PermissionBroker::with_sender(sender);
    let stderr_source = match ClaudeStderr::start(stderr) {
        Ok(source) => source,
        Err(error) => {
            if let Ok(mut process) = process.lock() {
                terminate_process(&mut process);
            }
            drop(process_job);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not drain Claude stderr: {error}"),
            ));
        }
    };
    let writer = ClaudeWriter {
        stdin: Arc::clone(&stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&mode_gate)),
    };
    let killer = ClaudeKiller {
        process: Arc::clone(&process),
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        permission_broker: Arc::clone(&permission_broker),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    let reader_dispatch = ClaudeReader::with_mode_gate(
        ClaudeView::new(Some(command.cwd.clone())),
        Arc::clone(&permission_broker),
        Arc::clone(&controls),
        Arc::clone(&mode_responses),
        Arc::clone(&next_id),
        ClaudeModeGateWiring::new(Arc::clone(&stdin), mode_gate),
    );
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        switcher: Some(Box::new(ClaudeSwitcher {
            stdin: Arc::clone(&stdin),
            next_id: Arc::clone(&next_id),
            mode_responses,
        })),
        child: Box::new(StdioWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
        reader: Box::new(BufReader::new(stdout)),
        reader_dispatch: Some(Box::new(reader_dispatch)),
        stderr: Some(Box::new(stderr_source)),
        permission_broker: Some(permission_broker),
        os_handle,
        peer_session_id: None,
        agent_version: None,
    })
}

fn terminate_process(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

fn claude_permission_sender(
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
) -> Arc<PermissionSender> {
    Arc::new(move |id, result| {
        let frame = {
            let controls = controls
                .lock()
                .map_err(|_| io::Error::other("Claude permission map lock poisoned"))?;
            let pending = controls.get(&id);
            let Some(pending) = pending else {
                return Err(io::Error::other(
                    "Claude permission response had no matching control request",
                ));
            };
            control_response_frame(&pending.request_id, &pending.input, &result)
        };
        let mut bytes = serde_json::to_vec(&frame)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        let result = write_child_stdin(&stdin, &bytes, "Claude");
        if result.is_ok() {
            controls
                .lock()
                .map_err(|_| io::Error::other("Claude permission map lock poisoned"))?
                .remove(&id);
        }
        result
    })
}

fn control_response_frame(request_id: &str, input: &Value, result: &Value) -> Value {
    let outcome = result
        .pointer("/outcome/outcome")
        .and_then(Value::as_str)
        .unwrap_or("");
    let option_id = result
        .pointer("/outcome/optionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let response = if outcome == "selected" && option_id == "allow" {
        serde_json::json!({
            "behavior": "allow",
            "updatedInput": input,
        })
    } else {
        let message = match outcome {
            "cancelled" => "The permission request was cancelled.",
            _ => "The user declined this command.",
        };
        serde_json::json!({
            "behavior": "deny",
            "message": message,
        })
    };
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": response,
        }
    })
}

fn frame_user_message(text: &str) -> io::Result<Vec<u8>> {
    let frame = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": text}]
        }
    });
    let mut bytes = serde_json::to_vec(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    Ok(bytes)
}

struct ClaudeWriter {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: Vec<u8>,
    mode_gate: Option<ClaudeModeGateRef>,
}

impl Write for ClaudeWriter {
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
        let bytes = frame_user_message(&text)?;
        if let Some(mode_gate) = &self.mode_gate {
            let mut gate = mode_gate
                .lock()
                .map_err(|_| io::Error::other("Claude mode gate lock poisoned"))?;
            match &gate.state {
                ClaudeModeGateState::Ready => {}
                ClaudeModeGateState::Failed(message) => {
                    return Err(io::Error::other(message.clone()));
                }
                ClaudeModeGateState::AwaitingResponse { .. } => {
                    gate.pending_frames.push(bytes);
                    return Ok(());
                }
            }
        }
        write_child_stdin(&self.stdin, &bytes, "Claude")
    }
}

struct ClaudeKiller {
    process: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    permission_broker: Arc<PermissionBroker>,
    cancelled: Arc<AtomicBool>,
}

struct ClaudeSwitcher {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    mode_responses: ClaudeModeResponses,
}

/// Build a Claude control_request frame.
fn interrupt_frame_bytes(request_id: &str) -> Option<Vec<u8>> {
    control_request_frame_bytes(request_id, serde_json::json!({"subtype": "interrupt"}))
}

fn control_request_frame_bytes(request_id: &str, request: Value) -> Option<Vec<u8>> {
    let frame = serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": request,
    });
    let mut bytes = serde_json::to_vec(&frame).ok()?;
    bytes.push(b'\n');
    Some(bytes)
}

fn start_initial_mode(
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    next_id: &AtomicU64,
    requested_mode: String,
) -> io::Result<ClaudeModeGateRef> {
    let request_id = format!(
        "initial-permission-mode-{}",
        next_id.fetch_add(1, Ordering::Relaxed)
    );
    let mode_gate = Arc::new(Mutex::new(ClaudeModeGate {
        state: ClaudeModeGateState::AwaitingResponse {
            request_id: request_id.clone(),
            requested_mode: requested_mode.clone(),
        },
        pending_frames: Vec::new(),
    }));
    let bytes = control_request_frame_bytes(
        &request_id,
        serde_json::json!({
            "subtype": "set_permission_mode",
            "mode": requested_mode,
        }),
    )
    .ok_or_else(|| io::Error::other("Could not encode Claude mode request."))?;
    if let Err(error) = write_child_stdin(stdin, &bytes, "Claude") {
        if let Ok(mut gate) = mode_gate.lock() {
            gate.state = ClaudeModeGateState::Failed(error.to_string());
        }
        return Err(error);
    }
    Ok(mode_gate)
}

/// Flush the prompts queued behind the gate and mark it Ready. The caller
/// holds the gate lock; a write failure leaves the gate awaiting so the
/// caller can fail it through the normal path.
fn flush_gate_frames(
    gate: &mut ClaudeModeGate,
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    view: &mut ClaudeView,
    mode_id: &str,
) -> Option<io::Error> {
    view.set_mode(mode_id);
    for bytes in std::mem::take(&mut gate.pending_frames) {
        if let Err(error) = write_child_stdin(stdin, &bytes, "Claude") {
            return Some(error);
        }
    }
    gate.state = ClaudeModeGateState::Ready;
    None
}

fn fail_initial_mode_parts(
    mode_gate: &ClaudeModeGateRef,
    stdin: Option<&Arc<Mutex<Option<ChildStdin>>>>,
    runtime: &Arc<SessionRuntime>,
    expected_request_id: Option<&str>,
    require_awaiting: bool,
    message: &str,
) -> bool {
    let should_publish = match mode_gate.lock() {
        Ok(mut gate) => {
            let can_fail = match &gate.state {
                ClaudeModeGateState::Failed(_) => false,
                ClaudeModeGateState::Ready => !require_awaiting,
                ClaudeModeGateState::AwaitingResponse { request_id, .. } => expected_request_id
                    .map(|expected| expected == request_id)
                    .unwrap_or(true),
            };
            if can_fail {
                drop(std::mem::take(&mut gate.pending_frames));
                gate.state = ClaudeModeGateState::Failed(message.to_string());
                true
            } else {
                false
            }
        }
        Err(_) => expected_request_id.is_none(),
    };
    if !should_publish {
        return false;
    }
    if let Some(stdin) = stdin {
        if let Ok(mut stdin) = stdin.lock() {
            *stdin = None;
        }
    }
    let _ = runtime.publish_agent_event(
        SessionEvent::AgentError {
            message: message.to_string(),
        },
        None,
    );
    true
}

/// The frame write happens on a spawned thread because a full stdin pipe
/// would otherwise block the caller holding the session lock path.
fn send_interrupt_frame(stdin: Arc<Mutex<Option<ChildStdin>>>, request_id: String) {
    let _ = std::thread::Builder::new()
        .name("claude-interrupt".to_string())
        .spawn(move || {
            if let Some(bytes) = interrupt_frame_bytes(&request_id) {
                let _ = write_child_stdin(&stdin, &bytes, "Claude");
            }
        });
}

fn send_control_request_frame(
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    request_id: String,
    request: Value,
) {
    let _ = std::thread::Builder::new()
        .name("claude-set-model".to_string())
        .spawn(move || {
            if let Some(bytes) = control_request_frame_bytes(&request_id, request) {
                let _ = write_child_stdin(&stdin, &bytes, "Claude");
            }
        });
}

impl ModelSwitcher for ClaudeSwitcher {
    fn set_model(&self, model_id: Option<&str>, effort: Option<&str>) -> Result<(), WireError> {
        if let Some(model_id) = model_id {
            let request_id = format!("set-model-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
            send_control_request_frame(
                Arc::clone(&self.stdin),
                request_id,
                serde_json::json!({
                    "subtype": "set_model",
                    "model": model_id,
                }),
            );
        }
        if let Some(effort) = effort {
            let request_id = format!(
                "set-effort-{}",
                self.next_id.fetch_add(1, Ordering::Relaxed)
            );
            send_control_request_frame(
                Arc::clone(&self.stdin),
                request_id,
                serde_json::json!({
                    "subtype": "apply_flag_settings",
                    "settings": {"effortLevel": effort},
                }),
            );
        }
        Ok(())
    }

    fn set_mode(&self, mode_id: &str) -> Result<(), WireError> {
        let request_id = format!(
            "set-permission-mode-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed)
        );
        let (sender, receiver) = mpsc::channel();
        self.mode_responses
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "Claude mode response map is unavailable."))?
            .insert(request_id.clone(), sender);
        let bytes = control_request_frame_bytes(
            &request_id,
            serde_json::json!({
                "subtype": "set_permission_mode",
                "mode": mode_id,
            }),
        )
        .ok_or_else(|| WireError::new(ErrorCode::Io, "Could not encode Claude mode request."))?;
        if let Err(error) = write_child_stdin(&self.stdin, &bytes, "Claude") {
            let _ = self
                .mode_responses
                .lock()
                .map(|mut responses| responses.remove(&request_id));
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not send Claude mode request: {error}"),
            ));
        }
        match receiver.recv_timeout(CONTROL_RESPONSE_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(WireError::new(ErrorCode::InvalidRequest, message)),
            Err(error) => {
                let _ = self
                    .mode_responses
                    .lock()
                    .map(|mut responses| responses.remove(&request_id));
                Err(WireError::new(
                    ErrorCode::Io,
                    format!("Claude mode response timed out: {error}"),
                ))
            }
        }
    }

    fn clone_switcher(&self) -> Box<dyn ModelSwitcher> {
        Box::new(Self {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            mode_responses: Arc::clone(&self.mode_responses),
        })
    }
}

impl SessionKiller for ClaudeKiller {
    /// Soft interrupt: ask the CLI to abort the current turn. The process,
    /// stdin, and the kill guard stay untouched so later turns keep working.
    fn interrupt(&mut self) {
        // A kill already closed stdin and drained the broker; a late
        // interrupt would only spawn a thread doomed to BrokenPipe.
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let request_id = format!("interrupt-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        send_interrupt_frame(Arc::clone(&self.stdin), request_id);
        self.permission_broker.cancel_pending();
    }

    fn kill(&mut self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            let request_id = format!("interrupt-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
            send_interrupt_frame(Arc::clone(&self.stdin), request_id);
            self.permission_broker.close();
        }
        if let Ok(mut process) = self.process.lock() {
            let _ = process.kill();
        }
        if let Ok(mut stdin) = self.stdin.lock() {
            *stdin = None;
        }
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            process: Arc::clone(&self.process),
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
            permission_broker: Arc::clone(&self.permission_broker),
            cancelled: Arc::clone(&self.cancelled),
        })
    }
}

struct ClaudeReader {
    buffer: Vec<u8>,
    discarding_oversized_line: bool,
    view: ClaudeView,
    permission_broker: Arc<PermissionBroker>,
    controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
    mode_responses: ClaudeModeResponses,
    next_id: Arc<AtomicU64>,
    stdin: Option<Arc<Mutex<Option<ChildStdin>>>>,
    mode_gate: Option<ClaudeModeGateRef>,
    initial_mode_timeout: Duration,
    initial_mode_timer_started: bool,
    initial_mode_timer_cancel: Option<Sender<()>>,
    initial_mode_timer_thread: Option<JoinHandle<()>>,
}

fn observe_mcp_status(value: &Value, runtime: &SessionRuntime) {
    if value.get("type").and_then(Value::as_str) != Some("system")
        || value.get("subtype").and_then(Value::as_str) != Some("init")
    {
        return;
    }
    let Some(server) = value
        .get("mcp_servers")
        .and_then(Value::as_array)
        .and_then(|servers| {
            servers.iter().find(|server| {
                server.get("name").and_then(Value::as_str)
                    == Some(crate::mcp_broker::MCP_SERVER_NAME)
            })
        })
    else {
        return;
    };
    match server.get("status").and_then(Value::as_str) {
        Some("connected") => {
            // The provider frame is only a hint. The broker owns readiness
            // once it has authenticated and served tools/list.
        }
        Some("failed") | Some("error") => {
            runtime.fail_mcp("Claude reported that the MCP broker failed.");
        }
        _ => {}
    }
}

impl ClaudeReader {
    fn new(
        view: ClaudeView,
        permission_broker: Arc<PermissionBroker>,
        controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
        mode_responses: ClaudeModeResponses,
        next_id: Arc<AtomicU64>,
    ) -> Self {
        Self {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            view,
            permission_broker,
            controls,
            mode_responses,
            next_id,
            stdin: None,
            mode_gate: None,
            initial_mode_timeout: CONTROL_RESPONSE_TIMEOUT,
            initial_mode_timer_started: false,
            initial_mode_timer_cancel: None,
            initial_mode_timer_thread: None,
        }
    }

    fn with_mode_gate(
        view: ClaudeView,
        permission_broker: Arc<PermissionBroker>,
        controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
        mode_responses: ClaudeModeResponses,
        next_id: Arc<AtomicU64>,
        wiring: ClaudeModeGateWiring,
    ) -> Self {
        let mut reader = Self::new(view, permission_broker, controls, mode_responses, next_id);
        reader.stdin = Some(wiring.stdin);
        reader.mode_gate = Some(wiring.gate);
        reader.initial_mode_timeout = wiring.timeout;
        reader
    }

    fn publish(&self, runtime: &SessionRuntime, event: SessionEvent) {
        self.publish_with_seq(runtime, event, None);
    }

    fn publish_with_seq(
        &self,
        runtime: &SessionRuntime,
        event: SessionEvent,
        event_seq: Option<u64>,
    ) {
        let _ = runtime.publish_agent_event_with_seq(event, None, event_seq);
    }

    fn dispatch_line(&mut self, line: &str, runtime: &Arc<SessionRuntime>) {
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(error) => {
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!("Malformed Claude output was skipped: {error}"),
                    },
                );
                return;
            }
        };
        let value = runtime.redact_mcp_value(&value);
        observe_mcp_status(&value, runtime);
        let event_seq = runtime.journal_agent_envelope(&value);
        if self.dispatch_control_response(&value, runtime) {
            return;
        }
        if is_can_use_tool(&value) {
            self.dispatch_permission(&value, runtime, event_seq);
            return;
        }
        for event in self.view.ingest(&value) {
            let event = if matches!(&event, SessionEvent::SessionManifest { .. }) {
                let event = self.manifest_with_requested_mode(event);
                let event = runtime.store_session_manifest(event);
                if let Some(session_id) = self.view.peer_session_id() {
                    runtime.set_peer_session_id(session_id.to_string());
                }
                event
            } else {
                event
            };
            self.publish_with_seq(runtime, event, event_seq);
        }
    }

    fn manifest_with_requested_mode(&self, event: SessionEvent) -> SessionEvent {
        let Some(mode_gate) = &self.mode_gate else {
            return event;
        };
        let requested_mode = mode_gate.lock().ok().and_then(|gate| match &gate.state {
            ClaudeModeGateState::AwaitingResponse { requested_mode, .. } => {
                Some(requested_mode.clone())
            }
            ClaudeModeGateState::Ready | ClaudeModeGateState::Failed(_) => None,
        });
        let Some(requested_mode) = requested_mode else {
            return event;
        };
        match event {
            SessionEvent::SessionManifest {
                provider_id,
                current_model_id,
                models,
                mut modes,
            } => {
                if let Some(modes) = &mut modes {
                    modes.current_mode_id = requested_mode;
                }
                SessionEvent::SessionManifest {
                    provider_id,
                    current_model_id,
                    models,
                    modes,
                }
            }
            event => event,
        }
    }

    fn start_initial_mode_timeout(&mut self, runtime: &Arc<SessionRuntime>) {
        if self.initial_mode_timer_started {
            return;
        }
        self.initial_mode_timer_started = true;
        let Some(mode_gate) = self.mode_gate.as_ref().cloned() else {
            return;
        };
        let Some(stdin) = self.stdin.as_ref().cloned() else {
            return;
        };
        let Some(request_id) = mode_gate.lock().ok().and_then(|gate| match &gate.state {
            ClaudeModeGateState::AwaitingResponse { request_id, .. } => Some(request_id.clone()),
            _ => None,
        }) else {
            return;
        };
        let (cancel_tx, cancel_rx) = mpsc::channel();
        self.initial_mode_timer_cancel = Some(cancel_tx);
        let timer_gate = Arc::clone(&mode_gate);
        let timer_stdin = Arc::clone(&stdin);
        let timer_runtime = Arc::clone(runtime);
        let timer_timeout = self.initial_mode_timeout;
        match std::thread::Builder::new()
            .name("claude-mode-timeout".to_string())
            .spawn(move || {
                if cancel_rx.recv_timeout(timer_timeout).is_err() {
                    fail_initial_mode_parts(
                        &timer_gate,
                        Some(&timer_stdin),
                        &timer_runtime,
                        Some(&request_id),
                        true,
                        "Claude mode response timed out; queued prompt(s) were not delivered because Claude never confirmed the permission mode.",
                    );
                }
            }) {
            Ok(thread) => self.initial_mode_timer_thread = Some(thread),
            Err(_) => {
                self.initial_mode_timer_cancel = None;
                self.fail_initial_mode(runtime, "Could not start Claude mode response timeout.");
            }
        }
    }

    fn cancel_initial_mode_timeout(&mut self) {
        if let Some(cancel) = &self.initial_mode_timer_cancel {
            let _ = cancel.send(());
        }
        if let Some(thread) = self.initial_mode_timer_thread.take() {
            let _ = thread.join();
        }
    }

    fn fail_initial_mode(&mut self, runtime: &Arc<SessionRuntime>, message: &str) {
        self.cancel_initial_mode_timeout();
        if let Some(mode_gate) = &self.mode_gate {
            fail_initial_mode_parts(
                mode_gate,
                self.stdin.as_ref(),
                runtime,
                None,
                false,
                message,
            );
        } else {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: message.to_string(),
                },
            );
        }
    }

    fn complete_initial_mode(
        &mut self,
        runtime: &Arc<SessionRuntime>,
        request_id: &str,
        result: Result<String, String>,
    ) {
        let Some(mode_gate) = self.mode_gate.as_ref().cloned() else {
            return;
        };
        self.cancel_initial_mode_timeout();
        match result {
            Err(message) => {
                fail_initial_mode_parts(
                    &mode_gate,
                    self.stdin.as_ref(),
                    runtime,
                    Some(request_id),
                    true,
                    &message,
                );
            }
            Ok(mode_id) => {
                let write_error = {
                    let Ok(mut gate) = mode_gate.lock() else {
                        self.fail_initial_mode(runtime, "Claude mode gate is unavailable.");
                        return;
                    };
                    if !matches!(
                        &gate.state,
                        ClaudeModeGateState::AwaitingResponse {
                            request_id: expected,
                            ..
                        } if expected == request_id
                    ) {
                        return;
                    }
                    flush_gate_frames(
                        &mut gate,
                        self.stdin.as_ref().expect("mode gate has stdin"),
                        &mut self.view,
                        &mode_id,
                    )
                };
                if let Some(error) = write_error {
                    self.fail_initial_mode(
                        runtime,
                        &format!("Could not send queued Claude prompt: {error}"),
                    );
                }
            }
        }
    }

    fn dispatch_control_response(&mut self, value: &Value, runtime: &Arc<SessionRuntime>) -> bool {
        if value.get("type").and_then(Value::as_str) != Some("control_response") {
            return false;
        }
        let Some(request_id) = value
            .pointer("/response/request_id")
            .and_then(Value::as_str)
        else {
            return true;
        };
        let initial = self.mode_gate.as_ref().and_then(|mode_gate| {
            mode_gate.lock().ok().and_then(|gate| match &gate.state {
                ClaudeModeGateState::AwaitingResponse {
                    request_id: expected,
                    requested_mode,
                } if expected == request_id => Some(requested_mode.clone()),
                _ => None,
            })
        });
        if let Some(requested_mode) = initial {
            let result = match value.pointer("/response/subtype").and_then(Value::as_str) {
                Some("success") => Ok(value
                    .pointer("/response/response/mode")
                    .and_then(Value::as_str)
                    .unwrap_or(&requested_mode)
                    .to_string()),
                Some("error") => Err(value
                    .pointer("/response/error")
                    .and_then(Value::as_str)
                    .unwrap_or("Claude rejected the initial permission mode.")
                    .to_string()),
                _ => Err("Claude returned an invalid permission mode response.".to_string()),
            };
            self.complete_initial_mode(runtime, request_id, result);
            return true;
        }
        let sender = self
            .mode_responses
            .lock()
            .ok()
            .and_then(|mut responses| responses.remove(request_id));
        let Some(sender) = sender else {
            return true;
        };
        let result = match value.pointer("/response/subtype").and_then(Value::as_str) {
            Some("success") => {
                if let Some(mode_id) = value
                    .pointer("/response/response/mode")
                    .and_then(Value::as_str)
                {
                    self.view.set_mode(mode_id);
                }
                Ok(())
            }
            Some("error") => Err(value
                .pointer("/response/error")
                .and_then(Value::as_str)
                .unwrap_or("Claude rejected the permission mode change.")
                .to_string()),
            _ => Err("Claude returned an invalid permission mode response.".to_string()),
        };
        let _ = sender.send(result);
        true
    }

    fn dispatch_permission(
        &mut self,
        value: &Value,
        runtime: &Arc<SessionRuntime>,
        event_seq: Option<u64>,
    ) {
        let Some(request_id) = value.get("request_id").and_then(Value::as_str) else {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "Claude permission request had no request_id.".to_string(),
                },
            );
            return;
        };
        let Some(request) = value.get("request") else {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "Claude permission request had no request body.".to_string(),
                },
            );
            return;
        };
        let tool_name = request
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("tool");
        let display_name = request
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(tool_name);
        let tool_use_id = request
            .get("tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or(request_id)
            .to_string();
        let input = request.get("input").cloned().unwrap_or(Value::Null);
        let description = request
            .get("decision_reason")
            .and_then(Value::as_str)
            .map(str::to_string);
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_string);
        let event = SessionEvent::PermissionRequest {
            tool_call_id: tool_use_id,
            title: display_name.to_string(),
            description,
            command,
            args: None,
            cwd: None,
            env: None,
            options: vec![
                PermissionOption {
                    option_id: "allow".to_string(),
                    name: "Allow once".to_string(),
                    kind: "allow_once".to_string(),
                },
                PermissionOption {
                    option_id: "deny".to_string(),
                    name: "Deny".to_string(),
                    kind: "reject_once".to_string(),
                },
            ],
        };
        let acp_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut controls) = self.controls.lock() {
            controls.insert(
                acp_id,
                ClaudePendingControl {
                    request_id: request_id.to_string(),
                    input: input.clone(),
                },
            );
        }
        if let Err(error) = self
            .permission_broker
            .register(acp_id, event.clone(), runtime)
        {
            let _ = self.permission_broker.send(
                acp_id,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            );
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!("Could not queue Claude permission request: {error}"),
                },
            );
            return;
        }
        if runtime.permission_delivery_enabled() == Some(false) {
            let _ = self.permission_broker.respond(
                match &event {
                    SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id,
                    _ => "",
                },
                devboule_protocol::PermissionOutcome::Deny,
            );
            return;
        }
        let tool_call_id = match &event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id.as_str(),
            _ => return,
        };
        match self.permission_broker.auto_answer(tool_call_id, runtime) {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!(
                            "Could not auto-answer Claude permission request: {error}"
                        ),
                    },
                );
                return;
            }
        }
        self.publish_with_seq(runtime, event, event_seq);
    }
}

impl ReaderDispatch for ClaudeReader {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String> {
        self.start_initial_mode_timeout(runtime);
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
                    self.publish(
                        runtime,
                        SessionEvent::AgentError {
                            message: format!(
                                "Claude input line exceeded {MAX_LINE_BYTES} bytes and was discarded."
                            ),
                        },
                    );
                }
                break;
            };
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            if line.len() > MAX_LINE_BYTES {
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!(
                            "Claude input line exceeded {MAX_LINE_BYTES} bytes and was discarded."
                        ),
                    },
                );
                continue;
            }
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let line = String::from_utf8_lossy(line);
            self.dispatch_line(&line, runtime);
        }
        Ok(())
    }

    fn finish(&mut self, runtime: &Arc<SessionRuntime>) {
        self.cancel_initial_mode_timeout();
        if let Some(mode_gate) = &self.mode_gate {
            fail_initial_mode_parts(
                mode_gate,
                self.stdin.as_ref(),
                runtime,
                None,
                true,
                "Claude exited before confirming the permission mode; queued prompt(s) were not delivered because Claude never confirmed the permission mode.",
            );
        }
        self.permission_broker.close();
        if !self.buffer.is_empty() {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "Claude agent ended with an unterminated output line.".to_string(),
                },
            );
        }
    }
}

fn is_can_use_tool(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("control_request")
        && value
            .get("request")
            .and_then(|request| request.get("subtype"))
            .and_then(Value::as_str)
            == Some("can_use_tool")
}

struct ClaudeStderr {
    state: Arc<Mutex<ClaudeStderrState>>,
    handle: Option<JoinHandle<()>>,
}

struct ClaudeStderrState {
    runtime: Option<Arc<SessionRuntime>>,
    pending: std::collections::VecDeque<String>,
}

impl ClaudeStderr {
    fn start(stderr: ChildStderr) -> io::Result<Self> {
        let state = Arc::new(Mutex::new(ClaudeStderrState {
            runtime: None,
            pending: std::collections::VecDeque::new(),
        }));
        let thread_state = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("session-claude-stderr".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(stderr);
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) => return,
                        Ok(_) => {
                            let line = line
                                .trim_end_matches('\n')
                                .trim_end_matches('\r')
                                .to_string();
                            let runtime = match thread_state.lock() {
                                Ok(mut state) => {
                                    if let Some(runtime) = &state.runtime {
                                        Some(Arc::clone(runtime))
                                    } else {
                                        if state.pending.len() < 256 {
                                            state.pending.push_back(line.clone());
                                        }
                                        None
                                    }
                                }
                                Err(_) => return,
                            };
                            if let Some(runtime) = runtime {
                                let _ = runtime.publish_agent_event(
                                    SessionEvent::AgentStderr {
                                        data: runtime.redact_mcp_text(&line),
                                    },
                                    None,
                                );
                            }
                        }
                        Err(_) => return,
                    }
                }
            })?;
        Ok(Self {
            state,
            handle: Some(handle),
        })
    }
}

impl StderrSource for ClaudeStderr {
    fn spawn(mut self: Box<Self>, runtime: Arc<SessionRuntime>) -> io::Result<JoinHandle<()>> {
        let pending = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| io::Error::other("Claude stderr lock poisoned"))?;
            state.runtime = Some(Arc::clone(&runtime));
            std::mem::take(&mut state.pending)
        };
        for line in pending {
            let _ = runtime.publish_agent_event(
                SessionEvent::AgentStderr {
                    data: runtime.redact_mcp_text(&line),
                },
                None,
            );
        }
        self.handle
            .take()
            .ok_or_else(|| io::Error::other("Claude stderr drain was already consumed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ConnHandle, PendingEvent};
    use devboule_protocol::PermissionOutcome;
    use std::path::PathBuf;
    use std::process::{Child, ChildStdin, ChildStdout};

    const CLAUDE_MODE_CAPTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/wire/claude-set-mode.jsonl"
    ));

    fn drain(conn: &ConnHandle) -> Vec<SessionEvent> {
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

    fn attached(broker: &Arc<PermissionBroker>) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
        let runtime =
            SessionRuntime::for_acp("s.claude.test".to_string(), None, Arc::clone(broker));
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach");
        conn.track_with_agent_replay(
            "s.claude.test",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        (runtime, conn)
    }

    #[test]
    fn mcp_status_is_parsed_as_a_hint_and_failure_is_reported() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.require_mcp();
        let ready = serde_json::json!({
            "type": "system",
            "subtype": "init",
            "mcp_servers": [{"name": "devboule", "status": "connected"}]
        });
        observe_mcp_status(&ready, &runtime);
        assert!(runtime
            .wait_for_mcp_ready(std::time::Duration::from_millis(1))
            .is_err());

        let failed = serde_json::json!({
            "type": "system",
            "subtype": "init",
            "mcp_servers": [{"name": "devboule", "status": "failed"}]
        });
        observe_mcp_status(&failed, &runtime);
        let error = runtime
            .wait_for_mcp_ready(std::time::Duration::from_secs(1))
            .expect_err("provider failure must wake the gate");
        assert!(error.message.contains("Claude reported"));
    }

    #[test]
    fn interrupt_frame_matches_the_measured_control_request_wire() {
        let bytes = interrupt_frame_bytes("interrupt-7").expect("frame");
        let line = std::str::from_utf8(&bytes).expect("utf8");
        assert!(line.ends_with('\n'));
        let value: Value = serde_json::from_str(line.trim_end()).expect("json");
        assert_eq!(value["type"], "control_request");
        assert_eq!(value["request_id"], "interrupt-7");
        assert_eq!(value["request"]["subtype"], "interrupt");
    }

    fn claude_model(model_id: &str) -> devboule_protocol::SessionModel {
        devboule_protocol::SessionModel {
            model_id: model_id.to_string(),
            name: model_id.to_string(),
            description: None,
            context_tokens: None,
            current_effort: None,
            efforts: None,
        }
    }

    fn pinned(argv: Vec<String>, model_id: Option<&str>) -> Vec<String> {
        launch_with_model(launch_in_bypass_mode(argv), model_id)
    }

    #[test]
    fn claude_launch_always_uses_bypass_permission_mode() {
        let args = launch_in_bypass_mode(vec![
            "-p".to_string(),
            "--permission-mode".to_string(),
            "plan".to_string(),
        ]);
        assert_eq!(
            args.windows(2)
                .find(|pair| pair[0] == "--permission-mode")
                .map(|pair| pair[1].as_str()),
            Some("bypassPermissions")
        );
        assert_eq!(
            args.iter()
                .filter(|arg| arg.as_str() == "--permission-mode")
                .count(),
            1
        );
    }

    #[test]
    fn claude_launch_pins_the_catalog_model() {
        let derived = vec![
            claude_model("claude-sonnet-5"),
            claude_model("claude-opus-5"),
        ];
        let args = pinned(
            vec!["-p".to_string()],
            crate::claude_catalog::default_model_id(&derived).as_deref(),
        );
        assert_eq!(
            args.windows(2)
                .find(|pair| pair[0] == "--model")
                .map(|pair| pair[1].as_str()),
            Some("claude-opus-5")
        );

        let no_opus = vec![
            claude_model("claude-sonnet-5"),
            claude_model("claude-haiku-5"),
        ];
        let args = pinned(
            vec!["-p".to_string()],
            crate::claude_catalog::default_model_id(&no_opus).as_deref(),
        );
        assert_eq!(
            args.windows(2)
                .find(|pair| pair[0] == "--model")
                .map(|pair| pair[1].as_str()),
            Some("claude-sonnet-5")
        );
        assert_eq!(args.iter().filter(|arg| *arg == "--model").count(), 1);

        // An explicit launch model is replaced, never duplicated.
        let replaced = launch_with_model(
            vec![
                "-p".to_string(),
                "--model".to_string(),
                "sonnet".to_string(),
                "--model=haiku".to_string(),
            ],
            Some("claude-opus-5"),
        );
        assert_eq!(replaced, ["-p", "--model", "claude-opus-5"]);
    }

    #[test]
    fn set_mode_frame_matches_the_measured_control_request_wire() {
        let bytes = control_request_frame_bytes(
            "measure-acceptEdits",
            serde_json::json!({
                "subtype": "set_permission_mode",
                "mode": "acceptEdits",
            }),
        )
        .expect("frame");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(
            value,
            serde_json::json!({
                "type": "control_request",
                "request_id": "measure-acceptEdits",
                "request": {
                    "subtype": "set_permission_mode",
                    "mode": "acceptEdits",
                }
            })
        );
    }

    #[test]
    fn measured_set_mode_control_responses_resolve_success_and_error() {
        let mode_responses = Arc::new(Mutex::new(HashMap::new()));
        let (success_tx, success_rx) = mpsc::channel();
        let (error_tx, error_rx) = mpsc::channel();
        mode_responses.lock().expect("mode responses").extend([
            ("measure-acceptEdits".to_string(), success_tx),
            ("measure-bypassPermissions".to_string(), error_tx),
        ]);
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = ClaudeReader::new(
            ClaudeView::new(Some(PathBuf::from(r"C:\work"))),
            Arc::clone(&broker),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::clone(&mode_responses),
            Arc::new(AtomicU64::new(1)),
        );
        let runtime = Arc::new(SessionRuntime::new());
        let mut lines = CLAUDE_MODE_CAPTURE.lines();
        let success = lines.next().expect("measured success response");
        let error = lines.next().expect("measured error response");
        assert!(lines.next().is_none());
        reader
            .feed(format!("{success}\n{error}\n").as_bytes(), &runtime)
            .expect("feed");
        assert_eq!(success_rx.recv().expect("success response"), Ok(()));
        assert_eq!(
            error_rx.recv().expect("error response"),
            Err("Cannot set permission mode to bypassPermissions because the session was not launched with --dangerously-skip-permissions".to_string())
        );
    }

    #[test]
    fn writer_frames_buffered_text_as_a_user_message() {
        let bytes = frame_user_message("Reply with exactly one word: PONG").expect("frame");
        let line = std::str::from_utf8(&bytes).expect("utf8");
        assert!(line.ends_with('\n'));
        let value: Value = serde_json::from_str(line.trim_end()).expect("json");
        assert_eq!(value["type"], "user");
        assert_eq!(value["message"]["role"], "user");
        assert_eq!(value["message"]["content"][0]["type"], "text");
        assert_eq!(
            value["message"]["content"][0]["text"],
            "Reply with exactly one word: PONG"
        );
    }

    struct InitialModeHarness {
        child: Child,
        stdin: Arc<Mutex<Option<ChildStdin>>>,
        stdout: BufReader<ChildStdout>,
        gate: ClaudeModeGateRef,
        next_id: Arc<AtomicU64>,
    }

    fn initial_mode_test_setup() -> InitialModeHarness {
        let mut child = std::process::Command::new("node")
            .args([
                "-e",
                "process.stdin.on('data', data => process.stdout.write(data))",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("node is required for the Claude mode gate test");
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let stdout = child.stdout.take().expect("stdout");
        let next_id = Arc::new(AtomicU64::new(1));
        let gate =
            start_initial_mode(&stdin, &next_id, "default".to_string()).expect("mode request");
        InitialModeHarness {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            gate,
            next_id,
        }
    }

    fn initial_mode_test_reader(
        broker: &Arc<PermissionBroker>,
        stdin: Arc<Mutex<Option<ChildStdin>>>,
        gate: ClaudeModeGateRef,
        next_id: Arc<AtomicU64>,
        mode_responses: ClaudeModeResponses,
    ) -> ClaudeReader {
        initial_mode_test_reader_with_timeout(
            broker,
            stdin,
            gate,
            next_id,
            mode_responses,
            CONTROL_RESPONSE_TIMEOUT,
        )
    }

    fn initial_mode_test_reader_with_timeout(
        broker: &Arc<PermissionBroker>,
        stdin: Arc<Mutex<Option<ChildStdin>>>,
        gate: ClaudeModeGateRef,
        next_id: Arc<AtomicU64>,
        mode_responses: ClaudeModeResponses,
        timeout: Duration,
    ) -> ClaudeReader {
        ClaudeReader::with_mode_gate(
            ClaudeView::new(Some(PathBuf::from(r"C:\work"))),
            Arc::clone(broker),
            Arc::new(Mutex::new(HashMap::new())),
            mode_responses,
            next_id,
            ClaudeModeGateWiring {
                stdin,
                gate,
                timeout,
            },
        )
    }

    fn read_json_line(stdout: &mut BufReader<ChildStdout>) -> Value {
        let mut line = String::new();
        stdout.read_line(&mut line).expect("child output");
        serde_json::from_str(&line).expect("child output json")
    }

    fn initial_mode_response(request: &Value) -> Value {
        initial_mode_response_with_mode(request, "default")
    }

    fn initial_mode_response_with_mode(request: &Value, mode: &str) -> Value {
        serde_json::json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request["request_id"],
                "response": {"mode": mode}
            }
        })
    }

    fn initial_mode_error(request: &Value, message: &str) -> Value {
        serde_json::json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": request["request_id"],
                "error": message
            }
        })
    }

    #[test]
    fn initial_claude_mode_response_flushes_prompt_without_init() {
        let mut harness = initial_mode_test_setup();
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = initial_mode_test_reader(
            &broker,
            Arc::clone(&harness.stdin),
            Arc::clone(&harness.gate),
            Arc::clone(&harness.next_id),
            Arc::new(Mutex::new(HashMap::new())),
        );
        let mut writer = ClaudeWriter {
            stdin: Arc::clone(&harness.stdin),
            pending: Vec::new(),
            mode_gate: reader.mode_gate.clone(),
        };
        writer.write_all(b"Reply DONE").expect("buffer prompt");
        writer.flush().expect("queue prompt");

        let request = read_json_line(&mut harness.stdout);
        assert_eq!(request["request"]["subtype"], "set_permission_mode");
        assert_eq!(request["request"]["mode"], "default");
        let runtime = Arc::new(SessionRuntime::new());
        let response = initial_mode_response(&request);
        reader
            .feed(format!("{response}\n").as_bytes(), &runtime)
            .expect("mode response");
        let prompt = read_json_line(&mut harness.stdout);
        assert_eq!(prompt["type"], "user");
        assert_eq!(prompt["message"]["content"][0]["text"], "Reply DONE");
        drop(writer);
        let _ = harness.child.kill();
        let _ = harness.child.wait();
    }

    #[test]
    fn claude_prompts_waiting_for_initial_mode_response_flush_in_order() {
        let mut harness = initial_mode_test_setup();
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = initial_mode_test_reader(
            &broker,
            Arc::clone(&harness.stdin),
            Arc::clone(&harness.gate),
            Arc::clone(&harness.next_id),
            Arc::new(Mutex::new(HashMap::new())),
        );
        let mut writer = ClaudeWriter {
            stdin: Arc::clone(&harness.stdin),
            pending: Vec::new(),
            mode_gate: Some(Arc::clone(&harness.gate)),
        };
        writer.write_all(b"FIRST").expect("buffer first prompt");
        writer.flush().expect("queue first prompt");
        writer.write_all(b"SECOND").expect("buffer second prompt");
        writer.flush().expect("queue second prompt");

        let request = read_json_line(&mut harness.stdout);
        let runtime = Arc::new(SessionRuntime::new());
        let response = initial_mode_response(&request);
        reader
            .feed(format!("{response}\n").as_bytes(), &runtime)
            .expect("mode response");
        let first = read_json_line(&mut harness.stdout);
        let second = read_json_line(&mut harness.stdout);
        assert_eq!(first["message"]["content"][0]["text"], "FIRST");
        assert_eq!(second["message"]["content"][0]["text"], "SECOND");
        drop(writer);
        let _ = harness.child.kill();
        let _ = harness.child.wait();
    }

    #[test]
    fn initial_claude_mode_request_is_written_before_the_first_prompt() {
        let mut harness = initial_mode_test_setup();
        let mut writer = ClaudeWriter {
            stdin: Arc::clone(&harness.stdin),
            pending: Vec::new(),
            mode_gate: Some(Arc::clone(&harness.gate)),
        };
        writer.write_all(b"FIRST").expect("buffer prompt");
        writer.flush().expect("queue prompt");

        let request = read_json_line(&mut harness.stdout);
        assert_eq!(request["type"], "control_request");
        assert_eq!(request["request"]["subtype"], "set_permission_mode");
        drop(writer);
        let _ = harness.child.kill();
        let _ = harness.child.wait();
    }

    #[test]
    fn initial_claude_mode_error_publishes_once_and_closes_stdin() {
        let mut harness = initial_mode_test_setup();
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = initial_mode_test_reader(
            &broker,
            Arc::clone(&harness.stdin),
            Arc::clone(&harness.gate),
            Arc::clone(&harness.next_id),
            Arc::new(Mutex::new(HashMap::new())),
        );
        let mut writer = ClaudeWriter {
            stdin: Arc::clone(&harness.stdin),
            pending: Vec::new(),
            mode_gate: Some(Arc::clone(&harness.gate)),
        };
        writer.write_all(b"DROP ME").expect("buffer prompt");
        writer.flush().expect("queue prompt");
        let request = read_json_line(&mut harness.stdout);
        let (runtime, conn) = attached(&broker);
        reader
            .feed(
                format!("{}\n", initial_mode_error(&request, "rejected")).as_bytes(),
                &runtime,
            )
            .expect("mode error");

        let events = drain(&conn);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::AgentError { .. }))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message } if message == "rejected"
        )));
        let gate = harness.gate.lock().expect("gate");
        assert!(
            matches!(&gate.state, ClaudeModeGateState::Failed(message) if message == "rejected")
        );
        assert!(gate.pending_frames.is_empty());
        drop(gate);
        assert!(harness.stdin.lock().expect("stdin").is_none());
        drop(writer);
        let _ = harness.child.kill();
        let _ = harness.child.wait();
    }

    #[test]
    fn initial_claude_mode_timeout_publishes_once_and_drops_frames() {
        let mut harness = initial_mode_test_setup();
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = initial_mode_test_reader_with_timeout(
            &broker,
            Arc::clone(&harness.stdin),
            Arc::clone(&harness.gate),
            Arc::clone(&harness.next_id),
            Arc::new(Mutex::new(HashMap::new())),
            Duration::ZERO,
        );
        let mut writer = ClaudeWriter {
            stdin: Arc::clone(&harness.stdin),
            pending: Vec::new(),
            mode_gate: Some(Arc::clone(&harness.gate)),
        };
        writer.write_all(b"DROP ME").expect("buffer prompt");
        writer.flush().expect("queue prompt");
        let (runtime, conn) = attached(&broker);
        let _request = read_json_line(&mut harness.stdout);
        reader.feed(b"", &runtime).expect("start timeout");
        reader
            .initial_mode_timer_thread
            .take()
            .expect("timeout thread")
            .join()
            .expect("timeout thread join");
        let events = drain(&conn);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::AgentError { .. }))
                .count(),
            1
        );
        let gate = harness.gate.lock().expect("gate");
        assert!(
            matches!(&gate.state, ClaudeModeGateState::Failed(message) if message == "Claude mode response timed out; queued prompt(s) were not delivered because Claude never confirmed the permission mode.")
        );
        assert!(gate.pending_frames.is_empty());
        drop(gate);
        assert!(harness.stdin.lock().expect("stdin").is_none());
        drop(writer);
        let _ = harness.child.kill();
        let _ = harness.child.wait();
    }

    #[test]
    fn initial_claude_mode_timeout_after_success_is_a_noop() {
        let mut harness = initial_mode_test_setup();
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = initial_mode_test_reader_with_timeout(
            &broker,
            Arc::clone(&harness.stdin),
            Arc::clone(&harness.gate),
            Arc::clone(&harness.next_id),
            Arc::new(Mutex::new(HashMap::new())),
            Duration::from_secs(1),
        );
        let mut writer = ClaudeWriter {
            stdin: Arc::clone(&harness.stdin),
            pending: Vec::new(),
            mode_gate: Some(Arc::clone(&harness.gate)),
        };
        writer.write_all(b"KEEP ME").expect("buffer prompt");
        writer.flush().expect("queue prompt");
        let request = read_json_line(&mut harness.stdout);
        let (runtime, conn) = attached(&broker);
        reader
            .feed(
                format!("{}\n", initial_mode_response(&request)).as_bytes(),
                &runtime,
            )
            .expect("mode response");
        let prompt = read_json_line(&mut harness.stdout);
        assert_eq!(prompt["message"]["content"][0]["text"], "KEEP ME");
        assert!(drain(&conn)
            .iter()
            .all(|event| !matches!(event, SessionEvent::AgentError { .. })));
        assert!(reader.initial_mode_timer_thread.is_none());
        let gate = harness.gate.lock().expect("gate");
        assert!(matches!(&gate.state, ClaudeModeGateState::Ready));
        assert!(gate.pending_frames.is_empty());
        drop(gate);
        assert!(harness.stdin.lock().expect("stdin").is_some());
        drop(writer);
        let _ = harness.child.kill();
        let _ = harness.child.wait();
    }

    #[test]
    fn claude_mode_timeout_on_a_ready_gate_is_a_noop() {
        let mut harness = initial_mode_test_setup();
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = initial_mode_test_reader(
            &broker,
            Arc::clone(&harness.stdin),
            Arc::clone(&harness.gate),
            Arc::clone(&harness.next_id),
            Arc::new(Mutex::new(HashMap::new())),
        );
        let mut writer = ClaudeWriter {
            stdin: Arc::clone(&harness.stdin),
            pending: Vec::new(),
            mode_gate: Some(Arc::clone(&harness.gate)),
        };
        writer.write_all(b"KEEP ME").expect("buffer prompt");
        writer.flush().expect("queue prompt");
        let _request = read_json_line(&mut harness.stdout);
        let (runtime, conn) = attached(&broker);
        reader.feed(b"", &runtime).expect("start timeout");

        // Hold the gate so the fired deadline cannot inspect it before the
        // Ready transition lands; dropping the timer sender is the deadline
        // firing, without a sleep.
        let mut gate = harness.gate.lock().expect("gate");
        drop(reader.initial_mode_timer_cancel.take());
        assert!(
            flush_gate_frames(&mut gate, &harness.stdin, &mut reader.view, "default",).is_none()
        );
        drop(gate);

        reader
            .initial_mode_timer_thread
            .take()
            .expect("timeout thread")
            .join()
            .expect("timeout thread join");
        let prompt = read_json_line(&mut harness.stdout);
        assert_eq!(prompt["message"]["content"][0]["text"], "KEEP ME");
        assert!(drain(&conn)
            .iter()
            .all(|event| !matches!(event, SessionEvent::AgentError { .. })));
        let gate = harness.gate.lock().expect("gate");
        assert!(matches!(&gate.state, ClaudeModeGateState::Ready));
        assert!(gate.pending_frames.is_empty());
        drop(gate);
        assert!(harness.stdin.lock().expect("stdin").is_some());
        drop(writer);
        let _ = harness.child.kill();
        let _ = harness.child.wait();
    }

    #[test]
    fn claude_finish_while_initial_mode_is_pending_publishes_once_without_flushing() {
        let mut harness = initial_mode_test_setup();
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = initial_mode_test_reader(
            &broker,
            Arc::clone(&harness.stdin),
            Arc::clone(&harness.gate),
            Arc::clone(&harness.next_id),
            Arc::new(Mutex::new(HashMap::new())),
        );
        let mut writer = ClaudeWriter {
            stdin: Arc::clone(&harness.stdin),
            pending: Vec::new(),
            mode_gate: Some(Arc::clone(&harness.gate)),
        };
        writer.write_all(b"DROP ON EXIT").expect("buffer prompt");
        writer.flush().expect("queue prompt");
        let _request = read_json_line(&mut harness.stdout);
        let (runtime, conn) = attached(&broker);
        reader.finish(&runtime);
        reader.finish(&runtime);
        let events = drain(&conn);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::AgentError { .. }))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message }
                if message == "Claude exited before confirming the permission mode; queued prompt(s) were not delivered because Claude never confirmed the permission mode."
        )));
        let gate = harness.gate.lock().expect("gate");
        assert!(
            matches!(&gate.state, ClaudeModeGateState::Failed(message) if message == "Claude exited before confirming the permission mode; queued prompt(s) were not delivered because Claude never confirmed the permission mode.")
        );
        assert!(gate.pending_frames.is_empty());
        drop(gate);
        assert!(harness.stdin.lock().expect("stdin").is_none());
        drop(writer);
        let _ = harness.child.kill();
        let _ = harness.child.wait();
    }

    #[test]
    fn user_mode_switch_during_initial_mode_is_fifo_and_wins_in_the_view() {
        let mut harness = initial_mode_test_setup();
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mode_responses = Arc::new(Mutex::new(HashMap::new()));
        let mut reader = initial_mode_test_reader(
            &broker,
            Arc::clone(&harness.stdin),
            Arc::clone(&harness.gate),
            Arc::clone(&harness.next_id),
            Arc::clone(&mode_responses),
        );
        let switcher = ClaudeSwitcher {
            stdin: Arc::clone(&harness.stdin),
            next_id: Arc::clone(&harness.next_id),
            mode_responses,
        };
        let user_mode = std::thread::spawn(move || switcher.set_mode("acceptEdits"));
        let initial_request = read_json_line(&mut harness.stdout);
        let user_request = read_json_line(&mut harness.stdout);
        assert_eq!(initial_request["request"]["mode"], "default");
        assert_eq!(user_request["request"]["mode"], "acceptEdits");
        assert_ne!(initial_request["request_id"], user_request["request_id"]);

        let (runtime, conn) = attached(&broker);
        let init = serde_json::json!({
            "type": "system",
            "subtype": "init",
            "session_id": "s1",
            "model": "model-a",
            "permissionMode": "default"
        });
        reader
            .feed(format!("{init}\n").as_bytes(), &runtime)
            .expect("init");
        reader
            .feed(
                format!("{}\n", initial_mode_response(&initial_request)).as_bytes(),
                &runtime,
            )
            .expect("initial mode response");
        reader
            .feed(
                format!(
                    "{}\n",
                    initial_mode_response_with_mode(&user_request, "acceptEdits")
                )
                .as_bytes(),
                &runtime,
            )
            .expect("user mode response");
        assert!(user_mode.join().expect("mode thread").is_ok());

        let assistant = serde_json::json!({
            "type": "assistant",
            "message": {
                "model": "model-b",
                "id": "message-b",
                "role": "assistant",
                "content": []
            }
        });
        reader
            .feed(format!("{assistant}\n").as_bytes(), &runtime)
            .expect("assistant model change");
        let events = drain(&conn);
        let current_mode = events.iter().rev().find_map(|event| match event {
            SessionEvent::SessionManifest {
                modes: Some(modes), ..
            } => Some(modes.current_mode_id.clone()),
            _ => None,
        });
        assert_eq!(current_mode.as_deref(), Some("acceptEdits"));
        let _ = harness.child.kill();
        let _ = harness.child.wait();
    }

    #[test]
    fn control_response_allow_and_deny_match_the_measured_wire() {
        let input = serde_json::json!({
            "command": r"cmd /c del /q C:\Windows\Temp\devboule-nonexistent.txt",
            "description": "Delete a nonexistent temp file"
        });
        let allow = control_response_frame(
            "e73c118e-6742-481e-b60a-e8486a9bde4e",
            &input,
            &serde_json::json!({"outcome": {"outcome": "selected", "optionId": "allow"}}),
        );
        assert_eq!(allow["type"], "control_response");
        assert_eq!(allow["response"]["subtype"], "success");
        assert_eq!(
            allow["response"]["request_id"],
            "e73c118e-6742-481e-b60a-e8486a9bde4e"
        );
        assert_eq!(allow["response"]["response"]["behavior"], "allow");
        assert_eq!(allow["response"]["response"]["updatedInput"], input);

        let deny = control_response_frame(
            "620d31b5-1123-4170-b3d6-7465dc7ceced",
            &input,
            &serde_json::json!({"outcome": {"outcome": "selected", "optionId": "deny"}}),
        );
        assert_eq!(deny["response"]["response"]["behavior"], "deny");
        assert_eq!(
            deny["response"]["response"]["message"],
            "The user declined this command."
        );
    }

    fn test_reader(
        broker: Arc<PermissionBroker>,
        controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
    ) -> ClaudeReader {
        ClaudeReader::new(
            ClaudeView::new(Some(PathBuf::from(r"C:\work"))),
            broker,
            controls,
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(AtomicU64::new(1)),
        )
    }

    #[test]
    fn reader_assembles_a_line_split_across_feed_chunks() {
        let (broker, _) = {
            let sent = Arc::new(Mutex::new(Vec::<Value>::new()));
            let sender: Arc<PermissionSender> = Arc::new(move |_, _| Ok(()));
            let _ = sent;
            (PermissionBroker::for_test(sender), ())
        };
        let mut reader = test_reader(broker.clone(), Arc::new(Mutex::new(HashMap::new())));
        let (runtime, conn) = attached(&broker);
        reader
            .feed(br#"{"type":"system","subtype":"ini"#, &runtime)
            .expect("partial");
        assert!(drain(&conn)
            .iter()
            .all(|event| !matches!(event, SessionEvent::SessionManifest { .. })));
        reader
            .feed(
                b"t\",\"session_id\":\"abc\",\"model\":\"claude-opus-5\"}\n",
                &runtime,
            )
            .expect("rest");
        let events = drain(&conn);
        assert!(
            events.iter().any(|event| matches!(
                event,
                SessionEvent::SessionManifest {
                    provider_id,
                    current_model_id,
                    ..
                } if provider_id.as_deref() == Some("claude")
                    && current_model_id.as_deref() == Some("claude-opus-5")
            )),
            "split init line must become a manifest: {events:?}"
        );
        assert_eq!(runtime.peer_session_id().as_deref(), Some("abc"));
    }

    #[test]
    fn reader_parses_crlf_delimited_init_frames() {
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
        let (runtime, conn) = attached(&broker);
        // recon/probes/claude-perm-probe2-allow-host.txt system/init shape.
        let frames = concat!(
            r#"{"type":"system","subtype":"init","cwd":"C:\\tmp","session_id":"cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd","tools":["Bash","Read"],"model":"claude-opus-5[1m]","permissionMode":"default","claude_code_version":"2.1.260"}"#,
            "\r\n",
            r#"{"type":"system","subtype":"init","cwd":"C:\\tmp","session_id":"eb3f000a-87c3-4278-affb-cf183769f7e2","tools":["Bash","Read"],"model":"claude-opus-5","permissionMode":"default","claude_code_version":"2.1.260"}"#,
            "\r\n",
        );
        reader.feed(frames.as_bytes(), &runtime).expect("crlf feed");
        let events = drain(&conn);
        let manifests: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::SessionManifest {
                    current_model_id, ..
                } => current_model_id.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(
            manifests,
            ["claude-opus-5[1m]", "claude-opus-5"],
            "both CRLF frames must parse: {events:?}"
        );
        assert_eq!(
            runtime.peer_session_id().as_deref(),
            Some("eb3f000a-87c3-4278-affb-cf183769f7e2")
        );
    }

    #[test]
    fn reader_discards_a_huge_unterminated_line_without_killing_the_session() {
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
        let (runtime, conn) = attached(&broker);
        reader
            .feed(&vec![b'x'; MAX_LINE_BYTES + 1], &runtime)
            .expect("oversized input is reported, not fatal");
        assert!(reader.buffer.is_empty());
        let events = drain(&conn);
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message } if message.contains("exceeded")
        )));
    }

    #[test]
    fn permission_allow_and_deny_write_control_response_frames() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let controls = Arc::new(Mutex::new(HashMap::new()));
        let captured_for_sender = Arc::clone(&captured);
        let controls_for_sender = Arc::clone(&controls);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            let pending: ClaudePendingControl = controls_for_sender
                .lock()
                .expect("controls")
                .remove(&id)
                .expect("pending control");
            let frame = control_response_frame(&pending.request_id, &pending.input, &result);
            captured_for_sender.lock().expect("captured").push(frame);
            Ok(())
        });
        let broker = PermissionBroker::for_test(sender);
        let mut reader = test_reader(Arc::clone(&broker), Arc::clone(&controls));
        let (runtime, conn) = attached(&broker);
        let line = serde_json::json!({
            "type": "control_request",
            "request_id": "e73c118e-6742-481e-b60a-e8486a9bde4e",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Bash",
                "display_name": "Bash",
                "input": {
                    "command": r"cmd /c del /q C:\Windows\Temp\devboule-nonexistent.txt",
                    "description": "Delete a nonexistent temp file"
                },
                "tool_use_id": "toolu_01FgWLJkmeyU9wAGYkx3YFXu",
                "decision_reason": "This command requires approval"
            }
        });
        reader
            .feed(format!("{line}\n").as_bytes(), &runtime)
            .expect("feed");
        let events = drain(&conn);
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::PermissionRequest { tool_call_id, .. }
                if tool_call_id == "toolu_01FgWLJkmeyU9wAGYkx3YFXu"
        )));
        broker
            .respond(
                "toolu_01FgWLJkmeyU9wAGYkx3YFXu",
                PermissionOutcome::AllowOnce,
            )
            .expect("allow");
        let frames = captured.lock().expect("captured");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["response"]["response"]["behavior"], "allow");
        assert_eq!(
            frames[0]["response"]["request_id"],
            "e73c118e-6742-481e-b60a-e8486a9bde4e"
        );
        drop(frames);

        let captured = Arc::new(Mutex::new(Vec::new()));
        let controls = Arc::new(Mutex::new(HashMap::new()));
        let captured_for_sender = Arc::clone(&captured);
        let controls_for_sender = Arc::clone(&controls);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            let pending: ClaudePendingControl = controls_for_sender
                .lock()
                .expect("controls")
                .remove(&id)
                .expect("pending control");
            let frame = control_response_frame(&pending.request_id, &pending.input, &result);
            captured_for_sender.lock().expect("captured").push(frame);
            Ok(())
        });
        let broker = PermissionBroker::for_test(sender);
        let mut reader = test_reader(Arc::clone(&broker), Arc::clone(&controls));
        let (runtime, _conn) = attached(&broker);
        reader
            .feed(format!("{line}\n").as_bytes(), &runtime)
            .expect("feed deny");
        broker
            .respond("toolu_01FgWLJkmeyU9wAGYkx3YFXu", PermissionOutcome::Deny)
            .expect("deny");
        let frames = captured.lock().expect("captured");
        assert_eq!(frames[0]["response"]["response"]["behavior"], "deny");
    }

    #[test]
    fn claude_bypass_auto_answers_can_use_tool_without_client_prompt() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sent_for_sender = Arc::clone(&sent);
        let sender: Arc<PermissionSender> = Arc::new(move |_, result| {
            sent_for_sender.lock().expect("sent").push(result);
            Ok(())
        });
        let broker = PermissionBroker::for_test(sender);
        let controls = Arc::new(Mutex::new(HashMap::new()));
        let mut reader = test_reader(Arc::clone(&broker), controls);
        let (runtime, conn) = attached(&broker);
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypassPermissions".to_string(),
                available_modes: Vec::new(),
            }),
        });
        let line = serde_json::json!({
            "type": "control_request",
            "request_id": "claude-bypass-request",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Bash",
                "display_name": "Bash",
                "input": {"command": "echo devboule-probe"},
                "tool_use_id": "claude-bypass-tool",
            }
        });
        reader
            .feed(format!("{line}\n").as_bytes(), &runtime)
            .expect("feed bypass request");
        assert!(!drain(&conn)
            .iter()
            .any(|event| matches!(event, SessionEvent::PermissionRequest { .. })));
        assert_eq!(broker.pending_len(), 0);
        assert_eq!(
            sent.lock().expect("sent")[0]["outcome"]["optionId"],
            "allow"
        );

        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
        let (runtime, conn) = attached(&broker);
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "default".to_string(),
                available_modes: Vec::new(),
            }),
        });
        reader
            .feed(format!("{line}\n").as_bytes(), &runtime)
            .expect("feed default request");
        assert!(drain(&conn).iter().any(|event| matches!(
            event,
            SessionEvent::PermissionRequest { tool_call_id, .. }
                if tool_call_id == "claude-bypass-tool"
        )));
        broker
            .respond("claude-bypass-tool", PermissionOutcome::Deny)
            .expect("deny default request");
    }

    #[test]
    fn soft_interrupt_cancels_the_pending_permission_and_keeps_the_broker_open() {
        let mut command = Command::new("ping");
        command
            .args(["-t", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let child = command.spawn().expect("ping");
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sent_for_sender = Arc::clone(&sent);
        let sender: Arc<PermissionSender> = Arc::new(move |_, result| {
            sent_for_sender.lock().expect("sent").push(result);
            Ok(())
        });
        let broker = PermissionBroker::for_test(sender);
        let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
        let (runtime, conn) = attached(&broker);
        let first = serde_json::json!({
            "type": "control_request",
            "request_id": "req-before-stop",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Bash",
                "display_name": "Bash",
                "input": {"command": "echo one"},
                "tool_use_id": "tool-before-stop"
            }
        });
        reader
            .feed(format!("{first}\n").as_bytes(), &runtime)
            .expect("feed pending request");
        assert!(drain(&conn).iter().any(|event| matches!(
            event,
            SessionEvent::PermissionRequest { tool_call_id, .. }
                if tool_call_id == "tool-before-stop"
        )));

        let mut killer = ClaudeKiller {
            process: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(1)),
            permission_broker: Arc::clone(&broker),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        killer.interrupt();
        let stopped = drain(&conn);
        assert!(stopped.iter().any(|event| matches!(
            event,
            SessionEvent::PermissionResolved {
                tool_call_id,
                selected_option_id: None,
                ..
            } if tool_call_id == "tool-before-stop"
        )));
        assert_eq!(broker.pending_len(), 0);

        let second = serde_json::json!({
            "type": "control_request",
            "request_id": "req-after-stop",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Bash",
                "display_name": "Bash",
                "input": {"command": "echo two"},
                "tool_use_id": "tool-after-stop"
            }
        });
        reader
            .feed(format!("{second}\n").as_bytes(), &runtime)
            .expect("feed request after the soft stop");
        let after = drain(&conn);
        assert!(after.iter().any(|event| matches!(
            event,
            SessionEvent::PermissionRequest { tool_call_id, .. }
                if tool_call_id == "tool-after-stop"
        )));
        assert!(!after.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message } if message.contains("closed")
        )));
        assert_eq!(broker.pending_len(), 1);
    }

    #[test]
    fn kill_does_not_wait_for_the_child_to_cooperate() {
        let mut command = Command::new("ping");
        command
            .args(["-t", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().expect("ping");
        let stdin = child.stdin.take();
        let process = Arc::new(Mutex::new(child));
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut killer = ClaudeKiller {
            process: Arc::clone(&process),
            stdin: Arc::new(Mutex::new(stdin)),
            next_id: Arc::new(AtomicU64::new(1)),
            permission_broker: broker,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        killer.kill();
        let started = std::time::Instant::now();
        loop {
            let done = process
                .lock()
                .expect("process")
                .try_wait()
                .expect("wait")
                .is_some();
            if done {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "kill must not wait for the child to read stdin"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
