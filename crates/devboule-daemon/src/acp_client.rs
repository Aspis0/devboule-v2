//! ACP stdio transport for live agent sessions.
//!
//! This module owns only the process and protocol adapters. The parent
//! session module still owns the runtime, attachment queue, coalescer,
//! journal, liveness monitor, registry and teardown order.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use devboule_protocol::{ErrorCode, PermissionOption, PermissionOutcome, SessionEvent, WireError};

use crate::paths::RuntimePaths;
use crate::process_tree::JobObject;
use crate::server::ServerState;

use super::PtyCommand;
use super::{
    ReaderDispatch, SessionKiller, SessionRuntime, SpawnedSession, StderrSource, WaitableChild,
};

const COMMAND_ENV: &str = "DEVBOULE_ACP_COMMAND";

/// Two minutes gives a person enough time to inspect a command while still
/// bounding an ACP agent that is waiting on a viewer who has gone away. ACP
/// has no permission deadline of its own, so expiry sends `Cancelled`: no
/// operation was granted and the agent already understands that state.
pub const ACP_PERMISSION_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_PENDING_ACP_PERMISSIONS: usize = 32;
const MAX_ACP_PERMISSION_FIELD_BYTES: usize = 8 * 1024;
const MAX_ACP_PERMISSION_LINE_BYTES: usize = 256 * 1024;
const MAX_ACP_PERMISSION_OPTIONS: usize = 32;

type PermissionSender = dyn Fn(u64, serde_json::Value) -> io::Result<()> + Send + Sync;

struct PendingPermission {
    acp_id: u64,
    tool_call_id: String,
    session_id: String,
    request: SessionEvent,
    runtime: std::sync::Weak<SessionRuntime>,
    done: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

pub(super) struct PermissionBroker {
    sender: Arc<PermissionSender>,
    pending: Mutex<HashMap<String, Arc<PendingPermission>>>,
    require_journal: bool,
    #[cfg(test)]
    after_take_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    fail_next_timeout_spawn: AtomicBool,
}

#[derive(Debug)]
pub(super) enum PermissionResponseError {
    NotFound,
    InvalidRequest(String),
    Io(io::Error),
}

impl fmt::Display for PermissionResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("permission request is no longer pending"),
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::Io(error) => write!(
                formatter,
                "could not answer ACP permission request: {error}"
            ),
        }
    }
}

impl PermissionBroker {
    fn new(stdin: Arc<Mutex<ChildStdin>>) -> Arc<Self> {
        Arc::new(Self {
            sender: Arc::new(move |id, result| {
                let mut bytes = serde_json::to_vec(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                }))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                bytes.push(b'\n');
                let mut stdin = stdin
                    .lock()
                    .map_err(|_| io::Error::other("ACP stdin lock poisoned"))?;
                stdin.write_all(&bytes)?;
                stdin.flush()
            }),
            pending: Mutex::new(HashMap::new()),
            require_journal: true,
            #[cfg(test)]
            after_take_hook: Mutex::new(None),
            #[cfg(test)]
            fail_next_timeout_spawn: AtomicBool::new(false),
        })
    }

    #[cfg(test)]
    fn for_test(sender: Arc<PermissionSender>) -> Arc<Self> {
        Arc::new(Self {
            sender,
            pending: Mutex::new(HashMap::new()),
            require_journal: false,
            #[cfg(test)]
            after_take_hook: Mutex::new(None),
            #[cfg(test)]
            fail_next_timeout_spawn: AtomicBool::new(false),
        })
    }

    fn register(
        &self,
        acp_id: u64,
        request: SessionEvent,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<Arc<PendingPermission>, PermissionResponseError> {
        let tool_call_id = match &request {
            SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id.clone(),
            _ => {
                return Err(PermissionResponseError::InvalidRequest(
                    "not a permission request".to_string(),
                ));
            }
        };
        validate_permission_request(&tool_call_id, &request)?;
        let pending = Arc::new(PendingPermission {
            acp_id,
            tool_call_id: tool_call_id.clone(),
            session_id: runtime.session_id.clone(),
            request,
            runtime: Arc::downgrade(runtime),
            done: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        });
        let mut entries = self
            .pending
            .lock()
            .map_err(|_| io_error("permission broker lock poisoned"))?;
        if entries.contains_key(&tool_call_id) {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "permission request {tool_call_id} is already pending"
            )));
        }
        let session_pending = entries
            .values()
            .filter(|pending| pending.session_id == runtime.session_id)
            .count();
        if session_pending >= MAX_PENDING_ACP_PERMISSIONS {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "session has reached the maximum of {MAX_PENDING_ACP_PERMISSIONS} pending permission requests"
            )));
        }
        entries.insert(tool_call_id, Arc::clone(&pending));
        Ok(pending)
    }

    pub(super) fn respond(
        &self,
        tool_call_id: &str,
        outcome: PermissionOutcome,
    ) -> Result<(), PermissionResponseError> {
        let pending = self.take(tool_call_id, None)?;
        #[cfg(test)]
        self.run_after_take_hook();
        let option = match &pending.request {
            SessionEvent::PermissionRequest { options, .. } => select_option(options, outcome),
            _ => None,
        };
        let Some(option) = option else {
            let reason = unsupported_outcome_reason(&pending, outcome);
            return match self.complete(
                &pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
                "cancelled",
            ) {
                Ok(()) => Err(PermissionResponseError::InvalidRequest(reason)),
                Err(error) => Err(error),
            };
        };
        // Only the exact one-shot ACP option is selectable. A durable option
        // is never substituted for the label the user saw; if it is the only
        // option, the request is cancelled and the UI receives the reason.
        let result = serde_json::json!({
            "outcome": { "outcome": "selected", "optionId": option.option_id }
        });
        self.complete(
            &pending,
            result,
            match outcome {
                PermissionOutcome::AllowOnce => "allow_once",
                PermissionOutcome::Deny => "deny",
            },
        )
    }

    fn expire(&self, tool_call_id: &str, expected: &Arc<PendingPermission>) -> bool {
        self.cancel(tool_call_id, expected, "timeout")
    }

    fn cancel(
        &self,
        tool_call_id: &str,
        expected: &Arc<PendingPermission>,
        journal_outcome: &str,
    ) -> bool {
        let Ok(pending) = self.take(tool_call_id, Some(expected)) else {
            return false;
        };
        self.complete(
            &pending,
            serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            journal_outcome,
        )
        .is_ok()
    }

    fn cancel_all(&self) {
        let pending = self
            .pending
            .lock()
            .map(|mut entries| entries.drain().map(|(_, pending)| pending).collect())
            .unwrap_or_else(|_| Vec::new());
        for pending in pending {
            let _ = self.complete(
                &pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
                "cancelled",
            );
        }
    }

    fn take(
        &self,
        tool_call_id: &str,
        expected: Option<&Arc<PendingPermission>>,
    ) -> Result<Arc<PendingPermission>, PermissionResponseError> {
        let mut entries = self
            .pending
            .lock()
            .map_err(|_| io_error("permission broker lock poisoned"))?;
        let Some(current) = entries.get(tool_call_id) else {
            return Err(PermissionResponseError::NotFound);
        };
        if let Some(expected) = expected {
            if !Arc::ptr_eq(current, expected) {
                return Err(PermissionResponseError::NotFound);
            }
        }
        entries
            .remove(tool_call_id)
            .ok_or(PermissionResponseError::NotFound)
    }

    fn complete(
        &self,
        pending: &Arc<PendingPermission>,
        result: serde_json::Value,
        journal_outcome: &str,
    ) -> Result<(), PermissionResponseError> {
        let runtime = pending.runtime.upgrade();
        let recorded = runtime
            .as_ref()
            .map(|runtime| {
                runtime.record_permission_decision(
                    &pending.tool_call_id,
                    journal_outcome,
                    &pending.request,
                ) || !self.require_journal
            })
            .unwrap_or(!self.require_journal);
        if !recorded {
            let send_result = (self.sender)(
                pending.acp_id,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            );
            if let Some(runtime) = runtime {
                runtime.remove_permission_request(&pending.tool_call_id);
            }
            self.mark_done(pending);
            return match send_result {
                Ok(()) => Err(PermissionResponseError::Io(io::Error::other(
                    "permission decision was not journaled; ACP request was cancelled",
                ))),
                Err(error) => Err(PermissionResponseError::Io(error)),
            };
        }
        let send_result = (self.sender)(pending.acp_id, result);
        if let Some(runtime) = runtime {
            runtime.remove_permission_request(&pending.tool_call_id);
        }
        self.mark_done(pending);
        send_result.map_err(PermissionResponseError::Io)
    }

    fn mark_done(&self, pending: &Arc<PendingPermission>) {
        let (done, wake) = &*pending.done;
        if let Ok(mut completed) = done.lock() {
            *completed = true;
            wake.notify_all();
        }
    }

    #[cfg(test)]
    fn pending_len(&self) -> usize {
        self.pending
            .lock()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }

    #[cfg(test)]
    fn set_after_take_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut stored) = self.after_take_hook.lock() {
            *stored = Some(hook);
        }
    }

    #[cfg(test)]
    fn run_after_take_hook(&self) {
        let hook = self
            .after_take_hook
            .lock()
            .ok()
            .and_then(|mut stored| stored.take());
        if let Some(hook) = hook {
            hook();
        }
    }

    #[cfg(test)]
    fn fail_next_timeout_spawn(&self) {
        self.fail_next_timeout_spawn.store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn take_timeout_spawn_failure(&self) -> bool {
        self.fail_next_timeout_spawn.swap(false, Ordering::AcqRel)
    }
}

fn validate_permission_request(
    tool_call_id: &str,
    request: &SessionEvent,
) -> Result<(), PermissionResponseError> {
    validate_permission_field("tool_call_id", tool_call_id)?;
    let SessionEvent::PermissionRequest {
        title,
        description,
        command,
        cwd,
        options,
        ..
    } = request
    else {
        return Err(PermissionResponseError::InvalidRequest(
            "not a permission request".to_string(),
        ));
    };
    validate_permission_field("title", title)?;
    for (field, value) in [
        ("description", description.as_deref()),
        ("command", command.as_deref()),
        ("cwd", cwd.as_deref()),
    ] {
        if let Some(value) = value {
            validate_permission_field(field, value)?;
        }
    }
    if options.is_empty() {
        return Err(PermissionResponseError::InvalidRequest(
            "permission request has no options".to_string(),
        ));
    }
    if options.len() > MAX_ACP_PERMISSION_OPTIONS {
        return Err(PermissionResponseError::InvalidRequest(format!(
            "permission request has more than the maximum of {MAX_ACP_PERMISSION_OPTIONS} options"
        )));
    }
    for option in options {
        validate_permission_field("option_id", &option.option_id)?;
        validate_permission_field("option name", &option.name)?;
        validate_permission_field("option kind", &option.kind)?;
    }
    Ok(())
}

fn validate_permission_field(field: &str, value: &str) -> Result<(), PermissionResponseError> {
    if value.is_empty() {
        return Err(PermissionResponseError::InvalidRequest(format!(
            "permission request has an empty {field}"
        )));
    }
    if value.len() > MAX_ACP_PERMISSION_FIELD_BYTES {
        return Err(PermissionResponseError::InvalidRequest(format!(
            "permission request {field} exceeds {MAX_ACP_PERMISSION_FIELD_BYTES} bytes"
        )));
    }
    Ok(())
}

fn select_option(
    options: &[PermissionOption],
    outcome: PermissionOutcome,
) -> Option<&PermissionOption> {
    let kind = match outcome {
        PermissionOutcome::AllowOnce => "allow_once",
        PermissionOutcome::Deny => "reject_once",
    };
    options.iter().find(|option| option.kind == kind)
}

fn unsupported_outcome_reason(pending: &PendingPermission, outcome: PermissionOutcome) -> String {
    let (label, required_kind) = match outcome {
        PermissionOutcome::AllowOnce => ("Allow once", "allow_once"),
        PermissionOutcome::Deny => ("Deny", "reject_once"),
    };
    let offered = match &pending.request {
        SessionEvent::PermissionRequest { options, .. } => options
            .iter()
            .map(|option| option.kind.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    };
    format!(
        "Could not honor {label}: ACP did not offer the exact one-shot option '{required_kind}' (offered: {offered}); request was cancelled"
    )
}

fn io_error(message: &str) -> PermissionResponseError {
    PermissionResponseError::Io(io::Error::other(message))
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
    let mut argv: Vec<String> = match std::env::var(COMMAND_ENV) {
        Ok(argv) => serde_json::from_str(&argv).map_err(|error| {
            WireError::new(
                ErrorCode::InvalidRequest,
                format!("{COMMAND_ENV} must be a non-empty JSON string array: {error}"),
            )
        })?,
        Err(_) => {
            let Some(agent) = crate::provider_catalog::first_acp_available() else {
                return Err(WireError::new(
                    ErrorCode::Io,
                    format!(
                        "No ACP-capable agent was found on PATH. Set {COMMAND_ENV} to a non-empty JSON string array to choose an ACP command explicitly."
                    ),
                ));
            };
            let acp_command = agent
                .acp_command
                .expect("an ACP-capable catalog entry has an ACP command");
            let mut argv = Vec::with_capacity(acp_command.len());
            argv.push(agent.executable.to_string_lossy().into_owned());
            argv.extend(acp_command.into_iter().skip(1));
            argv
        }
    };
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
    );
    Ok(SpawnedSession {
        process_job,
        master: None,
        killer: Box::new(killer),
        child: Box::new(AcpWaitableChild { process }),
        writer: Arc::new(Mutex::new(Box::new(writer) as Box<dyn Write + Send>)),
        reader: Box::new(reader),
        reader_dispatch: Some(Box::new(reader_dispatch)),
        stderr: Some(Box::new(stderr_source)),
        permission_broker: Some(Arc::clone(&transport.permission_broker)),
    })
}

fn terminate_process(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

struct AcpTransport {
    stdin: Arc<Mutex<ChildStdin>>,
    permission_broker: Arc<PermissionBroker>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashSet<u64>>>,
    session_id: Mutex<Option<String>>,
}

impl AcpTransport {
    fn new(stdin: ChildStdin) -> Self {
        let stdin = Arc::new(Mutex::new(stdin));
        Self {
            permission_broker: PermissionBroker::new(Arc::clone(&stdin)),
            stdin,
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
    permission_broker: Arc<PermissionBroker>,
    cancelled: Arc<AtomicBool>,
}

impl SessionKiller for AcpKiller {
    fn kill(&mut self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.permission_broker.cancel_all();
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
            permission_broker: Arc::clone(&self.permission_broker),
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
    discarding_oversized_line: bool,
    pending: Arc<Mutex<HashSet<u64>>>,
    session_id: String,
    permission_broker: Arc<PermissionBroker>,
}

impl AcpReader {
    fn new(
        pending: Arc<Mutex<HashSet<u64>>>,
        session_id: String,
        permission_broker: Arc<PermissionBroker>,
    ) -> Self {
        Self {
            buffer: Vec::new(),
            discarding_oversized_line: false,
            pending,
            session_id,
            permission_broker,
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
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String> {
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
        if value.get("method").and_then(serde_json::Value::as_str)
            == Some("session/request_permission")
        {
            self.dispatch_permission(&value, runtime);
            return;
        }
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

    fn cancel_permission_request(&self, id: u64, runtime: &SessionRuntime, reason: String) {
        let _ = (self.permission_broker.sender)(
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
                cwd,
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
        if delivery == Some(false) {
            let _ =
                self.permission_broker
                    .cancel(&tool_call_id, &pending, "capability_not_supported");
            return;
        }
        let broker = Arc::clone(&self.permission_broker);
        let tool_call_id_for_timeout = tool_call_id.clone();
        let timeout_pending = Arc::clone(&pending);
        #[cfg(test)]
        let timeout_spawn_forced = self.permission_broker.take_timeout_spawn_failure();
        #[cfg(not(test))]
        let timeout_spawn_forced = false;
        let timeout_spawn = if timeout_spawn_forced {
            Err(io::Error::other("test timeout spawn failure"))
        } else {
            std::thread::Builder::new()
                .name("acp-permission-timeout".to_string())
                .spawn(move || {
                    let (done, wake) = &*timeout_pending.done;
                    let deadline = std::time::Instant::now() + ACP_PERMISSION_TIMEOUT;
                    let Ok(mut completed) = done.lock() else {
                        return;
                    };
                    while !*completed {
                        let remaining =
                            deadline.saturating_duration_since(std::time::Instant::now());
                        if remaining.is_zero() {
                            break;
                        }
                        let Ok((next, timed_out)) = wake.wait_timeout(completed, remaining) else {
                            return;
                        };
                        completed = next;
                        if timed_out.timed_out() {
                            break;
                        }
                    }
                    if !*completed {
                        drop(completed);
                        let _ = broker.expire(&tool_call_id_for_timeout, &timeout_pending);
                    }
                })
        };
        let timeout_started = timeout_spawn.is_ok();
        if let Err(error) = timeout_spawn {
            let _ = self
                .permission_broker
                .cancel(&tool_call_id, &pending, "timeout_spawn_failed");
            self.publish(
                runtime,
                SessionEvent::AgentError {
                    message: format!(
                        "Could not start the ACP permission deadline; the request was cancelled: {error}"
                    ),
                },
            );
        }
        if timeout_started {
            let _ = runtime.publish_agent_event(event, None);
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
            // Thinking, plans, modes and models remain outside this slice.
            // Unknown future updates are safe to ignore.
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
    use super::{
        complete_lines, AcpReader, PermissionBroker, PermissionSender, ACP_PERMISSION_TIMEOUT,
        MAX_ACP_PERMISSION_FIELD_BYTES, MAX_ACP_PERMISSION_LINE_BYTES, MAX_PENDING_ACP_PERMISSIONS,
    };
    use crate::journal::Journal;
    use crate::session::{ConnHandle, ReaderDispatch, SessionRuntime};
    use devboule_protocol::{PermissionOption, PermissionOutcome, SessionEvent};
    use rusqlite::Connection;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn permission_with_kinds(tool_call_id: &str, kinds: &[(&str, &str)]) -> SessionEvent {
        SessionEvent::PermissionRequest {
            tool_call_id: tool_call_id.to_string(),
            title: "Run command".to_string(),
            description: None,
            command: Some("echo test".to_string()),
            cwd: None,
            options: kinds
                .iter()
                .map(|(option_id, kind)| PermissionOption {
                    option_id: (*option_id).to_string(),
                    name: (*kind).to_string(),
                    kind: (*kind).to_string(),
                })
                .collect(),
        }
    }

    fn permission(tool_call_id: &str) -> SessionEvent {
        permission_with_kinds(
            tool_call_id,
            &[("allow", "allow_once"), ("deny", "reject_once")],
        )
    }

    fn permission_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "devboule-permission-{label}-{}-{nonce}.sqlite",
            std::process::id()
        ))
    }

    fn test_broker() -> (Arc<PermissionBroker>, Arc<Mutex<SentResponses>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sent_for_sender = Arc::clone(&sent);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            Ok(())
        });
        (PermissionBroker::for_test(sender), sent)
    }

    type SentResponses = Vec<(u64, serde_json::Value)>;

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
    fn durable_only_permission_is_cancelled_and_reports_why() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                51,
                permission_with_kinds("durable-only", &[("always", "allow_always")]),
                &runtime,
            )
            .expect("register");

        let error = broker
            .respond("durable-only", PermissionOutcome::AllowOnce)
            .expect_err("a durable option must not satisfy allow once");
        assert!(error.to_string().contains("allow_once"));
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 51);
        assert_eq!(sent[0].1["outcome"]["outcome"], "cancelled");
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn invalid_response_interleaving_preserves_new_registration() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                52,
                permission_with_kinds("reused", &[("always", "allow_always")]),
                &runtime,
            )
            .expect("old register");
        let broker_for_hook = Arc::downgrade(&broker);
        let runtime_for_hook = Arc::clone(&runtime);
        broker.set_after_take_hook(Arc::new(move || {
            broker_for_hook
                .upgrade()
                .expect("broker")
                .register(53, permission("reused"), &runtime_for_hook)
                .expect("new registration");
        }));

        let error = broker
            .respond("reused", PermissionOutcome::Deny)
            .expect_err("old request has no one-shot deny option");
        assert!(error.to_string().contains("reject_once"));
        broker
            .respond("reused", PermissionOutcome::AllowOnce)
            .expect("new registration remains answerable");

        let sent = sent.lock().expect("sent lock");
        assert!(sent
            .iter()
            .any(|(id, result)| { *id == 52 && result["outcome"]["outcome"] == "cancelled" }));
        assert!(sent
            .iter()
            .any(|(id, result)| { *id == 53 && result["outcome"]["optionId"] == "allow" }));
    }

    #[test]
    fn broker_rejects_permission_floods_at_the_per_session_limit() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        for index in 0..MAX_PENDING_ACP_PERMISSIONS {
            broker
                .register(
                    index as u64,
                    permission(&format!("flood-{index}")),
                    &runtime,
                )
                .expect("within limit");
        }
        let error = match broker.register(999, permission("flood-over-limit"), &runtime) {
            Ok(_) => panic!("limit must reject another request"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("maximum"));
        assert_eq!(broker.pending_len(), MAX_PENDING_ACP_PERMISSIONS);
    }

    #[test]
    fn timeout_spawn_failure_cancels_and_removes_the_request() {
        let (broker, sent) = test_broker();
        broker.fail_next_timeout_spawn();
        let reader = AcpReader::new(
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
        let reader = AcpReader::new(
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
        let mut reader = AcpReader::new(
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
        assert!(second.pull_events().is_empty());
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
        assert!(conn.pull_events().is_empty());
    }

    #[test]
    fn two_permission_requests_correlate_independently() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(11, permission("first"), &runtime)
            .expect("first");
        broker
            .register(12, permission("second"), &runtime)
            .expect("second");
        broker
            .respond("second", PermissionOutcome::Deny)
            .expect("second response");
        broker
            .respond("first", PermissionOutcome::AllowOnce)
            .expect("first response");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, 12);
        assert_eq!(sent[0].1["outcome"]["optionId"], "deny");
        assert_eq!(sent[1].0, 11);
        assert_eq!(sent[1].1["outcome"]["optionId"], "allow");
    }

    #[test]
    fn permission_response_races_timeout_with_one_journaled_reply() {
        let path = permission_path("race");
        let journal = Arc::new(Journal::open(&path).expect("journal"));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let recorded_before_send = Arc::new(AtomicBool::new(false));
        let sender_started = Arc::new(Barrier::new(2));
        let sender_release = Arc::new(Barrier::new(2));
        let path_for_sender = path.clone();
        let sent_for_sender = Arc::clone(&sent);
        let recorded_for_sender = Arc::clone(&recorded_before_send);
        let entered_for_sender = Arc::clone(&sender_started);
        let release_for_sender = Arc::clone(&sender_release);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            let conn = Connection::open(&path_for_sender).expect("inspect journal");
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM permissions WHERE session_id = ?1 AND request_id = ?2",
                    ["s.permission.race", "race"],
                    |row| row.get(0),
                )
                .expect("permission row count");
            recorded_for_sender.store(count == 1, Ordering::Release);
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            entered_for_sender.wait();
            release_for_sender.wait();
            Ok(())
        });
        let broker = PermissionBroker::for_test(sender);
        let runtime = Arc::new(SessionRuntime::for_acp(
            "s.permission.race".to_string(),
            Some(Arc::clone(&journal)),
            Arc::clone(&broker),
        ));
        let pending = broker
            .register(21, permission("race"), &runtime)
            .expect("register");
        let start = Arc::new(Barrier::new(3));
        let respond_broker = Arc::clone(&broker);
        let respond_start = Arc::clone(&start);
        let respond_thread = thread::spawn(move || {
            respond_start.wait();
            respond_broker.respond("race", PermissionOutcome::AllowOnce)
        });
        let expire_broker = Arc::clone(&broker);
        let expire_start = Arc::clone(&start);
        let expire_thread = thread::spawn(move || {
            expire_start.wait();
            expire_broker.expire("race", &pending)
        });
        start.wait();
        sender_started.wait();
        sender_release.wait();
        let _ = respond_thread.join().expect("respond thread");
        let _ = expire_thread.join().expect("expiry thread");

        journal.flush().expect("journal flush");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.iter().filter(|(id, _)| *id == 21).count(), 1);
        assert_eq!(sent.len(), 1);
        assert!(recorded_before_send.load(Ordering::Acquire));
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn daemon_shutdown_cancels_outstanding_request_before_reconnect() {
        let (old, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        old.register(31, permission("dead"), &runtime)
            .expect("register");
        old.cancel_all();
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(old.pending_len(), 0);
        drop(old);
    }

    #[test]
    fn duplicate_or_conflicting_responses_are_rejected() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(41, permission("once"), &runtime)
            .expect("register");
        broker
            .respond("once", PermissionOutcome::AllowOnce)
            .expect("first response");
        assert!(matches!(
            broker.respond("once", PermissionOutcome::AllowOnce),
            Err(super::PermissionResponseError::NotFound)
        ));
        assert!(matches!(
            broker.respond("once", PermissionOutcome::Deny),
            Err(super::PermissionResponseError::NotFound)
        ));
        assert_eq!(ACP_PERMISSION_TIMEOUT.as_secs(), 120);
    }
}
