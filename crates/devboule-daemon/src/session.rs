//! Daemon-owned PTY sessions.
//!
//! This is the M2 terminal backend moved out of the Tauri process. The PTY
//! plumbing follows the permissively licensed `portable-pty` pattern used by
//! terax-ai (Apache-2.0): `native_pty_system`/`openpty`, an explicit
//! `PtySize`, `CommandBuilder`, `take_writer`, `try_clone_reader`, and a
//! reader thread. v2 deliberately has no sandbox/AppContainer broker, so
//! Windows and Unix use the same native portable-pty path.
//!
//! SCREEN STATE (M3.5):
//! Every output chunk is applied to a headless terminal emulator
//! ([`crate::screen::Screen`]) under the session state lock. The emulator is
//! the screen authority, the same shape Zed's pty-host RFC and tmux use: on
//! attach the client gets one `Snapshot(as_of_seq)` of the visible grid, then
//! ordinary live output chunks with strictly greater sequences. There is no
//! byte replay for a live screen and no replay cursor. Coalesced frames are
//! additionally enqueued to the conversation journal off this thread
//! (`try_send`, never a disk wait); the journal stays the durable transcript.
//! A recovered session has no emulator and replays the journal instead.
//! Terminal bytes are converted with UTF-8-lossy at the coalesced-flush
//! boundary so a read that splits a UTF-8 codepoint cannot panic.
//!
//! THE INVARIANT:
//! A snapshot carrying `as_of_seq = N` is exactly the emulator state after
//! every chunk with sequence `<= N` has been applied and before any chunk
//! with sequence `> N`. The boundary is on application to the emulator — not
//! the pipe write, not the journal commit, not client receipt. Capture of the
//! screen and registration of a new attachment happen under ONE hold of the
//! state lock, so output can never fall into neither the snapshot nor the
//! attachment's unsent queue. When an attachment's unsent queue exceeds
//! [`PENDING_OUTPUT_BUDGET_BYTES`], the unsent suffix is discarded and
//! replaced by a fresh snapshot at the current boundary.
//!
//! DEVICE STATUS REPLIES:
//! The emulator answers terminal queries (ConPTY's startup `ESC[6n` among
//! them) with `PtyWrite` events. Those replies go straight back to the PTY
//! writer from the publish path — never through the journal, a snapshot, or
//! a client pipe. ConPTY stalls its render pipeline until the query is
//! answered; the daemon is the single responder.
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
//! Unsent live output waits in one bounded per-attachment queue
//! ([`StreamState::pending`]), not in a byte-history ring. The connection
//! writer pulls at most [`PULL_BATCH`] items per turn, so a slow client
//! leaves the bulk of the backlog inside the budgeted queue, where the
//! snapshot replacement above can coalesce it. Blocking the PTY reader is
//! wrong (it stalls ConPTY's render pipeline), so back-pressure is expressed
//! as state: the slow viewer is resynchronised, the process is never stalled.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

use devboule_protocol::{
    compose_session_id, cursor_replay_ok, validate_session_id, Cursor, CursorShape, ErrorCode,
    JournalStats, OwnerId, ScreenCursor, Session, SessionEvent, SessionEventEnvelope, SessionKind,
    SessionState, WireError,
};

use crate::journal::{new_session_record, output_record, Journal, PersistStatus, Replay};
use crate::outbound::ConnOut;
use crate::paths::RuntimePaths;
use crate::process_tree::JobObject;
use crate::screen::{Screen, ScreenSnapshot, SnapshotCursorShape};
use crate::server::ServerState;

/// Unsent live output one attachment may hold before the unsent suffix is
/// dropped and replaced by a fresh screen snapshot at the current sequence.
///
/// 256 KiB is the old ring's capacity: large enough that a healthy client is
/// never resynchronised (32 full 8 KiB coalesce frames), small enough that a
/// stalled client's queue stays bounded by roughly one worst-case snapshot.
pub const PENDING_OUTPUT_BUDGET_BYTES: usize = 256 * 1024;
/// Frame-count twin of [`PENDING_OUTPUT_BUDGET_BYTES`]: bounds the per-frame
/// JSON envelope overhead of a backlog of tiny frames.
pub const PENDING_OUTPUT_BUDGET_FRAMES: u64 = 64;

/// Session events pulled from one session per connection-writer turn.
/// Deliberately small: items left behind stay in the session's budgeted
/// pending queue where slow-client coalescing can still replace them, and a
/// pull never moves an unbounded batch into connection-local state.
const PULL_BATCH: usize = 16;

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

/// Five minutes separates a real thinking pause from a session that deserves
/// a liveness warning. A shorter threshold would turn normal terminal pauses
/// into noise and make the signal less trustworthy.
pub const SESSION_SILENCE_THRESHOLD: Duration = Duration::from_secs(300);

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

#[derive(Clone, Debug, PartialEq, Eq)]
struct SequencedChunk {
    seq: u64,
    data: Vec<u8>,
}

/// Transcript replay buffer for a recovered session.
///
/// A recovered session loads its journal records here once at hydration and
/// serves cursor-based replays from the union of these chunks and a fresh
/// journal read. It is NOT the live screen mechanism: a live attach receives
/// a screen snapshot, never this buffer.
#[derive(Debug, Default)]
struct Scrollback {
    chunks: VecDeque<SequencedChunk>,
}

impl Scrollback {
    fn push(&mut self, seq: u64, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.chunks.push_back(SequencedChunk {
            seq,
            data: data.to_vec(),
        });
    }

    fn needs_journal_replay(&self, from_cursor: Option<u64>, next_seq: u64) -> bool {
        let cursor = from_cursor.unwrap_or(0);
        self.chunks
            .front()
            .map(|chunk| chunk.seq > cursor.saturating_add(1))
            .unwrap_or(next_seq > cursor.saturating_add(1))
    }

    #[cfg(test)]
    fn replay_after(&self, from_cursor: Option<u64>) -> Vec<SessionEvent> {
        self.replay_after_with_journal(from_cursor, &[])
    }

    fn replay_after_with_journal(
        &self,
        from_cursor: Option<u64>,
        journal_outputs: &[(u64, String)],
    ) -> Vec<SessionEvent> {
        let cursor = from_cursor.unwrap_or(0);
        let mut outputs = BTreeMap::<u64, String>::new();
        for chunk in &self.chunks {
            if chunk.seq > cursor {
                outputs.insert(chunk.seq, String::from_utf8_lossy(&chunk.data).into_owned());
            }
        }
        // Prefer the journal copy for a sequence present in both sources. It
        // is the durable copy and makes the seam a set union, never two
        // envelopes for one sequence.
        for (seq, data) in journal_outputs {
            if *seq > cursor {
                outputs.insert(*seq, data.clone());
            }
        }
        outputs
            .into_iter()
            .map(|(seq, data)| SessionEvent::Output { seq, data })
            .collect()
    }
}

struct Attachment {
    conn_id: u64,
    outbound: Arc<ConnOut>,
}

/// One item queued for the attached viewer, in wire order.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingItem {
    /// Screen state at `as_of_seq`. Always the first item of an attachment;
    /// also the replacement emitted when a slow viewer's queue exceeds the
    /// budget. The owned grid is rendered to ANSI outside every lock.
    Snapshot {
        as_of_seq: u64,
        screen: ScreenSnapshot,
    },
    /// An applied output chunk, forwarded verbatim.
    Output { seq: u64, data: String },
}

#[derive(Clone, Copy, Debug)]
enum Disposition {
    Running,
    Silent,
    Exited,
    Recovered { truncated: bool },
}

struct StreamState {
    /// Session-wide monotonic output counter. Labels journal records and
    /// live events; it is NOT a replay cursor and never advances because a
    /// frame was written to a pipe.
    next_seq: u64,
    /// Greatest sequence whose complete chunk has been applied to the
    /// emulator. This is the snapshot boundary (`as_of_seq`).
    last_applied_seq: u64,
    generation: u64,
    /// The headless emulator. `None` for a recovered transcript, which has
    /// no live process and serves cursor-based journal replays instead.
    screen: Option<Screen>,
    /// The single attached viewer, if any.
    attached: Option<Attachment>,
    /// Unsent items for the attachment, in wire order. Bounded: when the
    /// Output extent exceeds the budget, the whole queue is replaced by one
    /// fresh snapshot.
    pending: VecDeque<PendingItem>,
    /// Byte extent of `pending`'s Output items (snapshots are not counted;
    /// a replacement resets this to zero).
    pending_bytes: usize,
    /// Frame count of `pending`'s Output items.
    pending_frames: u64,
    /// Transcript replay buffer. Unused by live sessions, which never
    /// replay bytes to synchronise a screen.
    scrollback: Scrollback,
    /// Reader has seen EOF. Further publish_output is dropped.
    output_closed: bool,
    /// Child::wait returned. Output may still be in the ConPTY buffer.
    process_exited: bool,
    exit_code: Option<u32>,
    last_publish: Option<Instant>,
    exit_at: Option<Instant>,
    pending_silences: VecDeque<u64>,
    disposition: Disposition,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OutputMetrics {
    /// Peak byte extent of one session's unsent attachment queue.
    pub(crate) peak_pending_bytes: u64,
    /// Bytes discarded when a slow viewer's queue was replaced by a fresh
    /// snapshot. Not lost screen state — subsumed by the snapshot — but
    /// useful as pressure telemetry.
    pub(crate) coalesced_bytes: u64,
    /// Frames discarded by the same replacement.
    pub(crate) coalesced_frames: u64,
}

/// Stream state is one mutex on purpose. Every step that must be atomic
/// with respect to output application happens under this single hold:
/// apply-to-emulator + boundary update + per-attachment enqueue, and screen
/// capture + attachment registration. Holding it across attach registration
/// makes attach ordering exact: the subscriber is registered with its
/// snapshot already captured, and only then can the reader publish the next
/// live chunk. Subscribe-with-state, subscribe-before-live.
struct SessionRuntime {
    session_id: String,
    journal: Option<Arc<Journal>>,
    stream: Mutex<StreamState>,
    /// The PTY input side, for emulator-generated replies (DSR/CPR). Writes
    /// here are the fast path: never behind the journal, a snapshot, or a
    /// client. `None` for transcript sessions and before spawn finishes.
    pty_writer: OnceLock<Arc<Mutex<Box<dyn Write + Send>>>>,
    /// A failed journal write is a fact about this session, not about the
    /// daemon or a later session. It remains true for the session lifetime.
    journal_degraded: AtomicBool,
    /// A poisoned stream lock or terminal parser panic means the screen can
    /// no longer be trusted. Such a session is dead, not a session to recover
    /// by continuing with possibly corrupted state.
    terminal_dead: AtomicBool,
    /// Kept outside `stream` so a poisoned stream can still wake its viewer
    /// and deliver the degraded + exit terminal markers.
    attachment_notify: Mutex<Option<Arc<ConnOut>>>,
    /// Output that resumes a silent stream, exit, and teardown wake the
    /// deadline sleeper without turning it back into a one-second poller.
    liveness_wake: Arc<(Mutex<bool>, Condvar)>,
    /// The last generation is also needed if the stream lock is poisoned
    /// before the EOF path can read its generation.
    generation: AtomicU64,
    peak_pending_bytes: AtomicUsize,
    coalesced_bytes: AtomicU64,
    coalesced_frames: AtomicU64,
    journal_replays: AtomicU64,
    reader_finished: AtomicBool,
    child_reaped: AtomicBool,
    published_frames: AtomicU64,
    published_bytes: AtomicUsize,
}

impl SessionRuntime {
    #[cfg(test)]
    fn new() -> Self {
        Self::with_journal(String::new(), None)
    }

    fn with_journal(session_id: String, journal: Option<Arc<Journal>>) -> Self {
        Self {
            session_id,
            journal,
            stream: Mutex::new(StreamState {
                next_seq: 1,
                last_applied_seq: 0,
                generation: 1,
                screen: Some(Screen::new(INITIAL_COLS, INITIAL_ROWS)),
                attached: None,
                pending: VecDeque::new(),
                pending_bytes: 0,
                pending_frames: 0,
                scrollback: Scrollback::default(),
                output_closed: false,
                process_exited: false,
                exit_code: None,
                // A session with no first output yet is still observable as
                // alive; its age starts when the runtime is created.
                last_publish: Some(Instant::now()),
                exit_at: None,
                pending_silences: VecDeque::new(),
                disposition: Disposition::Running,
            }),
            pty_writer: OnceLock::new(),
            journal_degraded: AtomicBool::new(false),
            terminal_dead: AtomicBool::new(false),
            attachment_notify: Mutex::new(None),
            liveness_wake: Arc::new((Mutex::new(false), Condvar::new())),
            generation: AtomicU64::new(1),
            peak_pending_bytes: AtomicUsize::new(0),
            coalesced_bytes: AtomicU64::new(0),
            coalesced_frames: AtomicU64::new(0),
            journal_replays: AtomicU64::new(0),
            reader_finished: AtomicBool::new(false),
            child_reaped: AtomicBool::new(false),
            published_frames: AtomicU64::new(0),
            published_bytes: AtomicUsize::new(0),
        }
    }

    fn from_replay(session_id: String, journal: Option<Arc<Journal>>, replay: Replay) -> Arc<Self> {
        let runtime = Arc::new(Self::with_journal(session_id, journal));
        let mut stream = runtime
            .stream
            .lock()
            .expect("new transcript runtime stream lock");
        runtime
            .generation
            .store(replay.generation, Ordering::Release);
        stream.generation = replay.generation;
        stream.next_seq = replay.last_seq.saturating_add(1);
        stream.last_applied_seq = replay.last_seq;
        stream.output_closed = true;
        stream.process_exited = true;
        stream.last_publish = None;
        stream.exit_at = None;
        stream.pending_silences.clear();
        stream.disposition = Disposition::Recovered {
            truncated: replay.truncated,
        };
        // A recovered session is a transcript, not a live process: no
        // emulator, no snapshot, no live queue. Cursor-based journal
        // replay below serves its attaches.
        stream.screen = None;
        for event in replay.events {
            match event {
                SessionEvent::Output { seq, data } => {
                    stream.scrollback.push(seq, data.as_bytes());
                }
                SessionEvent::Exit { code } => {
                    stream.exit_code = code;
                    stream.disposition = Disposition::Exited;
                }
                SessionEvent::Recovered { truncated } => {
                    stream.disposition = Disposition::Recovered { truncated };
                }
                SessionEvent::Silent { .. } => {}
                SessionEvent::JournalDegraded => {
                    runtime.journal_degraded.store(true, Ordering::Release);
                }
                // Snapshots are screen state, never journal records; a
                // recovered session replays transcript events only.
                SessionEvent::Snapshot { .. } => {}
            }
        }
        drop(stream);
        runtime
    }

    /// True when this runtime is a recovered transcript (no emulator, no
    /// live process). Attaches to it replay the journal instead of
    /// synchronising a screen.
    fn is_transcript(&self) -> bool {
        self.lock_stream()
            .map(|stream| stream.screen.is_none())
            .unwrap_or(false)
    }

    fn lock_stream(&self) -> Result<MutexGuard<'_, StreamState>, ()> {
        match self.stream.lock() {
            Ok(stream) => Ok(stream),
            Err(_) => {
                self.mark_terminal_dead("session stream lock poisoned");
                Err(())
            }
        }
    }

    fn mark_terminal_dead(&self, reason: &str) {
        if !self.terminal_dead.swap(true, Ordering::AcqRel) {
            eprintln!("session {} marked dead: {reason}", self.session_id);
        }
        self.mark_journal_degraded();
        self.notify_attachment();
    }

    fn set_attachment_notify(&self, outbound: Option<Arc<ConnOut>>) {
        match self.attachment_notify.lock() {
            Ok(mut current) => *current = outbound,
            Err(_) => eprintln!(
                "session {} could not update attachment notification: lock poisoned",
                self.session_id
            ),
        }
    }

    fn notify_attachment(&self) {
        match self.attachment_notify.lock() {
            Ok(current) => {
                if let Some(outbound) = &*current {
                    outbound.notify();
                }
            }
            Err(_) => eprintln!(
                "session {} could not notify its attachment: lock poisoned",
                self.session_id
            ),
        }
    }

    fn notify_liveness(&self) {
        let (wake_lock, wake) = &*self.liveness_wake;
        if let Ok(mut notified) = wake_lock.lock() {
            *notified = true;
            wake.notify_one();
        }
    }

    /// Return `None` when the monitor should stop, `Some(None)` when it should
    /// wait for output or exit, and `Some(Some(deadline))` for a timed wait.
    fn next_liveness_deadline(&self) -> Option<Option<Instant>> {
        let stream = self.lock_stream().ok()?;
        if stream.process_exited || stream.output_closed {
            return None;
        }
        if !matches!(stream.disposition, Disposition::Running) {
            return Some(None);
        }
        let Some(last_publish) = stream.last_publish else {
            return Some(None);
        };
        Some(last_publish.checked_add(SESSION_SILENCE_THRESHOLD))
    }

    fn wait_for_liveness(&self, deadline: Option<Instant>) {
        let (wake_lock, wake) = &*self.liveness_wake;
        let Ok(mut notified) = wake_lock.lock() else {
            return;
        };
        if *notified {
            *notified = false;
            return;
        }
        match deadline {
            Some(deadline) => {
                let wait = deadline.saturating_duration_since(Instant::now());
                if wait.is_zero() {
                    drop(notified);
                    // `mark_silent_if_due` intentionally uses a strict
                    // greater-than threshold; avoid a hot loop at the exact
                    // deadline while preserving that boundary.
                    std::thread::sleep(Duration::from_millis(1));
                    return;
                }
                if let Ok((mut notified, _)) = wake.wait_timeout(notified, wait) {
                    *notified = false;
                }
            }
            None => {
                if let Ok(mut notified) = wake.wait(notified) {
                    *notified = false;
                }
            }
        }
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn publish_output(&self, data: &str) {
        let pty_replies;
        let seq;
        let generation;
        let was_silent;
        {
            let Ok(mut stream) = self.lock_stream() else {
                return;
            };
            if stream.output_closed {
                // Bytes after EOF are neither applied nor journalled; no
                // sequence is consumed for them.
                if let Some(attached) = &stream.attached {
                    attached.outbound.notify();
                }
                eprintln!(
                    "session {} dropped terminal output after EOF ({} bytes)",
                    self.session_id,
                    data.len()
                );
                return;
            }
            // ONE critical section: allocate the sequence, apply the complete
            // chunk to the emulator, then — only after parsing completed —
            // advance the boundary and enqueue the live update. Releasing the
            // lock anywhere before the boundary update would let an attach
            // capture a snapshot that claims a chunk that was only queued.
            seq = stream.next_seq;
            stream.next_seq = stream.next_seq.saturating_add(1);
            pty_replies = match stream.screen.as_mut() {
                Some(screen) => {
                    match catch_unwind(AssertUnwindSafe(|| screen.feed(data.as_bytes()))) {
                        Ok(replies) => replies,
                        Err(_) => {
                            drop(stream);
                            self.mark_terminal_dead("terminal parser panicked");
                            return;
                        }
                    }
                }
                // Transcript runtimes have no reader thread; unreachable, but
                // the boundary must still stay honest if it ever happened.
                None => Vec::new(),
            };
            stream.last_applied_seq = seq;
            stream.last_publish = Some(Instant::now());
            was_silent = matches!(stream.disposition, Disposition::Silent);
            // Output is an observed sign of life while the process is still
            // running. Bytes drained after Child::wait are not a revival and
            // must not turn an observed exit back into Live.
            if !stream.process_exited {
                stream.disposition = Disposition::Running;
            }
            generation = stream.generation;
            let (coalesced_bytes, coalesced_frames) = enqueue_output(&mut stream, seq, data);
            self.peak_pending_bytes
                .fetch_max(stream.pending_bytes, Ordering::Relaxed);
            self.coalesced_bytes
                .fetch_add(coalesced_bytes, Ordering::Relaxed);
            self.coalesced_frames
                .fetch_add(coalesced_frames, Ordering::Relaxed);
            if let Some(attached) = &stream.attached {
                attached.outbound.notify();
            }
        }
        if was_silent {
            self.notify_liveness();
        }
        // Terminal query replies (DSR/CPR) go straight back to the PTY:
        // ConPTY stalls its render pipeline until they are answered, so they
        // must not wait for the journal, snapshot encoding, or a client.
        self.write_pty_replies(&pty_replies);
        self.published_frames.fetch_add(1, Ordering::Relaxed);
        self.published_bytes
            .fetch_add(data.len(), Ordering::Relaxed);
        // The journal append is asynchronous exactly as before: its failure
        // degrades the transcript, never the screen boundary.
        if let Some(journal) = &self.journal {
            let accepted = journal.try_append(output_record(
                self.session_id.clone(),
                generation,
                seq,
                data.as_bytes(),
            ));
            if !accepted || journal.is_session_degraded(&self.session_id) {
                self.mark_journal_degraded();
            }
        }
    }

    /// Forward emulator-generated replies to the PTY input side. Best
    /// effort: a dead PTY has a dead reader that ends the session anyway.
    fn write_pty_replies(&self, replies: &[String]) {
        if replies.is_empty() {
            return;
        }
        let Some(writer) = self.pty_writer.get() else {
            eprintln!(
                "session {} dropped {} terminal query replies: PTY writer unavailable",
                self.session_id,
                replies.len()
            );
            return;
        };
        let Ok(mut writer) = writer.lock() else {
            self.mark_terminal_dead("PTY writer lock poisoned");
            return;
        };
        for reply in replies {
            if let Err(error) = writer.write_all(reply.as_bytes()) {
                eprintln!(
                    "session {} could not answer a terminal query: {error}",
                    self.session_id
                );
                return;
            }
        }
        if let Err(error) = writer.flush() {
            eprintln!(
                "session {} could not flush a terminal query reply: {error}",
                self.session_id
            );
        }
    }

    fn record_output_loss(&self) {
        let Ok(stream) = self.lock_stream() else {
            return;
        };
        // Bytes lost between the reader and the emulator were never applied
        // to the screen and never journalled, so no sequence is consumed:
        // seq counts applied chunks, and the boundary stays an honest
        // statement about the emulator. The lost bytes are simply absent
        // from the transcript.
        if let Some(attached) = &stream.attached {
            attached.outbound.notify();
        }
    }

    pub(crate) fn output_metrics(&self) -> OutputMetrics {
        OutputMetrics {
            peak_pending_bytes: self.peak_pending_bytes.load(Ordering::Relaxed) as u64,
            coalesced_bytes: self.coalesced_bytes.load(Ordering::Relaxed),
            coalesced_frames: self.coalesced_frames.load(Ordering::Relaxed),
        }
    }

    fn mark_journal_degraded(&self) {
        if !self.journal_degraded.swap(true, Ordering::AcqRel) {
            if let Some(journal) = &self.journal {
                journal.note_session_degraded(&self.session_id);
            }
        }
        self.notify_attachment();
    }

    /// Mark the running stream silent at an injected observation time. The
    /// monitor uses `Instant::now`; the parameter keeps the transition
    /// boundary deterministic in unit tests.
    fn mark_silent_if_due(&self, now: Instant) -> Option<u64> {
        let elapsed_ms;
        {
            let Ok(mut stream) = self.lock_stream() else {
                return None;
            };
            if stream.process_exited || !matches!(stream.disposition, Disposition::Running) {
                return None;
            }
            let Some(last_publish) = stream.last_publish else {
                return None;
            };
            let elapsed = now.saturating_duration_since(last_publish);
            if elapsed <= SESSION_SILENCE_THRESHOLD {
                return None;
            }
            elapsed_ms = elapsed.as_millis().try_into().unwrap_or(u64::MAX);
            stream.disposition = Disposition::Silent;
            stream.pending_silences.push_back(elapsed_ms);
        }
        self.notify_attachment();
        Some(elapsed_ms)
    }

    fn liveness_monitor_should_stop(&self) -> bool {
        self.lock_stream()
            .map(|stream| stream.process_exited || stream.output_closed)
            .unwrap_or(true)
    }

    fn refresh_journal_degradation(&self) {
        if self
            .journal
            .as_ref()
            .is_some_and(|journal| journal.is_session_degraded(&self.session_id))
        {
            self.mark_journal_degraded();
        }
    }

    fn journal_degraded(&self) -> bool {
        self.journal_degraded.load(Ordering::Acquire)
    }

    fn replay_journal_outputs(&self, from_seq: u64, generation: u64) -> Vec<(u64, String)> {
        let Some(journal) = &self.journal else {
            return Vec::new();
        };
        let replay = match journal.replay(&self.session_id, from_seq) {
            Ok(replay) => replay,
            Err(error) => {
                self.mark_journal_degraded();
                eprintln!(
                    "journal replay failed for live session {} from seq {}: {error}",
                    self.session_id, from_seq
                );
                return Vec::new();
            }
        };
        if replay.generation != generation {
            self.mark_journal_degraded();
            eprintln!(
                "journal replay generation mismatch for live session {}: journal={} live={}",
                self.session_id, replay.generation, generation
            );
            return Vec::new();
        }
        if replay.truncated {
            self.mark_journal_degraded();
        }
        self.journal_replays.fetch_add(1, Ordering::Relaxed);
        replay
            .events
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::Output { seq, data } => Some((seq, data)),
                SessionEvent::Exit { .. }
                | SessionEvent::Recovered { .. }
                | SessionEvent::Silent { .. }
                | SessionEvent::JournalDegraded
                // Snapshots are not output chunks and are not sourced from
                // the historical journal replay path.
                | SessionEvent::Snapshot { .. } => None,
            })
            .collect()
    }

    #[cfg(test)]
    fn journal_replay_count(&self) -> u64 {
        self.journal_replays.load(Ordering::Relaxed)
    }

    /// Register this connection as the single attached viewer and synchronise
    /// it with the current screen state.
    ///
    /// A second different connection is rejected. The same connection
    /// re-attaching replaces its stream with a fresh snapshot.
    ///
    /// THE LOAD-BEARING SECTION: screen capture and attachment registration
    /// happen under ONE hold of the state lock. Building the copy first and
    /// registering afterwards would let output fall into the interval and
    /// land in neither the snapshot nor the queue — the exact bug class M3
    /// shipped. After this function returns, the attachment's first outbound
    /// item is `Snapshot(as_of_seq)` and every subsequent publish enqueues an
    /// Output with a strictly greater sequence.
    fn try_attach(&self, from_cursor: Option<Cursor>, conn: &ConnHandle) -> Result<u64, WireError> {
        if self.terminal_dead.load(Ordering::Acquire) {
            return Err(process_gone());
        }
        let Ok(mut stream) = self.lock_stream() else {
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
        // A live attach starts at the current screen snapshot: the cursor's
        // sequence is ignored, but its generation must still match so a
        // client holding a cursor from a recreated process gets a loud error
        // instead of a silently different stream.
        if let Some(cursor) = from_cursor {
            cursor_replay_ok(stream.generation, cursor)?;
        }
        let as_of_seq = stream.last_applied_seq;
        let screen = stream.screen.as_ref().map(Screen::snapshot);
        stream.attached = Some(Attachment {
            conn_id: conn.id,
            outbound: Arc::clone(&conn.outbound),
        });
        self.set_attachment_notify(Some(Arc::clone(&conn.outbound)));
        stream.pending.clear();
        stream.pending_bytes = 0;
        stream.pending_frames = 0;
        stream.pending_silences.clear();
        if let Some(screen) = screen {
            stream
                .pending
                .push_back(PendingItem::Snapshot { as_of_seq, screen });
        }
        Ok(stream.generation)
    }

    fn detach_if_conn(&self, conn_id: u64) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        if stream
            .attached
            .as_ref()
            .is_some_and(|attached| attached.conn_id == conn_id)
        {
            stream.attached = None;
            self.set_attachment_notify(None);
            // The unsent queue belonged to the departing viewer. A new
            // attach must start from a fresh snapshot, not from a stale
            // stream assembled for a client that is gone.
            stream.pending.clear();
            stream.pending_bytes = 0;
            stream.pending_frames = 0;
            stream.pending_silences.clear();
        }
    }

    fn mark_exited(&self, code: Option<u32>) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        if stream.process_exited {
            return;
        }
        stream.process_exited = true;
        stream.exit_code = code;
        stream.exit_at = Some(Instant::now());
        stream.disposition = Disposition::Exited;
        stream.pending_silences.clear();
        if let Some(attached) = &stream.attached {
            attached.outbound.notify();
        }
        drop(stream);
        self.notify_liveness();
        // Child::wait returns before ConPTY EOFs (ARCHITETTURA §1.7). Record
        // that the process was observed, but do not freeze last_seq: drain
        // frames still need seqs. Ended (exit row) is written at EOF.
        if let Some(journal) = &self.journal {
            if let Err(error) = journal.mark_reaped(&self.session_id, code) {
                self.mark_journal_degraded();
                eprintln!(
                    "journal could not record process exit for {}: {error}",
                    self.session_id
                );
            }
        }
    }

    fn close_output(&self) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        stream.output_closed = true;
        if let Some(attached) = &stream.attached {
            attached.outbound.notify();
        }
        drop(stream);
        self.notify_liveness();
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
        let Ok(mut stream) = self.lock_stream() else {
            return 1;
        };
        stream.generation = stream.generation.saturating_add(1);
        self.generation.store(stream.generation, Ordering::Release);
        stream.next_seq = 1;
        stream.last_applied_seq = 0;
        // A new generation is a new process: a fresh emulator, not the old
        // grid carrying over.
        stream.screen = Some(Screen::new(INITIAL_COLS, INITIAL_ROWS));
        stream.pending.clear();
        stream.pending_bytes = 0;
        stream.pending_frames = 0;
        stream.output_closed = false;
        stream.process_exited = false;
        stream.exit_code = None;
        stream.last_publish = None;
        stream.exit_at = None;
        stream.pending_silences.clear();
        stream.disposition = Disposition::Running;
        stream.generation
    }

    #[cfg(test)]
    fn transcript_chunks(&self) -> Vec<(u64, String)> {
        let stream = self.stream.lock().unwrap();
        stream
            .scrollback
            .chunks
            .iter()
            .map(|chunk| (chunk.seq, String::from_utf8_lossy(&chunk.data).into_owned()))
            .collect()
    }

    #[cfg(test)]
    fn last_applied_seq(&self) -> u64 {
        self.stream.lock().unwrap().last_applied_seq
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

/// Enqueue one applied chunk for the attached viewer and enforce the
/// slow-viewer budget. Called with the state lock held, after the emulator
/// boundary advanced.
///
/// When the unsent Output extent exceeds the budget, the WHOLE unsent queue
/// is discarded and replaced by a fresh snapshot at the current boundary.
/// Pipe order still delivers the newer snapshot after anything older that
/// already reached the wire, and the snapshot subsumes everything before it.
fn enqueue_output(stream: &mut StreamState, seq: u64, data: &str) -> (u64, u64) {
    if stream.attached.is_none() {
        return (0, 0);
    }
    stream.pending.push_back(PendingItem::Output {
        seq,
        data: data.to_owned(),
    });
    stream.pending_bytes += data.len();
    stream.pending_frames += 1;
    if stream.pending_bytes <= PENDING_OUTPUT_BUDGET_BYTES
        && stream.pending_frames <= PENDING_OUTPUT_BUDGET_FRAMES
    {
        return (0, 0);
    }
    let as_of_seq = stream.last_applied_seq;
    let screen = stream.screen.as_ref().map(Screen::snapshot);
    let discarded_bytes = stream.pending_bytes as u64;
    let discarded_frames = stream.pending_frames;
    stream.pending.clear();
    stream.pending_bytes = 0;
    stream.pending_frames = 0;
    if let Some(screen) = screen {
        stream
            .pending
            .push_back(PendingItem::Snapshot { as_of_seq, screen });
    }
    (discarded_bytes, discarded_frames)
}

/// Render an owned captured screen into the wire snapshot event. Called with
/// no locks held: the ANSI presenter is O(rows x cols) and must never run
/// inside the state mutex.
fn snapshot_event(as_of_seq: u64, screen: ScreenSnapshot) -> SessionEvent {
    SessionEvent::Snapshot {
        as_of_seq,
        cols: screen.cols,
        rows: screen.rows,
        data: screen.render_ansi(),
        cursor: ScreenCursor {
            row: screen.cursor.row,
            col: screen.cursor.col,
            visible: screen.cursor.visible,
            blinking: screen.cursor.blinking,
            shape: match screen.cursor.shape {
                SnapshotCursorShape::Block => CursorShape::Block,
                SnapshotCursorShape::Underline => CursorShape::Underline,
                SnapshotCursorShape::Bar => CursorShape::Bar,
            },
        },
        alternate_screen: screen.alternate_screen,
        bracketed_paste: screen.bracketed_paste,
        line_wrap: screen.line_wrap,
        title: screen.title,
    }
}

/// Per-connection handle: RPC outbound plus the sessions this client pulls.
pub struct ConnHandle {
    pub id: u64,
    pub outbound: Arc<ConnOut>,
    attached: Mutex<HashMap<String, PullState>>,
    next_attachment_generation: AtomicU64,
}

struct PullState {
    runtime: Arc<SessionRuntime>,
    /// Whether this pull follows the transcript replay contract (recovered
    /// session) or the live snapshot contract.
    transcript: bool,
    /// Transcript-only: last replay sequence the client accounted for.
    /// Live sessions keep no replay cursor; their screen boundary is the
    /// snapshot's `as_of_seq`.
    transcript_cursor: Option<u64>,
    exit_sent: bool,
    journal_degraded_sent: bool,
    generation: u64,
    attachment_generation: u64,
}

#[derive(Debug)]
pub(crate) struct PendingEvent {
    pub(crate) session_id: String,
    pub(crate) attachment_generation: u64,
    pub(crate) envelope: SessionEventEnvelope,
}

impl ConnHandle {
    pub fn new(id: u64) -> Arc<Self> {
        Arc::new(Self {
            id,
            outbound: ConnOut::new(),
            attached: Mutex::new(HashMap::new()),
            next_attachment_generation: AtomicU64::new(1),
        })
    }

    fn track(
        &self,
        session_id: &str,
        runtime: Arc<SessionRuntime>,
        transcript: bool,
        transcript_cursor: Option<u64>,
        generation: u64,
    ) {
        let attachment_generation = self
            .next_attachment_generation
            .fetch_add(1, Ordering::Relaxed);
        let mut map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        map.insert(
            session_id.to_string(),
            PullState {
                runtime,
                transcript,
                transcript_cursor,
                exit_sent: false,
                journal_degraded_sent: false,
                generation,
                attachment_generation,
            },
        );
        self.outbound.notify();
    }

    fn untrack(&self, session_id: &str) {
        self.attached
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(session_id);
    }

    /// Return the next one-shot wake needed to emit an exit after its drain
    /// window. Ordinary live sessions return `None`, so the connection writer
    /// remains asleep until a request or PTY notification arrives.
    pub fn next_exit_wake(&self) -> Option<Duration> {
        let map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        map.values()
            .filter_map(|pull| {
                if pull.exit_sent {
                    return None;
                }
                if pull.runtime.terminal_dead.load(Ordering::Acquire) {
                    return Some(Duration::ZERO);
                }
                let Ok(stream) = pull.runtime.lock_stream() else {
                    return Some(Duration::ZERO);
                };
                if SessionRuntime::ready_for_exit(&stream) {
                    // Drain elapsed (or EOF) since the last pull. There is no
                    // notify at that instant; a zero timeout makes the writer
                    // loop instead of waiting forever.
                    return Some(Duration::ZERO);
                }
                if !stream.process_exited {
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
            registry.drop_transcript_if_idle(&id);
        }
    }

    /// Pull replay + live output + exit for every session this connection
    /// is attached to. Called from the writer thread; does not send.
    pub(crate) fn event_is_current(&self, session_id: &str, attachment_generation: u64) -> bool {
        self.attached
            .lock()
            .map(|map| map.get(session_id).map(|pull| pull.attachment_generation))
            .unwrap_or_else(|error| {
                let map = error.into_inner();
                map.get(session_id).map(|pull| pull.attachment_generation)
            })
            == Some(attachment_generation)
    }

    /// Record delivery only after the corresponding envelope was written to
    /// the connection. Only the transcript replay cursor advances here: live
    /// screen state is synchronised by snapshots, so an Output written to a
    /// live stream must not look like a replay position.
    pub(crate) fn event_sent(&self, event: &PendingEvent) {
        let mut map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let remove = {
            let Some(pull) = map.get_mut(&event.session_id) else {
                return;
            };
            if pull.attachment_generation != event.attachment_generation {
                return;
            }
            match &event.envelope.event {
                SessionEvent::Output { seq, .. } => {
                    if let Some(cursor) = pull.transcript_cursor.as_mut() {
                        *cursor = *seq;
                    }
                    false
                }
                SessionEvent::Exit { .. } | SessionEvent::Recovered { .. } => true,
                SessionEvent::Silent { .. }
                | SessionEvent::JournalDegraded
                | SessionEvent::Snapshot { .. } => false,
            }
        };
        if remove {
            map.remove(&event.session_id);
        }
    }

    pub(crate) fn pull_events(&self) -> Vec<PendingEvent> {
        let mut map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut events = Vec::new();
        for (session_id, pull) in map.iter_mut() {
            if pull.transcript {
                pull_transcript_events(session_id, pull, &mut events);
            } else {
                pull_live_events(session_id, pull, &mut events);
            }
        }
        events
    }
}

/// Live pull: drain a bounded batch from the attachment's pending queue and
/// convert it to wire events. Snapshot ANSI is rendered HERE, with no locks
/// held — never inside the state mutex. Only when the queue is fully empty
/// may JournalDegraded or the exit event be appended, so neither can
/// overtake output that is still queued.
fn pull_live_events(session_id: &str, pull: &mut PullState, events: &mut Vec<PendingEvent>) {
    if pull.runtime.terminal_dead.load(Ordering::Acquire) {
        push_dead_events(session_id, pull, events);
        return;
    }
    let mut drained = Vec::with_capacity(PULL_BATCH);
    let degraded;
    let silent_event;
    let mut exit_event = None;
    {
        let Ok(mut stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        while drained.len() < PULL_BATCH {
            let Some(item) = stream.pending.pop_front() else {
                break;
            };
            match &item {
                PendingItem::Output { data, .. } => {
                    stream.pending_bytes = stream.pending_bytes.saturating_sub(data.len());
                    stream.pending_frames = stream.pending_frames.saturating_sub(1);
                }
                PendingItem::Snapshot { .. } => {}
            }
            drained.push(item);
        }
        degraded = !pull.journal_degraded_sent && pull.runtime.journal_degraded();
        if degraded {
            pull.journal_degraded_sent = true;
        }
        silent_event = stream
            .pending_silences
            .pop_front()
            .map(|elapsed_ms| SessionEvent::Silent { elapsed_ms });
        if !pull.exit_sent && stream.pending.is_empty() && SessionRuntime::ready_for_exit(&stream) {
            exit_event = Some(match stream.disposition {
                Disposition::Recovered { truncated } => SessionEvent::Recovered { truncated },
                Disposition::Running | Disposition::Silent | Disposition::Exited => {
                    SessionEvent::Exit {
                        code: stream.exit_code,
                    }
                }
            });
            pull.exit_sent = true;
        }
    }
    for item in drained {
        let event = match item {
            PendingItem::Snapshot { as_of_seq, screen } => snapshot_event(as_of_seq, screen),
            PendingItem::Output { seq, data } => SessionEvent::Output { seq, data },
        };
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event,
            },
        });
    }
    if degraded {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event: SessionEvent::JournalDegraded,
            },
        });
    }
    if let Some(event) = silent_event {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event,
            },
        });
    }
    if let Some(event) = exit_event {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event,
            },
        });
    }
}

fn push_dead_events(session_id: &str, pull: &mut PullState, events: &mut Vec<PendingEvent>) {
    if !pull.journal_degraded_sent {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event: SessionEvent::JournalDegraded,
            },
        });
        pull.journal_degraded_sent = true;
    }
    if !pull.exit_sent {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event: SessionEvent::Exit { code: None },
            },
        });
        pull.exit_sent = true;
    }
}

/// Transcript pull: cursor-based journal/scrollback replay for a recovered
/// session. This is the M2/M3 replay contract, kept for transcripts only.
fn pull_transcript_events(session_id: &str, pull: &mut PullState, events: &mut Vec<PendingEvent>) {
    if pull.runtime.terminal_dead.load(Ordering::Acquire) {
        push_dead_events(session_id, pull, events);
        return;
    }
    let cursor = pull.transcript_cursor;
    let needs_journal = {
        let Ok(stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        stream
            .scrollback
            .needs_journal_replay(cursor, stream.next_seq)
    };
    let journal_outputs = if needs_journal {
        pull.runtime
            .replay_journal_outputs(cursor.unwrap_or(0), pull.generation)
    } else {
        Vec::new()
    };
    let replay = {
        let Ok(stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        stream
            .scrollback
            .replay_after_with_journal(cursor, &journal_outputs)
    };
    for event in replay {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event,
            },
        });
    }
    if !pull.journal_degraded_sent && pull.runtime.journal_degraded() {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event: SessionEvent::JournalDegraded,
            },
        });
        pull.journal_degraded_sent = true;
    }
    if !pull.exit_sent {
        let Ok(stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        if SessionRuntime::ready_for_exit(&stream) {
            let event = match stream.disposition {
                Disposition::Recovered { truncated } => SessionEvent::Recovered { truncated },
                Disposition::Running | Disposition::Silent | Disposition::Exited => {
                    SessionEvent::Exit {
                        code: stream.exit_code,
                    }
                }
            };
            events.push(PendingEvent {
                session_id: session_id.to_string(),
                attachment_generation: pull.attachment_generation,
                envelope: SessionEventEnvelope {
                    session_id: session_id.to_string(),
                    generation: pull.generation,
                    event,
                },
            });
            pull.exit_sent = true;
        }
    }
}

/// The registry owns this value; the reader and command paths keep Arcs to
/// the endpoints/runtime they need after releasing the map lock.
struct PtySession {
    metadata: Session,
    owner: OwnerId,
    process_job: JobObject,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child_wait: Option<JoinHandle<Option<u32>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader_handle: Option<JoinHandle<()>>,
    coalesce_handle: Option<JoinHandle<()>>,
    liveness_handle: Option<JoinHandle<()>>,
    runtime: Arc<SessionRuntime>,
    exited: Arc<AtomicBool>,
    /// Set by `stop`: the process dies but the session object stays. The
    /// reader must not remove the registry entry or call session_finished.
    preserve_on_exit: Arc<AtomicBool>,
}

struct TranscriptSession {
    metadata: Session,
    owner: OwnerId,
    runtime: Arc<SessionRuntime>,
}

enum RegistryEntry {
    Live(PtySession),
    Transcript(TranscriptSession),
}

impl RegistryEntry {
    fn owner(&self) -> &OwnerId {
        match self {
            Self::Live(session) => &session.owner,
            Self::Transcript(session) => &session.owner,
        }
    }

    fn runtime(&self) -> Arc<SessionRuntime> {
        match self {
            Self::Live(session) => Arc::clone(&session.runtime),
            Self::Transcript(session) => Arc::clone(&session.runtime),
        }
    }

    fn to_session(&self) -> Session {
        match self {
            Self::Live(session) => live_session_view(session),
            Self::Transcript(session) => session.metadata.clone(),
        }
    }

    fn as_live(&self) -> Option<&PtySession> {
        match self {
            Self::Live(session) => Some(session),
            Self::Transcript(_) => None,
        }
    }

    fn as_live_mut(&mut self) -> Option<&mut PtySession> {
        match self {
            Self::Live(session) => Some(session),
            Self::Transcript(_) => None,
        }
    }
}

fn elapsed_ms_since_last_life(
    last_publish: Option<Instant>,
    exit_at: Option<Instant>,
    process_exited: bool,
    now: Instant,
) -> Option<u64> {
    let origin = if process_exited {
        exit_at
    } else {
        last_publish
    }?;
    Some(
        now.saturating_duration_since(origin)
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    )
}

fn live_session_view(session: &PtySession) -> Session {
    let mut metadata = session.metadata.clone();
    if session.runtime.terminal_dead.load(Ordering::Acquire) {
        metadata.state = SessionState::Ended {
            generation: session.runtime.generation(),
            code: None,
        };
        return metadata;
    }
    let Ok(stream) = session.runtime.lock_stream() else {
        metadata.state = SessionState::Ended {
            generation: session.runtime.generation(),
            code: None,
        };
        return metadata;
    };
    metadata.state = match stream.disposition {
        Disposition::Running => SessionState::Live {
            generation: stream.generation,
        },
        Disposition::Silent => SessionState::Silent {
            generation: stream.generation,
        },
        Disposition::Exited => SessionState::Ended {
            generation: stream.generation,
            code: stream.exit_code,
        },
        Disposition::Recovered { truncated } => SessionState::Recovered {
            generation: stream.generation,
            truncated,
        },
    };
    metadata.elapsed_ms = elapsed_ms_since_last_life(
        stream.last_publish,
        stream.exit_at,
        stream.process_exited,
        Instant::now(),
    );
    metadata
}

fn process_gone() -> WireError {
    WireError::new(ErrorCode::InvalidRequest, "This terminal process is gone.")
}

fn unauthorized() -> WireError {
    WireError::new(
        ErrorCode::Unauthorized,
        "This client is not authorized to use that session.",
    )
}

fn owner_from_session_id(session_id: &str, user: &str) -> Result<OwnerId, WireError> {
    let mut parts = session_id.splitn(3, '.');
    if parts.next() != Some("s") {
        return Err(unauthorized());
    }
    let client = parts.next().ok_or_else(unauthorized)?;
    if parts.next().is_none() {
        return Err(unauthorized());
    }
    OwnerId::new(user, client).map_err(|_| unauthorized())
}

fn check_owner(entry: &RegistryEntry, owner: &OwnerId) -> Result<(), WireError> {
    if entry.owner() == owner {
        Ok(())
    } else {
        Err(unauthorized())
    }
}

#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<Mutex<HashMap<String, RegistryEntry>>>,
    paths: RuntimePaths,
    journal: Option<Arc<Journal>>,
}

impl SessionRegistry {
    pub fn new(paths: RuntimePaths, journal: Option<Arc<Journal>>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            paths,
            journal,
        }
    }

    pub(crate) fn output_metrics(&self) -> OutputMetrics {
        let Ok(map) = self.inner.lock() else {
            return OutputMetrics::default();
        };
        map.values()
            .map(RegistryEntry::runtime)
            .map(|runtime| runtime.output_metrics())
            .fold(OutputMetrics::default(), |mut total, metrics| {
                total.peak_pending_bytes = total.peak_pending_bytes.max(metrics.peak_pending_bytes);
                total.coalesced_bytes = total
                    .coalesced_bytes
                    .saturating_add(metrics.coalesced_bytes);
                total.coalesced_frames = total
                    .coalesced_frames
                    .saturating_add(metrics.coalesced_frames);
                total
            })
    }

    /// Wire view of the journal writer's counters for the Status reply.
    /// `None` when the journal could not be opened: there is no writer
    /// whose behaviour could be counted, and inventing zeros would claim
    /// an integrity nobody observed.
    pub fn journal_stats(&self) -> Option<JournalStats> {
        self.journal.as_ref().map(|journal| {
            let snapshot = journal.stats();
            JournalStats {
                accepted_frames: snapshot.accepted_frames,
                accepted_bytes: snapshot.accepted_bytes,
                committed_frames: snapshot.committed_frames,
                committed_bytes: snapshot.committed_bytes,
                failed_frames: snapshot.failed_frames,
            }
        })
    }

    pub fn flush_journal(&self) {
        if let Some(journal) = &self.journal {
            let _ = journal.flush();
            journal.shutdown();
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
            state: SessionState::Live { generation: 1 },
            elapsed_ms: Some(0),
        };
        let command = match command {
            Some(command) => command,
            None => resolve_pty_command(&self.paths)?,
        };
        // Journal the row BEFORE spawn. A short-lived command (cmd /c echo)
        // can EOF and enqueue MarkEnded before this function would otherwise
        // reach try_upsert, and the journal thread would then see a missing
        // session and leave status=live — recovered-as-killed on reopen.
        if let Some(journal) = &self.journal {
            let mut record = new_session_record(
                metadata.id.clone(),
                owner.user.clone(),
                metadata.workspace_id.clone(),
                metadata.kind.clone(),
                metadata.title.clone(),
            );
            record.status = PersistStatus::Live;
            journal.try_upsert(record);
        }
        spawn_session(state, self, metadata.clone(), owner.clone(), command)?;
        Ok(metadata)
    }

    pub fn attach(
        &self,
        session_id: &str,
        from_cursor: Option<Cursor>,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        let runtime = match self.runtime_for_owner(session_id, owner) {
            Ok(runtime) => runtime,
            Err(error) if error.code == ErrorCode::SessionNotFound => {
                self.hydrate_transcript(session_id, from_cursor, owner)?
            }
            Err(error) => return Err(error),
        };
        let generation = runtime.try_attach(from_cursor, conn)?;
        // A live attach synchronises the screen (snapshot first, live after)
        // and keeps no replay cursor. A transcript attach replays the journal
        // from the cursor. These are different products and must not share a
        // replay state machine.
        let transcript = runtime.is_transcript();
        let transcript_cursor = if transcript {
            Some(from_cursor.map(|cursor| cursor.seq).unwrap_or(0))
        } else {
            None
        };
        conn.track(
            session_id,
            Arc::clone(&runtime),
            transcript,
            transcript_cursor,
            generation,
        );
        // The journal writer records asynchronous failures in shared state;
        // attach must import that fact before returning even when the PTY is
        // otherwise quiet and no status request or later output occurs.
        runtime.refresh_journal_degradation();
        Ok(())
    }

    fn hydrate_transcript(
        &self,
        session_id: &str,
        from_cursor: Option<Cursor>,
        owner: &OwnerId,
    ) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let journal = self.journal.as_ref().ok_or_else(not_found)?;
        let record = journal
            .list()?
            .into_iter()
            .find(|row| row.id == session_id)
            .ok_or_else(not_found)?;
        let session_owner = owner_from_session_id(session_id, &record.owner)?;
        if &session_owner != owner {
            return Err(unauthorized());
        }
        journal.pin(session_id)?;
        let from_seq = from_cursor.map(|cursor| cursor.seq).unwrap_or(0);
        let replay = match journal.replay(session_id, from_seq) {
            Ok(replay) => replay,
            Err(error) => {
                journal.unpin(session_id);
                return Err(error.into());
            }
        };
        if let Some(cursor) = from_cursor {
            if let Err(error) = cursor_replay_ok(replay.generation, cursor) {
                journal.unpin(session_id);
                return Err(error);
            }
        }
        let metadata = record.to_session();
        let runtime =
            SessionRuntime::from_replay(session_id.to_string(), Some(Arc::clone(journal)), replay);
        {
            let Ok(mut map) = self.inner.lock() else {
                journal.unpin(session_id);
                return Err(internal("Session state is unavailable."));
            };
            if let Some(existing) = map.get(session_id) {
                check_owner(existing, owner)?;
                journal.unpin(session_id);
                return Ok(existing.runtime());
            }
            map.insert(
                session_id.to_string(),
                RegistryEntry::Transcript(TranscriptSession {
                    metadata,
                    owner: session_owner,
                    runtime: Arc::clone(&runtime),
                }),
            );
        }
        Ok(runtime)
    }

    pub fn detach(
        &self,
        session_id: &str,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        let runtime = self.runtime_for_owner(session_id, owner)?;
        runtime.detach_if_conn(conn.id);
        conn.untrack(session_id);
        self.drop_transcript_if_idle(session_id);
        Ok(())
    }

    fn drop_transcript_if_idle(&self, session_id: &str) {
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        let is_idle_transcript = map.get(session_id).is_some_and(|entry| {
            matches!(entry, RegistryEntry::Transcript(session) if {
                session
                    .runtime
                    .stream
                    .lock()
                    .map(|stream| stream.attached.is_none())
                    .unwrap_or(true)
            })
        });
        if is_idle_transcript {
            map.remove(session_id);
            if let Some(journal) = &self.journal {
                journal.unpin(session_id);
            }
        }
    }

    fn detach_runtime(&self, session_id: &str, conn_id: u64) {
        if let Ok(runtime) = self.runtime(session_id) {
            runtime.detach_if_conn(conn_id);
        }
    }

    pub fn close(&self, session_id: &str, owner: &OwnerId) -> Result<bool, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let session = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            if let Some(entry) = map.get(session_id) {
                check_owner(entry, owner)?;
            }
            map.remove(session_id)
        };
        match session {
            Some(RegistryEntry::Live(session)) => {
                if let Some(journal) = &self.journal {
                    journal.try_mark_closed(session_id);
                    journal.unpin(session_id);
                }
                teardown_session(session);
                Ok(true)
            }
            Some(RegistryEntry::Transcript(_)) => {
                if let Some(journal) = &self.journal {
                    journal.try_mark_closed(session_id);
                    journal.unpin(session_id);
                }
                Ok(false)
            }
            None => {
                if let Some(journal) = &self.journal {
                    let known = journal.list()?.into_iter().find(|row| row.id == session_id);
                    if let Some(record) = known {
                        let session_owner = owner_from_session_id(session_id, &record.owner)?;
                        if &session_owner != owner {
                            return Err(unauthorized());
                        }
                        journal.try_mark_closed(session_id);
                        return Ok(false);
                    }
                }
                Err(not_found())
            }
        }
    }

    pub fn stop(&self, session_id: &str, owner: &OwnerId) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let mut killer = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let session = map.get_mut(session_id).ok_or_else(not_found)?;
            check_owner(session, owner)?;
            let session = session.as_live_mut().ok_or_else(process_gone)?;
            session.preserve_on_exit.store(true, Ordering::SeqCst);
            session.killer.clone_killer()
        };
        let _ = killer.kill();
        Ok(())
    }

    pub fn send(&self, session_id: &str, text: &str, owner: &OwnerId) -> Result<(), WireError> {
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
            let entry = map.get(session_id).ok_or_else(not_found)?;
            check_owner(entry, owner)?;
            let session = entry.as_live().ok_or_else(process_gone)?;
            Arc::clone(&session.writer)
        };
        let mut writer = writer
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        writer.write_all(text.as_bytes()).map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not send input to the terminal: {error}"),
            )
        })?;
        writer.flush().map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not flush input to the terminal: {error}"),
            )
        })
    }

    pub fn resize(
        &self,
        session_id: &str,
        cols: u16,
        rows: u16,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let (runtime, master) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            check_owner(entry, owner)?;
            let session = entry.as_live().ok_or_else(process_gone)?;
            (Arc::clone(&session.runtime), Arc::clone(&session.master))
        };
        // Resize is serialized with emulator parsing under the SAME state
        // lock as publish_output, in one defined order: emulator dimensions
        // first, then the PTY. A snapshot therefore sees the resize as wholly
        // before or wholly after itself, and no chunk is parsed into a grid
        // that is mid-resize.
        let mut stream = runtime
            .stream
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let Some(screen) = stream.screen.as_mut() else {
            return Err(process_gone());
        };
        let (previous_cols, previous_rows) = screen.dimensions();
        screen.resize(cols.max(1), rows.max(1));
        let master = master
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        if master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .is_err()
        {
            // Keep emulator and PTY in agreement: undo the grid change.
            screen.resize(previous_cols, previous_rows);
            return Err(WireError::new(
                ErrorCode::Io,
                "Could not resize the terminal.",
            ));
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Session>, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let mut sessions: Vec<Session> = map.values().map(RegistryEntry::to_session).collect();
        drop(map);
        if let Some(journal) = &self.journal {
            if let Ok(rows) = journal.list() {
                for row in rows {
                    if sessions.iter().any(|session| session.id == row.id) {
                        continue;
                    }
                    sessions.push(row.to_session());
                }
            }
        }
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(sessions)
    }

    /// Pull asynchronous journal-loop failures into live runtimes so their
    /// attached event channels can report degradation even when the PTY has
    /// gone quiet since the failed write.
    pub fn refresh_journal_degradation(&self) {
        let runtimes = self
            .inner
            .lock()
            .map(|map| map.values().map(RegistryEntry::runtime).collect::<Vec<_>>())
            .unwrap_or_default();
        for runtime in runtimes {
            runtime.refresh_journal_degradation();
        }
    }

    pub fn has_live_journal_degradation(&self) -> bool {
        self.inner
            .lock()
            .map(|map| map.values().any(|entry| entry.runtime().journal_degraded()))
            .unwrap_or(true)
    }

    fn runtime(&self, session_id: &str) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let session = map.get(session_id).ok_or_else(not_found)?;
        Ok(session.runtime())
    }

    fn runtime_for_owner(
        &self,
        session_id: &str,
        owner: &OwnerId,
    ) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let session = map.get(session_id).ok_or_else(not_found)?;
        check_owner(session, owner)?;
        Ok(session.runtime())
    }
}

pub fn spawn_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    command: PtyCommand,
) -> Result<(), WireError> {
    // On Windows portable-pty selects ConPTY internally. ConPTY may issue a
    // DSR query (`ESC[6n`) at startup and stalls its render pipeline until it
    // is answered. The DAEMON is the single responder: publish_output routes
    // the emulator's PtyWrite replies straight back to this writer. Clients
    // must not answer DSR themselves (a second reply would reach the child).
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| pty_wire_error("Could not open the terminal.", error))?;
    let mut child = pair
        .slave
        .spawn_command(command.to_command_builder())
        .map_err(|error| pty_wire_error("Could not start the terminal shell.", error))?;

    // portable-pty 0.9 exposes the native Windows process handle on Child,
    // but does not expose CREATE_SUSPENDED. Assign immediately after spawn so
    // the normal race window is only the interval between CreateProcessW and
    // these calls. Closing it completely would require adapting portable-pty's
    // ConPTY CreateProcessW seam to create suspended and resume after both
    // assignments; that is deliberately not part of this milestone.
    #[cfg(windows)]
    let process_job = {
        let process_job = match JobObject::new() {
            Ok(process_job) => process_job,
            Err(error) => {
                terminate_spawned_child(pair, child);
                return Err(WireError::new(
                    ErrorCode::Io,
                    format!("Could not create the terminal process job: {error}"),
                ));
            }
        };
        let process_handle = match child.as_raw_handle() {
            Some(process_handle) => process_handle,
            None => {
                terminate_spawned_child(pair, child);
                return Err(WireError::new(
                    ErrorCode::Io,
                    "The terminal process has no native handle.",
                ));
            }
        };
        if let Err(error) = state
            .process_job
            .assign(process_handle)
            .and_then(|()| process_job.assign(process_handle))
        {
            terminate_spawned_child(pair, child);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not contain the terminal process: {error}"),
            ));
        }
        process_job
    };

    #[cfg(not(windows))]
    let process_job = JobObject::new().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not create the terminal process job: {error}"),
        )
    })?;

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

    let runtime = Arc::new(SessionRuntime::with_journal(
        metadata.id.clone(),
        registry.journal.clone(),
    ));
    let exited = Arc::new(AtomicBool::new(false));
    let master = Arc::new(Mutex::new(pair.master));
    let writer = Arc::new(Mutex::new(writer));
    // Register before the reader thread starts: ConPTY's startup DSR can be
    // read within milliseconds, and the reply path needs the writer.
    runtime
        .pty_writer
        .set(Arc::clone(&writer))
        .ok()
        .expect("pty writer registered exactly once");
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
    let monitor_runtime = Arc::clone(&runtime);
    let liveness_handle = std::thread::Builder::new()
        .name(format!("session-liveness-{id}"))
        .spawn(move || {
            // This remains one sleeping thread per live session for now. If
            // session counts grow large, replace it with one shared sweeper.
            loop {
                if monitor_runtime.liveness_monitor_should_stop() {
                    return;
                }
                let Some(deadline) = monitor_runtime.next_liveness_deadline() else {
                    return;
                };
                monitor_runtime.wait_for_liveness(deadline);
                if monitor_runtime.liveness_monitor_should_stop() {
                    return;
                }
                monitor_runtime.mark_silent_if_due(Instant::now());
            }
        })
        .ok();
    let session = PtySession {
        metadata,
        owner: owner.clone(),
        process_job,
        master,
        killer,
        child_wait,
        writer,
        reader_handle: None,
        coalesce_handle: None,
        liveness_handle,
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
        map.insert(id.clone(), RegistryEntry::Live(session));
    }

    let (coalesce_tx, coalesce_rx) = mpsc::channel::<Vec<u8>>();
    let coalesce_runtime = Arc::clone(&runtime);
    let coalesce_handle = match std::thread::Builder::new()
        .name(format!("session-coalesce-{id}"))
        .spawn(move || coalesce_loop(coalesce_rx, coalesce_runtime))
    {
        Ok(handle) => Some(handle),
        Err(_) => {
            let _ = registry.close(&id, &owner);
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
            let _ = registry.close(&id, &owner);
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
        if let Some(session) = map.get_mut(&id).and_then(RegistryEntry::as_live_mut) {
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
                if coalesce_tx.send(buf[..n].to_vec()).is_err() {
                    runtime.record_output_loss();
                    eprintln!("session {id} dropped {n} terminal bytes: coalescer unavailable");
                    break;
                }
            }
            Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                runtime.record_output_loss();
                eprintln!("session {id} stopped reading terminal output: {error}");
                break;
            }
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
    let Some(session) = map.get_mut(id).and_then(RegistryEntry::as_live_mut) else {
        return false;
    };
    session.reader_handle = None;
    let preserve = session.preserve_on_exit.load(Ordering::SeqCst);
    if preserve {
        let coalesce = session.coalesce_handle.take();
        session.exited.store(true, Ordering::SeqCst);
        drop(map);
        join_coalesce(coalesce, runtime);
        journal_mark_ended(registry, runtime);
        runtime.close_output();
        return false;
    }
    let Some(RegistryEntry::Live(mut session)) = map.remove(id) else {
        return false;
    };
    drop(map);
    session.reader_handle = None;
    let coalesce = session.coalesce_handle.take();
    let child_wait = session.child_wait.take();
    let liveness = session.liveness_handle.take();
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
    bounded_join(liveness);
    join_coalesce(coalesce, runtime);
    let _ = session_runtime;
    journal_mark_ended(registry, runtime);
    runtime.close_output();
    true
}

fn journal_mark_ended(registry: &SessionRegistry, runtime: &SessionRuntime) {
    let Some(journal) = &registry.journal else {
        return;
    };
    let (generation, code) = match runtime.lock_stream() {
        Ok(stream) => (stream.generation, stream.exit_code),
        Err(_) => (runtime.generation(), None),
    };
    // EOF path: waiting on the journal here does not stall a live PTY. The
    // terminal marker is critical and must not be dropped behind a full
    // output queue.
    if let Err(error) = journal.mark_ended_blocking(&runtime.session_id, generation, code) {
        runtime.mark_journal_degraded();
        eprintln!(
            "journal could not record terminal exit for {}: {error}",
            runtime.session_id
        );
    }
}

fn wait_child(mut child: Box<dyn Child + Send + Sync>) -> Option<u32> {
    child.wait().ok().map(|status| status.exit_code())
}

fn terminate_spawned_child(pair: portable_pty::PtyPair, mut child: Box<dyn Child + Send + Sync>) {
    let mut killer = child.clone_killer();
    let _ = killer.kill();
    drop(pair.master);
    let _ = child.wait();
}

/// Kill + drop writer/master + wait + bounded reader join. ORDER IS
/// LOAD-BEARING: on Windows, waiting while the ConPTY master is alive can
/// deadlock the ConPTY host. Dropping the master also unblocks the
/// reader's blocking read.
fn teardown_session(session: PtySession) {
    session.exited.store(true, Ordering::SeqCst);
    let PtySession {
        process_job,
        master,
        mut killer,
        child_wait,
        writer,
        reader_handle,
        coalesce_handle,
        liveness_handle,
        runtime,
        exited: _,
        owner: _,
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
    // Closing the per-session KILL_ON_JOB_CLOSE job terminates the root and
    // every descendant before wait(). The daemon-wide job remains open for
    // other sessions and is the crash/no-cleanup backstop.
    drop(process_job);
    // 3) Reap after the PTY endpoints are closed; this prevents a zombie
    //    and avoids the Windows ConPTY wait deadlock. The waiter thread
    //    owns Child::wait so we join it here instead of calling wait()
    //    ourselves.
    bounded_join(child_wait);
    bounded_join(liveness_handle);
    // 4) Best-effort bounded join. JoinHandle has no timed join; the
    //    endpoint close above makes the reader finish promptly, while this
    //    small budget prevents shutdown from accumulating a hang across
    //    sessions.
    // The order above is intentional: the coalescer gets every chance to
    // publish before output is closed. If its bounded join still gives up,
    // any pending bytes may be discarded by the later finish(). Surface that
    // loss through the same per-session degradation signal used for journal
    // failures instead of silently claiming completeness.
    join_coalesce(coalesce_handle, &runtime);
    bounded_join(reader_handle);
    runtime.finish(None);
}

fn join_coalesce(handle: Option<JoinHandle<()>>, runtime: &SessionRuntime) {
    if !bounded_join(handle) {
        runtime.mark_journal_degraded();
        eprintln!(
            "session {} coalesce thread exceeded teardown join budget; scrollback may be truncated",
            runtime.session_id
        );
    }
}

fn bounded_join<T>(handle: Option<JoinHandle<T>>) -> bool {
    if let Some(handle) = handle {
        let deadline = Instant::now() + READER_JOIN_BUDGET;
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
            return true;
        }
        return false;
    }
    true
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
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine terminal directory: {error}"),
        )
    })?;
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

fn pty_wire_error(context: &str, error: impl std::fmt::Display) -> WireError {
    let detail = error.to_string();
    eprintln!("{context} {detail}");
    let message = match extract_os_error_code(&detail) {
        Some(code) => format!(
            "{context} (OS error {code}: {}).",
            os_error_description(code)
        ),
        None => format!("{context} (unknown OS error)."),
    };
    WireError::new(ErrorCode::Io, message)
}

fn extract_os_error_code(detail: &str) -> Option<u32> {
    detail
        .rsplit_once("(os error ")?
        .1
        .strip_suffix(')')?
        .parse()
        .ok()
}

fn os_error_description(code: u32) -> &'static str {
    match code {
        8 => "not enough memory",
        232 => "no data",
        1450 => "no system resources",
        _ => "unknown error",
    }
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

    /// A Write sink that records everything, standing in for the PTY input
    /// side so the DSR fast path is observable without a ConPTY.
    struct SharedSink(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn sink_runtime() -> (Arc<SessionRuntime>, Arc<Mutex<Vec<u8>>>) {
        let runtime = Arc::new(SessionRuntime::new());
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(SharedSink(Arc::clone(&received)))));
        runtime
            .pty_writer
            .set(sink)
            .ok()
            .expect("sink registered once");
        (runtime, received)
    }

    /// Pull until the session queue is empty, recording delivery like the
    /// connection writer does.
    fn drain(conn: &ConnHandle) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        loop {
            let batch = conn.pull_events();
            if batch.is_empty() {
                return events;
            }
            for event in &batch {
                conn.event_sent(event);
            }
            events.extend(batch.into_iter().map(|pending| pending.envelope.event));
        }
    }

    fn apply_snapshot_state(screen: &mut Screen, event: &SessionEvent) {
        let SessionEvent::Snapshot {
            data,
            cursor,
            bracketed_paste,
            line_wrap,
            title,
            ..
        } = event
        else {
            return;
        };
        screen.process(data.as_bytes());
        let shape = match cursor.shape {
            CursorShape::Block => 1,
            CursorShape::Underline => 3,
            CursorShape::Bar => 5,
        } + u16::from(!cursor.blinking);
        let state = format!(
            "\x1b[{};{}H\x1b[?25{}\x1b[{shape} q\x1b[?2004{}\x1b[?7{}{}",
            cursor.row + 1,
            cursor.col + 1,
            if cursor.visible { 'h' } else { 'l' },
            if *bracketed_paste { 'h' } else { 'l' },
            if *line_wrap { 'h' } else { 'l' },
            title
                .as_deref()
                .map(|title| format!("\x1b]2;{}\x1b\\", title))
                .unwrap_or_default(),
        );
        screen.process(state.as_bytes());
    }

    /// Deterministic flood chunk with attributes, cursor motion, CJK and a
    /// line break, so screen equality is exercised beyond plain text.
    fn flood_chunk(index: usize) -> String {
        let shade = 31 + (index % 7);
        format!("\x1b[{shade}mchunk {index:06}\x1b[0m \u{754c}\r\n")
    }

    fn attach_tracked(runtime: &Arc<SessionRuntime>, conn: &Arc<ConnHandle>) -> u64 {
        let generation = runtime.try_attach(None, conn).expect("attach");
        let transcript = runtime.is_transcript();
        conn.track(
            "s.a.1",
            Arc::clone(runtime),
            transcript,
            Some(0),
            generation,
        );
        generation
    }

    #[test]
    fn poisoned_stream_is_dead_and_not_reused() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        let poisoned = Arc::clone(&runtime);
        let panic = std::thread::spawn(move || {
            let _stream = poisoned.stream.lock().expect("stream lock");
            panic!("simulate a terminal-state panic");
        });
        assert!(panic.join().is_err());

        runtime.publish_output("must not be applied");
        let events = drain(&conn);
        assert!(runtime.terminal_dead.load(Ordering::Acquire));
        assert!(matches!(
            events.as_slice(),
            [
                SessionEvent::JournalDegraded,
                SessionEvent::Exit { code: None }
            ]
        ));
        assert_eq!(runtime.try_attach(None, &conn), Err(process_gone()));
    }

    #[test]
    fn silence_transition_is_emitted_once_after_the_threshold() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        let _ = drain(&conn);
        let last_publish = runtime
            .stream
            .lock()
            .expect("stream lock")
            .last_publish
            .expect("new sessions have an observed start time");

        assert_eq!(
            runtime.mark_silent_if_due(
                last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(42)
            ),
            Some(SESSION_SILENCE_THRESHOLD.as_millis() as u64 + 42)
        );
        assert_eq!(
            drain(&conn),
            vec![SessionEvent::Silent {
                elapsed_ms: SESSION_SILENCE_THRESHOLD.as_millis() as u64 + 42,
            }]
        );
        assert_eq!(
            runtime.mark_silent_if_due(
                last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_secs(1)
            ),
            None
        );
        assert!(
            drain(&conn).is_empty(),
            "silence is a transition, not a tick"
        );
    }

    #[test]
    fn queued_silence_is_dropped_when_output_precedes_a_reattach() {
        let runtime = Arc::new(SessionRuntime::new());
        let first = Arc::new(ConnHandle::new(1));
        attach_tracked(&runtime, &first);
        let _ = drain(&first);
        let last_publish = runtime
            .stream
            .lock()
            .expect("stream lock")
            .last_publish
            .expect("new sessions have an observed start time");

        runtime.mark_silent_if_due(
            last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(1),
        );
        runtime.publish_output("resumed");
        runtime.detach_if_conn(first.id);

        let second = Arc::new(ConnHandle::new(2));
        attach_tracked(&runtime, &second);
        let events = drain(&second);
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, SessionEvent::Silent { .. })),
            "a reattached client must not receive stale silence: {events:?}"
        );
    }

    #[test]
    fn silence_is_dropped_when_the_session_exits() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = Arc::new(ConnHandle::new(1));
        attach_tracked(&runtime, &conn);
        let _ = drain(&conn);
        let last_publish = runtime
            .stream
            .lock()
            .expect("stream lock")
            .last_publish
            .expect("new sessions have an observed start time");

        runtime.mark_silent_if_due(
            last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(1),
        );
        runtime.finish(Some(7));

        assert_eq!(
            drain(&conn),
            vec![SessionEvent::Exit { code: Some(7) }],
            "exit must be the only terminal transition delivered after silence"
        );
    }

    #[test]
    fn elapsed_time_uses_exit_for_ended_and_stays_unknown_for_recovered() {
        let now = Instant::now();
        let last_publish = Some(now - Duration::from_secs(3600));
        let exit_at = Some(now - Duration::from_secs(7));

        assert_eq!(
            elapsed_ms_since_last_life(last_publish, exit_at, true, now),
            Some(7_000)
        );
        assert_eq!(
            elapsed_ms_since_last_life(last_publish, None, false, now),
            Some(3_600_000)
        );
        assert_eq!(
            elapsed_ms_since_last_life(None, None, true, now),
            None,
            "journal-only recovered sessions have no monotonic timestamp"
        );
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
    fn attach_delivers_snapshot_then_live_with_exact_boundary() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.publish_output("before");
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        assert_eq!(runtime.last_applied_seq(), 1);

        let events = drain(&conn);
        let [SessionEvent::Snapshot {
            as_of_seq, data, ..
        }] = &events[..]
        else {
            panic!("expected a single snapshot, got {events:?}");
        };
        assert_eq!(*as_of_seq, 1);
        assert!(data.contains("before"), "snapshot data: {data:?}");

        runtime.publish_output("after");
        let events = drain(&conn);
        assert_eq!(
            events,
            vec![SessionEvent::Output {
                seq: 2,
                data: "after".to_string()
            }]
        );
    }

    #[test]
    fn attach_during_flood_never_duplicates_or_skips() {
        let runtime = Arc::new(SessionRuntime::new());
        let flood_runtime = Arc::clone(&runtime);
        let flood = std::thread::Builder::new()
            .name("flood".into())
            .spawn(move || {
                for index in 1..=4_000 {
                    flood_runtime.publish_output(&flood_chunk(index));
                    if index % 32 == 0 {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            })
            .expect("flood thread");

        let mut seen = std::collections::HashSet::new();
        let mut covered_to = 0u64;
        for epoch in 0u64..25 {
            let conn = ConnHandle::new(epoch + 1);
            attach_tracked(&runtime, &conn);
            let events = drain(&conn);
            assert!(
                !events.is_empty(),
                "epoch {epoch} saw nothing: attach must enqueue a snapshot"
            );
            // Spread the attach epochs across the flood's lifetime.
            std::thread::sleep(Duration::from_millis(4));
            let mut expected = None;
            for event in &events {
                match event {
                    SessionEvent::Snapshot { as_of_seq, .. } => {
                        assert!(
                            *as_of_seq >= covered_to,
                            "snapshot boundary moved backwards at epoch {epoch}"
                        );
                        covered_to = (*as_of_seq).max(covered_to);
                        expected = Some(as_of_seq + 1);
                    }
                    SessionEvent::Output { seq, .. } => {
                        if let Some(expected_seq) = expected {
                            assert_eq!(
                                *seq, expected_seq,
                                "output skipped or duplicated at epoch {epoch}"
                            );
                        }
                        expected = Some(seq + 1);
                        assert!(seen.insert(*seq), "sequence {seq} delivered twice");
                        covered_to = (*seq).max(covered_to);
                    }
                    SessionEvent::Exit { .. } => {}
                    other => panic!("unexpected event at epoch {epoch}: {other:?}"),
                }
            }
            runtime.detach_if_conn(conn.id);
        }
        flood.join().expect("flood thread joins");

        // The flood is complete: one final attach must now deliver (or
        // subsume) everything it published.
        let conn = ConnHandle::new(999);
        attach_tracked(&runtime, &conn);
        for event in drain(&conn) {
            match event {
                SessionEvent::Snapshot { as_of_seq, .. } => covered_to = as_of_seq.max(covered_to),
                SessionEvent::Output { seq, .. } => {
                    assert!(seen.insert(seq), "sequence {seq} delivered twice");
                    covered_to = seq.max(covered_to);
                }
                _ => {}
            }
        }
        assert_eq!(
            covered_to, 4_000,
            "the flood was not fully delivered or subsumed"
        );
    }

    #[test]
    fn reattach_mid_flood_screen_equals_a_fresh_emulator() {
        let runtime = Arc::new(SessionRuntime::new());
        let mut reference = Screen::new(INITIAL_COLS, INITIAL_ROWS);

        fn apply(screen: &mut Screen, event: &SessionEvent) {
            match event {
                SessionEvent::Snapshot { .. } => apply_snapshot_state(screen, event),
                SessionEvent::Output { data, .. } => screen.process(data.as_bytes()),
                _ => {}
            }
        }

        // Phase 1: publish while detached, then attach and synchronise.
        for index in 1..=60 {
            let chunk = flood_chunk(index);
            runtime.publish_output(&chunk);
            reference.process(chunk.as_bytes());
        }
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
        for event in drain(&conn) {
            apply(&mut client, &event);
        }
        assert_eq!(
            client.snapshot(),
            reference.snapshot(),
            "snapshot state must equal the emulator after phase 1"
        );

        // Phase 2: live chunks while attached, then reattach from scratch.
        for index in 61..=120 {
            let chunk = flood_chunk(index);
            runtime.publish_output(&chunk);
            reference.process(chunk.as_bytes());
        }
        for event in drain(&conn) {
            apply(&mut client, &event);
        }
        assert_eq!(client.snapshot(), reference.snapshot());

        runtime.detach_if_conn(conn.id);
        let conn = ConnHandle::new(2);
        attach_tracked(&runtime, &conn);
        let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
        for event in drain(&conn) {
            apply(&mut client, &event);
        }
        assert_eq!(
            client.snapshot(),
            reference.snapshot(),
            "snapshot + subsequent events must equal a fresh emulator fed the whole stream"
        );
    }

    #[test]
    fn slow_client_is_resynchronised_with_a_snapshot() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);

        // Stop reading: publish well past the frame budget without pulling.
        for index in 1..=200 {
            runtime.publish_output(&format!("slow-{index:04}\r\n"));
        }

        let events = drain(&conn);
        let mut expected = None;
        let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
        let mut reference = Screen::new(INITIAL_COLS, INITIAL_ROWS);
        let mut seen = std::collections::HashSet::new();
        for event in &events {
            match event {
                SessionEvent::Snapshot { as_of_seq, .. } => {
                    expected = Some(as_of_seq + 1);
                    apply_snapshot_state(&mut client, event);
                }
                SessionEvent::Output { seq, data } => {
                    assert_eq!(*seq, expected.expect("outputs follow the snapshot"));
                    expected = Some(seq + 1);
                    assert!(seen.insert(*seq), "sequence {seq} delivered twice");
                    client.process(data.as_bytes());
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        for index in 1..=200 {
            reference.process(format!("slow-{index:04}\r\n").as_bytes());
        }
        assert_eq!(
            client.snapshot(),
            reference.snapshot(),
            "the resynchronised screen must still be the true screen"
        );
    }

    #[test]
    fn pending_queue_never_exceeds_byte_or_frame_budget() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        let payload = "x".repeat(COALESCE_MAX_BYTES);

        for _ in 0..200 {
            runtime.publish_output(&payload);
            let stream = runtime.stream.lock().expect("stream lock");
            assert!(stream.pending_bytes <= PENDING_OUTPUT_BUDGET_BYTES);
            assert!(stream.pending_frames <= PENDING_OUTPUT_BUDGET_FRAMES);
        }
    }

    #[test]
    fn dsr_reply_is_written_straight_to_the_pty() {
        let (runtime, received) = sink_runtime();
        // No attachment, no journal, no snapshot: the query is answered on
        // the publish path itself.
        runtime.publish_output("\x1b[2;3H\x1b[6n");
        assert_eq!(
            String::from_utf8(received.lock().unwrap().clone()).expect("utf8"),
            "\x1b[2;3R",
            "one one-based CPR reply, routed to the PTY writer"
        );
        runtime.publish_output("plain");
        assert_eq!(received.lock().unwrap().len(), 6, "no extra replies");
    }

    #[test]
    fn control_path_stays_responsive_under_flood() {
        let runtime = Arc::new(SessionRuntime::new());
        let flood_runtime = Arc::clone(&runtime);
        let stop = Arc::new(AtomicBool::new(false));
        let flood_stop = Arc::clone(&stop);
        let flood = std::thread::Builder::new()
            .name("flood".into())
            .spawn(move || {
                let chunk = "x".repeat(COALESCE_MAX_BYTES);
                while !flood_stop.load(Ordering::Acquire) {
                    for _ in 0..16 {
                        flood_runtime.publish_output(&chunk);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
            .expect("flood thread");

        let mut worst = Duration::ZERO;
        for epoch in 0..200u64 {
            let started = Instant::now();
            let conn = ConnHandle::new(epoch + 1);
            runtime.try_attach(None, &conn).expect("attach under flood");
            runtime.detach_if_conn(conn.id);
            worst = worst.max(started.elapsed());
        }
        stop.store(true, Ordering::Release);
        flood.join().expect("flood thread joins");
        // Screen capture + registration is two grid copies under the lock;
        // if the publish path ever held the mutex across slow work, this
        // would blow far past the bound. 1 s is orders of magnitude above
        // the observed cost and 30x below the RPC timeout this milestone
        // exists to fix.
        assert!(
            worst < Duration::from_secs(1),
            "state lock starved under flood: {worst:?}"
        );
    }

    #[test]
    fn transcript_cursor_replays_only_after() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.bump_generation();
        let conn = ConnHandle::new(1);
        let generation = runtime
            .try_attach(
                Some(Cursor {
                    generation: 2,
                    seq: 1,
                }),
                &conn,
            )
            .unwrap();
        conn.track("s.a.1", Arc::clone(&runtime), true, Some(1), generation);
        runtime.stream.lock().unwrap().scrollback.push(2, b"two");
        runtime.finish(Some(0));
        let events = drain(&conn);
        assert_eq!(
            events,
            vec![
                SessionEvent::Output {
                    seq: 2,
                    data: "two".to_string()
                },
                SessionEvent::Exit { code: Some(0) },
            ]
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
    fn live_attach_ends_with_snapshot_output_then_exit() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.publish_output("before");
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        runtime.publish_output("after");
        runtime.finish(Some(0));
        let events = drain(&conn);
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                SessionEvent::Snapshot { .. } => "snapshot",
                SessionEvent::Output { .. } => "output",
                SessionEvent::Exit { .. } => "exit",
                SessionEvent::Recovered { .. } => "recovered",
                SessionEvent::Silent { .. } => "silent",
                SessionEvent::JournalDegraded => "journal_degraded",
            })
            .collect();
        assert_eq!(kinds, ["snapshot", "output", "exit"]);
        // Exit must not overtake queued output.
        assert!(matches!(
            events.last(),
            Some(SessionEvent::Exit { code: Some(0) })
        ));
    }

    #[test]
    fn journal_keeps_every_frame_for_recovery() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        journal
            .upsert_blocking(new_session_record(
                "s.recover.1",
                "S-1-5-21-1",
                None,
                SessionKind::Terminal,
                "Terminal",
            ))
            .unwrap();
        let runtime = Arc::new(SessionRuntime::with_journal(
            "s.recover.1".into(),
            Some(Arc::clone(&journal)),
        ));
        let payload = "x".repeat(8192);
        for _ in 1..=300 {
            runtime.publish_output(&payload);
        }
        runtime.finish(Some(0));
        journal.flush().unwrap();

        let replay = journal.replay("s.recover.1", 0).unwrap();
        let seqs: Vec<u64> = replay
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { seq, .. } => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(seqs, (1..=300).collect::<Vec<_>>());

        // The recovered runtime is a transcript: no emulator, journal replay
        // instead of a snapshot. Hydrating at 150 leaves a prefix that only
        // a journal read can fill, so the attach below exercises the seam.
        let replay_late = journal.replay("s.recover.1", 150).unwrap();
        let recovered = SessionRuntime::from_replay(
            "s.recover.1".into(),
            Some(Arc::clone(&journal)),
            replay_late,
        );
        assert!(recovered.is_transcript());
        assert_eq!(recovered.transcript_chunks().len(), 150);
        let conn = ConnHandle::new(1);
        attach_tracked(&recovered, &conn);
        let events = drain(&conn);
        let seqs: Vec<u64> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { seq, .. } => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(seqs, (1..=300).collect::<Vec<_>>());
        assert_eq!(
            recovered.journal_replay_count(),
            1,
            "one journal read filled the prefix, not one per pull"
        );
        drop(recovered);
        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn journal_keeps_drain_bytes_after_reap() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-drain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        journal
            .upsert_blocking(new_session_record(
                "s.drain.1",
                "S-1-5-21-1",
                None,
                SessionKind::Terminal,
                "Terminal",
            ))
            .unwrap();
        let runtime = Arc::new(SessionRuntime::with_journal(
            "s.drain.1".into(),
            Some(Arc::clone(&journal)),
        ));
        runtime.publish_output("HEAD");
        journal.flush().unwrap();
        runtime.mark_exited(Some(0));
        journal.flush().unwrap();
        let tail = "X".repeat(3953);
        runtime.publish_output(&tail);
        journal.flush().unwrap();
        runtime.close_output();
        journal.try_mark_ended("s.drain.1", 1, Some(0));
        journal.flush().unwrap();
        assert_eq!(runtime.published_frames.load(Ordering::Relaxed), 2);
        let stats = journal.stats();
        assert_eq!(stats.accepted_frames, 2);
        assert_eq!(stats.committed_frames, 2);
        assert_eq!(stats.failed_frames, 0);
        let replay = journal.replay("s.drain.1", 0).unwrap();
        let replay_bytes: usize = replay
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { data, .. } => Some(data.len()),
                _ => None,
            })
            .sum();
        assert_eq!(replay_bytes, 4 + 3953, "journal silently lost drain bytes");
        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_exit_wake_is_zero_once_the_drain_has_elapsed() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        runtime.try_attach(None, &conn).unwrap();
        conn.track("s.a.1", Arc::clone(&runtime), false, None, 1);
        assert_eq!(conn.next_exit_wake(), None);
        runtime.mark_exited(Some(0));
        let wake = conn.next_exit_wake().expect("drain timer");
        assert!(wake <= EXIT_DRAIN);
        std::thread::sleep(EXIT_DRAIN + Duration::from_millis(10));
        assert_eq!(conn.next_exit_wake(), Some(Duration::ZERO));
        let events = conn.pull_events();
        assert!(
            events
                .iter()
                .any(|envelope| matches!(envelope.envelope.event, SessionEvent::Exit { .. })),
            "zero wake must let the writer emit Exit, got {events:?}"
        );
        for event in &events {
            conn.event_sent(event);
        }
    }

    #[test]
    fn live_journal_degradation_reaches_attached_client_once() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);

        runtime.publish_output("still live");
        runtime.mark_journal_degraded();
        runtime.mark_journal_degraded();

        let events = drain(&conn);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::JournalDegraded))
                .count(),
            1,
            "degradation must be delivered exactly once: {events:?}"
        );
        assert!(events
            .iter()
            .all(|event| !matches!(event, SessionEvent::Exit { .. })));
        assert!(!runtime.stream.lock().unwrap().process_exited);
    }

    #[test]
    fn recovered_pull_ends_with_recovered_not_exit() {
        let replay = crate::journal::Replay {
            generation: 1,
            last_seq: 1,
            truncated: false,
            events: vec![
                SessionEvent::Output {
                    seq: 1,
                    data: "hello".to_string(),
                },
                SessionEvent::Recovered { truncated: false },
            ],
        };
        let runtime = SessionRuntime::from_replay("s.a.1".to_string(), None, replay);
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        let events = drain(&conn);
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                SessionEvent::Output { .. } => "output",
                SessionEvent::Exit { .. } => "exit",
                SessionEvent::Recovered { .. } => "recovered",
                SessionEvent::Silent { .. } => "silent",
                SessionEvent::JournalDegraded => "journal_degraded",
                SessionEvent::Snapshot { .. } => "snapshot",
            })
            .collect();
        assert_eq!(kinds, ["output", "recovered"]);
    }

    #[test]
    fn coalesce_constants_are_small_enough_for_echo() {
        const {
            assert!(COALESCE_MAX_BYTES <= 16 * 1024);
        }
        const {
            assert!(COALESCE_MAX_BYTES >= 1024);
        }
        assert!(COALESCE_FLUSH <= Duration::from_millis(16));
    }

    #[test]
    fn pty_error_exposes_only_the_os_code_to_clients() {
        let detail = "CreateProcessW command=C:\\Users\\secret\\shell.exe (os error 1450)";
        let code = extract_os_error_code(detail).expect("OS error code");
        assert_eq!(code, 1450);
        assert_eq!(os_error_description(code), "no system resources");
        let wire = pty_wire_error("Could not start the terminal shell.", detail);
        assert_eq!(
            wire.message,
            "Could not start the terminal shell. (OS error 1450: no system resources)."
        );
        assert!(!wire.message.contains("secret"));
    }
}
