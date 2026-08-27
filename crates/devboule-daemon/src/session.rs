//! Daemon-owned PTY sessions.
//!
//! This is the M2 terminal backend moved out of the Tauri process. The PTY
//! plumbing follows the permissively licensed `portable-pty` pattern used by
//! terax-ai (Apache-2.0): `native_pty_system`/`openpty`, an explicit
//! `PtySize`, `CommandBuilder`, `take_writer`, `try_clone_reader`, and a
//! reader thread. v2 deliberately has no sandbox/AppContainer broker, so
//! Windows and Unix use the same native portable-pty path.
//!
//! The scrollback is memory-only. It is never persisted or logged. Terminal
//! bytes are converted with UTF-8-lossy at the coalesced-flush boundary so a
//! read that splits a UTF-8 codepoint cannot panic; xterm.js consumes the
//! resulting stream.
//!
//! LOCKING ORDER:
//! - The session registry lock is never held across blocking PTY I/O.
//! - `writer` and `master` are cloned under the registry lock, then their
//!   locks are taken after the registry lock has been released.
//! - Teardown removes the session first, then kills, drops writer/master,
//!   waits for the child, and only then bounded-joins the reader. This
//!   order is load-bearing on Windows because waiting while a ConPTY
//!   master remains open can deadlock.
//!
//! STREAMING:
//! M2 rejected coalescing because the in-process Channel was free (ConPTY
//! itself was the floor at ~0.52 MiB/s, ~7k msg/s, median 67-byte chunks).
//! M3b puts NDJSON and a named pipe on that path; 7k tiny frames/s is a
//! different proposition. The reader coalesces into one seq-assigned chunk
//! per [`COALESCE_MAX_BYTES`] or [`COALESCE_FLUSH`], whichever comes first.
//! Seq is assigned at flush so the stream stays contiguous.
//!
//! Live output is not copied into the per-connection RPC queue. The 256 KiB
//! ring is the buffer; the connection writer pulls it. A slow client cannot
//! OOM the daemon. Blocking the PTY reader is wrong (it stalls the watched
//! process). Dropping bytes from the ring is wrong (the scrollback would
//! lie). If the writer lags past the ring, the client skips the same bytes
//! a late attach would skip.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

use devboule_protocol::{
    compose_session_id, cursor_replay_ok, validate_session_id, Cursor, ErrorCode, OwnerId, Session,
    SessionEvent, SessionEventEnvelope, SessionKind, WireError,
};

use crate::outbound::ConnOut;
use crate::paths::RuntimePaths;
use crate::server::ServerState;

pub const RING_CAPACITY: usize = 256 * 1024;
const READ_CHUNK: usize = 16 * 1024;
const INITIAL_COLS: u16 = 120;
const INITIAL_ROWS: u16 = 32;
pub const MAX_WRITE_BYTES: usize = 64 * 1024;
const READER_JOIN_BUDGET: Duration = Duration::from_millis(150);
const SHELL_OVERRIDE_ENV: &str = "DEVBOULE_SHELL";
const TEST_PTY_COMMAND_FILE: &str = ".test-pty-command";

/// Accumulate reader output until this many bytes, then assign one seq.
/// 8 KiB is half a ConPTY read and far under the 1 MiB frame cap.
pub const COALESCE_MAX_BYTES: usize = 8 * 1024;
/// Flush multi-byte coalesced output after this much quiet. 4 ms is half a
/// 120 Hz frame and enough to merge a flood of 67-byte ConPTY chunks. The
/// one-byte interactive case uses [`COALESCE_EAGER_BYTES`] instead.
pub const COALESCE_FLUSH: Duration = Duration::from_millis(4);
/// A single-byte ConPTY read is the interactive keystroke case. Publish it
/// immediately so a platform timer rounding the quiet wait cannot add a
/// frame-sized delay; larger reads retain the flood coalescing path.
pub const COALESCE_EAGER_BYTES: usize = 1;

/// After the child has been reaped, keep draining ConPTY for this long
/// before emitting `exit`. `Child::wait` returns before the last bytes
/// have been read; dropping them would truncate the live stream.
const EXIT_DRAIN: Duration = Duration::from_millis(200);

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
#[allow(dead_code)]
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

struct Attachment {
    conn_id: u64,
    outbound: Arc<ConnOut>,
}

struct StreamState {
    next_seq: u64,
    generation: u64,
    scrollback: Scrollback,
    attached: Option<Attachment>,
    /// Reader has seen EOF. Further publish_output is dropped.
    output_closed: bool,
    /// Child::wait returned. Output may still be in the ConPTY buffer.
    process_exited: bool,
    exit_code: Option<u32>,
    last_publish: Option<Instant>,
    exit_at: Option<Instant>,
}

/// Stream state is one mutex on purpose. Holding it across attach
/// registration makes attach ordering exact: the subscriber is registered
/// and its pull cursor is set, and only then can the reader publish the
/// next live chunk. Subscribe-before-replay.
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
                generation: 1,
                scrollback: Scrollback::default(),
                attached: None,
                output_closed: false,
                process_exited: false,
                exit_code: None,
                last_publish: None,
                exit_at: None,
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
        if stream.output_closed {
            return;
        }
        let seq = stream.next_seq;
        stream.next_seq = stream.next_seq.saturating_add(1);
        stream.scrollback.push(seq, data.as_bytes());
        stream.last_publish = Some(Instant::now());
        self.peak_ring_bytes
            .fetch_max(stream.scrollback.bytes, Ordering::Relaxed);
        if let Some(attached) = &stream.attached {
            attached.outbound.notify();
        }
    }

    /// Register this connection as the single attached viewer. A second
    /// different connection is rejected. The same connection re-attaching
    /// replaces its cursor.
    fn try_attach(&self, from_cursor: Option<Cursor>, conn: &ConnHandle) -> Result<u64, WireError> {
        let Ok(mut stream) = self.stream.lock() else {
            return Err(internal("Session state is unavailable."));
        };
        if let Some(current) = &stream.attached {
            if current.conn_id != conn.id {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "session is already attached to another client",
                ));
            }
        }
        if let Some(cursor) = from_cursor {
            cursor_replay_ok(stream.generation, cursor)?;
        }
        stream.attached = Some(Attachment {
            conn_id: conn.id,
            outbound: Arc::clone(&conn.outbound),
        });
        Ok(stream.generation)
    }

    fn detach_if_conn(&self, conn_id: u64) {
        if let Ok(mut stream) = self.stream.lock() {
            if stream
                .attached
                .as_ref()
                .is_some_and(|attached| attached.conn_id == conn_id)
            {
                stream.attached = None;
            }
        }
    }

    fn mark_exited(&self, code: Option<u32>) {
        let Ok(mut stream) = self.stream.lock() else {
            return;
        };
        if stream.process_exited {
            return;
        }
        stream.process_exited = true;
        stream.exit_code = code;
        stream.exit_at = Some(Instant::now());
        if let Some(attached) = &stream.attached {
            attached.outbound.notify();
        }
    }

    fn close_output(&self) {
        let Ok(mut stream) = self.stream.lock() else {
            return;
        };
        stream.output_closed = true;
        if let Some(attached) = &stream.attached {
            attached.outbound.notify();
        }
    }

    fn finish(&self, code: Option<u32>) {
        self.mark_exited(code);
        self.close_output();
    }

    fn ready_for_exit(stream: &StreamState) -> bool {
        if stream.output_closed {
            return true;
        }
        if !stream.process_exited {
            return false;
        }
        let origin = stream.last_publish.or(stream.exit_at);
        origin.is_none_or(|instant| instant.elapsed() >= EXIT_DRAIN)
    }

    #[cfg(test)]
    fn bump_generation(&self) -> u64 {
        let Ok(mut stream) = self.stream.lock() else {
            return 1;
        };
        stream.generation = stream.generation.saturating_add(1);
        stream.next_seq = 1;
        stream.output_closed = false;
        stream.process_exited = false;
        stream.exit_code = None;
        stream.last_publish = None;
        stream.exit_at = None;
        stream.generation
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
    fn attached_conn_id(&self) -> Option<u64> {
        self.stream
            .lock()
            .unwrap()
            .attached
            .as_ref()
            .map(|attached| attached.conn_id)
    }
}

/// Per-connection handle: RPC outbound plus the sessions this client pulls.
pub struct ConnHandle {
    pub id: u64,
    pub outbound: Arc<ConnOut>,
    attached: Mutex<HashMap<String, PullState>>,
}

struct PullState {
    runtime: Arc<SessionRuntime>,
    sent_seq: Option<u64>,
    exit_sent: bool,
    generation: u64,
}

impl ConnHandle {
    pub fn new(id: u64) -> Arc<Self> {
        Arc::new(Self {
            id,
            outbound: ConnOut::new(),
            attached: Mutex::new(HashMap::new()),
        })
    }

    fn track(
        &self,
        session_id: &str,
        runtime: Arc<SessionRuntime>,
        from_seq: Option<u64>,
        generation: u64,
    ) {
        if let Ok(mut map) = self.attached.lock() {
            map.insert(
                session_id.to_string(),
                PullState {
                    runtime,
                    sent_seq: from_seq,
                    exit_sent: false,
                    generation,
                },
            );
        }
        self.outbound.notify();
    }

    fn untrack(&self, session_id: &str) {
        if let Ok(mut map) = self.attached.lock() {
            map.remove(session_id);
        }
    }

    /// Return the next one-shot wake needed to emit an exit after its drain
    /// window. Ordinary live sessions return `None`, so the connection writer
    /// remains asleep until a request or PTY notification arrives.
    pub fn next_exit_wake(&self) -> Option<Duration> {
        let map = self.attached.lock().ok()?;
        map.values()
            .filter_map(|pull| {
                let stream = pull.runtime.stream.lock().ok()?;
                if stream.output_closed
                    || !stream.process_exited
                    || SessionRuntime::ready_for_exit(&stream)
                {
                    return None;
                }
                let origin = stream.last_publish.or(stream.exit_at)?;
                Some(EXIT_DRAIN.saturating_sub(origin.elapsed()))
            })
            .min()
    }

    /// Drop every subscription this connection holds. The processes stay.
    pub fn detach_all(&self, registry: &SessionRegistry) {
        let ids = self
            .attached
            .lock()
            .map(|mut map| map.drain().map(|(id, _)| id).collect::<Vec<_>>())
            .unwrap_or_default();
        for id in ids {
            registry.detach_runtime(&id, self.id);
        }
    }

    /// Pull replay + live output + exit for every session this connection
    /// is attached to. Called from the writer thread; does not send.
    pub fn pull_events(&self) -> Vec<SessionEventEnvelope> {
        let Ok(mut map) = self.attached.lock() else {
            return Vec::new();
        };
        let mut events = Vec::new();
        let mut finished_ids = Vec::new();
        for (session_id, pull) in map.iter_mut() {
            let Ok(stream) = pull.runtime.stream.lock() else {
                continue;
            };
            let replay = stream.scrollback.replay_after(pull.sent_seq);
            for event in replay {
                if let SessionEvent::Output { seq, .. } = &event {
                    pull.sent_seq = Some(*seq);
                }
                events.push(SessionEventEnvelope {
                    session_id: session_id.clone(),
                    generation: pull.generation,
                    event,
                });
            }
            if !pull.exit_sent && SessionRuntime::ready_for_exit(&stream) {
                events.push(SessionEventEnvelope {
                    session_id: session_id.clone(),
                    generation: pull.generation,
                    event: SessionEvent::Exit {
                        code: stream.exit_code,
                    },
                });
                pull.exit_sent = true;
                finished_ids.push(session_id.clone());
            }
        }
        for id in finished_ids {
            map.remove(&id);
        }
        events
    }
}

/// The registry owns this value; the reader and command paths keep Arcs to
/// the endpoints/runtime they need after releasing the map lock.
struct PtySession {
    metadata: Session,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child_wait: Option<JoinHandle<Option<u32>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader_handle: Option<JoinHandle<()>>,
    coalesce_handle: Option<JoinHandle<()>>,
    runtime: Arc<SessionRuntime>,
    exited: Arc<AtomicBool>,
    /// Set by `stop`: the process dies but the session object stays. The
    /// reader must not remove the registry entry or call session_finished.
    preserve_on_exit: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<Mutex<HashMap<String, PtySession>>>,
    paths: RuntimePaths,
}

impl SessionRegistry {
    pub fn new(paths: RuntimePaths) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            paths,
        }
    }

    pub fn create(
        &self,
        state: &Arc<ServerState>,
        owner: &OwnerId,
        workspace_id: Option<String>,
        kind: SessionKind,
        command: Option<PtyCommand>,
    ) -> Result<Session, WireError> {
        if kind != SessionKind::Terminal {
            return Err(WireError::new(
                ErrorCode::Unimplemented,
                "Only terminal sessions are available.",
            ));
        }
        let unique = format!("{:08x}", SESSION_COUNTER.fetch_add(1, Ordering::Relaxed));
        let id = compose_session_id(&owner.session_token(), &unique)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let metadata = Session {
            id: id.clone(),
            workspace_id,
            kind,
            title: "Terminal".to_string(),
        };
        let command = match command {
            Some(command) => command,
            None => resolve_pty_command(&self.paths)?,
        };
        spawn_session(state, self, metadata.clone(), command)?;
        Ok(metadata)
    }

    pub fn attach(
        &self,
        session_id: &str,
        from_cursor: Option<Cursor>,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        let runtime = self.runtime(session_id)?;
        let generation = runtime.try_attach(from_cursor, conn)?;
        let from_seq = from_cursor.map(|cursor| cursor.seq);
        conn.track(session_id, runtime, from_seq, generation);
        Ok(())
    }

    pub fn detach(&self, session_id: &str, conn: &ConnHandle) -> Result<(), WireError> {
        let runtime = self.runtime(session_id)?;
        runtime.detach_if_conn(conn.id);
        conn.untrack(session_id);
        Ok(())
    }

    fn detach_runtime(&self, session_id: &str, conn_id: u64) {
        if let Ok(runtime) = self.runtime(session_id) {
            runtime.detach_if_conn(conn_id);
        }
    }

    pub fn close(&self, session_id: &str) -> Result<bool, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let session = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?
            .remove(session_id);
        match session {
            Some(session) => {
                teardown_session(session);
                Ok(true)
            }
            None => Err(not_found()),
        }
    }

    pub fn stop(&self, session_id: &str) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let mut killer = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let session = map.get_mut(session_id).ok_or_else(not_found)?;
            session.preserve_on_exit.store(true, Ordering::SeqCst);
            session.killer.clone_killer()
        };
        let _ = killer.kill();
        Ok(())
    }

    pub fn send(&self, session_id: &str, text: &str) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        if text.len() > MAX_WRITE_BYTES {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session input is too large.",
            ));
        }
        let writer = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let session = map.get(session_id).ok_or_else(not_found)?;
            Arc::clone(&session.writer)
        };
        let mut writer = writer
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        writer
            .write_all(text.as_bytes())
            .map_err(|_| WireError::new(ErrorCode::Io, "Could not send input to the terminal."))?;
        writer
            .flush()
            .map_err(|_| WireError::new(ErrorCode::Io, "Could not flush input to the terminal."))
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let master = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let session = map.get(session_id).ok_or_else(not_found)?;
            Arc::clone(&session.master)
        };
        let master = master
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| WireError::new(ErrorCode::Io, "Could not resize the terminal."))
    }

    pub fn list(&self) -> Result<Vec<Session>, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let mut sessions: Vec<Session> = map
            .values()
            .map(|session| session.metadata.clone())
            .collect();
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(sessions)
    }

    fn runtime(&self, session_id: &str) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let session = map.get(session_id).ok_or_else(not_found)?;
        Ok(Arc::clone(&session.runtime))
    }
}

pub fn spawn_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    command: PtyCommand,
) -> Result<(), WireError> {
    // On Windows portable-pty selects ConPTY internally. The frontend must
    // keep xterm stdin/onData enabled: ConPTY may issue a DSR query
    // (`ESC[6n`) at startup and stalls its render pipeline until the
    // terminal answers it.
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|_| WireError::new(ErrorCode::Io, "Could not open the terminal."))?;
    let mut child = pair
        .slave
        .spawn_command(command.to_command_builder())
        .map_err(|_| WireError::new(ErrorCode::Io, "Could not start the terminal shell."))?;
    let killer = child.clone_killer();

    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(_) => {
            let mut killer = killer;
            let _ = killer.kill();
            drop(pair.master);
            let _ = child.wait();
            return Err(WireError::new(
                ErrorCode::Io,
                "Could not attach to the terminal.",
            ));
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
            return Err(WireError::new(
                ErrorCode::Io,
                "Could not read from the terminal.",
            ));
        }
    };

    let runtime = Arc::new(SessionRuntime::new());
    let exited = Arc::new(AtomicBool::new(false));
    let master = Arc::new(Mutex::new(pair.master));
    let writer = Arc::new(Mutex::new(writer));
    let id = metadata.id.clone();
    let wait_runtime = Arc::clone(&runtime);
    let child_wait = std::thread::Builder::new()
        .name(format!("session-wait-{id}"))
        .spawn(move || {
            let code = wait_child(child);
            wait_runtime
                .child_reaped
                .store(code.is_some(), Ordering::Release);
            wait_runtime.mark_exited(code);
            code
        })
        .ok();
    let session = PtySession {
        metadata,
        master,
        killer,
        child_wait,
        writer,
        reader_handle: None,
        coalesce_handle: None,
        runtime: Arc::clone(&runtime),
        exited: Arc::clone(&exited),
        preserve_on_exit: Arc::new(AtomicBool::new(false)),
    };

    // Insert BEFORE starting the reader. A shell can exit before the reader
    // thread gets scheduled; inserting later would let EOF cleanup miss the
    // map entry and strand the session.
    {
        let Ok(mut map) = registry.inner.lock() else {
            teardown_session(session);
            return Err(internal("Session state is unavailable."));
        };
        map.insert(id.clone(), session);
    }

    let (coalesce_tx, coalesce_rx) = mpsc::channel::<Vec<u8>>();
    let coalesce_runtime = Arc::clone(&runtime);
    let coalesce_handle = match std::thread::Builder::new()
        .name(format!("session-coalesce-{id}"))
        .spawn(move || coalesce_loop(coalesce_rx, coalesce_runtime))
    {
        Ok(handle) => Some(handle),
        Err(_) => {
            let _ = registry.close(&id);
            return Err(WireError::new(
                ErrorCode::Internal,
                "Could not start the terminal reader.",
            ));
        }
    };

    let reader_registry = registry.clone();
    let reader_id = id.clone();
    let reader_runtime = Arc::clone(&runtime);
    let reader_state = Arc::downgrade(state);
    let reader_handle = match std::thread::Builder::new()
        .name(format!("session-pty-{id}"))
        .spawn(move || {
            reader_loop(
                reader_registry,
                reader_state,
                reader_id,
                reader,
                reader_runtime,
                coalesce_tx,
            );
        }) {
        Ok(handle) => handle,
        Err(_) => {
            let _ = registry.close(&id);
            return Err(WireError::new(
                ErrorCode::Internal,
                "Could not start the terminal reader.",
            ));
        }
    };

    // The child can exit before this lock is acquired. In that case EOF
    // cleanup already removed the session; join the now-finished reader
    // here instead of leaking its handle.
    let mut orphaned_reader = Some(reader_handle);
    let mut orphaned_coalesce = coalesce_handle;
    if let Ok(mut map) = registry.inner.lock() {
        if let Some(session) = map.get_mut(&id) {
            session.reader_handle = orphaned_reader.take();
            session.coalesce_handle = orphaned_coalesce.take();
        }
    }
    if let Some(reader_handle) = orphaned_reader {
        let _ = reader_handle.join();
    }
    if let Some(coalesce_handle) = orphaned_coalesce {
        let _ = coalesce_handle.join();
    }
    Ok(())
}

fn reader_loop(
    registry: SessionRegistry,
    state: Weak<ServerState>,
    id: String,
    mut reader: Box<dyn Read + Send>,
    runtime: Arc<SessionRuntime>,
    coalesce_tx: mpsc::Sender<Vec<u8>>,
) {
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let _ = coalesce_tx.send(buf[..n].to_vec());
            }
            Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    drop(coalesce_tx);

    // EOF means the child ended. `stop` keeps the session object; `close`
    // and a natural exit remove it. session_finished is only for a removal
    // so a stopped-but-listed session still holds the idle-exit gate.
    let removed = finish_reader_session(&registry, &id, &runtime);
    runtime.reader_finished.store(true, Ordering::Release);
    if removed {
        if let Some(state) = state.upgrade() {
            state.session_finished();
        }
    }
}

fn coalesce_loop(rx: mpsc::Receiver<Vec<u8>>, runtime: Arc<SessionRuntime>) {
    let mut pending = Vec::new();
    loop {
        let received = if pending.is_empty() {
            rx.recv().ok()
        } else {
            match rx.recv_timeout(COALESCE_FLUSH) {
                Ok(bytes) => Some(bytes),
                Err(RecvTimeoutError::Timeout) => {
                    flush_coalesced(&mut pending, &runtime);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => None,
            }
        };
        match received {
            Some(bytes) => {
                pending.extend_from_slice(&bytes);
                if pending.len() >= COALESCE_MAX_BYTES || pending.len() == COALESCE_EAGER_BYTES {
                    flush_coalesced(&mut pending, &runtime);
                }
            }
            None => {
                flush_coalesced(&mut pending, &runtime);
                break;
            }
        }
    }
}

fn flush_coalesced(pending: &mut Vec<u8>, runtime: &SessionRuntime) {
    if pending.is_empty() {
        return;
    }
    let data = String::from_utf8_lossy(pending).into_owned();
    pending.clear();
    runtime.publish_output(&data);
}

/// Returns whether the registry entry was removed (so the caller can
/// decrement the live-session count). `None` from the lock means another
/// path already took the session — do not session_finished again.
fn finish_reader_session(registry: &SessionRegistry, id: &str, runtime: &SessionRuntime) -> bool {
    let Ok(mut map) = registry.inner.lock() else {
        return false;
    };
    let Some(session) = map.get_mut(id) else {
        return false;
    };
    session.reader_handle = None;
    if session.preserve_on_exit.load(Ordering::SeqCst) {
        session.exited.store(true, Ordering::SeqCst);
        runtime.close_output();
        return false;
    }
    let mut session = map.remove(id).expect("get_mut saw the key");
    drop(map);
    session.reader_handle = None;
    let coalesce = session.coalesce_handle.take();
    let child_wait = session.child_wait.take();
    let PtySession {
        master,
        writer,
        killer,
        runtime: session_runtime,
        exited,
        ..
    } = session;
    exited.store(true, Ordering::SeqCst);
    drop(killer);
    drop(writer);
    drop(master);
    bounded_join(child_wait);
    bounded_join(coalesce);
    let _ = session_runtime;
    runtime.close_output();
    true
}

fn wait_child(mut child: Box<dyn Child + Send + Sync>) -> Option<u32> {
    child.wait().ok().map(|status| status.exit_code())
}

/// Kill + drop writer/master + wait + bounded reader join. ORDER IS
/// LOAD-BEARING: on Windows, waiting while the ConPTY master is alive can
/// deadlock the ConPTY host. Dropping the master also unblocks the
/// reader's blocking read.
fn teardown_session(session: PtySession) {
    session.exited.store(true, Ordering::SeqCst);
    let PtySession {
        master,
        mut killer,
        child_wait,
        writer,
        reader_handle,
        coalesce_handle,
        runtime,
        exited: _,
        metadata: _,
        preserve_on_exit: _,
    } = session;

    // 1) Kill first. ChildKiller is separate so this cannot race with wait().
    let _ = killer.kill();
    drop(killer);
    // 2) Drop writer and master BEFORE wait(). The writer owns another
    //    master-side handle, and ConPTY's host can remain alive while either
    //    handle is open. Closing them also unblocks the reader. The registry
    //    entry was removed before this function, so only transient
    //    command-side Arc clones remain.
    drop(writer);
    drop(master);
    // 3) Reap after the PTY endpoints are closed; this prevents a zombie
    //    and avoids the Windows ConPTY wait deadlock. The waiter thread
    //    owns Child::wait so we join it here instead of calling wait()
    //    ourselves.
    bounded_join(child_wait);
    // 4) Best-effort bounded join. JoinHandle has no timed join; the
    //    endpoint close above makes the reader finish promptly, while this
    //    small budget prevents shutdown from accumulating a hang across
    //    sessions.
    bounded_join(coalesce_handle);
    bounded_join(reader_handle);
    runtime.finish(None);
}

fn bounded_join<T>(handle: Option<JoinHandle<T>>) {
    if let Some(handle) = handle {
        let deadline = Instant::now() + READER_JOIN_BUDGET;
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
}

fn resolve_pty_command(paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    #[cfg(debug_assertions)]
    {
        if let Some(command) = load_test_pty_command(paths) {
            return Ok(command);
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = paths;
    }
    shell_command()
}

#[cfg(debug_assertions)]
fn load_test_pty_command(paths: &RuntimePaths) -> Option<PtyCommand> {
    let path = paths.dir.join(TEST_PTY_COMMAND_FILE);
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let program = value.get("program")?.as_str()?.to_string();
    let args = value
        .get("args")
        .and_then(|args| args.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let cwd = value
        .get("cwd")
        .and_then(|cwd| cwd.as_str())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;
    Some(PtyCommand::new(program, args, cwd, Vec::new()))
}

fn shell_command() -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir()
        .map_err(|_| WireError::new(ErrorCode::Io, "Could not determine terminal directory."))?;
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

fn not_found() -> WireError {
    WireError::new(ErrorCode::SessionNotFound, "No session with that id.")
}

fn internal(message: impl Into<String>) -> WireError {
    WireError::new(ErrorCode::Internal, message)
}

/// Write a spawn override the next `session_create` will consume. Honored
/// only in debug builds of the daemon (see [`load_test_pty_command`]).
pub fn write_test_pty_command(paths: &RuntimePaths, command: &PtyCommand) -> std::io::Result<()> {
    paths.ensure_dir()?;
    let body = serde_json::json!({
        "program": command.program,
        "args": command.args,
        "cwd": command.cwd,
    });
    std::fs::write(paths.dir.join(TEST_PTY_COMMAND_FILE), body.to_string())
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

    #[test]
    fn attach_rejects_a_second_connection() {
        let runtime = SessionRuntime::new();
        let first = ConnHandle::new(1);
        let second = ConnHandle::new(2);
        runtime.try_attach(None, &first).expect("first");
        let err = runtime.try_attach(None, &second).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRequest);
        assert!(err.message.contains("already attached"));
        assert_eq!(runtime.attached_conn_id(), Some(1));
    }

    #[test]
    fn same_connection_can_reattach() {
        let runtime = SessionRuntime::new();
        let conn = ConnHandle::new(7);
        runtime.try_attach(None, &conn).expect("first");
        runtime
            .try_attach(
                Some(Cursor {
                    generation: 1,
                    seq: 0,
                }),
                &conn,
            )
            .expect("reattach");
        assert_eq!(runtime.attached_conn_id(), Some(7));
    }

    #[test]
    fn stale_generation_is_rejected() {
        let runtime = SessionRuntime::new();
        runtime.bump_generation();
        let conn = ConnHandle::new(1);
        let err = runtime
            .try_attach(
                Some(Cursor {
                    generation: 1,
                    seq: 0,
                }),
                &conn,
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::SessionGenerationMismatch);
    }

    #[test]
    fn detach_clears_only_this_connection() {
        let runtime = SessionRuntime::new();
        let conn = ConnHandle::new(3);
        runtime.try_attach(None, &conn).expect("attach");
        runtime.detach_if_conn(3);
        assert_eq!(runtime.attached_conn_id(), None);
    }

    #[test]
    fn pull_replays_then_live_then_exit() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.publish_output("before");
        let conn = ConnHandle::new(1);
        let generation = runtime
            .try_attach(
                Some(Cursor {
                    generation: 1,
                    seq: 0,
                }),
                &conn,
            )
            .unwrap();
        conn.track("s.a.1", Arc::clone(&runtime), Some(0), generation);
        runtime.publish_output("after");
        runtime.finish(Some(0));
        let events = conn.pull_events();
        let kinds: Vec<_> = events
            .iter()
            .map(|envelope| match &envelope.event {
                SessionEvent::Output { data, .. } => data.as_str(),
                SessionEvent::Exit { .. } => "exit",
            })
            .collect();
        assert_eq!(kinds, ["before", "after", "exit"]);
        assert!(conn.pull_events().is_empty());
    }

    #[test]
    fn coalesce_constants_are_small_enough_for_echo() {
        const { assert!(COALESCE_MAX_BYTES <= 16 * 1024) };
        const { assert!(COALESCE_MAX_BYTES >= 1024) };
        assert!(COALESCE_FLUSH <= Duration::from_millis(16));
    }
}
