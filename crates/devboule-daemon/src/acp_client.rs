//! ACP stdio transport for live agent sessions.
//!
//! This module owns only the process and protocol adapters. The parent
//! session module still owns the runtime, attachment queue, coalescer,
//! journal, liveness monitor, registry and teardown order.

use std::collections::{HashSet, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use agent_client_protocol::schema::v1::{
    ClientCapabilities, FileSystemCapabilities, Implementation, InitializeRequest,
};
use agent_client_protocol::schema::ProtocolVersion;
use devboule_protocol::{ErrorCode, PermissionOption, SessionEvent, WireError};

use super::acp_host::{AcpHost, RpcError, RpcRespond};
use crate::acp_view::{
    classify_line, merge_handshake_manifest, view_from_envelope_in, AcpLineKind,
};
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::server::ServerState;

use super::permission_broker::{
    PermissionBroker, MAX_ACP_PERMISSION_FIELD_BYTES, MAX_ACP_PERMISSION_OPTIONS,
};
use super::PtyCommand;
use super::{
    write_child_stdin, ReaderDispatch, SessionKiller, SessionRuntime, SpawnedSession, StderrSource,
    StdioWaitableChild,
};

const COMMAND_ENV: &str = "DEVBOULE_ACP_COMMAND";
/// Test/direct-command counterpart to [`COMMAND_ENV`]. A direct command has
/// no catalog row to identify it; tests set this to the stub provider id so a
/// later resume can still exercise the named-provider path.
const COMMAND_PROVIDER_ENV: &str = "DEVBOULE_ACP_PROVIDER_ID";

/// Silence after `session/prompt` with no inbound traffic and no outstanding
/// client work. Grok stays mute instead of erroring when `terminal` is missing.
pub const ACP_TURN_SILENCE: Duration = Duration::from_secs(60);
const TURN_TIMEOUT_ENV: &str = "DEVBOULE_ACP_TURN_TIMEOUT_MS";
const MAX_ACP_PERMISSION_LINE_BYTES: usize = 256 * 1024;

fn turn_silence() -> Duration {
    std::env::var(TURN_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Duration::from_millis)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(ACP_TURN_SILENCE)
}

fn advertised_initialize_params() -> Result<serde_json::Value, WireError> {
    let request = InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(
            ClientCapabilities::new()
                .fs(FileSystemCapabilities::new()
                    .read_text_file(true)
                    .write_text_file(true))
                .terminal(true),
        )
        .client_info(Implementation::new("devboule", env!("CARGO_PKG_VERSION")));
    serde_json::to_value(request).map_err(|error| {
        WireError::new(
            ErrorCode::Internal,
            format!("Could not encode ACP initialize request: {error}"),
        )
    })
}

#[derive(Clone, Copy)]
enum PromptPhase {
    Idle,
    Live(u64),
    Abandoned(u64),
}

struct TurnWatch {
    silence: Duration,
    last_activity: Mutex<Instant>,
    prompt: Mutex<PromptPhase>,
    client_work: std::sync::atomic::AtomicU64,
    stop: AtomicBool,
    runtime: Mutex<Option<Weak<SessionRuntime>>>,
    cancel: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    broker: Mutex<Option<Weak<PermissionBroker>>>,
}

impl TurnWatch {
    fn new() -> Arc<Self> {
        let watch = Arc::new(Self {
            silence: turn_silence(),
            last_activity: Mutex::new(Instant::now()),
            prompt: Mutex::new(PromptPhase::Idle),
            client_work: std::sync::atomic::AtomicU64::new(0),
            stop: AtomicBool::new(false),
            runtime: Mutex::new(None),
            cancel: Mutex::new(None),
            broker: Mutex::new(None),
        });
        let thread_watch = Arc::downgrade(&watch);
        let _ = std::thread::Builder::new()
            .name("acp-turn-timeout".to_string())
            .spawn(move || loop {
                let Some(watch) = thread_watch.upgrade() else {
                    return;
                };
                if watch.stop.load(Ordering::Acquire) {
                    return;
                }
                watch.tick();
                drop(watch);
                std::thread::sleep(Duration::from_millis(50));
            });
        watch
    }

    fn set_cancel(&self, cancel: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.cancel.lock() {
            *slot = Some(cancel);
        }
    }

    fn bind_runtime(&self, runtime: &Arc<SessionRuntime>) {
        if let Ok(mut slot) = self.runtime.lock() {
            *slot = Some(Arc::downgrade(runtime));
        }
    }

    fn bind_broker(&self, broker: &Arc<PermissionBroker>) {
        if let Ok(mut slot) = self.broker.lock() {
            *slot = Some(Arc::downgrade(broker));
        }
    }

    fn note_activity(&self) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = Instant::now();
        }
    }

    fn begin_client_work(&self) {
        self.client_work.fetch_add(1, Ordering::AcqRel);
        self.note_activity();
    }

    fn end_client_work(&self) {
        self.client_work
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
        self.note_activity();
    }

    fn start_prompt(&self, id: u64) {
        if let Ok(mut prompt) = self.prompt.lock() {
            *prompt = PromptPhase::Live(id);
        }
        self.note_activity();
    }

    fn finish_prompt(&self, id: u64) -> bool {
        let Ok(mut prompt) = self.prompt.lock() else {
            return false;
        };
        match *prompt {
            PromptPhase::Live(current) if current == id => {
                *prompt = PromptPhase::Idle;
                true
            }
            PromptPhase::Abandoned(current) if current == id => false,
            _ => false,
        }
    }

    fn prompt_is_live(&self) -> bool {
        matches!(
            self.prompt.lock().ok().as_deref(),
            Some(PromptPhase::Live(_))
        )
    }

    fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn abandon_live_prompt(&self) -> Option<u64> {
        let Ok(mut prompt) = self.prompt.lock() else {
            return None;
        };
        match *prompt {
            PromptPhase::Live(id) => {
                *prompt = PromptPhase::Abandoned(id);
                Some(id)
            }
            _ => None,
        }
    }

    fn tick(&self) {
        if self.stop.load(Ordering::Acquire) {
            return;
        }
        if self.client_work.load(Ordering::Acquire) != 0 {
            return;
        }
        let pending_permission = self
            .broker
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
            .map(|broker| broker.pending_len() > 0)
            .unwrap_or(false);
        if pending_permission {
            return;
        }
        let idle = self
            .last_activity
            .lock()
            .ok()
            .map(|last| last.elapsed() >= self.silence)
            .unwrap_or(false);
        if !idle {
            return;
        }
        if self.client_work.load(Ordering::Acquire) != 0 {
            return;
        }
        if self
            .broker
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
            .map(|broker| broker.pending_len() > 0)
            .unwrap_or(false)
        {
            return;
        }
        let Some(prompt_id) = self.abandon_live_prompt() else {
            return;
        };
        if let Ok(cancel) = self.cancel.lock() {
            if let Some(cancel) = cancel.as_ref() {
                cancel();
            }
        }
        if let Some(runtime) = self
            .runtime
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
        {
            let _ = runtime.publish_agent_event(
                SessionEvent::AgentError {
                    message: format!(
                        "ACP prompt {prompt_id} stayed silent for {}s and was cancelled.",
                        self.silence.as_secs().max(1)
                    ),
                },
                None,
            );
        }
    }
}

impl Drop for TurnWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl PermissionBroker {
    fn new(stdin: Arc<Mutex<Option<ChildStdin>>>) -> Arc<Self> {
        Self::with_sender(Arc::new(move |id, result| {
            let mut bytes = serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            bytes.push(b'\n');
            write_child_stdin(&stdin, &bytes, "ACP")
        }))
    }
}

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
    let (provider_id, mut argv): (Option<String>, Vec<String>) = match std::env::var(COMMAND_ENV) {
        Ok(argv) => {
            let provider_id = std::env::var(COMMAND_PROVIDER_ENV)
                .ok()
                .filter(|id| !id.trim().is_empty());
            let argv = serde_json::from_str(&argv).map_err(|error| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("{COMMAND_ENV} must be a non-empty JSON string array: {error}"),
                )
            })?;
            (provider_id, argv)
        }
        Err(_) => {
            let Some(agent) = crate::provider_catalog::first_acp_available() else {
                return Err(WireError::new(
                    ErrorCode::Io,
                    format!(
                        "No ACP-capable agent was found on PATH. Set {COMMAND_ENV} to a non-empty JSON string array to choose an ACP command explicitly."
                    ),
                ));
            };
            (
                Some(agent.id.to_string()),
                agent
                    .acp_command
                    .expect("an ACP-capable catalog entry has an ACP command"),
            )
        }
    };
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("{COMMAND_ENV} must contain an executable."),
        ));
    }
    let program = argv.remove(0);
    let mut command = PtyCommand::new(program, argv, cwd, Vec::new());
    if let Some(provider_id) = provider_id {
        command = command.with_provider_id(provider_id);
    }
    Ok(command)
}

/// Resolve a specific ACP catalog agent by id. Local PATH agents and
/// registry npx-wrapper rows are both accepted; npx-wrapper is only
/// reachable through this explicit-id path.
pub(super) fn resolve_named(id: &str, paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine agent working directory: {error}"),
        )
    })?;
    // Direct-command test providers have no catalog entry. Keep this narrow:
    // only the exact provider identity paired with DEVBOULE_ACP_COMMAND may
    // use the override, preserving the normal named catalog resolution path.
    if std::env::var(COMMAND_ENV).is_ok()
        && std::env::var(COMMAND_PROVIDER_ENV).ok().as_deref() == Some(id)
    {
        let command = resolve_command(paths)?;
        if command.provider_id.as_deref() == Some(id) {
            return Ok(command);
        }
    }
    let Some(agent) = crate::provider_catalog::find_in_catalog(
        id,
        &crate::registry::CdnRegistryFetch,
        &paths.dir,
    ) else {
        return Err(WireError::new(
            ErrorCode::Io,
            format!("ACP agent '{id}' was not found on PATH or in the ACP registry."),
        ));
    };
    let Some(mut argv) = agent.acp_command else {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("Provider '{id}' is not an ACP agent."),
        ));
    };
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("ACP agent '{id}' resolved to an empty command."),
        ));
    }
    let program = argv.remove(0);
    Ok(PtyCommand::new(program, argv, cwd, Vec::new()).with_provider_id(id.to_string()))
}

/// Spawn the ACP peer directly, complete initialize + session/new, and return
/// adapters that the ordinary session machinery can own.
pub(super) fn spawn_process(
    state: &Arc<ServerState>,
    command: PtyCommand,
) -> Result<SpawnedSession, WireError> {
    spawn_process_with_load(state, command, None)
}

pub(super) fn spawn_process_resuming(
    state: &Arc<ServerState>,
    command: PtyCommand,
    peer_session_id: String,
) -> Result<SpawnedSession, WireError> {
    spawn_process_with_load(state, command, Some(peer_session_id))
}

fn spawn_process_with_load(
    state: &Arc<ServerState>,
    command: PtyCommand,
    load_session_id: Option<String>,
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
    let (process_job, os_handle) = {
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
        let os_handle = match ProcessHandle::duplicate(handle) {
            Ok(duplicated) => Some(duplicated),
            Err(error) => {
                eprintln!("could not duplicate ACP process handle for OS liveness: {error}");
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
            format!("Could not create the ACP process job: {error}"),
        )
    })?;
    #[cfg(not(windows))]
    let os_handle = None;

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
    let host = AcpHost::new(
        command.cwd.clone(),
        state.sessions.runtime_dir().to_path_buf(),
        Arc::clone(&state.process_job),
    );
    let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
    transport.bind_turn();
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
    let (deferred, handshake_manifest, peer_session_id) = match handshake(
        &transport,
        &mut reader,
        &command.cwd,
        command.provider_id.clone(),
        load_session_id.as_deref(),
    ) {
        Ok(handshake) => handshake,
        Err(error) => {
            let mut killer = AcpKiller {
                process: Arc::clone(&process),
                transport: Arc::clone(&transport),
                permission_broker: Arc::clone(&transport.permission_broker),
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
    };
    let session_id = transport.session_id();
    let writer = AcpWriter {
        transport: Arc::clone(&transport),
        pending: Vec::new(),
    };
    let killer = AcpKiller {
        process: Arc::clone(&process),
        transport: Arc::clone(&transport),
        permission_broker: Arc::clone(&transport.permission_broker),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    let reader_dispatch = AcpReader::new(
        transport.pending_ids(),
        session_id,
        Arc::clone(&transport.permission_broker),
        Arc::clone(&transport.host),
        Arc::clone(&transport.turn),
        Some(Arc::clone(&transport)),
        deferred,
        command.provider_id.clone(),
        handshake_manifest,
    );
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        child: Box::new(StdioWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
        reader: Box::new(reader),
        reader_dispatch: Some(Box::new(reader_dispatch)),
        stderr: Some(Box::new(stderr_source)),
        permission_broker: Some(Arc::clone(&transport.permission_broker)),
        os_handle,
        peer_session_id: Some(peer_session_id),
    })
}

fn terminate_process(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

struct AcpTransport {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    permission_broker: Arc<PermissionBroker>,
    host: Arc<AcpHost>,
    turn: Arc<TurnWatch>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashSet<u64>>>,
    session_id: Mutex<Option<String>>,
}

impl AcpTransport {
    fn new(stdin: ChildStdin, host: Arc<AcpHost>) -> Self {
        let stdin = Arc::new(Mutex::new(Some(stdin)));
        Self {
            permission_broker: PermissionBroker::new(Arc::clone(&stdin)),
            host,
            turn: TurnWatch::new(),
            stdin,
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashSet::new())),
            session_id: Mutex::new(None),
        }
    }

    fn send_result(
        &self,
        id: serde_json::Value,
        result: Result<serde_json::Value, RpcError>,
    ) -> io::Result<()> {
        let value = match result {
            Ok(result) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }),
            Err(error) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": error.to_json(),
            }),
        };
        self.send_line(&value)
    }

    fn send_line(&self, value: &serde_json::Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        write_child_stdin(&self.stdin, &bytes, "ACP")
    }

    fn close_stdin(&self) {
        if let Ok(mut stdin) = self.stdin.lock() {
            *stdin = None;
        }
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

    fn bind_turn(self: &Arc<Self>) {
        let cancel_transport = Arc::downgrade(self);
        self.turn.set_cancel(Arc::new(move || {
            if let Some(transport) = cancel_transport.upgrade() {
                transport.cancel();
            }
        }));
        self.turn.bind_broker(&self.permission_broker);
    }
}

impl Drop for AcpTransport {
    fn drop(&mut self) {
        self.turn.shutdown();
        self.host.shutdown();
        self.close_stdin();
    }
}

fn handshake(
    transport: &AcpTransport,
    reader: &mut BufReader<ChildStdout>,
    cwd: &std::path::Path,
    provider_id: Option<String>,
    load_session_id: Option<&str>,
) -> Result<(Vec<serde_json::Value>, Option<SessionEvent>, String), WireError> {
    let mut deferred = Vec::new();
    let initialize_id = transport
        .request("initialize", advertised_initialize_params()?)
        .map_err(acp_io_error)?;
    let initialize = read_response(transport, reader, initialize_id, &mut deferred)?;
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
    let (method, params) = match load_session_id {
        Some(session_id) => (
            "session/load",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd.to_string_lossy(),
                "mcpServers": []
            }),
        ),
        None => (
            "session/new",
            serde_json::json!({
                "cwd": cwd.to_string_lossy(),
                "mcpServers": []
            }),
        ),
    };
    let session_request_id = transport.request(method, params).map_err(acp_io_error)?;
    let session = read_response(transport, reader, session_request_id, &mut deferred)?;
    let session_id = match load_session_id {
        Some(session_id) if !session_id.is_empty() => session_id.to_string(),
        _ => session
            .get("result")
            .and_then(|result| result.get("sessionId"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                WireError::new(ErrorCode::Io, "ACP session/new returned no session id.")
            })?
            .to_string(),
    };
    transport.set_session_id(session_id.to_string());
    transport.host.set_session_id(session_id.to_string());
    let manifest = merge_handshake_manifest(
        initialize.get("result").unwrap_or(&serde_json::Value::Null),
        session.get("result").unwrap_or(&serde_json::Value::Null),
        provider_id,
    );
    Ok((deferred, manifest, session_id.to_string()))
}

fn read_response(
    transport: &AcpTransport,
    reader: &mut BufReader<ChildStdout>,
    expected_id: u64,
    deferred: &mut Vec<serde_json::Value>,
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
            deferred.push(value);
            continue;
        }
        if !transport.response_seen(expected_id) {
            eprintln!("skipping ACP response with an unknown id {expected_id}");
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(WireError::new(
                ErrorCode::Io,
                acp_request_error_message(error),
            ));
        }
        return Ok(value);
    }
}

/// Format a JSON-RPC error object for a user-facing message. Agents embed
/// structured payloads in the error object (qwen carries `authMethods`);
/// the string `message` field is what belongs in a chat banner, so the raw
/// serialized object is only the fallback when no string message exists.
fn acp_request_error_message(error: &serde_json::Value) -> String {
    let message = error.get("message").and_then(serde_json::Value::as_str);
    match (
        error.get("code").and_then(serde_json::Value::as_i64),
        message,
    ) {
        (Some(code), Some(message)) => format!("ACP request failed ({code}): {message}"),
        (None, Some(message)) => format!("ACP request failed: {message}"),
        _ => format!("ACP request failed: {error}"),
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
        let id = self.transport.request(
            "session/prompt",
            serde_json::json!({
                "sessionId": self.transport.session_id(),
                "prompt": [{ "type": "text", "text": prompt }]
            }),
        )?;
        self.transport.turn.start_prompt(id);
        Ok(())
    }
}

struct AcpKiller {
    process: Arc<Mutex<Child>>,
    transport: Arc<AcpTransport>,
    permission_broker: Arc<PermissionBroker>,
    cancelled: Arc<AtomicBool>,
}

impl SessionKiller for AcpKiller {
    /// Soft interrupt: ask the agent to cancel the current turn and release
    /// pending permission prompts. The process, the turn watch, and the
    /// kill guard stay untouched so later turns keep working.
    fn interrupt(&mut self) {
        // A kill already sent its own cancel and is tearing the peer down;
        // a late interrupt would only re-cancel a closing transport.
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        self.transport.cancel();
        self.permission_broker.cancel_all();
    }

    fn kill(&mut self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            let watchdog = Arc::clone(&self.process);
            let _ = std::thread::Builder::new()
                .name("acp-kill-watchdog".to_string())
                .spawn(move || {
                    std::thread::sleep(Duration::from_millis(150));
                    if let Ok(mut process) = watchdog.lock() {
                        let _ = process.kill();
                    }
                });
            let started = Instant::now();
            self.permission_broker.cancel_all();
            self.transport.turn.shutdown();
            self.transport.cancel();
            let remaining = Duration::from_millis(25).saturating_sub(started.elapsed());
            if !remaining.is_zero() {
                std::thread::sleep(remaining);
            }
        }
        if let Ok(mut process) = self.process.lock() {
            let _ = process.kill();
        }
        self.transport.close_stdin();
        self.transport.host.shutdown();
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            process: Arc::clone(&self.process),
            transport: Arc::clone(&self.transport),
            permission_broker: Arc::clone(&self.permission_broker),
            cancelled: Arc::clone(&self.cancelled),
        })
    }
}

struct AcpReader {
    buffer: Vec<u8>,
    discarding_oversized_line: bool,
    pending: Arc<Mutex<HashSet<u64>>>,
    session_id: String,
    permission_broker: Arc<PermissionBroker>,
    host: Arc<AcpHost>,
    turn: Arc<TurnWatch>,
    transport: Option<Arc<AcpTransport>>,
    deferred: Vec<serde_json::Value>,
    provider_id: Option<String>,
    handshake_manifest: Option<SessionEvent>,
    replay_count: AtomicU64,
}

impl AcpReader {
    #[allow(clippy::too_many_arguments)]
    fn new(
        pending: Arc<Mutex<HashSet<u64>>>,
        session_id: String,
        permission_broker: Arc<PermissionBroker>,
        host: Arc<AcpHost>,
        turn: Arc<TurnWatch>,
        transport: Option<Arc<AcpTransport>>,
        deferred: Vec<serde_json::Value>,
        provider_id: Option<String>,
        handshake_manifest: Option<SessionEvent>,
    ) -> Self {
        Self {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            pending,
            session_id,
            permission_broker,
            host,
            turn,
            transport,
            deferred,
            provider_id,
            handshake_manifest,
            replay_count: AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    fn for_test(
        pending: Arc<Mutex<HashSet<u64>>>,
        session_id: String,
        permission_broker: Arc<PermissionBroker>,
    ) -> Self {
        Self::for_test_on_host(
            pending,
            session_id,
            permission_broker,
            AcpHost::new(
                std::env::temp_dir(),
                std::env::temp_dir(),
                Arc::new(JobObject::new().expect("job")),
            ),
        )
    }

    #[cfg(test)]
    fn for_test_on_host(
        pending: Arc<Mutex<HashSet<u64>>>,
        session_id: String,
        permission_broker: Arc<PermissionBroker>,
        host: Arc<AcpHost>,
    ) -> Self {
        Self::new(
            pending,
            session_id,
            permission_broker,
            host,
            TurnWatch::new(),
            None,
            Vec::new(),
            None,
            None,
        )
    }

    #[cfg(test)]
    fn for_test_with_transport(
        pending: Arc<Mutex<HashSet<u64>>>,
        session_id: String,
        permission_broker: Arc<PermissionBroker>,
        host: Arc<AcpHost>,
        transport: Arc<AcpTransport>,
    ) -> Self {
        Self::new(
            pending,
            session_id,
            permission_broker,
            host,
            Arc::clone(&transport.turn),
            Some(transport),
            Vec::new(),
            None,
            None,
        )
    }

    fn publish(&self, runtime: &SessionRuntime, event: SessionEvent) {
        let _ = runtime.publish_agent_event(event, None);
    }

    fn with_provider(&self, event: SessionEvent) -> SessionEvent {
        match event {
            SessionEvent::SessionManifest {
                provider_id,
                current_model_id,
                models,
                modes,
            } => SessionEvent::SessionManifest {
                provider_id: provider_id.or_else(|| self.provider_id.clone()),
                current_model_id,
                models,
                modes,
            },
            other => other,
        }
    }
}

impl ReaderDispatch for AcpReader {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String> {
        self.turn.bind_runtime(runtime);
        self.host
            .bind_permission_gate(&self.permission_broker, runtime);
        if let Some(manifest) = self.handshake_manifest.take() {
            runtime.store_session_manifest(manifest.clone());
            self.publish(runtime, manifest);
        }
        if !self.deferred.is_empty() {
            let deferred = std::mem::take(&mut self.deferred);
            for value in deferred {
                self.dispatch_value(&value, runtime);
            }
        }
        // `reader_loop` supplies arbitrary chunks. Buffering here means a
        // split UTF-8/JSON line is never parsed as two messages.
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
                if self.buffer.len() > MAX_ACP_PERMISSION_LINE_BYTES {
                    self.buffer.clear();
                    self.discarding_oversized_line = true;
                    self.publish(
                        runtime,
                        SessionEvent::AgentError {
                            message: format!(
                                "ACP input line exceeded {MAX_ACP_PERMISSION_LINE_BYTES} bytes and was discarded."
                            ),
                        },
                    );
                }
                break;
            };
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            if line.len() > MAX_ACP_PERMISSION_LINE_BYTES {
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!(
                            "ACP input line exceeded {MAX_ACP_PERMISSION_LINE_BYTES} bytes and was discarded."
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
        self.turn.shutdown();
        self.host.shutdown();
        self.transport = None;
        let replay_count = self.replay_count.load(Ordering::Relaxed);
        if replay_count > 0 {
            eprintln!(
                "session {} dropped {} ACP replay notifications during resume",
                self.session_id, replay_count
            );
        }
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

fn bounded_permission_text(
    value: Option<&serde_json::Value>,
    field: &str,
    required: bool,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return if required {
            Err(format!("ACP permission request has no {field}"))
        } else {
            Ok(None)
        };
    };
    let Some(text) = value.as_str() else {
        return Err(format!("ACP permission request {field} must be a string"));
    };
    if text.is_empty() {
        return Err(format!("ACP permission request has an empty {field}"));
    }
    if text.len() > MAX_ACP_PERMISSION_FIELD_BYTES {
        return Err(format!(
            "ACP permission request {field} exceeds {MAX_ACP_PERMISSION_FIELD_BYTES} bytes"
        ));
    }
    Ok(Some(text.to_string()))
}

#[cfg(test)]
fn complete_lines(buffer: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
        lines.push(buffer.drain(..=newline).collect());
    }
    lines
}

impl AcpReader {
    fn dispatch_line(&self, line: &str, runtime: &Arc<SessionRuntime>) {
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
        self.dispatch_value(&value, runtime);
    }

    fn dispatch_value(&self, value: &serde_json::Value, runtime: &Arc<SessionRuntime>) {
        if matches!(classify_line(value), Some(AcpLineKind::Notification { .. }))
            && value
                .get("_meta")
                .and_then(|meta| meta.get("isReplay"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        {
            self.replay_count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.turn.note_activity();
        runtime.journal_agent_envelope(value);
        match classify_line(value) {
            Some(AcpLineKind::Request { method }) => {
                if method == "session/request_permission" {
                    self.dispatch_permission(value, runtime);
                    return;
                }
                self.dispatch_client_request(&method, value, runtime);
            }
            Some(AcpLineKind::Response) => {
                if let Some(id) = value.get("id").and_then(serde_json::Value::as_u64) {
                    self.dispatch_response(id, value, runtime);
                }
            }
            Some(AcpLineKind::Notification { .. }) => {
                if let Some(view) =
                    view_from_envelope_in(value, &self.session_id, Some(self.host.cwd()))
                {
                    let view = self.with_provider(view);
                    if matches!(view, SessionEvent::SessionManifest { .. }) {
                        runtime.store_session_manifest(view.clone());
                    }
                    self.publish(runtime, view);
                }
            }
            None => {}
        }
    }

    fn dispatch_client_request(
        &self,
        method: &str,
        value: &serde_json::Value,
        runtime: &Arc<SessionRuntime>,
    ) {
        let Some(id) = value.get("id").cloned() else {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!("ACP client request {method} had no id."),
                },
            );
            return;
        };
        let params = value
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let Some(transport) = &self.transport else {
            return;
        };
        self.turn.begin_client_work();
        let turn = Arc::clone(&self.turn);
        let transport = Arc::clone(transport);
        let respond: RpcRespond = Arc::new(move |response_id, result| {
            turn.end_client_work();
            let _ = transport.send_result(response_id, result);
        });
        self.host.dispatch(method, id, params, respond);
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
        if !self.turn.finish_prompt(id) {
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
        if let Some(view) = view_from_envelope_in(value, &self.session_id, Some(self.host.cwd())) {
            self.publish(runtime, view);
        }
    }

    fn cancel_permission_request(&self, id: u64, runtime: &SessionRuntime, reason: String) {
        let _ = self.permission_broker.send(
            id,
            serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
        );
        self.publish(runtime, SessionEvent::AgentError { message: reason });
    }

    fn dispatch_permission(&self, value: &serde_json::Value, runtime: &Arc<SessionRuntime>) {
        let Some(id) = value.get("id").and_then(serde_json::Value::as_u64) else {
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: "ACP permission request had no numeric id.".to_string(),
                },
            );
            return;
        };
        let Some(params) = value.get("params") else {
            self.cancel_permission_request(
                id,
                runtime,
                "ACP permission request had no params and was cancelled.".to_string(),
            );
            return;
        };
        if params.get("sessionId").and_then(serde_json::Value::as_str)
            != Some(self.session_id.as_str())
        {
            self.cancel_permission_request(
                id,
                runtime,
                "ACP permission request targeted another session and was cancelled.".to_string(),
            );
            return;
        }
        let tool_call = params
            .get("toolCall")
            .or_else(|| {
                params
                    .get("subject")
                    .and_then(|subject| subject.get("toolCall"))
            })
            .and_then(|tool_call| tool_call.get("toolCall").or(Some(tool_call)));
        let tool_call_id_value = tool_call
            .and_then(|tool_call| tool_call.get("toolCallId"))
            .or_else(|| params.get("toolCallId"));
        let options_value = params.get("options");
        let parsed = (|| -> Result<SessionEvent, String> {
            let tool_call_id = bounded_permission_text(tool_call_id_value, "tool_call_id", true)?
                .expect("required permission field has a value");
            let raw_options = options_value
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "ACP permission request had no options array".to_string())?;
            if raw_options.is_empty() {
                return Err("ACP permission request had no options".to_string());
            }
            if raw_options.len() > MAX_ACP_PERMISSION_OPTIONS {
                return Err(format!(
                    "ACP permission request has more than the maximum of {MAX_ACP_PERMISSION_OPTIONS} options"
                ));
            }
            let options = raw_options
                .iter()
                .map(|option| {
                    Ok(PermissionOption {
                        option_id: bounded_permission_text(
                            option.get("optionId"),
                            "option_id",
                            true,
                        )?
                        .expect("required permission field has a value"),
                        name: bounded_permission_text(option.get("name"), "option name", true)?
                            .expect("required permission field has a value"),
                        kind: bounded_permission_text(option.get("kind"), "option kind", true)?
                            .expect("required permission field has a value"),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let title = params
                .get("title")
                .or_else(|| tool_call.and_then(|call| call.get("title")))
                .map_or_else(
                    || Ok("Permission requested".to_string()),
                    |value| {
                        bounded_permission_text(Some(value), "title", true)
                            .map(|value| value.expect("required permission field has a value"))
                    },
                )?;
            let description =
                bounded_permission_text(params.get("description"), "description", false)?;
            let command = bounded_permission_text(
                params.get("command").or_else(|| {
                    params
                        .get("subject")
                        .and_then(|subject| subject.get("command"))
                }),
                "command",
                false,
            )?;
            let cwd = bounded_permission_text(
                params
                    .get("cwd")
                    .or_else(|| params.get("subject").and_then(|subject| subject.get("cwd"))),
                "cwd",
                false,
            )?;
            Ok(SessionEvent::PermissionRequest {
                tool_call_id,
                title,
                description,
                command,
                args: None,
                cwd,
                env: None,
                options,
            })
        })();
        let event = match parsed {
            Ok(event) => event,
            Err(reason) => {
                self.cancel_permission_request(
                    id,
                    runtime,
                    format!("ACP permission request was rejected: {reason}."),
                );
                return;
            }
        };
        let tool_call_id = match &event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id.clone(),
            _ => unreachable!("permission parser returned a different event"),
        };
        let delivery = runtime.permission_delivery_enabled();
        if !self.turn.prompt_is_live() {
            self.cancel_permission_request(
                id,
                runtime,
                "ACP permission request arrived after the turn ended and was cancelled."
                    .to_string(),
            );
            return;
        }
        let pending = match self.permission_broker.register(id, event.clone(), runtime) {
            Ok(pending) => pending,
            Err(error) => {
                self.cancel_permission_request(
                    id,
                    runtime,
                    format!("Could not queue ACP permission request: {error}"),
                );
                return;
            }
        };
        if !self.turn.prompt_is_live() {
            let _ = self
                .permission_broker
                .cancel(&tool_call_id, &pending, "cancelled");
            return;
        }
        if delivery == Some(false) {
            let _ =
                self.permission_broker
                    .cancel(&tool_call_id, &pending, "capability_not_supported");
            return;
        }
        let timeout_started = match self.permission_broker.arm_timeout(Arc::clone(&pending)) {
            Ok(()) => true,
            Err(error) => {
                let _ =
                    self.permission_broker
                        .cancel(&tool_call_id, &pending, "timeout_spawn_failed");
                self.publish(
                    runtime,
                    SessionEvent::AgentError {
                        message: format!(
                            "Could not start the ACP permission deadline; the request was cancelled: {error}"
                        ),
                    },
                );
                false
            }
        };
        if timeout_started {
            let _ = runtime.publish_agent_event(event, None);
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
    use super::super::permission_broker::{
        permission, permission_path, test_broker, PermissionBroker, MAX_ACP_PERMISSION_FIELD_BYTES,
    };
    use super::{complete_lines, AcpReader, MAX_ACP_PERMISSION_LINE_BYTES};
    use crate::journal::Journal;
    use crate::session::{ConnHandle, ReaderDispatch, SessionKiller, SessionRuntime};
    use devboule_protocol::{PermissionOutcome, SessionEvent};
    use std::collections::HashSet;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    #[test]
    fn journal_keeps_raw_envelope_and_replay_derives_the_view() {
        let path = permission_path("envelope");
        let journal = Journal::open(&path).expect("journal");
        journal
            .upsert_blocking(crate::journal::new_session_record(
                "s.envelope",
                "owner",
                None,
                devboule_protocol::SessionKind::Acp,
                "Agent",
            ))
            .expect("upsert");
        let envelope = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "01a06c70-ea2b-7882-ad27-aae8188fc243",
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "The"}
                }
            }
        });
        journal
            .append_blocking(
                crate::journal::acp_envelope_record("s.envelope", 1, 1, &envelope).expect("record"),
            )
            .expect("append");
        let replay = journal.replay("s.envelope", 0).expect("replay");
        assert!(
            replay.events.iter().any(|event| matches!(
                event,
                SessionEvent::AgentThought { text, .. } if text == "The"
            )),
            "replay lost the derived thought: {:?}",
            replay.events
        );
        journal.shutdown();
        let stored: serde_json::Value = {
            let conn = rusqlite::Connection::open(&path).expect("inspect");
            let payload: Vec<u8> = conn
                .query_row(
                    "SELECT payload FROM events WHERE session_id = ?1 AND kind = 'acp_envelope'",
                    ["s.envelope"],
                    |row| row.get(0),
                )
                .expect("payload");
            serde_json::from_slice(&payload).expect("json")
        };
        assert_eq!(stored["method"], "session/update");
        assert_eq!(
            stored["params"]["update"]["sessionUpdate"],
            "agent_thought_chunk"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn initialize_declares_only_implemented_fs_and_terminal() {
        let params = super::advertised_initialize_params().expect("initialize params");
        assert_eq!(params["clientCapabilities"]["fs"]["readTextFile"], true);
        assert_eq!(params["clientCapabilities"]["fs"]["writeTextFile"], true);
        assert_eq!(params["clientCapabilities"]["terminal"], true);
        assert!(
            params["clientCapabilities"].get("elicitation").is_none()
                || params["clientCapabilities"]["elicitation"].is_null()
        );
        assert_eq!(params["clientInfo"]["name"], "devboule");
    }

    #[test]
    fn ndjson_buffers_partial_lines_and_strips_crlf_at_dispatch_boundary() {
        let mut buffer = b"{\"id\":1}\r".to_vec();
        assert!(complete_lines(&mut buffer).is_empty());
        buffer.extend_from_slice(b"\n{\"id\":2");
        let lines = complete_lines(&mut buffer);
        assert_eq!(lines, vec![b"{\"id\":1}\r\n".to_vec()]);
        assert_eq!(buffer, b"{\"id\":2");
    }

    #[test]
    fn timeout_spawn_failure_cancels_and_removes_the_request() {
        let (broker, sent) = test_broker();
        broker.fail_next_timeout_spawn();
        let reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            Arc::clone(&broker),
        );
        let runtime = Arc::new(SessionRuntime::new());
        reader.dispatch_permission(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 61,
                "method": "session/request_permission",
                "params": {
                    "sessionId": "stub-session",
                    "title": "Run command",
                    "toolCall": {"toolCallId": "spawn-failure"},
                    "options": [{"optionId": "allow", "name": "Allow once", "kind": "allow_once"}]
                }
            }),
            &runtime,
        );
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 61);
        assert_eq!(sent[0].1["outcome"]["outcome"], "cancelled");
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn oversized_permission_field_is_cancelled_before_storage() {
        let (broker, sent) = test_broker();
        let reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            Arc::clone(&broker),
        );
        let runtime = Arc::new(SessionRuntime::new());
        reader.dispatch_permission(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 62,
                "method": "session/request_permission",
                "params": {
                    "sessionId": "stub-session",
                    "title": "x".repeat(MAX_ACP_PERMISSION_FIELD_BYTES + 1),
                    "toolCall": {"toolCallId": "oversized"},
                    "options": [{"optionId": "allow", "name": "Allow once", "kind": "allow_once"}]
                }
            }),
            &runtime,
        );
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 62);
        assert_eq!(sent[0].1["outcome"]["outcome"], "cancelled");
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn oversized_unterminated_line_is_dropped_and_reported() {
        let (broker, _) = test_broker();
        let mut reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            broker,
        );
        let runtime = Arc::new(SessionRuntime::new());
        reader
            .feed(&vec![b'x'; MAX_ACP_PERMISSION_LINE_BYTES + 1], &runtime)
            .expect("oversized input is reported, not fatal to the reader");
        assert!(reader.buffer.is_empty());
    }

    #[test]
    fn detached_permission_is_queued_for_capable_reattach_and_removed_after_expiry() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::for_acp(
            "s.permission.queue".to_string(),
            None,
            Arc::clone(&broker),
        ));
        let first = ConnHandle::new(1);
        let generation = runtime
            .try_attach(None, &first, true)
            .expect("first attach");
        first.track(
            "s.permission.queue",
            Arc::clone(&runtime),
            false,
            None,
            generation,
        );
        runtime.detach_if_conn(first.id);

        let request = permission("queued");
        let pending = broker
            .register(7, request.clone(), &runtime)
            .expect("register");
        runtime.publish_agent_event(request, None);

        let second = ConnHandle::new(2);
        let generation = runtime.try_attach(None, &second, true).expect("reattach");
        second.track(
            "s.permission.queue",
            Arc::clone(&runtime),
            false,
            None,
            generation,
        );
        assert!(second.pull_events().iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::PermissionRequest { ref tool_call_id, .. } if tool_call_id == "queued"
        )));

        assert!(broker.expire("queued", &pending));
        let after_expiry = second.pull_events();
        assert!(
            after_expiry.iter().any(|event| matches!(
                event.envelope.event,
                SessionEvent::PermissionResolved { ref tool_call_id } if tool_call_id == "queued"
            )),
            "expiry must tell the attached client the card is gone: {after_expiry:?}"
        );
        assert!(
            !after_expiry.iter().any(|event| matches!(
                event.envelope.event,
                SessionEvent::PermissionRequest { .. }
            )),
            "expiry must not re-deliver the request: {after_expiry:?}"
        );
    }

    #[test]
    fn detached_permission_expiry_is_not_replayed_on_later_reattach() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::for_acp(
            "s.permission.expired".to_string(),
            None,
            Arc::clone(&broker),
        ));
        let request = permission("expired-detached");
        let pending = broker
            .register(8, request.clone(), &runtime)
            .expect("register");
        runtime.publish_agent_event(request, None);
        assert!(broker.expire("expired-detached", &pending));

        let conn = ConnHandle::new(3);
        let generation = runtime.try_attach(None, &conn, true).expect("reattach");
        conn.track(
            "s.permission.expired",
            Arc::clone(&runtime),
            false,
            None,
            generation,
        );
        let events = conn.pull_events();
        assert!(
            !events.iter().any(|event| matches!(
                event.envelope.event,
                SessionEvent::PermissionRequest { .. }
            )),
            "expired permission must not replay as a request: {events:?}"
        );
    }

    #[test]
    fn reattach_reemits_the_stored_session_manifest() {
        let (broker, _) = test_broker();
        let runtime =
            SessionRuntime::for_acp("s.manifest.reattach".to_string(), None, Arc::clone(&broker));
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: Some("grok-4.6".to_string()),
            models: Vec::new(),
            modes: None,
        });

        let first = ConnHandle::new(1);
        let generation = runtime.try_attach(None, &first, true).expect("attach");
        first.track(
            "s.manifest.reattach",
            Arc::clone(&runtime),
            false,
            None,
            generation,
        );
        let first_events = first.pull_events();
        assert!(
            first_events.iter().any(|event| matches!(
                event.envelope.event,
                SessionEvent::SessionManifest {
                    ref current_model_id,
                    ..
                } if current_model_id.as_deref() == Some("grok-4.6")
            )),
            "first attach must deliver the stored manifest: {first_events:?}"
        );

        runtime.detach_if_conn(first.id);
        let second = ConnHandle::new(2);
        let generation = runtime.try_attach(None, &second, true).expect("reattach");
        second.track(
            "s.manifest.reattach",
            Arc::clone(&runtime),
            false,
            None,
            generation,
        );
        let second_events = second.pull_events();
        assert!(
            second_events.iter().any(|event| matches!(
                event.envelope.event,
                SessionEvent::SessionManifest {
                    ref current_model_id,
                    ..
                } if current_model_id.as_deref() == Some("grok-4.6")
            )),
            "reattach must re-emit the stored manifest: {second_events:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn reader_finish_releases_terminals_left_by_a_dead_agent() {
        use super::AcpHost;
        use crate::process_tree::JobObject;
        let cwd = std::env::temp_dir().join(format!(
            "devboule-acp-finish-cwd-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let runtime = std::env::temp_dir().join(format!(
            "devboule-acp-finish-rt-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&runtime).expect("runtime");
        let host = AcpHost::new(
            cwd.clone(),
            runtime.clone(),
            Arc::new(JobObject::new().expect("job")),
        );
        host.set_session_id("stub-session".to_string());
        let (broker, _) = test_broker();
        let session_runtime = Arc::new(SessionRuntime::for_acp(
            "stub-session".to_string(),
            None,
            Arc::clone(&broker),
        ));
        host.bind_permission_gate(&broker, &session_runtime);
        let allow_broker = Arc::clone(&broker);
        let allow = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(id) = allow_broker.pending_ids().into_iter().next() {
                    let _ = allow_broker.respond(&id, PermissionOutcome::AllowOnce);
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        host.test_create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "ping.exe",
            "args": ["-t", "127.0.0.1"]
        }))
        .expect("create lingering terminal");
        let _ = allow.join();
        assert_eq!(host.live_terminal_count(), 1);
        let (broker, _) = test_broker();
        let mut reader = AcpReader::for_test_on_host(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            broker,
            Arc::clone(&host),
        );
        let session_runtime = Arc::new(SessionRuntime::new());
        reader.finish(&session_runtime);
        assert_eq!(
            host.live_terminal_count(),
            0,
            "EOF must shut down ACP terminals the dead agent left behind"
        );
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(runtime);
    }

    #[cfg(windows)]
    #[test]
    fn killer_does_not_block_on_a_full_agent_stdin() {
        use super::{AcpHost, AcpKiller, AcpTransport};
        use crate::process_tree::JobObject;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let mut child = Command::new("ping.exe")
            .args(["-n", "99999", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("ping");
        let stdin = child.stdin.take().expect("stdin");
        let cwd = std::env::temp_dir();
        let host = AcpHost::new(cwd.clone(), cwd, Arc::new(JobObject::new().expect("job")));
        let transport = Arc::new(AcpTransport::new(stdin, host));
        transport.set_session_id("stub-session".to_string());
        let runtime = Arc::new(SessionRuntime::new());
        transport
            .permission_broker
            .register(1, permission("stuck-kill"), &runtime)
            .expect("pending permission so cancel_all must write stdin");
        let filler = {
            let transport = Arc::clone(&transport);
            thread::spawn(move || {
                let blob = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/prompt",
                    "params": { "pad": "x".repeat(4096) }
                });
                for _ in 0..64 {
                    if transport.send_line(&blob).is_err() {
                        break;
                    }
                }
            })
        };
        thread::sleep(Duration::from_millis(200));
        let mut killer = AcpKiller {
            process: Arc::new(Mutex::new(child)),
            permission_broker: Arc::clone(&transport.permission_broker),
            transport: Arc::clone(&transport),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let started = Instant::now();
        thread::spawn(move || {
            killer.kill();
            let _ = done_tx.send(started.elapsed());
        });
        let elapsed = done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("kill blocked waiting on agent stdin");
        let _ = filler.join();
        assert!(
            elapsed < Duration::from_secs(2),
            "kill blocked for {elapsed:?} waiting on agent stdin"
        );
    }

    #[cfg(windows)]
    #[test]
    fn kill_unblocks_a_pending_terminal_create_gate() {
        use super::{AcpHost, AcpKiller, AcpTransport};
        use crate::process_tree::JobObject;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::Instant;
        let mut child = Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("ping");
        let stdin = child.stdin.take().expect("stdin");
        let cwd = std::env::temp_dir().join(format!(
            "devboule-acp-kill-gate-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&cwd).expect("cwd");
        let host = AcpHost::new(
            cwd.clone(),
            cwd.clone(),
            Arc::new(JobObject::new().expect("job")),
        );
        host.set_session_id("stub-session".to_string());
        let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
        let runtime = SessionRuntime::for_acp(
            "stub-session".to_string(),
            None,
            Arc::clone(&transport.permission_broker),
        );
        host.bind_permission_gate(&transport.permission_broker, &runtime);
        let create_host = Arc::clone(&host);
        let create = thread::spawn(move || {
            create_host.test_create_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "command": "cmd.exe",
                "args": ["/c", "exit"]
            }))
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while transport.permission_broker.pending_len() == 0 {
            if create.is_finished() {
                panic!(
                    "create finished without a pending gate: {:?}",
                    create.join()
                );
            }
            if Instant::now() >= deadline {
                panic!("create never reached the terminal permission gate");
            }
            thread::sleep(Duration::from_millis(5));
        }
        let mut killer = AcpKiller {
            process: Arc::new(Mutex::new(child)),
            permission_broker: Arc::clone(&transport.permission_broker),
            transport: Arc::clone(&transport),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let started = Instant::now();
        killer.kill();
        let result = create.join().expect("create thread");
        let elapsed = started.elapsed();
        let _ = std::fs::remove_dir_all(&cwd);
        let error = result.expect_err("kill must deny the pending terminal create");
        assert!(
            elapsed < Duration::from_secs(2),
            "kill left the terminal gate blocked for {elapsed:?}"
        );
        assert_eq!(error.code, -32001);
        assert_eq!(error.message, "the user denied this command");
        assert_eq!(host.spawned_count(), 0);
    }

    fn attached_runtime(
        session_id: &str,
        broker: Arc<PermissionBroker>,
    ) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
        let runtime = SessionRuntime::for_acp(session_id.to_string(), None, Arc::clone(&broker));
        let conn = ConnHandle::new(1);
        let generation = runtime.try_attach(None, &conn, true).expect("attach");
        conn.track(session_id, Arc::clone(&runtime), false, None, generation);
        (runtime, conn)
    }

    fn event_kinds(conn: &ConnHandle) -> Vec<&'static str> {
        conn.pull_events()
            .into_iter()
            .map(|event| match event.envelope.event {
                SessionEvent::AgentFinished { .. } => "finished",
                SessionEvent::AgentError { .. } => "error",
                SessionEvent::PermissionRequest { .. } => "permission",
                SessionEvent::PermissionResolved { .. } => "permission_resolved",
                SessionEvent::AgentThought { .. } => "thought",
                SessionEvent::Snapshot { .. } => "snapshot",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn late_prompt_result_after_cancel_is_not_a_second_outcome() {
        let (broker, _) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let mut reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::from([7u64]))),
            "stub-session".to_string(),
            broker,
        );
        reader.turn.start_prompt(7);
        reader.turn.abandon_live_prompt();
        let _ = event_kinds(&conn);
        reader
            .feed(
                br#"{"jsonrpc":"2.0","id":7,"result":{"stopReason":"end_turn"}}"#
                    .as_ref()
                    .iter()
                    .copied()
                    .chain(std::iter::once(b'\n'))
                    .collect::<Vec<_>>()
                    .as_slice(),
                &runtime,
            )
            .expect("feed");
        let kinds = event_kinds(&conn);
        assert!(
            !kinds.contains(&"finished"),
            "timed-out turn published AgentFinished: {kinds:?}"
        );
    }

    #[test]
    fn permission_after_turn_cancel_is_not_shown_to_the_user() {
        let (broker, sent) = test_broker();
        let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let reader = AcpReader::for_test(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            Arc::clone(&broker),
        );
        reader.turn.start_prompt(1);
        reader.turn.abandon_live_prompt();
        let _ = event_kinds(&conn);
        reader.dispatch_permission(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 88,
                "method": "session/request_permission",
                "params": {
                    "sessionId": "stub-session",
                    "title": "Run command",
                    "toolCall": {"toolCallId": "late-perm"},
                    "options": [{"optionId": "allow", "name": "Allow once", "kind": "allow_once"}]
                }
            }),
            &runtime,
        );
        let kinds = event_kinds(&conn);
        assert_eq!(broker.pending_len(), 0, "late permission stayed pending");
        assert!(
            !kinds.contains(&"permission"),
            "cancelled turn published a permission prompt: {kinds:?}"
        );
        let sent = sent.lock().expect("sent lock");
        assert!(
            sent.iter()
                .any(|(id, result)| *id == 88 && result["outcome"]["outcome"] == "cancelled"),
            "agent was not told the late permission was cancelled: {sent:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn cancel_closure_does_not_keep_transport_alive() {
        use super::{AcpHost, AcpTransport};
        use crate::process_tree::JobObject;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut child = Command::new("ping.exe")
            .args(["-n", "2", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("ping");
        let stdin = child.stdin.take().expect("stdin");
        let cwd = std::env::temp_dir();
        let host = AcpHost::new(cwd.clone(), cwd, Arc::new(JobObject::new().expect("job")));
        let transport = Arc::new(AcpTransport::new(stdin, host));
        transport.bind_turn();
        let weak = Arc::downgrade(&transport);
        drop(transport);
        let leaked = weak.upgrade();
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            leaked.is_none(),
            "TurnWatch cancel closure kept AcpTransport alive after the session dropped it"
        );
    }

    #[cfg(windows)]
    #[test]
    fn reader_keeps_dispatching_while_a_host_call_is_blocked() {
        use super::{AcpHost, AcpTransport};
        use crate::process_tree::JobObject;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let cwd = std::env::temp_dir().join(format!(
            "devboule-acp-e-cwd-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let runtime_dir = std::env::temp_dir().join(format!(
            "devboule-acp-e-rt-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&runtime_dir).expect("runtime");
        let host = AcpHost::new(
            cwd.clone(),
            runtime_dir.clone(),
            Arc::new(JobObject::new().expect("job")),
        );
        host.set_session_id("stub-session".to_string());
        let gap = Arc::new(Barrier::new(2));
        host.set_create_gap(Arc::clone(&gap));
        let mut child = Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("ping");
        let stdin = child.stdin.take().expect("stdin");
        let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
        transport.set_session_id("stub-session".to_string());
        let (broker, _) = test_broker();
        let (session_runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
        let mut reader = AcpReader::for_test_with_transport(
            Arc::new(Mutex::new(HashSet::new())),
            "stub-session".to_string(),
            Arc::clone(&broker),
            Arc::clone(&host),
            transport,
        );
        let create = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "terminal/create",
            "params": {
                "sessionId": "stub-session",
                "command": "cmd.exe",
                "args": ["/c", "exit"]
            }
        });
        let thought = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "stub-session",
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "still-alive"}
                }
            }
        });
        let mut bytes = serde_json::to_vec(&create).expect("create line");
        bytes.push(b'\n');
        bytes.extend(serde_json::to_vec(&thought).expect("thought line"));
        bytes.push(b'\n');
        let feed_runtime = Arc::clone(&session_runtime);
        let feed_thread = thread::spawn(move || reader.feed(&bytes, &feed_runtime));
        let allow_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(id) = broker.pending_ids().into_iter().next() {
                broker
                    .respond(&id, PermissionOutcome::AllowOnce)
                    .expect("allow blocked terminal/create so it can hit the create gap");
                break;
            }
            if Instant::now() >= allow_deadline {
                panic!("terminal/create never registered a host permission");
            }
            thread::sleep(Duration::from_millis(5));
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut saw_thought = false;
        while Instant::now() < deadline {
            if conn.pull_events().iter().any(|event| {
                matches!(
                    &event.envelope.event,
                    SessionEvent::AgentThought { text, .. } if text == "still-alive"
                )
            }) {
                saw_thought = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        gap.wait();
        feed_thread.join().expect("feed thread").expect("feed");
        host.shutdown();
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(runtime_dir);
        assert!(
            saw_thought,
            "reader stayed blocked on terminal/create and never dispatched the next update"
        );
    }
}
