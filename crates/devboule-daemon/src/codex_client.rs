//! Codex app-server stdio transport for live agent sessions.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_protocol::{ErrorCode, NoticeSeverity, SessionEvent, WireError};
use serde_json::Value;

use super::permission_broker::{PermissionBroker, PermissionSender};
use super::session_runtime::SessionRuntime;
use super::{
    write_child_stdin, ModelSwitcher, PtyCommand, ReaderDispatch, SessionKiller, SpawnedSession,
    StderrSource, StdioWaitableChild,
};
use crate::codex_view::{
    catalog_from_response, mode_values, thread_mode_values, validate_mode, CodexCatalog,
    CodexState, CodexStdout, MAX_LINE_BYTES,
};
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::server::ServerState;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const KILL_GRACE: Duration = Duration::from_secs(2);

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
    let switcher = CodexSwitcher {
        state: Arc::clone(&state),
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
    };
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        switcher: Some(Box::new(switcher)),
        child: Box::new(StdioWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
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
    state: Arc<CodexState>,
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
            state: Arc::clone(&self.state),
        })
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
        decline_input_result, initialize_params, interrupt_params, mode_values, notification_frame,
        permission_decision, permission_decision_frame, send_interrupt_request,
        thread_start_params, turn_id_from_response, turn_start_params, validate_mode, CodexReader,
    };
    use crate::codex_view::{catalog_from_response, fixture_frames, CodexState, CodexView};
    use devboule_protocol::SessionEvent;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};

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
}
