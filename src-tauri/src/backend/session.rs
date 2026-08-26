//! The M2 terminal session backend.
//!
//! This is the app-hosted PTY portion of v1 ported to the v2 session contract.
//! The PTY plumbing follows the permissively licensed `portable-pty` pattern used
//! by terax-ai (Apache-2.0): `native_pty_system`/`openpty`, an explicit
//! `PtySize`, `CommandBuilder`, `take_writer`, `try_clone_reader`, and a reader
//! thread. v2 deliberately has no sandbox/AppContainer broker, so Windows and
//! Unix use the same native portable-pty path.
//!
//! The scrollback is memory-only. It is never persisted or logged. Terminal bytes
//! are converted with UTF-8-lossy at the reader boundary so a read that splits a
//! UTF-8 codepoint cannot panic; xterm.js consumes the resulting stream.
//!
//! LOCKING ORDER:
//! - The session registry lock is never held across blocking PTY I/O.
//! - `writer` and `master` are cloned under the registry lock, then their locks are
//!   taken after the registry lock has been released.
//! - Teardown removes the session first, then kills, drops writer/master, waits for
//!   the child, and only then bounded-joins the reader. This order is load-bearing
//!   on Windows because waiting while a ConPTY master remains open can deadlock.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

const RING_CAPACITY: usize = 256 * 1024;
const READ_CHUNK: usize = 16 * 1024;
const INITIAL_COLS: u16 = 120;
const INITIAL_ROWS: u16 = 32;
const MAX_WRITE_BYTES: usize = 64 * 1024;
const READER_JOIN_BUDGET: Duration = Duration::from_millis(150);
const SHELL_OVERRIDE_ENV: &str = "DEVBOULE_SHELL";

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Session kinds are intentionally an enum from day one. M2 implements Terminal;
/// ACP/Agent can be added as another serialized variant without changing the
/// command signatures or the existing `terminal` wire value.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Terminal,
    Acp,
}

/// Public session metadata returned by `session_create` and `sessions_list`.
/// `workspace_id` is optional in M2 because workspace lookup is not implemented
/// yet; the terminal starts in the app process's current directory.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub workspace_id: Option<String>,
    pub kind: SessionKind,
    pub title: String,
}

/// Events sent over the Tauri Channel supplied to `session_attach`.
///
/// `seq` starts at 1 and is contiguous for output chunks in one session. A
/// cursor means “the last output sequence already received”; replay therefore
/// sends chunks whose sequence is strictly greater than `from_cursor`.
/// Permission and ACP variants are intentionally reserved for M6: adding new
/// tagged variants is additive for consumers that ignore unknown event types.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    Output { seq: u64, data: String },
    Exit { code: Option<u32> },
}

/// Everything needed to spawn one PTY child, independent of the session kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

impl PtyCommand {
    pub fn new(
        program: impl Into<String>,
        args: Vec<String>,
        cwd: PathBuf,
        env: Vec<(String, String)>,
    ) -> Self {
        Self {
            program: program.into(),
            args,
            cwd,
            env,
        }
    }

    fn to_command_builder(&self) -> CommandBuilder {
        let mut command = CommandBuilder::new(&self.program);
        command.args(&self.args);
        command.cwd(&self.cwd);
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }
}

/// Append bytes to a bounded byte ring, dropping the oldest bytes first.
/// This pure helper is kept separate from the sequenced scrollback so its cap
/// policy remains directly unit-testable.
pub fn push_capped(ring: &mut VecDeque<u8>, data: &[u8], cap: usize) {
    if cap == 0 {
        ring.clear();
        return;
    }
    if data.len() >= cap {
        ring.clear();
        ring.extend(&data[data.len() - cap..]);
        return;
    }
    let overflow = (ring.len() + data.len()).saturating_sub(cap);
    if overflow > 0 {
        ring.drain(..overflow);
    }
    ring.extend(data);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SequencedChunk {
    seq: u64,
    data: Vec<u8>,
}

#[derive(Debug, Default)]
struct Scrollback {
    chunks: VecDeque<SequencedChunk>,
    bytes: usize,
}

impl Scrollback {
    fn push(&mut self, seq: u64, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let retained = if data.len() > RING_CAPACITY {
            data[data.len() - RING_CAPACITY..].to_vec()
        } else {
            data.to_vec()
        };
        while self.bytes + retained.len() > RING_CAPACITY {
            let Some(oldest) = self.chunks.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(oldest.data.len());
        }
        self.bytes += retained.len();
        self.chunks.push_back(SequencedChunk {
            seq,
            data: retained,
        });
    }

    fn replay_after(&self, from_cursor: Option<u64>) -> Vec<SessionEvent> {
        self.chunks
            .iter()
            .filter(|chunk| from_cursor.is_none_or(|cursor| chunk.seq > cursor))
            .map(|chunk| SessionEvent::Output {
                seq: chunk.seq,
                data: String::from_utf8_lossy(&chunk.data).into_owned(),
            })
            .collect()
    }
}

struct StreamState {
    next_seq: u64,
    scrollback: Scrollback,
    subscribers: Vec<Channel<SessionEvent>>,
    finished: bool,
    exit_code: Option<u32>,
}

/// Stream state is one mutex on purpose. Holding it across `Channel::send`
/// makes attach ordering exact: the subscriber is registered, its snapshot is
/// sent, and only then can the reader publish the next live chunk.
struct SessionRuntime {
    stream: Mutex<StreamState>,
    peak_ring_bytes: AtomicUsize,
    reader_finished: AtomicBool,
    child_reaped: AtomicBool,
}

impl SessionRuntime {
    fn new() -> Self {
        Self {
            stream: Mutex::new(StreamState {
                next_seq: 1,
                scrollback: Scrollback::default(),
                subscribers: Vec::new(),
                finished: false,
                exit_code: None,
            }),
            peak_ring_bytes: AtomicUsize::new(0),
            reader_finished: AtomicBool::new(false),
            child_reaped: AtomicBool::new(false),
        }
    }

    fn publish_output(&self, data: &str) {
        let Ok(mut stream) = self.stream.lock() else {
            return;
        };
        if stream.finished {
            return;
        }
        let seq = stream.next_seq;
        stream.next_seq = stream.next_seq.saturating_add(1);
        stream.scrollback.push(seq, data.as_bytes());
        self.peak_ring_bytes
            .fetch_max(stream.scrollback.bytes, Ordering::Relaxed);
        let event = SessionEvent::Output {
            seq,
            data: data.to_owned(),
        };
        stream
            .subscribers
            .retain(|channel| channel.send(event.clone()).is_ok());
    }

    /// Subscribe FIRST, then replay the ring through the SAME channel while the
    /// stream mutex is held. The reader cannot publish a chunk between those two
    /// operations, so no output is dropped in the attach startup window.
    fn attach(&self, from_cursor: Option<u64>, channel: Channel<SessionEvent>) {
        let Ok(mut stream) = self.stream.lock() else {
            return;
        };
        let channel_id = channel.id();
        stream.subscribers.push(channel.clone());

        let replay = stream.scrollback.replay_after(from_cursor);
        let replay_ok = replay.into_iter().all(|event| channel.send(event).is_ok());
        let exit_ok = if replay_ok && stream.finished {
            channel
                .send(SessionEvent::Exit {
                    code: stream.exit_code,
                })
                .is_ok()
        } else {
            replay_ok
        };
        if !exit_ok {
            stream
                .subscribers
                .retain(|subscriber| subscriber.id() != channel_id);
        }
    }

    fn finish(&self, code: Option<u32>) {
        let Ok(mut stream) = self.stream.lock() else {
            return;
        };
        if stream.finished {
            return;
        }
        stream.finished = true;
        stream.exit_code = code;
        let event = SessionEvent::Exit { code };
        for channel in &stream.subscribers {
            let _ = channel.send(event.clone());
        }
        stream.subscribers.clear();
    }

    #[cfg(test)]
    fn snapshot(&self) -> (Vec<(u64, String)>, usize) {
        let stream = self.stream.lock().unwrap();
        let chunks = stream
            .scrollback
            .chunks
            .iter()
            .map(|chunk| (chunk.seq, String::from_utf8_lossy(&chunk.data).into_owned()))
            .collect();
        (chunks, stream.scrollback.bytes)
    }
}

/// A live terminal session. The registry owns this value; the reader and command
/// paths keep Arcs to the endpoints/runtime they need after releasing the map lock.
struct PtySession {
    metadata: Session,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child: Option<Box<dyn Child + Send + Sync>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader_handle: Option<JoinHandle<()>>,
    runtime: Arc<SessionRuntime>,
    exited: Arc<AtomicBool>,
}

/// Tauri-managed registry of session id → live PTY session.
#[derive(Clone, Default)]
pub struct SessionState {
    inner: Arc<Mutex<HashMap<String, PtySession>>>,
}

impl SessionState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Create a terminal session. M2 supports only `SessionKind::Terminal`; the enum
/// already carries the additive ACP slot for M6.
#[tauri::command]
pub fn session_create(
    state: State<'_, SessionState>,
    workspace_id: Option<String>,
    kind: SessionKind,
) -> Result<Session, String> {
    if kind != SessionKind::Terminal {
        return Err("Only terminal sessions are available in M2.".to_string());
    }
    let metadata = Session {
        id: new_session_id(),
        workspace_id,
        kind,
        title: "Terminal".to_string(),
    };
    let command = shell_command()?;
    spawn_session(&state, metadata.clone(), command)?;
    Ok(metadata)
}

/// Attach to a session's output stream.
///
/// IMPORTANT STARTUP ORDER: `attach` registers the supplied Channel before it
/// snapshots/replays the ring, and both happen under the same stream mutex. Live
/// reader output therefore waits until replay has finished and then continues on
/// the same Channel; there is no subscribe/snapshot race or dropped chunk.
#[tauri::command]
pub fn session_attach(
    state: State<'_, SessionState>,
    id: String,
    from_cursor: Option<u64>,
    ch: Channel<SessionEvent>,
) -> Result<(), String> {
    validate_session_id(&id)?;
    let runtime = {
        let map = state
            .inner
            .lock()
            .map_err(|_| "Session state is unavailable.".to_string())?;
        let session = map
            .get(&id)
            .ok_or_else(|| "No session with that id.".to_string())?;
        Arc::clone(&session.runtime)
    };
    runtime.attach(from_cursor, ch);
    Ok(())
}

/// Send raw bytes to the terminal. No framing or intent handling is performed in M2.
#[tauri::command]
pub fn session_send(
    state: State<'_, SessionState>,
    id: String,
    text: String,
) -> Result<(), String> {
    validate_session_id(&id)?;
    if text.len() > MAX_WRITE_BYTES {
        return Err("Session input is too large.".to_string());
    }
    let writer = {
        let map = state
            .inner
            .lock()
            .map_err(|_| "Session state is unavailable.".to_string())?;
        let session = map
            .get(&id)
            .ok_or_else(|| "No session with that id.".to_string())?;
        Arc::clone(&session.writer)
    };
    let mut writer = writer
        .lock()
        .map_err(|_| "Session state is unavailable.".to_string())?;
    writer
        .write_all(text.as_bytes())
        .map_err(|_| "Could not send input to the terminal.".to_string())?;
    writer
        .flush()
        .map_err(|_| "Could not flush input to the terminal.".to_string())
}

/// Resize a terminal to the viewer's current geometry.
#[tauri::command]
pub fn session_resize(
    state: State<'_, SessionState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    validate_session_id(&id)?;
    let master = {
        let map = state
            .inner
            .lock()
            .map_err(|_| "Session state is unavailable.".to_string())?;
        let session = map
            .get(&id)
            .ok_or_else(|| "No session with that id.".to_string())?;
        Arc::clone(&session.master)
    };
    let master = master
        .lock()
        .map_err(|_| "Session state is unavailable.".to_string())?;
    master
        .resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|_| "Could not resize the terminal.".to_string())
}

/// Close a terminal, kill/reap its child, and release all PTY resources.
#[tauri::command]
pub fn session_close(state: State<'_, SessionState>, id: String) -> Result<(), String> {
    validate_session_id(&id)?;
    close_session(&state, &id);
    Ok(())
}

/// Return all currently live sessions in stable id order.
#[tauri::command]
pub fn sessions_list(state: State<'_, SessionState>) -> Result<Vec<Session>, String> {
    let map = state
        .inner
        .lock()
        .map_err(|_| "Session state is unavailable.".to_string())?;
    let mut sessions: Vec<Session> = map
        .values()
        .map(|session| session.metadata.clone())
        .collect();
    sessions.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(sessions)
}

/// App-exit reaper. Windows children do not necessarily die with the GUI, so
/// drain every live session and apply the same kill → endpoint drop → wait →
/// bounded reader-join ordering used by `session_close`.
pub fn kill_all_on_exit(state: &SessionState) {
    let sessions = state
        .inner
        .lock()
        .ok()
        .map(|mut map| map.drain().map(|(_, session)| session).collect::<Vec<_>>())
        .unwrap_or_default();
    for session in sessions {
        teardown_session(session);
    }
}

fn new_session_id() -> String {
    format!(
        "session-{}-{}",
        std::process::id(),
        SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

enum ReaderMode {
    Channel,
    #[cfg(test)]
    AtomicByteCounter(Arc<AtomicUsize>),
}

fn spawn_session(
    state: &SessionState,
    metadata: Session,
    command: PtyCommand,
) -> Result<(), String> {
    spawn_session_with_reader_mode(state, metadata, command, ReaderMode::Channel)
}

fn spawn_session_with_reader_mode(
    state: &SessionState,
    metadata: Session,
    command: PtyCommand,
    reader_mode: ReaderMode,
) -> Result<(), String> {
    // On Windows portable-pty selects ConPTY internally. The frontend must keep
    // xterm stdin/onData enabled: ConPTY may issue a DSR query (`ESC[6n`) at
    // startup and stalls its render pipeline until the terminal answers it.
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|_| "Could not open the terminal.".to_string())?;
    let mut child = pair
        .slave
        .spawn_command(command.to_command_builder())
        .map_err(|_| "Could not start the terminal shell.".to_string())?;
    let killer = child.clone_killer();

    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(_) => {
            let mut killer = killer;
            let _ = killer.kill();
            drop(pair.master);
            let _ = child.wait();
            return Err("Could not attach to the terminal.".to_string());
        }
    };
    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(_) => {
            let mut killer = killer;
            let _ = killer.kill();
            drop(writer);
            drop(pair.master);
            let _ = child.wait();
            return Err("Could not read from the terminal.".to_string());
        }
    };

    let runtime = Arc::new(SessionRuntime::new());
    let exited = Arc::new(AtomicBool::new(false));
    let master = Arc::new(Mutex::new(pair.master));
    let writer = Arc::new(Mutex::new(writer));
    let id = metadata.id.clone();
    let session = PtySession {
        metadata,
        master,
        killer,
        child: Some(child),
        writer,
        reader_handle: None,
        runtime: Arc::clone(&runtime),
        exited: Arc::clone(&exited),
    };

    // Insert BEFORE starting the reader. A shell can exit before the reader
    // thread gets scheduled; inserting later would let EOF cleanup miss the map
    // entry and strand the session.
    {
        let Ok(mut map) = state.inner.lock() else {
            teardown_session(session);
            return Err("Session state is unavailable.".to_string());
        };
        map.insert(id.clone(), session);
    }

    let reader_state = state.clone();
    let reader_id = id.clone();
    let reader_runtime = Arc::clone(&runtime);
    let reader_handle = match std::thread::Builder::new()
        .name(format!("session-pty-{id}"))
        .spawn(move || reader_loop(reader_state, reader_id, reader, reader_runtime, reader_mode))
    {
        Ok(handle) => handle,
        Err(_) => {
            close_session(state, &id);
            return Err("Could not start the terminal reader.".to_string());
        }
    };

    // The child can exit before this lock is acquired. In that case EOF cleanup
    // already removed the session; join the now-finished reader here instead of
    // leaking its handle.
    let mut orphaned_reader = Some(reader_handle);
    if let Ok(mut map) = state.inner.lock() {
        if let Some(session) = map.get_mut(&id) {
            session.reader_handle = orphaned_reader.take();
        }
    }
    if let Some(reader_handle) = orphaned_reader {
        let _ = reader_handle.join();
    }
    Ok(())
}

fn reader_loop(
    state: SessionState,
    id: String,
    mut reader: Box<dyn Read + Send>,
    runtime: Arc<SessionRuntime>,
    reader_mode: ReaderMode,
) {
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => match &reader_mode {
                ReaderMode::Channel => {
                    runtime.publish_output(&String::from_utf8_lossy(&buf[..n]));
                }
                #[cfg(test)]
                ReaderMode::AtomicByteCounter(counter) => {
                    counter.fetch_add(n, Ordering::Relaxed);
                }
            },
            Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }

    // EOF means the child ended on its own. Remove the map entry, drop PTY
    // endpoints before waiting, then reap it. The reader's own JoinHandle is
    // detached from the stored session before its value is dropped.
    let exit_code = finish_reader_session(&state, &id);
    runtime.finish(exit_code);
    runtime.reader_finished.store(true, Ordering::Release);
}

fn finish_reader_session(state: &SessionState, id: &str) -> Option<u32> {
    let mut session = state.inner.lock().ok().and_then(|mut map| map.remove(id))?;
    session.reader_handle = None;
    let PtySession {
        master,
        writer,
        killer,
        child,
        runtime,
        exited,
        ..
    } = session;
    exited.store(true, Ordering::SeqCst);
    drop(killer);
    drop(writer);
    drop(master);
    let exit_code = child.and_then(wait_child);
    runtime
        .child_reaped
        .store(exit_code.is_some(), Ordering::Release);
    exit_code
}

fn wait_child(mut child: Box<dyn Child + Send + Sync>) -> Option<u32> {
    child.wait().ok().map(|status| status.exit_code())
}

fn close_session(state: &SessionState, id: &str) {
    let session = state.inner.lock().ok().and_then(|mut map| map.remove(id));
    if let Some(session) = session {
        teardown_session(session);
    }
}

/// Kill + drop writer/master + wait + bounded reader join. ORDER IS LOAD-BEARING:
/// on Windows, waiting while the ConPTY master is alive can deadlock the ConPTY
/// host. Dropping the master also unblocks the reader's blocking read.
fn teardown_session(session: PtySession) {
    session.exited.store(true, Ordering::SeqCst);
    let PtySession {
        master,
        mut killer,
        child,
        writer,
        reader_handle,
        runtime,
        exited: _,
        metadata: _,
    } = session;

    // 1) Kill first. ChildKiller is separate so this cannot race with wait().
    let _ = killer.kill();
    drop(killer);
    // 2) Drop writer and master BEFORE wait(). The writer owns another master-side
    //    handle, and ConPTY's host can remain alive while either handle is open.
    //    Closing them also unblocks the reader. The registry entry was removed
    //    before this function, so only transient command-side Arc clones remain.
    drop(writer);
    drop(master);
    // 3) Reap after the PTY endpoints are closed; this prevents a zombie and avoids
    //    the Windows ConPTY wait deadlock.
    let child_reaped = child.is_some_and(|child| wait_child(child).is_some());
    runtime.child_reaped.store(child_reaped, Ordering::Release);
    // 4) Best-effort bounded join. JoinHandle has no timed join; the endpoint
    //    close above makes the reader finish promptly, while this small budget
    //    prevents app shutdown from accumulating a hang across sessions.
    if let Some(handle) = reader_handle {
        let deadline = Instant::now() + READER_JOIN_BUDGET;
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
    runtime.finish(None);
}

fn shell_command() -> Result<PtyCommand, String> {
    let cwd = std::env::current_dir()
        .map_err(|_| "Could not determine terminal directory.".to_string())?;
    let (program, args) = configured_shell();
    Ok(PtyCommand::new(program, args, cwd, Vec::new()))
}

fn configured_shell() -> (String, Vec<String>) {
    if let Ok(override_shell) = std::env::var(SHELL_OVERRIDE_ENV) {
        if !override_shell.trim().is_empty() {
            return (override_shell, shell_args());
        }
    }
    #[cfg(windows)]
    {
        let program = if executable_on_path("pwsh.exe") {
            "pwsh.exe"
        } else {
            "powershell.exe"
        };
        (
            program.to_string(),
            vec!["-NoLogo".to_string(), "-NoProfile".to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        let program = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        (program, shell_args())
    }
}

fn shell_args() -> Vec<String> {
    #[cfg(windows)]
    {
        vec!["-NoLogo".to_string(), "-NoProfile".to_string()]
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
fn executable_on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| {
        let candidate = Path::new(&directory).join(program);
        candidate.is_file()
    })
}

/// Validate an externally supplied session id before using it as a map key.
pub fn validate_session_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err("Invalid session id.".to_string());
    }
    if id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err("Invalid session id.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_capped_drops_oldest_bytes() {
        let mut ring = VecDeque::new();
        push_capped(&mut ring, b"abcd", 8);
        push_capped(&mut ring, b"efghij", 8);
        assert_eq!(ring.iter().copied().collect::<Vec<_>>(), b"cdefghij");
    }

    #[test]
    fn push_capped_large_chunk_keeps_only_tail() {
        let mut ring = VecDeque::new();
        push_capped(&mut ring, b"hello", 16);
        push_capped(&mut ring, b"0123456789abcdefXYZ", 8);
        assert_eq!(ring.iter().copied().collect::<Vec<_>>(), b"bcdefXYZ");
    }

    #[test]
    fn push_capped_zero_cap_is_empty() {
        let mut ring = VecDeque::new();
        push_capped(&mut ring, b"abc", 0);
        assert!(ring.is_empty());
    }

    #[test]
    fn validate_session_id_accepts_safe_ids_and_rejects_smuggling() {
        assert!(validate_session_id("session-123-1").is_ok());
        assert!(validate_session_id("a.b_c-2").is_ok());
        assert!(validate_session_id(&"x".repeat(64)).is_ok());
        assert!(validate_session_id("").is_err());
        assert!(validate_session_id(&"x".repeat(65)).is_err());
        assert!(validate_session_id("../other").is_err());
        assert!(validate_session_id("session id").is_err());
        assert!(validate_session_id("a:b").is_err());
    }

    #[test]
    fn cursor_replay_is_strictly_after_last_seen_sequence() {
        let mut scrollback = Scrollback::default();
        scrollback.push(1, b"one");
        scrollback.push(2, b"two");
        scrollback.push(3, b"three");
        assert_eq!(
            scrollback.replay_after(Some(1)),
            vec![
                SessionEvent::Output {
                    seq: 2,
                    data: "two".to_string()
                },
                SessionEvent::Output {
                    seq: 3,
                    data: "three".to_string()
                },
            ]
        );
        assert_eq!(scrollback.replay_after(None).len(), 3);
    }

    #[test]
    fn session_event_uses_a_type_tag() {
        let output = serde_json::to_value(SessionEvent::Output {
            seq: 7,
            data: "hi".to_string(),
        })
        .unwrap();
        assert_eq!(output["type"], "output");
        assert_eq!(output["seq"], 7);
        let exit = serde_json::to_value(SessionEvent::Exit { code: Some(0) }).unwrap();
        assert_eq!(exit["type"], "exit");
        assert_eq!(exit["code"], 0);
    }

    #[test]
    fn attach_replay_and_live_output_share_one_ordered_stream() {
        let runtime = SessionRuntime::new();
        runtime.publish_output("before");
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_for_channel = Arc::clone(&received);
        let channel = Channel::new(move |body| {
            let event: SessionEvent = body.deserialize()?;
            received_for_channel.lock().unwrap().push(event);
            Ok(())
        });
        runtime.attach(Some(0), channel);
        runtime.publish_output("after");
        assert_eq!(
            *received.lock().unwrap(),
            vec![
                SessionEvent::Output {
                    seq: 1,
                    data: "before".to_string()
                },
                SessionEvent::Output {
                    seq: 2,
                    data: "after".to_string()
                },
            ]
        );
    }

    #[cfg(windows)]
    fn answer_test_dsr(state: &SessionState, id: &str) {
        let writer = {
            let map = state.inner.lock().unwrap();
            Arc::clone(&map.get(id).unwrap().writer)
        };
        let mut writer = writer.lock().unwrap();
        writer.write_all(b"\x1b[1;1R").unwrap();
        writer.flush().unwrap();
    }

    #[cfg(windows)]
    fn start_test_dsr_pump(
        state: &SessionState,
        id: &str,
    ) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
        let writer = {
            let map = state.inner.lock().unwrap();
            Arc::clone(&map.get(id).unwrap().writer)
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_for_thread.load(Ordering::Acquire) {
                if let Ok(mut writer) = writer.lock() {
                    let _ = writer.write_all(b"\x1b[1;1R");
                    let _ = writer.flush();
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
        (stop, handle)
    }

    #[cfg(windows)]
    fn stop_test_dsr_pump(stop: Arc<AtomicBool>, handle: std::thread::JoinHandle<()>) {
        stop.store(true, Ordering::Release);
        let _ = handle.join();
    }

    #[test]
    fn ring_never_exceeds_256_kibibytes() {
        let runtime = SessionRuntime::new();
        runtime.publish_output(&"x".repeat(RING_CAPACITY / 2));
        runtime.publish_output(&"y".repeat(RING_CAPACITY));
        let (_, bytes) = runtime.snapshot();
        assert_eq!(bytes, RING_CAPACITY);
        assert_eq!(
            runtime.peak_ring_bytes.load(Ordering::Relaxed),
            RING_CAPACITY
        );
    }

    // Real PTY tests are Windows-gated and ignored by default because ConPTY
    // integration needs a desktop-capable runner. Run with `--ignored` locally.
    #[cfg(windows)]
    fn test_state() -> SessionState {
        SessionState::new()
    }

    #[cfg(windows)]
    fn attach_collecting(state: &SessionState, id: &str, received: Arc<Mutex<Vec<SessionEvent>>>) {
        let target = {
            let map = state.inner.lock().unwrap();
            Arc::clone(&map.get(id).unwrap().runtime)
        };
        let received_for_channel = Arc::clone(&received);
        target.attach(
            None,
            Channel::new(move |body| {
                received_for_channel
                    .lock()
                    .unwrap()
                    .push(body.deserialize()?);
                Ok(())
            }),
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
    fn real_pty_spawn_read_resize_and_teardown() {
        let state = test_state();
        let id = "session-test-echo".to_string();
        let session = Session {
            id: id.clone(),
            workspace_id: None,
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
        };
        let command = PtyCommand::new(
            "cmd.exe",
            vec!["/c".to_string(), "echo DEVBOULE_PTY_OK".to_string()],
            std::env::current_dir().unwrap(),
            Vec::new(),
        );
        spawn_session(&state, session, command).unwrap();
        // ConPTY can issue DSR (`ESC[6n`) before the viewer attaches. A real xterm
        // answers it through onData; this headless integration test does so here.
        answer_test_dsr(&state, &id);
        let (stop_dsr, dsr_thread) = start_test_dsr_pump(&state, &id);
        let received = Arc::new(Mutex::new(Vec::new()));
        attach_collecting(&state, &id, Arc::clone(&received));
        session_resize_inner(&state, &id, 100, 30).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if received.lock().unwrap().iter().any(|event| {
                matches!(event, SessionEvent::Output { data, .. } if data.contains("DEVBOULE_PTY_OK"))
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let saw_marker = received.lock().unwrap().iter().any(|event| {
            matches!(event, SessionEvent::Output { data, .. } if data.contains("DEVBOULE_PTY_OK"))
        });
        close_session(&state, &id);
        stop_test_dsr_pump(stop_dsr, dsr_thread);
        assert!(saw_marker);
        assert!(state.inner.lock().unwrap().is_empty());
    }

    #[cfg(windows)]
    fn session_resize_inner(
        state: &SessionState,
        id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        let master = {
            let map = state.inner.lock().unwrap();
            Arc::clone(&map.get(id).unwrap().master)
        };
        let result = master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string());
        result
    }

    #[cfg(windows)]
    struct ChannelTransportMetrics {
        bytes: usize,
        wall: Duration,
        chunk_sizes: Vec<usize>,
        seq_reordered: bool,
        peak_ring_bytes: usize,
        teardown: Duration,
        child_reaped: bool,
        clean: bool,
    }

    #[cfg(windows)]
    fn summarize_chunk_sizes(chunk_sizes: &[usize]) -> (usize, f64, usize) {
        let mut sorted = chunk_sizes.to_vec();
        sorted.sort_unstable();
        let min = *sorted.first().expect("transport produced no chunks");
        let max = *sorted.last().unwrap();
        let middle = sorted.len() / 2;
        let median = if sorted.len().is_multiple_of(2) {
            (sorted[middle - 1] + sorted[middle]) as f64 / 2.0
        } else {
            sorted[middle] as f64
        };
        (min, median, max)
    }

    #[cfg(windows)]
    fn benchmark_file_command(file_path: &Path) -> PtyCommand {
        // The test temp path is deliberately a simple filename. portable-pty
        // quotes each argv entry, while cmd.exe applies its own /c quoting rules.
        PtyCommand::new(
            "cmd.exe",
            vec!["/c".to_string(), format!("type {}", file_path.display())],
            std::env::current_dir().unwrap(),
            Vec::new(),
        )
    }

    #[cfg(windows)]
    fn run_channel_transport(
        state: &SessionState,
        id: &str,
        file_path: &Path,
        expected_file_bytes: usize,
        reader_mode: ReaderMode,
    ) -> ChannelTransportMetrics {
        let session = Session {
            id: id.to_string(),
            workspace_id: None,
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
        };
        spawn_session_with_reader_mode(
            state,
            session,
            benchmark_file_command(file_path),
            reader_mode,
        )
        .unwrap();
        let runtime = {
            let map = state.inner.lock().unwrap();
            Arc::clone(&map.get(id).unwrap().runtime)
        };
        let writer_for_channel = {
            let map = state.inner.lock().unwrap();
            Arc::clone(&map.get(id).unwrap().writer)
        };
        let observed = Arc::new(Mutex::new((
            0usize,
            Vec::<usize>::new(),
            None::<u64>,
            false,
        )));
        let observed_for_channel = Arc::clone(&observed);
        let start = Instant::now();
        runtime.attach(
            None,
            Channel::new(move |body| {
                let event: SessionEvent = body.deserialize()?;
                if let SessionEvent::Output { seq, data } = event {
                    if data.contains("\x1b[6n") {
                        let mut writer = writer_for_channel.lock().unwrap();
                        let _ = writer.write_all(b"\x1b[1;1R");
                        let _ = writer.flush();
                    }
                    let mut observed = observed_for_channel.lock().unwrap();
                    let expected = observed.2.map_or(seq, |last| last + 1);
                    if seq != expected {
                        observed.3 = true;
                    }
                    observed.2 = Some(seq);
                    observed.0 += data.len();
                    observed.1.push(data.len());
                }
                Ok(())
            }),
        );
        let (stop_dsr, dsr_thread) = start_test_dsr_pump(state, id);

        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            if observed.lock().unwrap().0 >= expected_file_bytes {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let complete = observed.lock().unwrap().0 >= expected_file_bytes;
        if !complete {
            close_session(state, id);
            stop_test_dsr_pump(stop_dsr, dsr_thread);
            panic!(
                "channel transport did not finish: bytes={} expected_file_bytes={expected_file_bytes}",
                observed.lock().unwrap().0
            );
        }

        let wall = start.elapsed();
        let (bytes, chunk_sizes, seq_reordered) = {
            let observed = observed.lock().unwrap();
            (observed.0, observed.1.clone(), observed.3)
        };
        let peak_ring_bytes = runtime.peak_ring_bytes.load(Ordering::Relaxed);
        let close_start = Instant::now();
        close_session(state, id);
        let teardown = close_start.elapsed();
        stop_test_dsr_pump(stop_dsr, dsr_thread);
        let child_reaped = runtime.child_reaped.load(Ordering::Acquire);
        let clean = state.inner.lock().unwrap().is_empty()
            && runtime.reader_finished.load(Ordering::Acquire)
            && child_reaped;
        ChannelTransportMetrics {
            bytes,
            wall,
            chunk_sizes,
            seq_reordered,
            peak_ring_bytes,
            teardown,
            child_reaped,
            clean,
        }
    }

    #[cfg(windows)]
    struct AtomicTransportMetrics {
        bytes: usize,
        wall: Duration,
        peak_ring_bytes: usize,
        teardown: Duration,
        child_reaped: bool,
        clean: bool,
    }

    #[cfg(windows)]
    fn run_atomic_transport(
        state: &SessionState,
        id: &str,
        file_path: &Path,
        expected_file_bytes: usize,
    ) -> AtomicTransportMetrics {
        let counter = Arc::new(AtomicUsize::new(0));
        let session = Session {
            id: id.to_string(),
            workspace_id: None,
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
        };
        spawn_session_with_reader_mode(
            state,
            session,
            benchmark_file_command(file_path),
            ReaderMode::AtomicByteCounter(Arc::clone(&counter)),
        )
        .unwrap();
        // B has no Channel callback to answer ConPTY's startup DSR query.
        answer_test_dsr(state, id);
        let (stop_dsr, dsr_thread) = start_test_dsr_pump(state, id);
        let runtime = {
            let map = state.inner.lock().unwrap();
            Arc::clone(&map.get(id).unwrap().runtime)
        };
        let start = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            if counter.load(Ordering::Acquire) >= expected_file_bytes {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let bytes = counter.load(Ordering::Acquire);
        if bytes < expected_file_bytes {
            close_session(state, id);
            stop_test_dsr_pump(stop_dsr, dsr_thread);
            panic!(
                "atomic transport did not finish: bytes={bytes} expected_file_bytes={expected_file_bytes}"
            );
        }

        let wall = start.elapsed();
        let peak_ring_bytes = runtime.peak_ring_bytes.load(Ordering::Relaxed);
        let close_start = Instant::now();
        close_session(state, id);
        let teardown = close_start.elapsed();
        stop_test_dsr_pump(stop_dsr, dsr_thread);
        let child_reaped = runtime.child_reaped.load(Ordering::Acquire);
        let clean = state.inner.lock().unwrap().is_empty()
            && runtime.reader_finished.load(Ordering::Acquire)
            && child_reaped;
        AtomicTransportMetrics {
            bytes,
            wall,
            peak_ring_bytes,
            teardown,
            child_reaped,
            clean,
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
    fn real_pty_channel_flood_correctness() {
        const LINES: usize = 50_000;
        const PAYLOAD: &str = "DEVBOULE_LOAD_0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        const DONE: &str = "DEVBOULE_LOAD_DONE";
        let state = test_state();
        let id = "session-test-load".to_string();
        let session = Session {
            id: id.clone(),
            workspace_id: None,
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
        };
        let command = PtyCommand::new(
            "pwsh.exe",
            vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!("$line = '{PAYLOAD}'; 1..{LINES} | ForEach-Object {{ $line }}; '{DONE}'"),
            ],
            std::env::current_dir().unwrap(),
            Vec::new(),
        );
        spawn_session(&state, session, command).unwrap();
        answer_test_dsr(&state, &id);
        let (stop_dsr, dsr_thread) = start_test_dsr_pump(&state, &id);
        let observed = Arc::new(Mutex::new((0usize, 0usize, None::<u64>, false, false)));
        let observed_for_channel = Arc::clone(&observed);
        let writer_for_channel = {
            let map = state.inner.lock().unwrap();
            Arc::clone(&map.get(&id).unwrap().writer)
        };
        let runtime = {
            let map = state.inner.lock().unwrap();
            Arc::clone(&map.get(&id).unwrap().runtime)
        };
        runtime.attach(
            None,
            Channel::new(move |body| {
                let event: SessionEvent = body.deserialize()?;
                if let SessionEvent::Output { seq, data } = event {
                    if data.contains("\x1b[6n") {
                        let mut writer = writer_for_channel.lock().unwrap();
                        let _ = writer.write_all(b"\x1b[1;1R");
                        let _ = writer.flush();
                    }
                    let mut observed = observed_for_channel.lock().unwrap();
                    let expected = observed.2.map_or(seq, |last| last + 1);
                    if seq != expected {
                        observed.3 = true;
                    }
                    if data.contains(DONE) {
                        observed.4 = true;
                    }
                    observed.2 = Some(seq);
                    observed.0 += data.len();
                    observed.1 += 1;
                }
                Ok(())
            }),
        );

        let start = Instant::now();
        let deadline = start + Duration::from_secs(60);
        while Instant::now() < deadline {
            if observed.lock().unwrap().4 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let wall = start.elapsed();
        if !observed.lock().unwrap().4 {
            close_session(&state, &id);
            stop_test_dsr_pump(stop_dsr, dsr_thread);
            panic!("load child did not emit its completion marker");
        }
        let (bytes, chunks, _, reordered, _) = *observed.lock().unwrap();
        let expected_bytes = LINES * PAYLOAD.len();
        let output_complete = bytes >= expected_bytes;
        let peak_ring_bytes = runtime.peak_ring_bytes.load(Ordering::Relaxed);
        let close_start = Instant::now();
        close_session(&state, &id);
        let teardown = close_start.elapsed();
        stop_test_dsr_pump(stop_dsr, dsr_thread);
        let clean = state.inner.lock().unwrap().is_empty()
            && runtime.reader_finished.load(Ordering::Acquire)
            && runtime.child_reaped.load(Ordering::Acquire);
        println!(
            "PTY_CORRECTNESS lines={LINES} expected_min_bytes={expected_bytes} bytes={bytes} chunks={chunks} wall_ms={} peak_ring_bytes={peak_ring_bytes} output_complete={output_complete} seq_reordered={reordered} child_reaped={} teardown_ms={} clean={clean}",
            wall.as_millis(),
            runtime.child_reaped.load(Ordering::Acquire),
            teardown.as_millis(),
        );
        assert!(
            output_complete,
            "the generator did not deliver its expected flood"
        );
        assert!(!reordered, "output sequence was dropped or reordered");
        assert!(peak_ring_bytes <= RING_CAPACITY);
        assert!(
            runtime.child_reaped.load(Ordering::Acquire),
            "child wait did not reap a status"
        );
        assert!(clean, "teardown left a session or reader thread");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
    fn real_pty_channel_file_transport_ab_benchmark() {
        const DATA_LINES: usize = 200_000;
        const PAYLOAD: &str = "DEVBOULE_TRANSPORT_0123456789abcdefghijklmnopqrstuvwxyz0123456789";

        let file_path = std::env::temp_dir().join(format!(
            "devboule-pty-transport-{}-{}.txt",
            std::process::id(),
            SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        {
            let file = std::fs::File::create(&file_path).unwrap();
            let mut file = std::io::BufWriter::new(file);
            for _ in 0..DATA_LINES {
                file.write_all(PAYLOAD.as_bytes()).unwrap();
                file.write_all(b"\r\n").unwrap();
            }
            file.flush().unwrap();
        }
        let expected_file_bytes = std::fs::metadata(&file_path).unwrap().len() as usize;

        // A and B use the same file, command line, PTY size, and 16 KiB reader.
        // A retains the production Channel path; B replaces publication with one
        // atomic raw-byte counter and therefore performs no serialization or send.
        let channel = run_channel_transport(
            &test_state(),
            "session-test-transport-channel",
            &file_path,
            expected_file_bytes,
            ReaderMode::Channel,
        );
        let atomic = run_atomic_transport(
            &test_state(),
            "session-test-transport-counter",
            &file_path,
            expected_file_bytes,
        );
        let (channel_min, channel_median, channel_max) =
            summarize_chunk_sizes(&channel.chunk_sizes);
        let channel_mib_s = channel.bytes as f64 / (1024.0 * 1024.0) / channel.wall.as_secs_f64();
        let atomic_mib_s = atomic.bytes as f64 / (1024.0 * 1024.0) / atomic.wall.as_secs_f64();
        println!(
            "PTY_AB scenario=channel bytes={} expected_file_bytes={expected_file_bytes} wall_ms={} mib_s={channel_mib_s:.2} messages={} messages_per_s={:.2} chunk_min={channel_min} chunk_median={channel_median:.1} chunk_max={channel_max} peak_ring_bytes={} seq_reordered={} child_reaped={} teardown_ms={} clean={}",
            channel.bytes,
            channel.wall.as_millis(),
            channel.chunk_sizes.len(),
            channel.chunk_sizes.len() as f64 / channel.wall.as_secs_f64(),
            channel.peak_ring_bytes,
            channel.seq_reordered,
            channel.child_reaped,
            channel.teardown.as_millis(),
            channel.clean,
        );
        println!(
            "PTY_AB scenario=atomic_counter bytes={} expected_file_bytes={expected_file_bytes} wall_ms={} mib_s={atomic_mib_s:.2} messages=n/a peak_ring_bytes={} seq_continuity=n/a child_reaped={} teardown_ms={} clean={}",
            atomic.bytes,
            atomic.wall.as_millis(),
            atomic.peak_ring_bytes,
            atomic.child_reaped,
            atomic.teardown.as_millis(),
            atomic.clean,
        );
        println!(
            "PTY_AB comparison atomic_over_channel_speedup={:.2}",
            atomic_mib_s / channel_mib_s
        );

        assert!(
            channel.bytes >= expected_file_bytes,
            "Channel output was truncated"
        );
        assert!(
            !channel.seq_reordered,
            "Channel output was dropped or reordered"
        );
        assert!(channel.peak_ring_bytes <= RING_CAPACITY);
        assert!(channel.child_reaped && channel.clean);
        assert!(
            atomic.bytes >= expected_file_bytes,
            "atomic output was truncated"
        );
        assert_eq!(atomic.peak_ring_bytes, 0);
        assert!(atomic.child_reaped && atomic.clean);

        println!("PTY_COALESCING skipped=atomic_counter_within_20_percent_of_channel");

        let _ = std::fs::remove_file(&file_path);
    }
}
