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
use tauri::ipc::Channel;
use tauri::State;

pub use devboule_protocol::{validate_session_id, Session, SessionEvent, SessionKind};

const RING_CAPACITY: usize = 256 * 1024;
const READ_CHUNK: usize = 16 * 1024;
const INITIAL_COLS: u16 = 120;
const INITIAL_ROWS: u16 = 32;
const MAX_WRITE_BYTES: usize = 64 * 1024;
const READER_JOIN_BUDGET: Duration = Duration::from_millis(150);
const SHELL_OVERRIDE_ENV: &str = "DEVBOULE_SHELL";

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

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
    /// stream mutex is held. M2 has one attached view per session, so the prior
    /// subscriber is replaced before registration. The reader cannot publish a
    /// chunk between those two operations, so no output is dropped in the attach
    /// startup window.
    fn attach(&self, from_cursor: Option<u64>, channel: Channel<SessionEvent>) {
        let Ok(mut stream) = self.stream.lock() else {
            return;
        };
        let channel_id = channel.id();
        stream.subscribers.clear();
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

    /// The child, reader, and scrollback intentionally stay alive so a later attach
    /// can replay output produced while no view existed.
    fn detach(&self) {
        if let Ok(mut stream) = self.stream.lock() {
            stream.subscribers.clear();
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

    #[cfg(test)]
    fn subscriber_count(&self) -> usize {
        self.stream.lock().unwrap().subscribers.len()
    }
}

/// The registry owns this value; the reader and command paths keep Arcs to the
/// endpoints/runtime they need after releasing the map lock.
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

/// M2 supports only `SessionKind::Terminal`; the enum already carries the
/// additive ACP slot for M6.
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
    let runtime = session_runtime(&state, &id)?;
    runtime.attach(from_cursor, ch);
    Ok(())
}

/// Detach the current view without touching the process, reader, registry, or
/// scrollback. M2 deliberately permits one attached view; a later attach
/// replaces any stale subscriber left behind by a view that failed to detach.
#[tauri::command]
pub fn session_detach(state: State<'_, SessionState>, id: String) -> Result<(), String> {
    detach_session(&state, &id)
}

/// No framing or intent handling is performed in M2.
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

fn session_runtime(state: &SessionState, id: &str) -> Result<Arc<SessionRuntime>, String> {
    validate_session_id(id)?;
    let map = state
        .inner
        .lock()
        .map_err(|_| "Session state is unavailable.".to_string())?;
    let session = map
        .get(id)
        .ok_or_else(|| "No session with that id.".to_string())?;
    Ok(Arc::clone(&session.runtime))
}

fn detach_session(state: &SessionState, id: &str) -> Result<(), String> {
    let runtime = session_runtime(state, id)?;
    runtime.detach();
    Ok(())
}

/// Windows children do not necessarily die with the GUI, so
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

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
