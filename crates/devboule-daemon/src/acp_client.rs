//! ACP stdio transport for live agent sessions.
//!
//! This module owns only the process and protocol adapters. The parent
//! session module still owns the runtime, attachment queue, coalescer,
//! journal, liveness monitor, registry and teardown order.

use std::collections::{HashSet, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use devboule_protocol::{ErrorCode, SessionEvent, WireError};

use crate::paths::RuntimePaths;
use crate::process_tree::JobObject;
use crate::server::ServerState;

use super::PtyCommand;
use super::{
    ReaderDispatch, SessionKiller, SessionRuntime, SpawnedSession, StderrSource, WaitableChild,
};

const COMMAND_ENV: &str = "DEVBOULE_ACP_COMMAND";
const DEFAULT_PROGRAM: &str = "gemini";
const DEFAULT_ARGUMENT: &str = "--acp";

/// Resolve a direct executable plus argument vector. The JSON-array override
/// is intentional: it has no shell grammar and therefore remains correct for
/// executable paths containing spaces.
pub(super) fn resolve_command(_paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine agent working directory: {error}"),
        )
    })?;
    let argv = std::env::var(COMMAND_ENV).unwrap_or_else(|_| {
        serde_json::to_string(&[DEFAULT_PROGRAM, DEFAULT_ARGUMENT])
            .expect("default ACP command is serializable")
    });
    let mut argv: Vec<String> = serde_json::from_str(&argv).map_err(|error| {
        WireError::new(
            ErrorCode::InvalidRequest,
            format!("{COMMAND_ENV} must be a non-empty JSON string array: {error}"),
        )
    })?;
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("{COMMAND_ENV} must contain an executable."),
        ));
    }
    let program = argv.remove(0);
    Ok(PtyCommand::new(program, argv, cwd, Vec::new()))
}

/// Spawn the ACP peer directly, complete initialize + session/new, and return
/// adapters that the ordinary session machinery can own.
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
        // CREATE_NO_WINDOW prevents a desktop app from flashing a console for
        // every ACP agent process.
        process.creation_flags(0x0800_0000);
    }
    let mut child = process.spawn().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not start ACP agent {}: {error}", command.program),
        )
    })?;

    #[cfg(windows)]
    let process_job = {
        use std::os::windows::io::AsRawHandle;
        let process_job = JobObject::new().map_err(|error| {
            terminate_process(&mut child);
            WireError::new(
                ErrorCode::Io,
                format!("Could not create the ACP process job: {error}"),
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
                format!("Could not contain the ACP agent process: {error}"),
            ));
        }
        process_job
    };

    #[cfg(not(windows))]
    let process_job = JobObject::new().map_err(|error| {
        terminate_process(&mut child);
        WireError::new(
            ErrorCode::Io,
            format!("Could not create the ACP process job: {error}"),
        )
    })?;

    let stdin = child.stdin.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "ACP agent did not provide stdin.")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "ACP agent did not provide stdout.")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        terminate_process(&mut child);
        WireError::new(ErrorCode::Io, "ACP agent did not provide stderr.")
    })?;

    let process = Arc::new(Mutex::new(child));
    let transport = Arc::new(AcpTransport::new(stdin));
    let mut stderr_source = match AcpStderr::start(stderr) {
        Ok(source) => source,
        Err(error) => {
            if let Ok(mut process) = process.lock() {
                terminate_process(&mut process);
            }
            drop(process_job);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not drain ACP stderr: {error}"),
            ));
        }
    };
    let mut reader = BufReader::new(stdout);
    if let Err(error) = handshake(&transport, &mut reader, &command.cwd) {
        let mut killer = AcpKiller {
            process: Arc::clone(&process),
            transport: Arc::clone(&transport),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        killer.kill();
        drop(killer);
        // AcpTransport owns the only stdin handle. Close it before waiting
        // for a peer that may require EOF to finish its shutdown path.
        drop(transport);
        if let Ok(mut process) = process.lock() {
            let _ = process.wait();
        }
        let stderr_lines = stderr_source.discard_and_join();
        drop(process_job);
        if stderr_lines.is_empty() {
            return Err(error);
        }
        return Err(WireError::new(
            error.code,
            format!(
                "{} Agent stderr: {}",
                error.message,
                stderr_lines.join(" | ")
            ),
        ));
    }
    let session_id = transport.session_id();
    let writer = AcpWriter {
        transport: Arc::clone(&transport),
        pending: Vec::new(),
    };
    let killer = AcpKiller {
        process: Arc::clone(&process),
        transport: Arc::clone(&transport),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    let reader_dispatch = AcpReader::new(transport.pending_ids(), session_id);
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        child: Box::new(AcpWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
        reader: Box::new(reader),
        reader_dispatch: Some(Box::new(reader_dispatch)),
        stderr: Some(Box::new(stderr_source)),
    })
}

fn terminate_process(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

struct AcpTransport {
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashSet<u64>>>,
    session_id: Mutex<Option<String>>,
}

impl AcpTransport {
    fn new(stdin: ChildStdin) -> Self {
        Self {
            stdin: Arc::new(Mutex::new(stdin)),
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashSet::new())),
            session_id: Mutex::new(None),
        }
    }

    fn send_line(&self, value: &serde_json::Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| io::Error::other("ACP stdin lock poisoned"))?;
        stdin.write_all(&bytes)?;
        // An ACP request is interactive input. Flush explicitly on every
        // platform; this is especially important for Windows pipe buffers.
        stdin.flush()
    }

    fn request(&self, method: &str, params: serde_json::Value) -> io::Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .map_err(|_| io::Error::other("ACP pending-id lock poisoned"))?
            .insert(id);
        if let Err(error) = self.send_line(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })) {
            let _ = self.pending.lock().map(|mut pending| pending.remove(&id));
            return Err(error);
        }
        Ok(id)
    }

    fn notify(&self, method: &str, params: serde_json::Value) -> io::Result<()> {
        self.send_line(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn response_seen(&self, id: u64) -> bool {
        self.pending
            .lock()
            .map(|mut pending| pending.remove(&id))
            .unwrap_or(false)
    }

    fn pending_ids(&self) -> Arc<Mutex<HashSet<u64>>> {
        Arc::clone(&self.pending)
    }

    fn set_session_id(&self, session_id: String) {
        if let Ok(mut current) = self.session_id.lock() {
            *current = Some(session_id);
        }
    }

    fn session_id(&self) -> String {
        self.session_id
            .lock()
            .ok()
            .and_then(|value| value.clone())
            .unwrap_or_default()
    }

    fn cancel(&self) {
        let session_id = self.session_id();
        if session_id.is_empty() {
            return;
        }
        let _ = self.notify(
            "session/cancel",
            serde_json::json!({ "sessionId": session_id }),
        );
    }
}

fn handshake(
    transport: &AcpTransport,
    reader: &mut BufReader<ChildStdout>,
    cwd: &std::path::Path,
) -> Result<(), WireError> {
    let protocol_version = serde_json::to_value(ProtocolVersion::V1).map_err(|error| {
        WireError::new(
            ErrorCode::Internal,
            format!("Could not encode ACP protocol version: {error}"),
        )
    })?;
    let initialize_id = transport
        .request(
            "initialize",
            serde_json::json!({
                "protocolVersion": protocol_version,
                "clientCapabilities": {},
                "clientInfo": { "name": "devboule", "version": env!("CARGO_PKG_VERSION") }
            }),
        )
        .map_err(acp_io_error)?;
    let initialize = read_response(transport, reader, initialize_id)?;
    let negotiated = initialize
        .get("result")
        .and_then(|result| result.get("protocolVersion"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            WireError::new(
                ErrorCode::Io,
                "ACP initialize returned no protocol version.",
            )
        })?;
    if negotiated != 1 {
        return Err(WireError::new(
            ErrorCode::Io,
            format!("ACP peer negotiated unsupported protocol version {negotiated}."),
        ));
    }
    let new_session_id = transport
        .request(
            "session/new",
            serde_json::json!({
                "cwd": cwd.to_string_lossy(),
                "mcpServers": []
            }),
        )
        .map_err(acp_io_error)?;
    let new_session = read_response(transport, reader, new_session_id)?;
    let session_id = new_session
        .get("result")
        .and_then(|result| result.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| WireError::new(ErrorCode::Io, "ACP session/new returned no session id."))?;
    transport.set_session_id(session_id.to_string());
    Ok(())
}

fn read_response(
    transport: &AcpTransport,
    reader: &mut BufReader<ChildStdout>,
    expected_id: u64,
) -> Result<serde_json::Value, WireError> {
    loop {
        let mut line = String::new();
        let count = reader.read_line(&mut line).map_err(acp_io_error)?;
        if count == 0 {
            return Err(WireError::new(
                ErrorCode::Io,
                "ACP agent closed stdout during handshake.",
            ));
        }
        let line = line.trim_end_matches('\n').trim_end_matches('\r');
        let value = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("skipping malformed ACP handshake line: {error}");
                continue;
            }
        };
        if value.get("id").and_then(serde_json::Value::as_u64) != Some(expected_id) {
            continue;
        }
        if !transport.response_seen(expected_id) {
            eprintln!("skipping ACP response with an unknown id {expected_id}");
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(WireError::new(
                ErrorCode::Io,
                format!("ACP request failed: {error}"),
            ));
        }
        return Ok(value);
    }
}

fn acp_io_error(error: io::Error) -> WireError {
    WireError::new(ErrorCode::Io, format!("ACP stdio failed: {error}"))
}

struct AcpWriter {
    transport: Arc<AcpTransport>,
    pending: Vec<u8>,
}

impl Write for AcpWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let prompt = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        self.transport.request(
            "session/prompt",
            serde_json::json!({
                "sessionId": self.transport.session_id(),
                "prompt": [{ "type": "text", "text": prompt }]
            }),
        )?;
        Ok(())
    }
}

struct AcpKiller {
    process: Arc<Mutex<Child>>,
    transport: Arc<AcpTransport>,
    cancelled: Arc<AtomicBool>,
}

impl SessionKiller for AcpKiller {
    fn kill(&mut self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.transport.cancel();
            // Give a compliant peer a brief window to answer the outstanding
            // prompt with stopReason=cancelled before the shutdown kill.
            std::thread::sleep(Duration::from_millis(25));
        }
        if let Ok(mut process) = self.process.lock() {
            let _ = process.kill();
        }
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            process: Arc::clone(&self.process),
            transport: Arc::clone(&self.transport),
            cancelled: Arc::clone(&self.cancelled),
        })
    }
}

struct AcpWaitableChild {
    process: Arc<Mutex<Child>>,
}

impl WaitableChild for AcpWaitableChild {
    fn wait(self: Box<Self>) -> Option<u32> {
        loop {
            let status = self.process.lock().ok()?.try_wait().ok()?;
            if let Some(status) = status {
                return status.code().and_then(|code| u32::try_from(code).ok());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

struct AcpReader {
    buffer: Vec<u8>,
    pending: Arc<Mutex<HashSet<u64>>>,
    session_id: String,
}

impl AcpReader {
    fn new(pending: Arc<Mutex<HashSet<u64>>>, session_id: String) -> Self {
        Self {
            buffer: Vec::new(),
            pending,
            session_id,
        }
    }

    fn publish(&self, runtime: &SessionRuntime, event: SessionEvent) {
        let _ = runtime.publish_agent_event(event, None);
    }

    fn publish_text(&self, runtime: &SessionRuntime, event: SessionEvent, text: &str) {
        let _ = runtime.publish_agent_event(event, Some(text));
    }
}

impl ReaderDispatch for AcpReader {
    fn feed(&mut self, bytes: &[u8], runtime: &SessionRuntime) -> Result<(), String> {
        // `reader_loop` supplies arbitrary chunks. Buffering here means a
        // split UTF-8/JSON line is never parsed as two messages.
        self.buffer.extend_from_slice(bytes);
        for line in complete_lines(&mut self.buffer) {
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let line = String::from_utf8_lossy(line);
            self.dispatch_line(&line, runtime);
        }
        Ok(())
    }

    fn finish(&mut self, runtime: &SessionRuntime) {
        if !self.buffer.is_empty() {
            eprintln!("skipping unterminated ACP output line");
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "ACP agent ended with an unterminated output line.".to_string(),
                },
            );
        }
    }
}

fn complete_lines(buffer: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
        lines.push(buffer.drain(..=newline).collect());
    }
    lines
}

impl AcpReader {
    fn dispatch_line(&self, line: &str, runtime: &SessionRuntime) {
        let value = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("skipping malformed ACP output line: {error}");
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!("Malformed ACP output was skipped: {error}"),
                    },
                );
                return;
            }
        };
        if let Some(id) = value.get("id").and_then(serde_json::Value::as_u64) {
            self.dispatch_response(id, &value, runtime);
            return;
        }
        if value.get("method").and_then(serde_json::Value::as_str) == Some("session/update") {
            self.dispatch_update(&value, runtime);
        }
    }

    fn dispatch_response(&self, id: u64, value: &serde_json::Value, runtime: &SessionRuntime) {
        let response_was_pending = self
            .pending
            .lock()
            .map(|mut pending| pending.remove(&id))
            .unwrap_or(false);
        if !response_was_pending {
            eprintln!("skipping ACP response with unknown id {id}");
            return;
        }
        if let Some(error) = value.get("error") {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!("ACP request {id} failed: {error}"),
                },
            );
            return;
        }
        if let Some(stop_reason) = value
            .get("result")
            .and_then(|result| result.get("stopReason"))
            .and_then(serde_json::Value::as_str)
        {
            self.publish(
                runtime,
                SessionEvent::AgentFinished {
                    stop_reason: stop_reason.to_string(),
                },
            );
        }
    }

    fn dispatch_update(&self, value: &serde_json::Value, runtime: &SessionRuntime) {
        let Some(params) = value.get("params") else {
            return;
        };
        if params.get("sessionId").and_then(serde_json::Value::as_str)
            != Some(self.session_id.as_str())
        {
            return;
        }
        let Some(update) = params.get("update") else {
            return;
        };
        match update
            .get("sessionUpdate")
            .and_then(serde_json::Value::as_str)
        {
            Some("agent_message_chunk") => {
                let Some(text) = update
                    .get("content")
                    .filter(|content| {
                        content.get("type").and_then(serde_json::Value::as_str) == Some("text")
                    })
                    .and_then(|content| content.get("text"))
                    .and_then(serde_json::Value::as_str)
                else {
                    return;
                };
                self.publish_text(
                    runtime,
                    SessionEvent::AgentMessage {
                        message_id: update
                            .get("messageId")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        text: text.to_string(),
                    },
                    text,
                );
            }
            Some("tool_call") => {
                self.publish(
                    runtime,
                    SessionEvent::AgentToolCall {
                        tool_call_id: update
                            .get("toolCallId")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        title: update
                            .get("title")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        status: update
                            .get("status")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("in_progress")
                            .to_string(),
                    },
                );
            }
            Some("tool_call_update") => {
                let text = update
                    .get("content")
                    .filter(|content| {
                        content.get("type").and_then(serde_json::Value::as_str) == Some("text")
                    })
                    .and_then(|content| content.get("text"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                self.publish(
                    runtime,
                    SessionEvent::AgentToolUpdate {
                        tool_call_id: update
                            .get("toolCallId")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        status: update
                            .get("status")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        text,
                    },
                );
            }
            // Thinking, plans, modes, models and permissions are intentionally
            // outside this slice. Unknown future updates are safe to ignore.
            Some(_) | None => {}
        }
    }
}

struct AcpStderr {
    state: Arc<Mutex<AcpStderrState>>,
    handle: Option<JoinHandle<()>>,
}

struct AcpStderrState {
    runtime: Option<Arc<SessionRuntime>>,
    pending: VecDeque<String>,
}

impl AcpStderr {
    fn start(stderr: ChildStderr) -> io::Result<Self> {
        let state = Arc::new(Mutex::new(AcpStderrState {
            runtime: None,
            pending: VecDeque::new(),
        }));
        let thread_state = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("session-acp-stderr".to_string())
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
                                        } else {
                                            eprintln!(
                                                "dropping ACP stderr while handshake is pending: {line}"
                                            );
                                        }
                                        None
                                    }
                                }
                                Err(_) => return,
                            };
                            if let Some(runtime) = runtime {
                                publish_stderr_line(&runtime, line);
                            }
                        }
                        Err(error) => {
                            let runtime = thread_state
                                .lock()
                                .ok()
                                .and_then(|state| state.runtime.clone());
                            if let Some(runtime) = runtime {
                                let _ = runtime.publish_agent_event(
                                    SessionEvent::AgentError {
                                        message: format!("Could not read ACP stderr: {error}"),
                                    },
                                    None,
                                );
                            } else {
                                eprintln!("could not read ACP stderr: {error}");
                            }
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            state,
            handle: Some(handle),
        })
    }

    fn discard_and_join(&mut self) -> Vec<String> {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if let Ok(mut state) = self.state.lock() {
            return state.pending.drain(..).collect();
        }
        Vec::new()
    }
}

impl StderrSource for AcpStderr {
    fn spawn(mut self: Box<Self>, runtime: Arc<SessionRuntime>) -> io::Result<JoinHandle<()>> {
        let pending = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| io::Error::other("ACP stderr lock poisoned"))?;
            state.runtime = Some(Arc::clone(&runtime));
            std::mem::take(&mut state.pending)
        };
        for line in pending {
            publish_stderr_line(&runtime, line);
        }
        self.handle
            .take()
            .ok_or_else(|| io::Error::other("ACP stderr drain was already consumed"))
    }
}

fn publish_stderr_line(runtime: &SessionRuntime, line: String) {
    let _ = runtime.publish_agent_event(SessionEvent::AgentStderr { data: line }, None);
}

#[cfg(test)]
mod tests {
    use super::complete_lines;

    #[test]
    fn ndjson_buffers_partial_lines_and_strips_crlf_at_dispatch_boundary() {
        let mut buffer = b"{\"id\":1}\r".to_vec();
        assert!(complete_lines(&mut buffer).is_empty());
        buffer.extend_from_slice(b"\n{\"id\":2");
        let lines = complete_lines(&mut buffer);
        assert_eq!(lines, vec![b"{\"id\":1}\r\n".to_vec()]);
        assert_eq!(buffer, b"{\"id\":2");
    }
}
