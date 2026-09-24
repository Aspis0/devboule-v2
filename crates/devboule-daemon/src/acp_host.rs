//! ACP client methods the agent calls on us: filesystem and terminals.
//!
//! Terminals reuse the daemon's ConPTY path with a fresh per-terminal Job Object. `terminal/create`
//! does not spawn until the user allows that exact command through the
//! permission broker (`allow_once` / `reject_once`). If the agent omits
//! `args`, the `command` string is a shell line: Windows writes it verbatim
//! to a tempfile and runs `cmd.exe /d /c <file>`; elsewhere `/bin/sh -c`.
//! The permission prompt shows the original line plus env. The guardian does not
//! otherwise parse the command string: an approved process runs with the
//! user's privileges. Existing path guards (`authorize_path`,
//! `touches_runtime`) still apply and are not bypassed.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};

use agent_client_protocol::schema::v1::{
    CreateTerminalRequest, CreateTerminalResponse, KillTerminalResponse, ReadTextFileRequest,
    ReadTextFileResponse, ReleaseTerminalResponse, TerminalExitStatus, TerminalId,
    TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use devboule_protocol::{PermissionEnvVar, PermissionOption, SessionEvent};
use portable_pty::{CommandBuilder, PtySize};

use super::permission_broker::{HostDecision, PermissionBroker};
use super::SessionRuntime;
use crate::process_tree::JobObject;

const MAX_FS_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_OUTPUT_LIMIT: u64 = 1024 * 1024;
const HARD_OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;
const MAX_ACP_TERMINALS: usize = 16;
const PTY_COLS: u16 = 80;
const PTY_ROWS: u16 = 24;

pub(super) type RpcRespond =
    Arc<dyn Fn(serde_json::Value, Result<serde_json::Value, RpcError>) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RpcError {
    pub code: i32,
    pub message: String,
}

/// The ACP schema's generic `ResourceNotFound` ("A given resource, such as
/// a file, was not found"): the code for **any** missed resource, not a
/// session code — this host answers it for terminals and file reads. The
/// client side reads it as evidence of a disowned session only when the
/// error payload also names the session it was asked to load
/// (`acp_client::session_disown`).
pub(super) const RESOURCE_NOT_FOUND: i32 = -32002;

impl RpcError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Method not found: {method}"),
        }
    }

    fn resource_not_found(message: impl Into<String>) -> Self {
        Self {
            code: RESOURCE_NOT_FOUND,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
        }
    }

    /// Server-defined JSON-RPC error for a user (or timeout/cancel) denial.
    ///
    /// Must not be `-32000`: ACP schema `ErrorCode::AuthRequired` is `-32000`
    /// (`agent-client-protocol-schema` `src/v1/error.rs`). An agent that maps
    /// that code would treat a denied command as a missing login. `-32002` is
    /// already `ResourceNotFound`. The free server-defined slot is `-32001`.
    fn denied(message: impl Into<String>) -> Self {
        Self {
            code: -32001,
            message: message.into(),
        }
    }

    pub(super) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "code": self.code, "message": self.message })
    }
}

pub(super) struct AcpHost {
    session_id: Mutex<String>,
    cwd: PathBuf,
    runtime_dir: PathBuf,
    terminals: Mutex<HashMap<String, TerminalSlot>>,
    next_terminal: AtomicU64,
    max_terminals: usize,
    gate: Mutex<Option<PermissionGate>>,
    #[cfg(test)]
    create_gap: Mutex<Option<Arc<std::sync::Barrier>>>,
    #[cfg(test)]
    spawned: std::sync::atomic::AtomicUsize,
}

#[derive(Clone)]
struct PermissionGate {
    broker: Weak<PermissionBroker>,
    runtime: Weak<SessionRuntime>,
}

enum TerminalSlot {
    Reserved,
    Live(Arc<AcpTerminal>),
}

struct AcpTerminal {
    output: Mutex<BoundedBuffer>,
    exit: Mutex<Option<TerminalExitStatus>>,
    exit_cvar: Condvar,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    master: Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>,
    job: Mutex<Option<JobObject>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    waiter: Mutex<Option<JoinHandle<()>>>,
    rpc_waiters: Mutex<Vec<JoinHandle<()>>>,
    released: AtomicBool,
    batch_file: Option<PathBuf>,
}

struct BoundedBuffer {
    bytes: Vec<u8>,
    limit: u64,
    truncated: bool,
}

impl BoundedBuffer {
    fn new(limit: u64) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            truncated: false,
        }
    }

    fn push(&mut self, data: &[u8]) {
        self.bytes.extend_from_slice(data);
        if (self.bytes.len() as u64) <= self.limit {
            return;
        }
        let excess = self.bytes.len() as u64 - self.limit;
        let mut drop_at = excess as usize;
        while drop_at < self.bytes.len() && is_utf8_continuation(self.bytes[drop_at]) {
            drop_at += 1;
        }
        self.bytes.drain(..drop_at);
        self.truncated = true;
    }

    fn snapshot(&self) -> (String, bool) {
        (
            String::from_utf8_lossy(&self.bytes).into_owned(),
            self.truncated,
        )
    }
}

fn is_utf8_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

impl AcpHost {
    pub(super) fn new(cwd: PathBuf, runtime_dir: PathBuf) -> Arc<Self> {
        Self::with_terminal_limit(cwd, runtime_dir, MAX_ACP_TERMINALS)
    }

    pub(super) fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn with_terminal_limit(cwd: PathBuf, runtime_dir: PathBuf, max_terminals: usize) -> Arc<Self> {
        Arc::new(Self {
            session_id: Mutex::new(String::new()),
            cwd: canonicalize_existing_or_lexical(&cwd),
            runtime_dir: canonicalize_existing_or_lexical(&runtime_dir),
            terminals: Mutex::new(HashMap::new()),
            next_terminal: AtomicU64::new(1),
            max_terminals,
            gate: Mutex::new(None),
            #[cfg(test)]
            create_gap: Mutex::new(None),
            #[cfg(test)]
            spawned: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    #[cfg(test)]
    pub(super) fn set_create_gap(&self, barrier: Arc<std::sync::Barrier>) {
        if let Ok(mut gap) = self.create_gap.lock() {
            *gap = Some(barrier);
        }
    }

    #[cfg(test)]
    pub(super) fn live_terminal_count(&self) -> usize {
        self.terminals.lock().map(|map| map.len()).unwrap_or(0)
    }

    #[cfg(test)]
    pub(super) fn spawned_count(&self) -> usize {
        self.spawned.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(super) fn test_create_terminal(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcError> {
        self.create_terminal(params)
    }

    pub(super) fn set_session_id(&self, session_id: String) {
        if let Ok(mut current) = self.session_id.lock() {
            *current = session_id;
        }
    }

    pub(super) fn bind_permission_gate(
        &self,
        broker: &Arc<PermissionBroker>,
        runtime: &Arc<SessionRuntime>,
    ) {
        if let Ok(mut gate) = self.gate.lock() {
            *gate = Some(PermissionGate {
                broker: Arc::downgrade(broker),
                runtime: Arc::downgrade(runtime),
            });
        }
    }

    fn await_user_permission(&self, event: SessionEvent) -> HostDecision {
        let Some(gate) = self.gate.lock().ok().and_then(|guard| guard.clone()) else {
            return HostDecision::Cancelled;
        };
        let Some(broker) = gate.broker.upgrade() else {
            return HostDecision::Cancelled;
        };
        let Some(runtime) = gate.runtime.upgrade() else {
            return HostDecision::Cancelled;
        };
        broker.request_host_permission(event, &runtime)
    }

    fn agent_process_exited(&self) -> bool {
        self.gate
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .and_then(|gate| gate.runtime.upgrade())
            .map(|runtime| runtime.process_exited())
            .unwrap_or(true)
    }

    fn release_reserved_slot(&self, id: &str) {
        if let Ok(mut terminals) = self.terminals.lock() {
            if matches!(terminals.get(id), Some(TerminalSlot::Reserved)) {
                terminals.remove(id);
            }
        }
    }

    fn current_session_id(&self) -> String {
        self.session_id
            .lock()
            .ok()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    pub(super) fn shutdown(&self) {
        let terminals = self
            .terminals
            .lock()
            .map(|mut map| drain_live_terminals(&mut map))
            .unwrap_or_default();
        for terminal in terminals {
            terminal.release();
        }
    }

    pub(super) fn dispatch(
        self: &Arc<Self>,
        method: &str,
        id: serde_json::Value,
        params: serde_json::Value,
        respond: RpcRespond,
    ) {
        let host = Arc::clone(self);
        let method = method.to_string();
        let respond_ok = Arc::clone(&respond);
        let id_ok = id.clone();
        if let Err(error) = thread::Builder::new()
            .name("acp-host-rpc".to_string())
            .spawn(move || host.dispatch_sync(&method, id_ok, params, respond_ok))
        {
            respond(
                id,
                Err(RpcError::internal(format!(
                    "Could not start ACP host work: {error}"
                ))),
            );
        }
    }

    fn dispatch_sync(
        self: &Arc<Self>,
        method: &str,
        id: serde_json::Value,
        params: serde_json::Value,
        respond: RpcRespond,
    ) {
        match method {
            "fs/read_text_file" => respond(id, self.read_text_file(params)),
            "fs/write_text_file" => respond(id, self.write_text_file(params)),
            "terminal/create" => respond(id, self.create_terminal(params)),
            "terminal/output" => respond(id, self.terminal_output(params)),
            "terminal/kill" => respond(id, self.kill_terminal(params)),
            "terminal/release" => respond(id, self.release_terminal(params)),
            "terminal/wait_for_exit" => self.wait_for_exit(id, params, respond),
            other => respond(id, Err(RpcError::method_not_found(other))),
        }
    }

    fn require_session(&self, session_id: &str) -> Result<(), RpcError> {
        let expected = self.current_session_id();
        if expected.is_empty() || session_id != expected {
            return Err(RpcError::invalid_params(
                "ACP client request targeted another session",
            ));
        }
        Ok(())
    }

    fn authorize_path(&self, path: &Path, access: FsAccess) -> Result<PathBuf, RpcError> {
        let resolved = resolve_path(path)?;
        if touches_runtime(&resolved, &self.runtime_dir) {
            return Err(RpcError::invalid_params(format!(
                "refusing to {access} daemon state: {}",
                path.display()
            )));
        }
        if !path_is_within(&resolved, &self.cwd) {
            return Err(RpcError::invalid_params(format!(
                "path is outside the session workspace: {}",
                path.display()
            )));
        }
        Ok(resolved)
    }

    fn read_text_file(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let request: ReadTextFileRequest = serde_json::from_value(params)
            .map_err(|error| RpcError::invalid_params(error.to_string()))?;
        self.require_session(request.session_id.0.as_ref())?;
        let path = self.authorize_path(&request.path, FsAccess::Read)?;
        let metadata = std::fs::metadata(&path).map_err(|error| fs_error(&path, error))?;
        if metadata.len() > MAX_FS_BYTES {
            return Err(RpcError::invalid_params(format!(
                "file exceeds {MAX_FS_BYTES} bytes"
            )));
        }
        let contents = std::fs::read_to_string(&path).map_err(|error| fs_error(&path, error))?;
        let sliced = slice_lines(&contents, request.line, request.limit)?;
        serde_json::to_value(ReadTextFileResponse::new(sliced))
            .map_err(|error| RpcError::internal(error.to_string()))
    }

    fn write_text_file(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let request: WriteTextFileRequest = serde_json::from_value(params)
            .map_err(|error| RpcError::invalid_params(error.to_string()))?;
        self.require_session(request.session_id.0.as_ref())?;
        let path = self.authorize_path(&request.path, FsAccess::Write)?;
        if request.content.len() as u64 > MAX_FS_BYTES {
            return Err(RpcError::invalid_params(format!(
                "write exceeds {MAX_FS_BYTES} bytes"
            )));
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|error| fs_error(parent, error))?;
            }
        }
        std::fs::write(&path, request.content.as_bytes())
            .map_err(|error| fs_error(&path, error))?;
        serde_json::to_value(WriteTextFileResponse::new())
            .map_err(|error| RpcError::internal(error.to_string()))
    }

    fn create_terminal(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let request: CreateTerminalRequest = serde_json::from_value(params)
            .map_err(|error| RpcError::invalid_params(error.to_string()))?;
        self.require_session(request.session_id.0.as_ref())?;
        if request.command.trim().is_empty() {
            return Err(RpcError::invalid_params("terminal command is empty"));
        }
        let cwd = match request.cwd {
            Some(path) => self.authorize_path(&path, FsAccess::Read)?,
            None => self.cwd.clone(),
        };
        let mut env = Vec::new();
        for variable in request.env {
            env.push((variable.name, variable.value));
        }
        let limit = request
            .output_byte_limit
            .unwrap_or(DEFAULT_OUTPUT_LIMIT)
            .min(HARD_OUTPUT_LIMIT);
        let id = {
            let mut terminals = self
                .terminals
                .lock()
                .map_err(|_| RpcError::internal("terminal map lock poisoned"))?;
            if terminals.len() >= self.max_terminals {
                return Err(RpcError::invalid_params(format!(
                    "session has reached the maximum of {} ACP terminals",
                    self.max_terminals
                )));
            }
            let id = format!("t-{}", self.next_terminal.fetch_add(1, Ordering::Relaxed));
            terminals.insert(id.clone(), TerminalSlot::Reserved);
            id
        };
        let plan = spawn_plan(&request.command, &request.args);
        let event = terminal_permission_event(&plan, &cwd, &env);
        match self.await_user_permission(event) {
            HostDecision::Allow => {
                if self.agent_process_exited() {
                    self.release_reserved_slot(&id);
                    return Err(RpcError::denied("the agent is gone"));
                }
            }
            HostDecision::Deny | HostDecision::Timeout | HostDecision::Cancelled => {
                self.release_reserved_slot(&id);
                return Err(RpcError::denied("the user denied this command"));
            }
        }
        #[cfg(test)]
        {
            let gap = self.create_gap.lock().ok().and_then(|guard| guard.clone());
            if let Some(barrier) = gap {
                barrier.wait();
            }
        }
        let prepared = match prepare_spawn(&plan, &self.runtime_dir, &id) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.release_reserved_slot(&id);
                return Err(error);
            }
        };
        #[cfg(test)]
        self.spawned.fetch_add(1, Ordering::SeqCst);
        let spawned = spawn_acp_terminal(
            &prepared.program,
            &prepared.args,
            &cwd,
            &env,
            limit,
            prepared.batch_file.clone(),
        );
        if spawned.is_err() {
            if let Some(path) = &prepared.batch_file {
                let _ = std::fs::remove_file(path);
            }
        }
        let mut terminals = match self.terminals.lock() {
            Ok(terminals) => terminals,
            Err(_) => {
                if let Ok(terminal) = spawned {
                    terminal.release();
                }
                return Err(RpcError::internal("terminal map lock poisoned"));
            }
        };
        match spawned {
            Ok(terminal) => {
                if terminals.remove(&id).is_none() {
                    drop(terminals);
                    terminal.release();
                    return Err(RpcError::internal(
                        "terminal slot was released before spawn completed",
                    ));
                }
                terminals.insert(id.clone(), TerminalSlot::Live(terminal));
                drop(terminals);
                serde_json::to_value(CreateTerminalResponse::new(TerminalId::new(id)))
                    .map_err(|error| RpcError::internal(error.to_string()))
            }
            Err(error) => {
                terminals.remove(&id);
                Err(error)
            }
        }
    }

    fn get_terminal(&self, terminal_id: &str) -> Result<Arc<AcpTerminal>, RpcError> {
        let terminals = self
            .terminals
            .lock()
            .map_err(|_| RpcError::internal("terminal map lock poisoned"))?;
        match terminals.get(terminal_id) {
            Some(TerminalSlot::Live(terminal)) => Ok(Arc::clone(terminal)),
            Some(TerminalSlot::Reserved) | None => Err(RpcError::resource_not_found(format!(
                "unknown terminal {terminal_id}"
            ))),
        }
    }

    fn terminal_output(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let request: TerminalOutputRequest = serde_json::from_value(params)
            .map_err(|error| RpcError::invalid_params(error.to_string()))?;
        self.require_session(request.session_id.0.as_ref())?;
        let terminal = self.get_terminal(request.terminal_id.0.as_ref())?;
        let (output, truncated) = terminal
            .output
            .lock()
            .map_err(|_| RpcError::internal("terminal output lock poisoned"))?
            .snapshot();
        let exit_status = terminal
            .exit
            .lock()
            .map_err(|_| RpcError::internal("terminal exit lock poisoned"))?
            .clone();
        let mut response = TerminalOutputResponse::new(output, truncated);
        if let Some(exit_status) = exit_status {
            response = response.exit_status(exit_status);
        }
        serde_json::to_value(response).map_err(|error| RpcError::internal(error.to_string()))
    }

    fn kill_terminal(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let request: agent_client_protocol::schema::v1::KillTerminalRequest =
            serde_json::from_value(params)
                .map_err(|error| RpcError::invalid_params(error.to_string()))?;
        self.require_session(request.session_id.0.as_ref())?;
        let terminal = self.get_terminal(request.terminal_id.0.as_ref())?;
        terminal.kill();
        serde_json::to_value(KillTerminalResponse::new())
            .map_err(|error| RpcError::internal(error.to_string()))
    }

    fn release_terminal(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let request: agent_client_protocol::schema::v1::ReleaseTerminalRequest =
            serde_json::from_value(params)
                .map_err(|error| RpcError::invalid_params(error.to_string()))?;
        self.require_session(request.session_id.0.as_ref())?;
        let terminal = {
            let mut terminals = self
                .terminals
                .lock()
                .map_err(|_| RpcError::internal("terminal map lock poisoned"))?;
            match terminals.remove(request.terminal_id.0.as_ref()) {
                Some(TerminalSlot::Live(terminal)) => terminal,
                Some(TerminalSlot::Reserved) | None => {
                    return Err(RpcError::resource_not_found(format!(
                        "unknown terminal {}",
                        request.terminal_id.0
                    )));
                }
            }
        };
        terminal.release();
        serde_json::to_value(ReleaseTerminalResponse::new())
            .map_err(|error| RpcError::internal(error.to_string()))
    }

    fn wait_for_exit(&self, id: serde_json::Value, params: serde_json::Value, respond: RpcRespond) {
        let request = match serde_json::from_value::<WaitForTerminalExitRequest>(params) {
            Ok(request) => request,
            Err(error) => {
                respond(id, Err(RpcError::invalid_params(error.to_string())));
                return;
            }
        };
        if let Err(error) = self.require_session(request.session_id.0.as_ref()) {
            respond(id, Err(error));
            return;
        }
        let terminal = match self.get_terminal(request.terminal_id.0.as_ref()) {
            Ok(terminal) => terminal,
            Err(error) => {
                respond(id, Err(error));
                return;
            }
        };
        let respond_ok = Arc::clone(&respond);
        let id_ok = id.clone();
        match std::thread::Builder::new()
            .name("acp-term-wait-rpc".to_string())
            .spawn({
                let terminal = Arc::clone(&terminal);
                move || {
                    let status = terminal.wait_exit();
                    let result = serde_json::to_value(WaitForTerminalExitResponse::new(status))
                        .map_err(|error| RpcError::internal(error.to_string()));
                    respond_ok(id_ok, result);
                }
            }) {
            Ok(handle) => terminal.push_rpc_waiter(handle),
            Err(error) => respond(
                id,
                Err(RpcError::internal(format!(
                    "Could not wait for terminal: {error}"
                ))),
            ),
        }
    }
}

impl Drop for AcpHost {
    fn drop(&mut self) {
        let terminals = self
            .terminals
            .lock()
            .map(|mut map| drain_live_terminals(&mut map))
            .unwrap_or_default();
        for terminal in terminals {
            terminal.release();
        }
    }
}

fn drain_live_terminals(map: &mut HashMap<String, TerminalSlot>) -> Vec<Arc<AcpTerminal>> {
    map.drain()
        .filter_map(|(_, slot)| match slot {
            TerminalSlot::Live(terminal) => Some(terminal),
            TerminalSlot::Reserved => None,
        })
        .collect()
}

impl AcpTerminal {
    fn push_output(&self, data: &[u8]) {
        if let Ok(mut output) = self.output.lock() {
            output.push(data);
        }
    }

    fn set_exit(&self, status: TerminalExitStatus) {
        if let Ok(mut exit) = self.exit.lock() {
            if exit.is_none() {
                *exit = Some(status);
                self.exit_cvar.notify_all();
            }
        }
    }

    fn wait_exit(&self) -> TerminalExitStatus {
        let Ok(mut exit) = self.exit.lock() else {
            return TerminalExitStatus::new();
        };
        while exit.is_none() {
            let Ok(next) = self.exit_cvar.wait(exit) else {
                return TerminalExitStatus::new();
            };
            exit = next;
        }
        exit.clone().unwrap_or_else(TerminalExitStatus::new)
    }

    fn kill(&self) {
        if let Ok(mut killer) = self.killer.lock() {
            let _ = killer.kill();
        }
        if let Ok(mut job) = self.job.lock() {
            drop(job.take());
        }
        self.drop_batch_file();
    }

    fn drop_batch_file(&self) {
        if let Some(path) = &self.batch_file {
            let _ = std::fs::remove_file(path);
        }
    }

    fn push_rpc_waiter(&self, handle: JoinHandle<()>) {
        if let Ok(mut waiters) = self.rpc_waiters.lock() {
            waiters.push(handle);
        }
    }

    fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        self.kill();
        if let Ok(mut master) = self.master.lock() {
            drop(master.take());
        }
        if let Ok(mut job) = self.job.lock() {
            drop(job.take());
        }
        if let Ok(mut waiter) = self.waiter.lock() {
            if let Some(handle) = waiter.take() {
                let _ = handle.join();
            }
        }
        if let Ok(mut reader) = self.reader.lock() {
            if let Some(handle) = reader.take() {
                let _ = handle.join();
            }
        }
        self.set_exit(TerminalExitStatus::new());
        let waiters = self
            .rpc_waiters
            .lock()
            .map(|mut waiters| waiters.drain(..).collect::<Vec<_>>())
            .unwrap_or_default();
        for handle in waiters {
            let _ = handle.join();
        }
        self.drop_batch_file();
    }
}

fn count_dsr_queries(bytes: &[u8]) -> usize {
    bytes
        .windows(4)
        .filter(|window| *window == b"\x1b[6n")
        .count()
}

#[derive(Clone, Copy)]
enum FsAccess {
    Read,
    Write,
}

impl std::fmt::Display for FsAccess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read => formatter.write_str("read"),
            Self::Write => formatter.write_str("write"),
        }
    }
}

fn canonicalize_existing_or_lexical(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| lexical_normalize(path));
    crate::workspace::plain_path(&canonical.to_string_lossy()).into()
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = out.pop();
            }
            Component::Normal(_) => out.push(component),
        }
    }
    out
}

fn resolve_path(path: &Path) -> Result<PathBuf, RpcError> {
    let lexical = lexical_normalize(path);
    if !lexical.is_absolute() {
        return Err(RpcError::invalid_params(format!(
            "path must be absolute: {}",
            path.display()
        )));
    }
    let mut current = lexical;
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        if current.as_os_str().is_empty() {
            return Err(RpcError::invalid_params(format!(
                "path has no existing ancestor: {}",
                path.display()
            )));
        }
        if current.exists() {
            let canonical =
                std::fs::canonicalize(&current).map_err(|error| fs_error(&current, error))?;
            let mut resolved: PathBuf =
                crate::workspace::plain_path(&canonical.to_string_lossy()).into();
            for part in missing.iter().rev() {
                resolved.push(part);
            }
            return Ok(resolved);
        }
        match (current.file_name(), current.parent()) {
            (Some(name), Some(parent)) if parent != current.as_path() => {
                missing.push(name.to_os_string());
                current = parent.to_path_buf();
            }
            _ => {
                return Err(RpcError::invalid_params(format!(
                    "path has no existing ancestor: {}",
                    path.display()
                )));
            }
        }
    }
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path: PathBuf = crate::workspace::plain_path(&path.to_string_lossy()).into();
    let root: PathBuf = crate::workspace::plain_path(&root.to_string_lossy()).into();
    let path_parts: Vec<Component<'_>> = path.components().collect();
    let root_parts: Vec<Component<'_>> = root.components().collect();
    if path_parts.len() < root_parts.len() {
        return false;
    }
    path_parts
        .iter()
        .zip(root_parts.iter())
        .take(root_parts.len())
        .all(|(left, right)| components_equal(left, right))
}

fn components_equal(left: &Component<'_>, right: &Component<'_>) -> bool {
    match (left, right) {
        (Component::Normal(left), Component::Normal(right)) => left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy()),
        _ => left == right,
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (file_identity(left), file_identity(right)) {
        (Some(left_id), Some(right_id)) => left_id == right_id,
        _ => false,
    }
}

#[cfg(windows)]
fn file_identity(path: &Path) -> Option<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;
    let file = std::fs::File::open(path).ok()?;
    let mut info = unsafe {
        std::mem::zeroed::<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION>()
    };
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle(
            file.as_raw_handle() as _,
            &mut info,
        )
    };
    if ok == 0 {
        return None;
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Some((info.dwVolumeSerialNumber, index))
}

#[cfg(unix)]
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = path.metadata().ok()?;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(any(windows, unix)))]
fn file_identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn touches_runtime(path: &Path, runtime: &Path) -> bool {
    if path_is_within(path, runtime) {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(runtime) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| same_file(path, &entry.path()))
}

fn fs_error(path: &Path, error: io::Error) -> RpcError {
    if error.kind() == io::ErrorKind::NotFound {
        RpcError::resource_not_found(format!("{}: {error}", path.display()))
    } else {
        RpcError::internal(format!("{}: {error}", path.display()))
    }
}

fn slice_lines(contents: &str, line: Option<u32>, limit: Option<u32>) -> Result<String, RpcError> {
    if line == Some(0) {
        return Err(RpcError::invalid_params("line is 1-based and cannot be 0"));
    }
    let start = line.unwrap_or(1).saturating_sub(1) as usize;
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    if start >= lines.len() {
        return Ok(String::new());
    }
    let end = match limit {
        Some(limit) => start.saturating_add(limit as usize).min(lines.len()),
        None => lines.len(),
    };
    Ok(lines[start..end].concat())
}

fn terminal_permission_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "terminal:{:x}-{:x}-{}",
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

enum SpawnPlan {
    Argv { program: String, args: Vec<String> },
    ShellLine { line: String },
}

struct PreparedSpawn {
    program: String,
    args: Vec<String>,
    batch_file: Option<PathBuf>,
}

/// ACP agents often put a whole shell line in `command` and omit `args`.
/// That is not an executable path. The permission prompt shows the original
/// line; Windows spawn writes it verbatim to a tempfile so Win32 quoting
/// cannot rewrite quotes inside the line.
fn spawn_plan(command: &str, args: &[String]) -> SpawnPlan {
    if args.is_empty() {
        SpawnPlan::ShellLine {
            line: command.to_string(),
        }
    } else {
        SpawnPlan::Argv {
            program: command.to_string(),
            args: args.to_vec(),
        }
    }
}

fn prepare_spawn(
    plan: &SpawnPlan,
    runtime_dir: &Path,
    terminal_id: &str,
) -> Result<PreparedSpawn, RpcError> {
    match plan {
        SpawnPlan::Argv { program, args } => Ok(PreparedSpawn {
            program: program.clone(),
            args: args.clone(),
            batch_file: None,
        }),
        SpawnPlan::ShellLine { line } => {
            #[cfg(windows)]
            {
                let batch_file = write_shell_batch(runtime_dir, terminal_id, line)?;
                Ok(PreparedSpawn {
                    program: "cmd.exe".to_string(),
                    args: vec![
                        "/d".to_string(),
                        "/c".to_string(),
                        batch_file.to_string_lossy().into_owned(),
                    ],
                    batch_file: Some(batch_file),
                })
            }
            #[cfg(not(windows))]
            {
                let _ = (runtime_dir, terminal_id);
                Ok(PreparedSpawn {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), line.clone()],
                    batch_file: None,
                })
            }
        }
    }
}

#[cfg(windows)]
fn write_shell_batch(
    runtime_dir: &Path,
    terminal_id: &str,
    line: &str,
) -> Result<PathBuf, RpcError> {
    static BATCH_SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = BATCH_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = runtime_dir.join(format!(
        "acp-{:x}-{seq}-{terminal_id}.cmd",
        std::process::id()
    ));
    std::fs::write(&path, format!("{line}\r\n"))
        .map_err(|error| RpcError::internal(format!("Could not write ACP shell batch: {error}")))?;
    Ok(path)
}

fn permission_env(env: &[(String, String)]) -> Option<Vec<PermissionEnvVar>> {
    if env.is_empty() {
        None
    } else {
        Some(
            env.iter()
                .map(|(name, value)| PermissionEnvVar {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
        )
    }
}

fn terminal_permission_event(
    plan: &SpawnPlan,
    cwd: &Path,
    env: &[(String, String)],
) -> SessionEvent {
    let (command, args) = match plan {
        SpawnPlan::Argv { program, args } => (program.clone(), Some(args.clone())),
        SpawnPlan::ShellLine { line } => (line.clone(), None),
    };
    SessionEvent::PermissionRequest {
        tool_call_id: terminal_permission_id(),
        title: "Run command".to_string(),
        description: None,
        command: Some(command),
        args,
        cwd: Some(cwd.to_string_lossy().into_owned()),
        env: permission_env(env),
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
        is_chooser: None,
        // A placeholder the daemon overwrites with the session's stored origin
        // before the request leaves for a subscriber.
        origin: devboule_protocol::SessionOrigin::unknown(),
        create_agent: None,
    }
}

fn spawn_acp_terminal(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    output_limit: u64,
    batch_file: Option<PathBuf>,
) -> Result<Arc<AcpTerminal>, RpcError> {
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: PTY_ROWS,
            cols: PTY_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| RpcError::internal(format!("Could not open ACP terminal: {error}")))?;
    let mut builder = CommandBuilder::new(program);
    builder.args(args);
    builder.cwd(cwd);
    for (key, value) in env {
        builder.env(key, value);
    }
    let mut child = pair.slave.spawn_command(builder).map_err(|error| {
        RpcError::internal(format!("Could not start ACP terminal command: {error}"))
    })?;
    let process_job = JobObject::new()
        .map_err(|error| RpcError::internal(format!("Could not create terminal job: {error}")))?;
    #[cfg(windows)]
    {
        let handle = child.as_raw_handle().ok_or_else(|| {
            RpcError::internal("ACP terminal process has no native handle".to_string())
        })?;
        // This terminal's own fresh job; why no shared job is ever an
        // assignment target is stated once, at open_pty_session.
        if let Err(error) = process_job.assign(handle) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RpcError::internal(format!(
                "Could not contain the ACP terminal process: {error}"
            )));
        }
    }
    let killer = child.clone_killer();
    let reader = pair.master.try_clone_reader().map_err(|error| {
        let _ = child.kill();
        RpcError::internal(format!("Could not read ACP terminal: {error}"))
    })?;
    // ConPTY issues ESC[6n at startup and stalls until a CPR reply. Regular
    // PTY sessions answer via the emulator; ACP terminals have no emulator,
    // so the reader replies here. Clients must not answer a second time.
    let writer = pair.master.take_writer().map_err(|error| {
        let _ = child.kill();
        RpcError::internal(format!("Could not write ACP terminal: {error}"))
    })?;
    let terminal = Arc::new(AcpTerminal {
        output: Mutex::new(BoundedBuffer::new(output_limit)),
        exit: Mutex::new(None),
        exit_cvar: Condvar::new(),
        killer: Mutex::new(killer),
        master: Mutex::new(Some(pair.master)),
        job: Mutex::new(Some(process_job)),
        reader: Mutex::new(None),
        waiter: Mutex::new(None),
        rpc_waiters: Mutex::new(Vec::new()),
        released: AtomicBool::new(false),
        batch_file,
    });
    let output_terminal = Arc::clone(&terminal);
    let reader_handle = std::thread::Builder::new()
        .name("acp-term-read".to_string())
        .spawn(move || {
            let mut reader = reader;
            let mut writer = writer;
            let mut buf = [0u8; 8192];
            let mut tail = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(count) => {
                        let chunk = &buf[..count];
                        output_terminal.push_output(chunk);
                        tail.extend_from_slice(chunk);
                        let queries = count_dsr_queries(&tail);
                        for _ in 0..queries {
                            let _ = writer.write_all(b"\x1b[1;1R");
                        }
                        if queries > 0 {
                            let _ = writer.flush();
                        }
                        if tail.len() > 3 {
                            tail.drain(..tail.len() - 3);
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .map_err(|error| RpcError::internal(format!("Could not drain ACP terminal: {error}")))?;
    let wait_terminal = Arc::clone(&terminal);
    let waiter_handle = std::thread::Builder::new()
        .name("acp-term-wait".to_string())
        .spawn(move || {
            let status = child.wait().ok();
            let mut exit = TerminalExitStatus::new();
            if let Some(status) = status {
                exit = exit.exit_code(status.exit_code());
                if let Some(signal) = status.signal() {
                    exit = exit.signal(signal.to_string());
                }
            }
            wait_terminal.set_exit(exit);
            wait_terminal.drop_batch_file();
        })
        .map_err(|error| RpcError::internal(format!("Could not wait ACP terminal: {error}")))?;
    if let Ok(mut reader) = terminal.reader.lock() {
        *reader = Some(reader_handle);
    }
    if let Ok(mut waiter) = terminal.waiter.lock() {
        *waiter = Some(waiter_handle);
    }
    Ok(terminal)
}

#[cfg(test)]
#[path = "acp_host_tests.rs"]
mod tests;
