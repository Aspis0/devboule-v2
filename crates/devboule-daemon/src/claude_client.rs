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
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
#[cfg(test)]
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
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::server::ServerState;

const COMMAND_ENV: &str = "DEVBOULE_CLAUDE_COMMAND";
const MAX_LINE_BYTES: usize = 10 * 1024 * 1024;

struct ClaudePendingControl {
    request_id: String,
    input: Value,
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

pub(super) fn spawn_process(
    state: &Arc<ServerState>,
    command: PtyCommand,
) -> Result<SpawnedSession, WireError> {
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
    let next_id = Arc::new(AtomicU64::new(1));
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
    };
    let killer = ClaudeKiller {
        process: Arc::clone(&process),
        stdin: Arc::clone(&stdin),
        next_id: Arc::clone(&next_id),
        permission_broker: Arc::clone(&permission_broker),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    let reader_dispatch = ClaudeReader::new(
        ClaudeView::new(Some(command.cwd.clone())),
        Arc::clone(&permission_broker),
        Arc::clone(&controls),
        Arc::clone(&next_id),
    );
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        switcher: Some(Box::new(ClaudeSwitcher {
            stdin: Arc::clone(&stdin),
            next_id: Arc::clone(&next_id),
        })),
        child: Box::new(StdioWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
        reader: Box::new(BufReader::new(stdout)),
        reader_dispatch: Some(Box::new(reader_dispatch)),
        stderr: Some(Box::new(stderr_source)),
        permission_broker: Some(permission_broker),
        os_handle,
        peer_session_id: None,
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
        let pending = controls
            .lock()
            .map_err(|_| io::Error::other("Claude permission map lock poisoned"))?
            .remove(&id);
        let Some(pending) = pending else {
            return Err(io::Error::other(
                "Claude permission response had no matching control request",
            ));
        };
        let frame = control_response_frame(&pending.request_id, &pending.input, &result);
        let mut bytes = serde_json::to_vec(&frame)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        write_child_stdin(&stdin, &bytes, "Claude")
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
}

/// The single control_request interrupt frame builder shared by the hard
/// stop and the soft turn interrupt, so the wire format cannot drift apart.
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

    fn clone_switcher(&self) -> Box<dyn ModelSwitcher> {
        Box::new(Self {
            stdin: Arc::clone(&self.stdin),
            next_id: Arc::clone(&self.next_id),
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
        self.permission_broker.cancel_all();
    }

    fn kill(&mut self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            let request_id = format!("interrupt-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
            send_interrupt_frame(Arc::clone(&self.stdin), request_id);
            self.permission_broker.cancel_all();
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
    next_id: Arc<AtomicU64>,
}

impl ClaudeReader {
    fn new(
        view: ClaudeView,
        permission_broker: Arc<PermissionBroker>,
        controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
        next_id: Arc<AtomicU64>,
    ) -> Self {
        Self {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            view,
            permission_broker,
            controls,
            next_id,
        }
    }

    fn publish(&self, runtime: &SessionRuntime, event: SessionEvent) {
        let _ = runtime.publish_agent_event(event, None);
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
        runtime.journal_agent_envelope(&value);
        if is_can_use_tool(&value) {
            self.dispatch_permission(&value, runtime);
            return;
        }
        for event in self.view.ingest(&value) {
            if let SessionEvent::SessionManifest { .. } = &event {
                runtime.store_session_manifest(event.clone());
                if let Some(session_id) = self.view.peer_session_id() {
                    runtime.set_peer_session_id(session_id.to_string());
                }
            }
            self.publish(runtime, event);
        }
    }

    fn dispatch_permission(&mut self, value: &Value, runtime: &Arc<SessionRuntime>) {
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
        let pending = match self
            .permission_broker
            .register(acp_id, event.clone(), runtime)
        {
            Ok(pending) => pending,
            Err(error) => {
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
        };
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
        if let Err(error) = self.permission_broker.arm_timeout(Arc::clone(&pending)) {
            let tool_call_id = match &event {
                SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id.as_str(),
                _ => "",
            };
            let _ = self
                .permission_broker
                .respond(tool_call_id, devboule_protocol::PermissionOutcome::Deny);
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!(
                        "Could not start the Claude permission deadline; the request was cancelled: {error}"
                    ),
                },
            );
            return;
        }
        self.publish(runtime, event);
    }
}

impl ReaderDispatch for ClaudeReader {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String> {
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
        self.permission_broker.cancel_all();
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
                                    SessionEvent::AgentStderr { data: line },
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
            let _ = runtime.publish_agent_event(SessionEvent::AgentStderr { data: line }, None);
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
        let generation = runtime.try_attach(None, &conn, true).expect("attach");
        conn.track(
            "s.claude.test",
            Arc::clone(&runtime),
            false,
            None,
            generation,
        );
        (runtime, conn)
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
