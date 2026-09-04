//! ACP client methods the agent calls on us: filesystem and terminals.
//!
//! Terminals reuse the daemon's ConPTY + Job Object path. There is no scope
//! guardian yet: any absolute path and any command the agent names is honored.
//! That surface is declared, not papered over.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use agent_client_protocol::schema::v1::{
    CreateTerminalRequest, CreateTerminalResponse, KillTerminalResponse, ReadTextFileRequest,
    ReadTextFileResponse, ReleaseTerminalResponse, TerminalExitStatus, TerminalId,
    TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use portable_pty::{CommandBuilder, PtySize};

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

    pub(super) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "code": self.code, "message": self.message })
    }
}

pub(super) struct AcpHost {
    session_id: Mutex<String>,
    cwd: PathBuf,
    runtime_dir: PathBuf,
    daemon_job: Arc<JobObject>,
    terminals: Mutex<HashMap<String, Arc<AcpTerminal>>>,
    next_terminal: AtomicU64,
    max_terminals: usize,
    #[cfg(test)]
    create_gap: Mutex<Option<Arc<std::sync::Barrier>>>,
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
            #[cfg(test)]
            create_gap: Mutex::new(None),
        })
    }

    #[cfg(test)]
    fn set_create_gap(&self, barrier: Arc<std::sync::Barrier>) {
        if let Ok(mut gap) = self.create_gap.lock() {
            *gap = Some(barrier);
        }
    }

    #[cfg(test)]
    fn live_terminal_count(&self) -> usize {
        self.terminals.lock().map(|map| map.len()).unwrap_or(0)
    }

    pub(super) fn set_session_id(&self, session_id: String) {
        if let Ok(mut current) = self.session_id.lock() {
            *current = session_id;
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
            .map(|mut map| {
                map.drain()
                    .map(|(_, terminal)| terminal)
                    .collect::<Vec<_>>()
            })
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
        let terminal = spawn_acp_terminal(
            &request.command,
            &request.args,
            &cwd,
            &env,
            Arc::clone(&self.daemon_job),
            limit,
        )?;
        #[cfg(test)]
        {
            let gap = self.create_gap.lock().ok().and_then(|guard| guard.clone());
            if let Some(barrier) = gap {
                barrier.wait();
            }
        }
        let mut terminals = self
            .terminals
            .lock()
            .map_err(|_| RpcError::internal("terminal map lock poisoned"))?;
        if terminals.len() >= self.max_terminals {
            drop(terminals);
            terminal.release();
            return Err(RpcError::invalid_params(format!(
                "session has reached the maximum of {} ACP terminals",
                self.max_terminals
            )));
        }
        let id = format!("t-{}", self.next_terminal.fetch_add(1, Ordering::Relaxed));
        terminals.insert(id.clone(), terminal);
        drop(terminals);
        serde_json::to_value(CreateTerminalResponse::new(TerminalId::new(id)))
            .map_err(|error| RpcError::internal(error.to_string()))
    }

    fn get_terminal(&self, terminal_id: &str) -> Result<Arc<AcpTerminal>, RpcError> {
        let terminals = self
            .terminals
            .lock()
            .map_err(|_| RpcError::internal("terminal map lock poisoned"))?;
        terminals
            .get(terminal_id)
            .cloned()
            .ok_or_else(|| RpcError::resource_not_found(format!("unknown terminal {terminal_id}")))
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
            terminals
                .remove(request.terminal_id.0.as_ref())
                .ok_or_else(|| {
                    RpcError::resource_not_found(format!(
                        "unknown terminal {}",
                        request.terminal_id.0
                    ))
                })?
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
            .map(|mut map| {
                map.drain()
                    .map(|(_, terminal)| terminal)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for terminal in terminals {
            terminal.release();
        }
    }
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
        if let Ok(mut waiters) = self.rpc_waiters.lock() {
            for handle in waiters.drain(..) {
                let _ = handle.join();
            }
        }
    }
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

fn spawn_acp_terminal(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    daemon_job: Arc<JobObject>,
    output_limit: u64,
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
    });
    let output_terminal = Arc::clone(&terminal);
    let reader_handle = std::thread::Builder::new()
        .name("acp-term-read".to_string())
        .spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(count) => output_terminal.push_output(&buf[..count]),
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
    use super::{slice_lines, AcpHost, BoundedBuffer, MAX_FS_BYTES};
    use crate::process_tree::JobObject;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

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
    fn terminal_echo_completes_and_keeps_output_after_kill_race() {
        let test = host();
        let host = test.host;
        let created = host
            .create_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "command": "cmd.exe",
                "args": ["/c", "echo PONG"]
            }))
            .expect("create");
        let terminal_id = created["terminalId"].clone();
        let start = std::sync::Arc::new(std::sync::Barrier::new(3));
        let (wait_tx, wait_rx) = std::sync::mpsc::channel();
        let wait_host = Arc::clone(&host);
        let wait_id = terminal_id.clone();
        let wait_start = Arc::clone(&start);
        let wait_thread = std::thread::spawn(move || {
            wait_start.wait();
            let (tx, rx) = std::sync::mpsc::channel();
            let respond: super::RpcRespond = Arc::new(move |_, result| {
                let _ = tx.send(result);
            });
            wait_host.wait_for_exit(
                serde_json::json!(1),
                serde_json::json!({
                    "sessionId": "stub-session",
                    "terminalId": wait_id
                }),
                respond,
            );
            let _ = wait_tx.send(rx.recv_timeout(std::time::Duration::from_secs(10)));
        });
        let kill_host = Arc::clone(&host);
        let kill_id = terminal_id.clone();
        let kill_start = Arc::clone(&start);
        let kill_thread = std::thread::spawn(move || {
            kill_start.wait();
            kill_host.kill_terminal(serde_json::json!({
                "sessionId": "stub-session",
                "terminalId": kill_id
            }))
        });
        start.wait();
        let wait_result = wait_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("wait thread")
            .expect("wait recv");
        let wait_value = wait_result.expect("wait_for_exit");
        assert!(wait_value.get("exitCode").is_some() || wait_value.get("signal").is_some());
        kill_thread.join().expect("kill thread").expect("kill");
        wait_thread.join().expect("wait join");
        let output = host
            .terminal_output(serde_json::json!({
                "sessionId": "stub-session",
                "terminalId": terminal_id
            }))
            .expect("output after kill still valid");
        let text = output["output"].as_str().unwrap_or("");
        assert!(
            text.contains("PONG") || output.get("exitStatus").is_some(),
            "terminal produced neither PONG nor exit status: {output}"
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
        host.create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "cmd.exe",
            "args": ["/c", "exit"]
        }))
        .expect("fill to one live terminal");
        let gap = Arc::new(std::sync::Barrier::new(2));
        host.set_create_gap(Arc::clone(&gap));
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
        host.shutdown();
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(runtime);
        assert!(
            live <= 2,
            "live terminals={live} successes={}",
            successes.load(std::sync::atomic::Ordering::SeqCst)
        );
    }
}
