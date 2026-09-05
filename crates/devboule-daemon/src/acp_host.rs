//! ACP client methods the agent calls on us: filesystem and terminals.
//!
//! Terminals reuse the daemon's ConPTY + Job Object path. `terminal/create`
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

use super::acp_client::{HostDecision, PermissionBroker};
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
            code: -32002,
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
    daemon_job: Arc<JobObject>,
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
    pub(super) fn new(cwd: PathBuf, runtime_dir: PathBuf, daemon_job: Arc<JobObject>) -> Arc<Self> {
        Self::with_terminal_limit(cwd, runtime_dir, daemon_job, MAX_ACP_TERMINALS)
    }

    fn with_terminal_limit(
        cwd: PathBuf,
        runtime_dir: PathBuf,
        daemon_job: Arc<JobObject>,
        max_terminals: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_id: Mutex::new(String::new()),
            cwd: canonicalize_existing_or_lexical(&cwd),
            runtime_dir: canonicalize_existing_or_lexical(&runtime_dir),
            daemon_job,
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
            Arc::clone(&self.daemon_job),
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
    bytes.windows(4).filter(|window| *window == b"\x1b[6n").count()
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
    std::fs::canonicalize(path)
        .map(strip_verbatim)
        .unwrap_or_else(|_| strip_verbatim(lexical_normalize(path)))
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

fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        const VERBATIM: &str = r"\\?\";
        const VERBATIM_UNC: &str = r"\\?\UNC\";
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(VERBATIM_UNC) {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(VERBATIM) {
            return PathBuf::from(rest);
        }
        path
    }
    #[cfg(not(windows))]
    {
        path
    }
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
            let mut resolved = strip_verbatim(canonical);
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
    let path = strip_verbatim(path.to_path_buf());
    let root = strip_verbatim(root.to_path_buf());
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
    Argv {
        program: String,
        args: Vec<String>,
    },
    ShellLine {
        line: String,
    },
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

fn prepare_spawn(plan: &SpawnPlan, runtime_dir: &Path, terminal_id: &str) -> Result<PreparedSpawn, RpcError> {
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
fn write_shell_batch(runtime_dir: &Path, terminal_id: &str, line: &str) -> Result<PathBuf, RpcError> {
    static BATCH_SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = BATCH_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = runtime_dir.join(format!(
        "acp-{:x}-{seq}-{terminal_id}.cmd",
        std::process::id()
    ));
    std::fs::write(&path, format!("{line}\r\n")).map_err(|error| {
        RpcError::internal(format!("Could not write ACP shell batch: {error}"))
    })?;
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
    }
}

fn spawn_acp_terminal(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    daemon_job: Arc<JobObject>,
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
        if let Err(error) = daemon_job
            .assign(handle)
            .and_then(|()| process_job.assign(handle))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RpcError::internal(format!(
                "Could not contain the ACP terminal process: {error}"
            )));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = &daemon_job;
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
mod tests {
    use super::super::acp_client::PermissionBroker;
    use super::super::{ConnHandle, SessionRuntime};
    use super::{slice_lines, AcpHost, BoundedBuffer, MAX_FS_BYTES};
    use crate::process_tree::JobObject;
    use devboule_protocol::{PermissionOutcome, SessionEvent};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct TestDirs {
        host: Arc<AcpHost>,
        cwd: PathBuf,
        runtime: PathBuf,
    }

    fn unique_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "devboule-acp-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("test dir");
        dir
    }

    fn host() -> TestDirs {
        let cwd = unique_dir("cwd");
        let runtime = unique_dir("runtime");
        let host = AcpHost::new(
            cwd.clone(),
            runtime.clone(),
            Arc::new(JobObject::new().expect("job")),
        );
        host.set_session_id("stub-session".to_string());
        TestDirs { host, cwd, runtime }
    }

    #[test]
    fn bounded_buffer_truncates_from_the_start_on_a_char_boundary() {
        let mut buffer = BoundedBuffer::new(3);
        buffer.push("éé".as_bytes());
        let (text, truncated) = buffer.snapshot();
        assert!(truncated);
        assert_eq!(text, "é");
    }

    #[test]
    fn line_slice_is_one_based_and_keeps_newlines() {
        let contents = "a\nb\nc\n";
        assert_eq!(
            slice_lines(contents, Some(2), Some(1)).expect("slice"),
            "b\n"
        );
        assert_eq!(
            slice_lines(contents, Some(1), None).expect("slice"),
            contents
        );
        assert!(slice_lines(contents, Some(0), None).is_err());
        assert_eq!(slice_lines(contents, Some(9), None).expect("empty"), "");
    }

    #[test]
    fn relative_path_is_rejected() {
        assert!(super::resolve_path(Path::new("relative.txt")).is_err());
        assert_eq!(MAX_FS_BYTES, 8 * 1024 * 1024);
    }

    #[test]
    fn fs_read_and_write_round_trip_absolute_paths() {
        let test = host();
        let path = test.cwd.join("note.txt");
        let write = test
            .host
            .write_text_file(serde_json::json!({
                "sessionId": "stub-session",
                "path": path,
                "content": "one\ntwo\nthree\n"
            }))
            .expect("write");
        assert!(write.is_object());
        let read = test
            .host
            .read_text_file(serde_json::json!({
                "sessionId": "stub-session",
                "path": path,
                "line": 2,
                "limit": 1
            }))
            .expect("read");
        assert_eq!(read["content"], "two\n");
        let relative = test.host.write_text_file(serde_json::json!({
            "sessionId": "stub-session",
            "path": "relative.txt",
            "content": "no"
        }));
        assert!(relative.is_err());
        let _ = std::fs::remove_dir_all(test.cwd);
        let _ = std::fs::remove_dir_all(test.runtime);
    }

    #[test]
    fn write_refuses_the_daemon_journal_even_when_the_path_is_absolute() {
        let test = host();
        let journal = test.runtime.join("journal.db");
        std::fs::write(&journal, b"precious").expect("seed journal");
        let result = test.host.write_text_file(serde_json::json!({
            "sessionId": "stub-session",
            "path": journal,
            "content": "wiped"
        }));
        assert!(result.is_err(), "journal write must be refused: {result:?}");
        assert!(
            test.host
                .read_text_file(serde_json::json!({
                    "sessionId": "stub-session",
                    "path": journal
                }))
                .is_err(),
            "journal read must be refused"
        );
        assert_eq!(
            std::fs::read(&journal).expect("journal still there"),
            b"precious"
        );
        let _ = std::fs::remove_dir_all(test.cwd);
        let _ = std::fs::remove_dir_all(test.runtime);
    }

    #[cfg(windows)]
    #[test]
    fn write_refuses_a_junction_that_escapes_the_workspace() {
        let test = host();
        let outside = unique_dir("outside");
        let link = test.cwd.join("escape");
        let status = std::process::Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                &link.to_string_lossy(),
                &outside.to_string_lossy(),
            ])
            .status()
            .expect("mklink");
        assert!(
            status.success(),
            "could not create junction for the escape test"
        );
        let stolen = link.join("stolen.txt");
        let result = test.host.write_text_file(serde_json::json!({
            "sessionId": "stub-session",
            "path": stolen,
            "content": "pwned"
        }));
        assert!(
            result.is_err(),
            "junction escape must be refused: {result:?}"
        );
        assert!(
            !outside.join("stolen.txt").exists(),
            "file was written through the junction"
        );
        let _ = std::fs::remove_dir(link);
        let _ = std::fs::remove_dir_all(outside);
        let _ = std::fs::remove_dir_all(test.cwd);
        let _ = std::fs::remove_dir_all(test.runtime);
    }

    #[test]
    fn write_creates_parent_directories_inside_the_workspace() {
        let test = host();
        let path = test.cwd.join("src").join("new").join("file.txt");
        test.host
            .write_text_file(serde_json::json!({
                "sessionId": "stub-session",
                "path": path,
                "content": "hello\n"
            }))
            .expect("write into new nested directory");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "hello\n"
        );
        let outside = test.cwd.join("..").join("escaped").join("nope.txt");
        let escaped = test.host.write_text_file(serde_json::json!({
            "sessionId": "stub-session",
            "path": outside,
            "content": "no"
        }));
        assert!(
            escaped.is_err(),
            "must not create parents outside the workspace: {escaped:?}"
        );
        let _ = std::fs::remove_dir_all(test.cwd);
        let _ = std::fs::remove_dir_all(test.runtime);
    }

    #[test]
    fn write_refuses_a_hard_link_to_the_journal() {
        let test = host();
        let journal = test.runtime.join("journal.db");
        std::fs::write(&journal, b"precious").expect("seed journal");
        let alias = test.cwd.join("innocent.txt");
        std::fs::hard_link(&journal, &alias).expect("hard link");
        let result = test.host.write_text_file(serde_json::json!({
            "sessionId": "stub-session",
            "path": alias,
            "content": "wiped"
        }));
        assert!(
            result.is_err(),
            "hard-link journal write must be refused: {result:?}"
        );
        assert_eq!(
            std::fs::read(&journal).expect("journal still there"),
            b"precious"
        );
        let _ = std::fs::remove_dir_all(test.cwd);
        let _ = std::fs::remove_dir_all(test.runtime);
    }

    #[cfg(windows)]
    #[test]
    fn kill_terminates_a_process_that_would_not_exit_alone() {
        let test = host();
        let (broker, _runtime) = bind_gate(&test.host);
        let _allow = AutoAllow::start(Arc::clone(&broker));
        let host = test.host;
        let created = host
            .create_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "command": "ping.exe",
                "args": ["-t", "127.0.0.1"]
            }))
            .expect("create");
        let terminal_id = created["terminalId"].clone();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let before = host
            .terminal_output(serde_json::json!({
                "sessionId": "stub-session",
                "terminalId": terminal_id
            }))
            .expect("output before kill");
        assert!(
            before.get("exitStatus").is_none() || before["exitStatus"].is_null(),
            "ping -t exited before kill: {before}"
        );
        host.kill_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        }))
        .expect("kill");
        let (tx, rx) = std::sync::mpsc::channel();
        let respond: super::RpcRespond = Arc::new(move |_, result| {
            let _ = tx.send(result);
        });
        host.wait_for_exit(
            serde_json::json!(1),
            serde_json::json!({
                "sessionId": "stub-session",
                "terminalId": terminal_id
            }),
            respond,
        );
        let exit = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("wait_for_exit after kill")
            .expect("kill must make the process exit");
        assert!(
            exit.get("exitCode").is_some() || exit.get("signal").is_some(),
            "kill left no exit status: {exit}"
        );
        host.release_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        }))
        .expect("release");
        let _ = std::fs::remove_dir_all(test.cwd);
        let _ = std::fs::remove_dir_all(test.runtime);
    }

    #[cfg(windows)]
    #[test]
    fn concurrent_creates_never_exceed_the_live_terminal_limit() {
        let cwd = unique_dir("term-cwd");
        let runtime = unique_dir("term-runtime");
        let host = AcpHost::with_terminal_limit(
            cwd.clone(),
            runtime.clone(),
            Arc::new(JobObject::new().expect("job")),
            2,
        );
        host.set_session_id("stub-session".to_string());
        let (broker, _runtime) = bind_gate(&host);
        let _allow = AutoAllow::start(Arc::clone(&broker));
        host.create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "cmd.exe",
            "args": ["/c", "exit"]
        }))
        .expect("fill to one live terminal");
        let start = Arc::new(std::sync::Barrier::new(3));
        let successes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let host = Arc::clone(&host);
            let start = Arc::clone(&start);
            let successes = Arc::clone(&successes);
            threads.push(std::thread::spawn(move || {
                start.wait();
                if host
                    .create_terminal(serde_json::json!({
                        "sessionId": "stub-session",
                        "command": "cmd.exe",
                        "args": ["/c", "exit"]
                    }))
                    .is_ok()
                {
                    successes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }));
        }
        start.wait();
        for thread in threads {
            thread.join().expect("create thread");
        }
        let live = host.live_terminal_count();
        let spawned = host.spawned_count();
        host.shutdown();
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(runtime);
        assert!(
            live <= 2,
            "live terminals={live} successes={}",
            successes.load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(
            spawned <= 2,
            "started {spawned} processes under a limit of 2 (map live={live})"
        );
    }

    struct AutoAllow {
        stop: Arc<std::sync::atomic::AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl AutoAllow {
        fn start(broker: Arc<PermissionBroker>) -> Self {
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop_thread = Arc::clone(&stop);
            let handle = std::thread::spawn(move || {
                while !stop_thread.load(std::sync::atomic::Ordering::SeqCst) {
                    for id in broker.pending_ids() {
                        let _ = broker.respond(&id, PermissionOutcome::AllowOnce);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
            Self {
                stop,
                handle: Some(handle),
            }
        }
    }

    impl Drop for AutoAllow {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn discard_broker() -> Arc<PermissionBroker> {
        PermissionBroker::for_test(Arc::new(|_, _| Ok(())))
    }

    fn bind_gate(host: &AcpHost) -> (Arc<PermissionBroker>, Arc<SessionRuntime>) {
        let broker = discard_broker();
        let runtime =
            SessionRuntime::for_acp("stub-session".to_string(), None, Arc::clone(&broker));
        host.bind_permission_gate(&broker, &runtime);
        (broker, runtime)
    }

    fn wait_for_pending(broker: &PermissionBroker, timeout: Duration) -> String {
        wait_for_pending_or_progress(broker, None, None, timeout).unwrap_or_else(|| {
            panic!(
                "timed out waiting for a pending terminal permission (pending={})",
                broker.pending_len()
            )
        })
    }

    fn wait_for_pending_or_progress(
        broker: &PermissionBroker,
        host: Option<&AcpHost>,
        thread: Option<&std::thread::JoinHandle<Result<serde_json::Value, super::RpcError>>>,
        timeout: Duration,
    ) -> Option<String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(id) = broker.pending_ids().into_iter().next() {
                return Some(id);
            }
            if thread.is_some_and(std::thread::JoinHandle::is_finished)
                || host.is_some_and(|host| host.spawned_count() > 0)
            {
                return None;
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn create_params(test: &TestDirs) -> serde_json::Value {
        serde_json::json!({
            "sessionId": "stub-session",
            "command": "cmd.exe",
            "args": ["/c", "echo", "gated"],
            "cwd": test.cwd,
        })
    }

    fn spawn_create(
        host: Arc<AcpHost>,
        params: serde_json::Value,
    ) -> std::thread::JoinHandle<Result<serde_json::Value, super::RpcError>> {
        std::thread::spawn(move || host.create_terminal(params))
    }

    #[test]
    fn terminal_create_does_not_spawn_before_a_permission_decision() {
        let test = host();
        let (broker, _runtime) = bind_gate(&test.host);
        let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
        let pending = wait_for_pending_or_progress(
            &broker,
            Some(&test.host),
            Some(&thread),
            Duration::from_secs(2),
        );
        let spawned = test.host.spawned_count();
        if let Some(ref id) = pending {
            let _ = broker.respond(id, PermissionOutcome::Deny);
        }
        let _ = thread.join();
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert_eq!(
            spawned, 0,
            "terminal/create spawned before a permission decision (spawned={spawned})"
        );
        assert!(
            pending.is_some(),
            "terminal/create never registered a host permission request"
        );
    }

    #[cfg(windows)]
    #[test]
    fn terminal_create_allow_spawns_the_command() {
        let test = host();
        let (broker, _runtime) = bind_gate(&test.host);
        let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
        let id = wait_for_pending(&broker, Duration::from_secs(2));
        assert_eq!(test.host.spawned_count(), 0);
        broker
            .respond(&id, PermissionOutcome::AllowOnce)
            .expect("allow");
        let created = thread
            .join()
            .expect("create thread")
            .expect("allowed create");
        assert!(
            created.get("terminalId").is_some(),
            "missing terminalId: {created}"
        );
        assert_eq!(test.host.spawned_count(), 1);
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
    }

    #[test]
    fn terminal_create_allow_after_exit_does_not_spawn() {
        let test = host();
        let (broker, runtime) = bind_gate(&test.host);
        let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
        let id = wait_for_pending(&broker, Duration::from_secs(2));
        runtime.mark_exited(Some(1));
        broker
            .respond(&id, PermissionOutcome::AllowOnce)
            .expect("allow after the agent is already dead");
        let error = thread
            .join()
            .expect("create thread")
            .expect_err("allow after OS death must not spawn");
        let spawned = test.host.spawned_count();
        let live = test.host.live_terminal_count();
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert_eq!(error.code, -32001);
        assert_eq!(error.message, "the agent is gone");
        assert_eq!(spawned, 0, "dead agent must not spawn (spawned={spawned})");
        assert_eq!(live, 0, "reserved slot must be released (live={live})");
    }

    #[cfg(windows)]
    fn spawn_innocuous() -> std::process::Child {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd.exe")
            .args(["/d", "/c", "ping", "-n", "30", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("spawn ping")
    }

    #[cfg(windows)]
    #[test]
    fn os_death_cancels_pending_terminal_create_without_eof() {
        use crate::process_tree::ProcessHandle;
        use std::os::windows::io::AsRawHandle;
        let test = host();
        let (broker, runtime) = bind_gate(&test.host);
        broker.set_timeout(Duration::from_secs(2));
        let wake = Arc::clone(&broker);
        runtime.set_on_os_death(Arc::new(move || wake.cancel_all()));
        let mut child = spawn_innocuous();
        let handle =
            ProcessHandle::duplicate(AsRawHandle::as_raw_handle(&child)).expect("duplicate");
        runtime.install_os_handle(handle);
        let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
        let _id = wait_for_pending(&broker, Duration::from_secs(2));
        child.kill().expect("kill ping");
        let _ = child.wait();
        let started = Instant::now();
        assert!(
            runtime.observe_os_liveness(),
            "OS observation must mark Exited without waiting on ACP stdout EOF"
        );
        let deadline = Instant::now() + Duration::from_millis(800);
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let error = thread
            .join()
            .expect("create thread")
            .expect_err("OS death must deny the pending gate");
        let elapsed = started.elapsed();
        let spawned = test.host.spawned_count();
        let live = test.host.live_terminal_count();
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert!(
            elapsed < Duration::from_millis(800),
            "pending terminal/create stayed blocked for {elapsed:?} after OS death"
        );
        assert_eq!(error.code, -32001);
        assert_eq!(spawned, 0);
        assert_eq!(live, 0);
    }

    #[test]
    fn terminal_create_deny_returns_server_error_and_releases_the_slot() {
        let test = host();
        let (broker, _runtime) = bind_gate(&test.host);
        let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
        let id = wait_for_pending(&broker, Duration::from_secs(2));
        broker.respond(&id, PermissionOutcome::Deny).expect("deny");
        let error = thread
            .join()
            .expect("create thread")
            .expect_err("deny must not create a terminal");
        test.host.shutdown();
        let spawned = test.host.spawned_count();
        let live = test.host.live_terminal_count();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert_eq!(
            error.code, -32001,
            "deny must be JSON-RPC -32001: {error:?}"
        );
        assert_eq!(error.message, "the user denied this command");
        assert_eq!(spawned, 0, "deny must not spawn (spawned={spawned})");
        assert_eq!(live, 0, "deny must release the reserved slot (live={live})");
    }

    #[test]
    fn terminal_create_timeout_denies_without_spawning() {
        let test = host();
        let (broker, _runtime) = bind_gate(&test.host);
        broker.set_timeout(Duration::from_millis(50));
        let started = Instant::now();
        let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
        let error = thread
            .join()
            .expect("create thread")
            .expect_err("timeout must deny");
        let elapsed = started.elapsed();
        test.host.shutdown();
        let spawned = test.host.spawned_count();
        let live = test.host.live_terminal_count();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert!(
            elapsed < Duration::from_secs(5),
            "timeout waited {elapsed:?} instead of the short test deadline"
        );
        assert_eq!(error.code, -32001);
        assert_eq!(error.message, "the user denied this command");
        assert_eq!(spawned, 0);
        assert_eq!(live, 0);
    }

    #[test]
    fn cancel_all_unblocks_a_pending_terminal_create_with_deny() {
        let test = host();
        let (broker, _runtime) = bind_gate(&test.host);
        let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
        let _id = wait_for_pending(&broker, Duration::from_secs(2));
        let started = Instant::now();
        broker.cancel_all();
        let error = thread
            .join()
            .expect("create thread")
            .expect_err("cancel_all must deny the gate");
        let elapsed = started.elapsed();
        test.host.shutdown();
        let spawned = test.host.spawned_count();
        let live = test.host.live_terminal_count();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert!(
            elapsed < Duration::from_secs(2),
            "cancel_all left the create thread blocked for {elapsed:?}"
        );
        assert_eq!(error.code, -32001);
        assert_eq!(error.message, "the user denied this command");
        assert_eq!(spawned, 0);
        assert_eq!(live, 0);
    }

    fn permission_rows(path: &Path) -> Vec<(String, String, serde_json::Value)> {
        let conn = rusqlite::Connection::open(path).expect("inspect journal");
        let mut stmt = conn
            .prepare(
                "SELECT request_id, outcome, payload FROM permissions ORDER BY ts_ms, request_id",
            )
            .expect("prepare");
        stmt.query_map([], |row| {
            let payload: Vec<u8> = row.get(2)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                serde_json::from_slice(&payload).expect("payload json"),
            ))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    #[test]
    fn terminal_create_decisions_are_journaled_with_spawn_payload() {
        let test = host();
        let path = test.runtime.join("journal.db");
        let journal = Arc::new(crate::journal::Journal::open(&path).expect("journal"));
        journal
            .upsert_blocking(crate::journal::new_session_record(
                "stub-session",
                "owner",
                None,
                devboule_protocol::SessionKind::Acp,
                "Agent",
            ))
            .expect("upsert");
        let broker = discard_broker();
        let runtime = SessionRuntime::for_acp(
            "stub-session".to_string(),
            Some(Arc::clone(&journal)),
            Arc::clone(&broker),
        );
        test.host.bind_permission_gate(&broker, &runtime);

        let deny_thread = spawn_create(Arc::clone(&test.host), create_params(&test));
        let deny_id = wait_for_pending(&broker, Duration::from_secs(2));
        broker
            .respond(&deny_id, PermissionOutcome::Deny)
            .expect("deny");
        let _ = deny_thread.join();

        broker.set_timeout(Duration::from_millis(50));
        let timeout_thread = spawn_create(Arc::clone(&test.host), create_params(&test));
        let _ = timeout_thread.join();
        broker.set_timeout(Duration::from_secs(120));

        let cancel_thread = spawn_create(Arc::clone(&test.host), create_params(&test));
        let _ = wait_for_pending(&broker, Duration::from_secs(2));
        broker.cancel_all();
        let _ = cancel_thread.join();

        journal.flush().expect("flush");
        let rows = permission_rows(&path);
        journal.shutdown();
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);

        let outcomes: Vec<&str> = rows
            .iter()
            .map(|(_, outcome, _)| outcome.as_str())
            .collect();
        assert!(outcomes.contains(&"deny"), "missing deny row: {outcomes:?}");
        assert!(
            outcomes.contains(&"timeout"),
            "missing timeout row: {outcomes:?}"
        );
        assert!(
            outcomes.contains(&"cancelled"),
            "missing cancelled row: {outcomes:?}"
        );
        for (request_id, _, payload) in &rows {
            assert!(
                request_id.starts_with("terminal:"),
                "host permission id should be synthetic: {request_id}"
            );
            assert_eq!(payload["command"], "cmd.exe");
            assert_eq!(payload["args"][0], "/c");
            assert_eq!(payload["args"][1], "echo");
            assert_eq!(payload["args"][2], "gated");
            let cwd = payload["cwd"].as_str().expect("cwd");
            assert!(
                cwd.contains("devboule-acp-cwd") || Path::new(cwd) == test.cwd.as_path(),
                "journaled cwd {cwd} was not the spawn cwd"
            );
        }
    }

    fn wait_for_output_containing(
        host: &AcpHost,
        terminal_id: &serde_json::Value,
        needle: &str,
        timeout: Duration,
    ) -> String {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        loop {
            if let Ok(output) = host.terminal_output(serde_json::json!({
                "sessionId": "stub-session",
                "terminalId": terminal_id
            })) {
                last = output["output"].as_str().unwrap_or("").to_string();
                if last.contains(needle) {
                    return last;
                }
            }
            if Instant::now() >= deadline {
                panic!("terminal output never contained {needle:?}; last={last:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_exit_code(host: &AcpHost, terminal_id: serde_json::Value) -> u32 {
        let (tx, rx) = std::sync::mpsc::channel();
        let respond: super::RpcRespond = Arc::new(move |_, result| {
            let _ = tx.send(result);
        });
        host.wait_for_exit(
            serde_json::json!(1),
            serde_json::json!({
                "sessionId": "stub-session",
                "terminalId": terminal_id
            }),
            respond,
        );
        let exit = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("wait_for_exit")
            .expect("exit status");
        exit["exitCode"]
            .as_u64()
            .expect("exitCode")
            .try_into()
            .expect("exit code fits u32")
    }

    #[cfg(windows)]
    #[test]
    fn terminal_create_shell_line_without_args_runs_through_cmd() {
        let test = host();
        let (broker, _runtime) = bind_gate(&test.host);
        let _allow = AutoAllow::start(Arc::clone(&broker));
        let created = test
            .host
            .create_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "command": "cmd /c echo devboule-gate-marker"
            }))
            .expect("create shell line");
        let terminal_id = created["terminalId"].clone();
        let output = wait_for_output_containing(
            &test.host,
            &terminal_id,
            "devboule-gate-marker",
            Duration::from_secs(5),
        );
        let code = wait_for_exit_code(&test.host, terminal_id.clone());
        test.host
            .release_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "terminalId": terminal_id
            }))
            .expect("release");
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert!(
            output.contains("devboule-gate-marker"),
            "shell-line output missed the marker: {output:?}"
        );
        assert_eq!(code, 0, "shell-line command must exit 0");
    }

    #[cfg(windows)]
    #[test]
    fn terminal_create_with_args_keeps_argv_semantics() {
        let test = host();
        let (broker, _runtime) = bind_gate(&test.host);
        let _allow = AutoAllow::start(Arc::clone(&broker));
        let created = test
            .host
            .create_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "command": "cmd.exe",
                "args": ["/c", "echo", "devboule-argv-marker"]
            }))
            .expect("create argv");
        let terminal_id = created["terminalId"].clone();
        let output = wait_for_output_containing(
            &test.host,
            &terminal_id,
            "devboule-argv-marker",
            Duration::from_secs(5),
        );
        let code = wait_for_exit_code(&test.host, terminal_id.clone());
        test.host
            .release_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "terminalId": terminal_id
            }))
            .expect("release");
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert!(
            output.contains("devboule-argv-marker"),
            "argv output missed the marker: {output:?}"
        );
        assert_eq!(code, 0);
    }

    #[cfg(windows)]
    #[test]
    fn terminal_create_shell_line_permission_shows_the_real_spawn_argv() {
        let test = host();
        let (broker, runtime) = bind_gate(&test.host);
        let conn = ConnHandle::new(1);
        let generation = runtime.try_attach(None, &conn, true).expect("attach");
        conn.track(
            "stub-session",
            Arc::clone(&runtime),
            false,
            None,
            generation,
        );
        let line = "cmd /c echo devboule-gate-marker";
        let thread = spawn_create(
            Arc::clone(&test.host),
            serde_json::json!({
                "sessionId": "stub-session",
                "command": line
            }),
        );
        let id = wait_for_pending(&broker, Duration::from_secs(2));
        let events = conn.pull_events();
        broker
            .respond(&id, PermissionOutcome::Deny)
            .expect("deny after inspecting the prompt");
        let _ = thread.join();
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        let request = events.iter().find_map(|event| match &event.envelope.event {
            SessionEvent::PermissionRequest { command, args, .. } => {
                Some((command.clone(), args.clone()))
            }
            _ => None,
        });
        let (command, args) = request.expect("shell-line create must publish a PermissionRequest");
        assert_eq!(command.as_deref(), Some(line));
        assert_eq!(args, None, "shell-line prompt must show the original line, not the tempfile argv");
    }

    #[cfg(windows)]
    #[test]
    fn terminal_create_shell_line_preserves_quoted_echo_text() {
        let test = host();
        let (broker, _runtime) = bind_gate(&test.host);
        let _allow = AutoAllow::start(Arc::clone(&broker));
        let created = test
            .host
            .create_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "command": "echo \"hello world\""
            }))
            .expect("create quoted echo");
        let terminal_id = created["terminalId"].clone();
        let output = wait_for_output_containing(
            &test.host,
            &terminal_id,
            "hello world",
            Duration::from_secs(5),
        );
        let code = wait_for_exit_code(&test.host, terminal_id.clone());
        test.host
            .release_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "terminalId": terminal_id
            }))
            .expect("release");
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert!(
            !output.contains(r#"\""#),
            "cmd.exe saw Win32-escaped quotes: {output:?}"
        );
        assert_eq!(code, 0);
    }

    #[test]
    fn cancel_all_rejects_later_terminal_create_without_registering() {
        let test = host();
        let (broker, runtime) = bind_gate(&test.host);
        let conn = ConnHandle::new(1);
        let generation = runtime.try_attach(None, &conn, true).expect("attach");
        conn.track(
            "stub-session",
            Arc::clone(&runtime),
            false,
            None,
            generation,
        );
        broker.cancel_all();
        let started = Instant::now();
        let error = test
            .host
            .create_terminal(create_params(&test))
            .expect_err("closed broker must deny create");
        let elapsed = started.elapsed();
        let events = conn.pull_events();
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert!(elapsed < Duration::from_secs(1), "closed create blocked {elapsed:?}");
        assert_eq!(error.code, -32001);
        assert_eq!(test.host.spawned_count(), 0);
        assert_eq!(broker.pending_len(), 0);
        assert!(
            !events.iter().any(|event| {
                matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })
            }),
            "closed broker published a permission request: {events:?}"
        );
    }

    #[test]
    fn terminal_create_denies_when_typed_permissions_are_absent() {
        let test = host();
        let (_broker, runtime) = bind_gate(&test.host);
        let conn = ConnHandle::new(1);
        let generation = runtime
            .try_attach(None, &conn, false)
            .expect("attach without typed_permissions");
        conn.track(
            "stub-session",
            Arc::clone(&runtime),
            false,
            None,
            generation,
        );
        let started = Instant::now();
        let error = test
            .host
            .create_terminal(create_params(&test))
            .expect_err("missing typed_permissions must deny");
        let elapsed = started.elapsed();
        let events = conn.pull_events();
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert!(elapsed < Duration::from_secs(1), "capability deny blocked {elapsed:?}");
        assert_eq!(error.code, -32001);
        assert_eq!(test.host.spawned_count(), 0);
        assert!(
            !events.iter().any(|event| {
                matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })
            }),
            "incapable client was shown a permission prompt: {events:?}"
        );
    }

    #[test]
    fn terminal_create_permission_includes_env() {
        let test = host();
        let (broker, runtime) = bind_gate(&test.host);
        let conn = ConnHandle::new(1);
        let generation = runtime.try_attach(None, &conn, true).expect("attach");
        conn.track(
            "stub-session",
            Arc::clone(&runtime),
            false,
            None,
            generation,
        );
        let thread = spawn_create(
            Arc::clone(&test.host),
            serde_json::json!({
                "sessionId": "stub-session",
                "command": "cmd.exe",
                "args": ["/c", "exit"],
                "env": [{ "name": "DB_GATE", "value": "SAFE" }]
            }),
        );
        let id = wait_for_pending(&broker, Duration::from_secs(2));
        let events = conn.pull_events();
        broker.respond(&id, PermissionOutcome::Deny).expect("deny");
        let _ = thread.join();
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        let env = events.iter().find_map(|event| match &event.envelope.event {
            SessionEvent::PermissionRequest { env, .. } => env.clone(),
            _ => None,
        });
        let env = env.expect("permission must include env");
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].name, "DB_GATE");
        assert_eq!(env[0].value, "SAFE");
    }

    #[test]
    fn dsr_counter_counts_every_query_in_a_chunk() {
        assert_eq!(super::count_dsr_queries(b"\x1b[6n\x1b[6n"), 2);
        assert_eq!(super::count_dsr_queries(b"abc"), 0);
        assert_eq!(super::count_dsr_queries(b"\x1b[6nX\x1b[6n"), 2);
    }

    #[cfg(windows)]
    #[test]
    fn shell_batch_paths_do_not_collide_across_hosts() {
        let dir = unique_dir("batch");
        let plan = super::spawn_plan("echo collide", &[]);
        let first = super::prepare_spawn(&plan, &dir, "t-1").expect("first batch");
        let second = super::prepare_spawn(&plan, &dir, "t-1").expect("second batch");
        let left = first.batch_file.expect("first path");
        let right = second.batch_file.expect("second path");
        let same = left == right;
        let _ = std::fs::remove_file(&left);
        let _ = std::fs::remove_file(&right);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            !same,
            "two hosts sharing a runtime_dir and terminal_id must not share a .cmd path: {left:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn terminal_kill_deletes_the_shell_batch_file() {
        let test = host();
        let (broker, _runtime) = bind_gate(&test.host);
        let _allow = AutoAllow::start(Arc::clone(&broker));
        let created = test
            .host
            .create_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "command": "echo hello"
            }))
            .expect("create");
        let terminal_id = created["terminalId"].as_str().expect("id").to_string();
        let suffix = format!("-{terminal_id}.cmd");
        let batch = std::fs::read_dir(&test.runtime)
            .expect("runtime dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("acp-") && name.ends_with(&suffix))
            })
            .expect("spawn must write a unique acp-*.cmd");
        assert!(batch.is_file(), "spawn must write {batch:?}");
        test.host
            .kill_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "terminalId": terminal_id
            }))
            .expect("kill");
        let exists_after_kill = batch.is_file();
        test.host.shutdown();
        let _ = std::fs::remove_dir_all(&test.cwd);
        let _ = std::fs::remove_dir_all(&test.runtime);
        assert!(
            !exists_after_kill,
            "kill must delete the shell batch file before release"
        );
    }

    #[cfg(windows)]
    #[test]
    fn terminal_create_shell_line_runs_when_runtime_dir_has_a_space() {
        let cwd = unique_dir("cwd");
        let runtime = std::env::temp_dir().join(format!(
            "acp gate space {}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&runtime).expect("spaced runtime");
        assert!(
            runtime.to_string_lossy().contains(' '),
            "fixture runtime dir must contain a space: {runtime:?}"
        );
        let host = AcpHost::new(
            cwd.clone(),
            runtime.clone(),
            Arc::new(JobObject::new().expect("job")),
        );
        host.set_session_id("stub-session".to_string());
        let (broker, _session) = bind_gate(&host);
        let _allow = AutoAllow::start(Arc::clone(&broker));
        let created = host
            .create_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "command": "echo SPACE_OK"
            }))
            .expect("create");
        let terminal_id = created["terminalId"].clone();
        let output = wait_for_output_containing(&host, &terminal_id, "SPACE_OK", Duration::from_secs(5));
        let code = wait_for_exit_code(&host, terminal_id.clone());
        host.release_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        }))
        .expect("release");
        host.shutdown();
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&runtime);
        assert!(
            output.contains("SPACE_OK"),
            "spaced runtime dir missed the marker: {output:?}"
        );
        assert_eq!(code, 0, "spaced runtime dir command must exit 0");
    }
}
