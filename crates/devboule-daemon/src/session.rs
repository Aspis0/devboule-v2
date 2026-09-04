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

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{Child, ChildKiller, MasterPty, PtySize};

#[cfg(test)]
use devboule_protocol::CursorShape;
use devboule_protocol::{
    compose_session_id, cursor_replay_ok, validate_session_id, Cursor, ErrorCode, JournalRetention,
    JournalStats, OwnerId, PermissionOutcome, RetentionPatch, Session, SessionEvent, SessionKind,
    SessionState, SessionStateSnapshot, TranscriptIntegrity, WireError,
};

use crate::journal::{new_session_record, output_record, Journal, PersistStatus, Replay};
use crate::outbound::ConnOut;
use crate::paths::RuntimePaths;
use crate::process_tree::JobObject;
use crate::screen::Screen;
use crate::server::ServerState;

#[path = "acp_client.rs"]
mod acp_client;
#[path = "event_pull.rs"]
mod event_pull;
#[path = "session_types.rs"]
mod session_types;
#[path = "shell_command.rs"]
mod shell_command;

pub use event_pull::ConnHandle;
pub(crate) use session_types::PendingEvent;
pub use session_types::PtyCommand;
use session_types::{
    Attachment, Disposition, OutputMetrics, PendingItem, PullState, RegistryEntry, Scrollback,
    StreamState, TranscriptSession,
};
use shell_command::resolve_pty_command;
pub use shell_command::write_test_pty_command;

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

fn integrity_counters(integrity: TranscriptIntegrity) -> (u64, u64) {
    match integrity {
        TranscriptIntegrity::Complete => (0, 0),
        TranscriptIntegrity::Truncated {
            dropped_frames,
            dropped_bytes,
            ..
        }
        | TranscriptIntegrity::Unverifiable {
            dropped_frames,
            dropped_bytes,
            ..
        } => (dropped_frames, dropped_bytes),
    }
}

const READ_CHUNK: usize = 16 * 1024;
const INITIAL_COLS: u16 = 120;
const INITIAL_ROWS: u16 = 32;
pub const MAX_WRITE_BYTES: usize = 64 * 1024;
const READER_JOIN_BUDGET: Duration = Duration::from_millis(150);

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

/// The transport-specific ACP module supplies these three small adapters;
/// the registry, runtime, coalescer, journal and attachment code stay shared.
pub(super) trait SessionKiller: Send + Sync {
    fn kill(&mut self);
    fn clone_killer(&self) -> Box<dyn SessionKiller>;
}

pub(super) trait WaitableChild: Send {
    fn wait(self: Box<Self>) -> Option<u32>;
}

pub(super) trait ReaderDispatch: Send {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String>;
    fn finish(&mut self, runtime: &Arc<SessionRuntime>);
}

pub(super) trait StderrSource: Send {
    fn spawn(self: Box<Self>, runtime: Arc<SessionRuntime>) -> std::io::Result<JoinHandle<()>>;
}

/// Stream state is one mutex on purpose. Every step that must be atomic
/// with respect to output application happens under this single hold:
/// apply-to-emulator + boundary update + per-attachment enqueue, and screen
/// capture + attachment registration. Holding it across attach registration
/// makes attach ordering exact: the subscriber is registered with its
/// snapshot already captured, and only then can the reader publish the next
/// live chunk. Subscribe-with-state, subscribe-before-live.
pub(super) struct SessionRuntime {
    session_id: String,
    journal: Option<Arc<Journal>>,
    permission_broker: Option<Arc<acp_client::PermissionBroker>>,
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
    journal_dropped_frames: AtomicU64,
    journal_dropped_bytes: AtomicU64,
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
    /// Transition notifications are suppressed until spawn has inserted all
    /// runtime state, so an extremely short-lived child cannot publish an
    /// exit before the corresponding create snapshot.
    transition_ready: AtomicBool,
    /// The wait thread and the post-create race check can observe the same
    /// exit. Only one of them may publish the exit transition.
    exit_transition_sent: AtomicBool,
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
            permission_broker: None,
            stream: Mutex::new(StreamState {
                next_seq: 1,
                last_applied_seq: 0,
                generation: 1,
                screen: Some(Screen::new(INITIAL_COLS, INITIAL_ROWS)),
                transcript: false,
                attached: None,
                pending: VecDeque::new(),
                pending_bytes: 0,
                pending_frames: 0,
                agent_backlog: VecDeque::new(),
                agent_backlog_bytes: 0,
                agent_backlog_frames: 0,
                typed_permissions: false,
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
            journal_dropped_frames: AtomicU64::new(0),
            journal_dropped_bytes: AtomicU64::new(0),
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
            transition_ready: AtomicBool::new(false),
            exit_transition_sent: AtomicBool::new(false),
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
        let integrity = replay.integrity;
        stream.disposition = match integrity {
            TranscriptIntegrity::Unverifiable { .. } => Disposition::Recovered { integrity },
            TranscriptIntegrity::Complete | TranscriptIntegrity::Truncated { .. } => {
                Disposition::Exited { integrity }
            }
        };
        let (dropped_frames, dropped_bytes) = integrity_counters(integrity);
        runtime
            .journal_dropped_frames
            .store(dropped_frames, Ordering::Release);
        runtime
            .journal_dropped_bytes
            .store(dropped_bytes, Ordering::Release);
        // A recovered session is a transcript, not a live process: no
        // emulator, no snapshot, no live queue. Cursor-based journal
        // replay below serves its attaches.
        stream.screen = None;
        stream.transcript = true;
        for event in replay.events {
            match event {
                SessionEvent::Output { seq, data } => {
                    stream.scrollback.push(seq, data.as_bytes());
                }
                SessionEvent::Exit { code } => {
                    stream.exit_code = code;
                    stream.disposition = Disposition::Exited { integrity };
                }
                SessionEvent::Recovered { integrity } => {
                    stream.disposition = Disposition::Recovered { integrity };
                }
                SessionEvent::Silent { .. } => {}
                SessionEvent::JournalDegraded {
                    dropped_frames,
                    dropped_bytes,
                } => {
                    runtime.journal_degraded.store(true, Ordering::Release);
                    runtime
                        .journal_dropped_frames
                        .fetch_max(dropped_frames, Ordering::AcqRel);
                    runtime
                        .journal_dropped_bytes
                        .fetch_max(dropped_bytes, Ordering::AcqRel);
                }
                SessionEvent::SessionsSnapshot { .. } => {}
                // Snapshots are screen state, never journal records; a
                // recovered session replays transcript events only.
                SessionEvent::Snapshot { .. } => {}
                SessionEvent::AgentMessage { .. }
                | SessionEvent::AgentToolCall { .. }
                | SessionEvent::AgentToolUpdate { .. }
                | SessionEvent::AgentFinished { .. }
                | SessionEvent::AgentError { .. }
                | SessionEvent::AgentStderr { .. }
                | SessionEvent::PermissionRequest { .. } => {}
            }
        }
        drop(stream);
        runtime
    }

    /// True when this runtime is a recovered transcript (no emulator, no
    /// live process). Attaches to it replay the journal instead of
    /// synchronising a screen.
    pub(crate) fn is_transcript(&self) -> bool {
        self.lock_stream()
            .map(|stream| stream.transcript)
            .unwrap_or(false)
    }

    fn for_acp(
        session_id: String,
        journal: Option<Arc<Journal>>,
        permission_broker: Arc<acp_client::PermissionBroker>,
    ) -> Arc<Self> {
        let mut state = Self::with_journal(session_id, journal);
        state.permission_broker = Some(permission_broker);
        let runtime = Arc::new(state);
        if let Ok(mut stream) = runtime.stream.lock() {
            // ACP has structured messages rather than a terminal screen, but
            // it is still a live session and must use the live attach path.
            stream.screen = None;
            stream.transcript = false;
        }
        runtime
    }

    pub(crate) fn lock_stream(&self) -> Result<MutexGuard<'_, StreamState>, ()> {
        match self.stream.lock() {
            Ok(stream) => Ok(stream),
            Err(error) => {
                // PoisonError owns the guard that was acquired before the
                // panic. Release it before the dead-session path takes any
                // other action, otherwise refreshing the disposition would
                // wait forever on the same poisoned mutex.
                drop(error);
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

    fn transition_ready(&self) -> bool {
        self.transition_ready.load(Ordering::Acquire)
    }

    fn process_exited(&self) -> bool {
        self.lock_stream()
            .map(|stream| stream.process_exited)
            .unwrap_or(true)
    }

    fn should_publish_exit_transition(&self) -> bool {
        self.transition_ready() && !self.exit_transition_sent.swap(true, Ordering::AcqRel)
    }

    fn publish_output(&self, data: &str) -> bool {
        let pty_replies;
        let seq;
        let generation;
        let was_silent;
        {
            let Ok(mut stream) = self.lock_stream() else {
                return false;
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
                return false;
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
                            return false;
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
        was_silent
    }

    fn publish_agent_event(&self, event: SessionEvent, journal_text: Option<&str>) -> bool {
        let was_silent;
        let journal_output;
        {
            let Ok(mut stream) = self.lock_stream() else {
                return false;
            };
            if stream.output_closed {
                eprintln!(
                    "session {} dropped ACP event after EOF: {:?}",
                    self.session_id, event
                );
                return false;
            }
            was_silent = matches!(stream.disposition, Disposition::Silent);
            if !stream.process_exited {
                stream.disposition = Disposition::Running;
            }
            stream.last_publish = Some(Instant::now());
            journal_output = journal_text.map(|text| {
                let seq = stream.next_seq;
                stream.next_seq = stream.next_seq.saturating_add(1);
                (stream.generation, seq, text.to_string())
            });
            enqueue_agent(&mut stream, event.clone());
            self.published_frames.fetch_add(1, Ordering::Relaxed);
            self.published_bytes.fetch_add(
                serde_json::to_vec(&event)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0),
                Ordering::Relaxed,
            );
            if let Some(attached) = &stream.attached {
                attached.outbound.notify();
            }
        }
        if was_silent {
            self.notify_liveness();
        }
        if let (Some(journal), Some((generation, seq, text))) = (&self.journal, journal_output) {
            let accepted = journal.try_append(output_record(
                self.session_id.clone(),
                generation,
                seq,
                text.as_bytes(),
            ));
            if !accepted || journal.is_session_degraded(&self.session_id) {
                self.mark_journal_degraded();
            }
        }
        was_silent
    }

    fn permission_broker(&self) -> Option<Arc<acp_client::PermissionBroker>> {
        self.permission_broker.as_ref().map(Arc::clone)
    }

    /// `None` means no client is attached. A detached request is retained so
    /// a later capable attach can display it; `Some(false)` means the current
    /// client is attached but did not negotiate typed permissions.
    pub(crate) fn permission_delivery_enabled(&self) -> Option<bool> {
        self.lock_stream()
            .ok()
            .and_then(|stream| stream.attached.as_ref().map(|_| stream.typed_permissions))
    }

    pub(crate) fn remove_permission_request(&self, tool_call_id: &str) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        {
            let StreamState {
                pending,
                pending_bytes,
                pending_frames,
                ..
            } = &mut *stream;
            remove_permission_from_queue(pending, pending_bytes, pending_frames, tool_call_id);
        }
        {
            let StreamState {
                agent_backlog,
                agent_backlog_bytes,
                agent_backlog_frames,
                ..
            } = &mut *stream;
            remove_permission_from_queue(
                agent_backlog,
                agent_backlog_bytes,
                agent_backlog_frames,
                tool_call_id,
            );
        }
        if let Some(attached) = &stream.attached {
            attached.outbound.notify();
        }
    }

    pub(crate) fn record_permission_decision(
        &self,
        tool_call_id: &str,
        outcome: &str,
        request: &SessionEvent,
    ) -> bool {
        let Some(journal) = &self.journal else {
            return false;
        };
        let Ok(payload) = serde_json::to_vec(request) else {
            self.mark_journal_degraded();
            return false;
        };
        match journal.record_permission(&self.session_id, tool_call_id, outcome, &payload) {
            Ok(()) => true,
            Err(_) => {
                self.mark_journal_degraded();
                false
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
        if let Some(journal) = &self.journal {
            let (frames, bytes) = journal.session_drop_counters(&self.session_id);
            self.journal_dropped_frames
                .fetch_max(frames, Ordering::AcqRel);
            self.journal_dropped_bytes
                .fetch_max(bytes, Ordering::AcqRel);
        }
        if !self.journal_degraded.swap(true, Ordering::AcqRel) {
            if let Some(journal) = &self.journal {
                journal.note_session_degraded(&self.session_id);
            }
        }
        self.refresh_exit_integrity();
        self.notify_attachment();
    }

    /// Mark the running stream silent at an injected observation time. The
    /// monitor uses `Instant::now`; the parameter keeps the transition
    /// boundary deterministic in unit tests.
    fn mark_silent_if_due(&self, now: Instant) -> Option<u64> {
        let elapsed_ms;
        {
            let mut stream = self.lock_stream().ok()?;
            if stream.process_exited || !matches!(stream.disposition, Disposition::Running) {
                return None;
            }
            let last_publish = stream.last_publish?;
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

    pub(crate) fn journal_degraded(&self) -> bool {
        self.journal_degraded.load(Ordering::Acquire)
    }

    fn journal_degraded_event(&self) -> SessionEvent {
        SessionEvent::JournalDegraded {
            dropped_frames: self.journal_dropped_frames.load(Ordering::Acquire),
            dropped_bytes: self.journal_dropped_bytes.load(Ordering::Acquire),
        }
    }

    pub(crate) fn replay_journal_outputs(
        &self,
        from_seq: u64,
        generation: u64,
    ) -> Vec<(u64, String)> {
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
        self.journal_replays.fetch_add(1, Ordering::Relaxed);
        replay
            .events
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::Output { seq, data } => Some((seq, data)),
                SessionEvent::Exit { .. }
                | SessionEvent::Recovered { .. }
                | SessionEvent::Silent { .. }
                | SessionEvent::JournalDegraded { .. }
                | SessionEvent::SessionsSnapshot { .. }
                // Snapshots are not output chunks and are not sourced from
                // the historical journal replay path.
                | SessionEvent::Snapshot { .. }
                | SessionEvent::AgentMessage { .. }
                | SessionEvent::AgentToolCall { .. }
                | SessionEvent::AgentToolUpdate { .. }
                | SessionEvent::AgentFinished { .. }
                | SessionEvent::AgentError { .. }
                | SessionEvent::AgentStderr { .. }
                | SessionEvent::PermissionRequest { .. } => None,
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
    fn try_attach(
        &self,
        from_cursor: Option<Cursor>,
        conn: &ConnHandle,
        typed_permissions: bool,
    ) -> Result<u64, WireError> {
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
        let preserve_agent_pending = stream
            .attached
            .as_ref()
            .is_some_and(|attached| attached.conn_id == conn.id)
            && !stream.transcript
            && stream.screen.is_none();
        if preserve_agent_pending {
            move_agent_pending_to_backlog(&mut stream);
        }
        stream.typed_permissions = typed_permissions;
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
        } else if !stream.transcript {
            let agent_backlog = std::mem::take(&mut stream.agent_backlog);
            stream.agent_backlog_bytes = 0;
            stream.agent_backlog_frames = 0;
            for item in agent_backlog {
                let is_permission = matches!(
                    &item,
                    PendingItem::Agent {
                        event: SessionEvent::PermissionRequest { .. },
                        ..
                    }
                );
                if is_permission && !typed_permissions {
                    if let PendingItem::Agent { bytes, .. } = &item {
                        stream.agent_backlog_bytes =
                            stream.agent_backlog_bytes.saturating_add(*bytes);
                        stream.agent_backlog_frames = stream.agent_backlog_frames.saturating_add(1);
                    }
                    stream.agent_backlog.push_back(item);
                } else {
                    stream.pending.push_back(item);
                }
            }
            stream.pending_bytes = stream
                .pending
                .iter()
                .filter_map(|item| match item {
                    PendingItem::Agent { bytes, .. } => Some(*bytes),
                    PendingItem::Output { data, .. } => Some(data.len()),
                    PendingItem::Snapshot { .. } => None,
                })
                .sum();
            stream.pending_frames = stream
                .pending
                .iter()
                .filter(|item| !matches!(item, PendingItem::Snapshot { .. }))
                .count() as u64;
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
            stream.typed_permissions = false;
            self.set_attachment_notify(None);
            // The unsent queue belonged to the departing viewer. A new
            // attach must start from a fresh snapshot, not from a stale
            // stream assembled for a client that is gone.
            if !stream.transcript && stream.screen.is_none() {
                move_agent_pending_to_backlog(&mut stream);
            } else {
                stream.pending.clear();
                stream.pending_bytes = 0;
                stream.pending_frames = 0;
            }
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
        stream.disposition = Disposition::Exited {
            integrity: self.terminated_integrity(),
        };
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
        self.refresh_exit_integrity();
    }

    fn terminated_integrity(&self) -> TranscriptIntegrity {
        if self.journal_degraded() {
            TranscriptIntegrity::Truncated {
                dropped_frames: self.journal_dropped_frames.load(Ordering::Acquire),
                dropped_bytes: self.journal_dropped_bytes.load(Ordering::Acquire),
                trimmed_bytes: 0,
            }
        } else {
            TranscriptIntegrity::Complete
        }
    }

    fn refresh_exit_integrity(&self) {
        // A poisoned stream is already handled by the caller's terminal-dead
        // path; do not re-enter that path while refreshing the disposition.
        let Ok(mut stream) = self.stream.lock() else {
            return;
        };
        if let Disposition::Exited { integrity } = &mut stream.disposition {
            *integrity = self.terminated_integrity();
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
        stream.agent_backlog.clear();
        stream.agent_backlog_bytes = 0;
        stream.agent_backlog_frames = 0;
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

fn enqueue_agent(stream: &mut StreamState, event: SessionEvent) {
    let permission = matches!(&event, SessionEvent::PermissionRequest { .. });
    if stream.attached.is_some() && (!permission || stream.typed_permissions) {
        push_bounded_agent(
            &mut stream.pending,
            &mut stream.pending_bytes,
            &mut stream.pending_frames,
            event,
        );
    } else {
        push_bounded_agent(
            &mut stream.agent_backlog,
            &mut stream.agent_backlog_bytes,
            &mut stream.agent_backlog_frames,
            event,
        );
    }
}

fn remove_permission_from_queue(
    queue: &mut VecDeque<PendingItem>,
    bytes_total: &mut usize,
    frames_total: &mut u64,
    tool_call_id: &str,
) {
    let mut retained = VecDeque::with_capacity(queue.len());
    while let Some(item) = queue.pop_front() {
        let remove = matches!(
            &item,
            PendingItem::Agent {
                event: SessionEvent::PermissionRequest { tool_call_id: current, .. },
                ..
            } if current == tool_call_id
        );
        if remove {
            if let PendingItem::Agent { bytes, .. } = &item {
                *bytes_total = bytes_total.saturating_sub(*bytes);
                *frames_total = frames_total.saturating_sub(1);
            }
        } else {
            retained.push_back(item);
        }
    }
    *queue = retained;
}

fn push_bounded_agent(
    queue: &mut VecDeque<PendingItem>,
    bytes_total: &mut usize,
    frames_total: &mut u64,
    event: SessionEvent,
) {
    let bytes = serde_json::to_vec(&event)
        .map(|value| value.len())
        .unwrap_or(0);
    queue.push_back(PendingItem::Agent { event, bytes });
    *bytes_total = bytes_total.saturating_add(bytes);
    *frames_total = frames_total.saturating_add(1);
    if *bytes_total <= PENDING_OUTPUT_BUDGET_BYTES && *frames_total <= PENDING_OUTPUT_BUDGET_FRAMES
    {
        return;
    }
    // ACP has no terminal screen to use as a replacement snapshot. Keep the
    // newest structured event and bound both the attached queue and the
    // detached backlog with the same limits.
    let newest = queue.pop_back();
    queue.clear();
    *bytes_total = 0;
    *frames_total = 0;
    if let Some(item) = newest {
        if let PendingItem::Agent { bytes, .. } = &item {
            *bytes_total = *bytes;
            *frames_total = 1;
        }
        queue.push_back(item);
    }
}

fn move_agent_pending_to_backlog(stream: &mut StreamState) {
    while let Some(item) = stream.pending.pop_front() {
        if let PendingItem::Agent { bytes, .. } = &item {
            stream.agent_backlog_bytes = stream.agent_backlog_bytes.saturating_add(*bytes);
            stream.agent_backlog_frames = stream.agent_backlog_frames.saturating_add(1);
            stream.agent_backlog.push_back(item);
        }
    }
    stream.pending_bytes = 0;
    stream.pending_frames = 0;
    if stream.agent_backlog_bytes > PENDING_OUTPUT_BUDGET_BYTES
        || stream.agent_backlog_frames > PENDING_OUTPUT_BUDGET_FRAMES
    {
        let newest = stream.agent_backlog.pop_back();
        stream.agent_backlog.clear();
        stream.agent_backlog_bytes = 0;
        stream.agent_backlog_frames = 0;
        if let Some(item) = newest {
            if let PendingItem::Agent { bytes, .. } = &item {
                stream.agent_backlog_bytes = *bytes;
                stream.agent_backlog_frames = 1;
            }
            stream.agent_backlog.push_back(item);
        }
    }
}

/// The registry owns this value; the reader and command paths keep Arcs to
/// the endpoints/runtime they need after releasing the map lock.
struct PtySession {
    metadata: Session,
    owner: OwnerId,
    process_job: JobObject,
    master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    killer: Box<dyn SessionKiller>,
    /// This is separate from the stdout reader: stderr must never be able to
    /// fill its pipe and stop the ACP child from producing responses.
    stderr_handle: Option<JoinHandle<()>>,
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

struct SpawnedSession {
    process_job: JobObject,
    master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    killer: Box<dyn SessionKiller>,
    child: Box<dyn WaitableChild>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader: Box<dyn Read + Send>,
    /// ACP supplies a structured decoder. Terminal sessions use the shared
    /// byte coalescer, which is constructed by `start_spawned_session`.
    reader_dispatch: Option<Box<dyn ReaderDispatch>>,
    stderr: Option<Box<dyn StderrSource>>,
    permission_broker: Option<Arc<acp_client::PermissionBroker>>,
}

struct PtyKiller {
    inner: Box<dyn ChildKiller + Send + Sync>,
}

impl SessionKiller for PtyKiller {
    fn kill(&mut self) {
        let _ = self.inner.kill();
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            inner: self.inner.clone_killer(),
        })
    }
}

struct PtyWaitableChild {
    child: Box<dyn Child + Send + Sync>,
}

impl WaitableChild for PtyWaitableChild {
    fn wait(mut self: Box<Self>) -> Option<u32> {
        self.child.wait().ok().map(|status| status.exit_code())
    }
}

struct TerminalReaderDispatch {
    tx: Option<mpsc::Sender<Vec<u8>>>,
}

impl ReaderDispatch for TerminalReaderDispatch {
    fn feed(&mut self, bytes: &[u8], _runtime: &Arc<SessionRuntime>) -> Result<(), String> {
        self.tx
            .as_ref()
            .ok_or_else(|| "terminal coalescer is unavailable".to_string())?
            .send(bytes.to_vec())
            .map_err(|_| "terminal coalescer is unavailable".to_string())
    }

    fn finish(&mut self, _runtime: &Arc<SessionRuntime>) {
        self.tx.take();
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
            integrity: session.runtime.terminated_integrity(),
        };
        return metadata;
    }
    let Ok(stream) = session.runtime.lock_stream() else {
        metadata.state = SessionState::Ended {
            generation: session.runtime.generation(),
            code: None,
            integrity: session.runtime.terminated_integrity(),
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
        Disposition::Exited { integrity } => SessionState::Ended {
            generation: stream.generation,
            code: stream.exit_code,
            integrity,
        },
        Disposition::Recovered { integrity } => SessionState::Recovered {
            generation: stream.generation,
            integrity,
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

type TransitionSink = Arc<dyn Fn(OwnerId) + Send + Sync>;

#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<Mutex<HashMap<String, RegistryEntry>>>,
    paths: RuntimePaths,
    journal: Option<Arc<Journal>>,
    transition_sink: Arc<Mutex<Option<TransitionSink>>>,
}

impl SessionRegistry {
    pub fn new(paths: RuntimePaths, journal: Option<Arc<Journal>>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            paths,
            journal,
            transition_sink: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn set_transition_sink(&self, sink: TransitionSink) {
        if let Ok(mut current) = self.transition_sink.lock() {
            *current = Some(sink);
        }
    }

    fn notify_transition(&self, owner: &OwnerId) {
        let sink = self
            .transition_sink
            .lock()
            .ok()
            .and_then(|current| current.clone());
        if let Some(sink) = sink {
            sink(owner.clone());
        }
    }

    pub(crate) fn state_snapshots(&self, owner: &OwnerId) -> Vec<SessionStateSnapshot> {
        let mut sessions = self
            .inner
            .lock()
            .map(|map| {
                map.values()
                    .filter(|entry| entry.owner() == owner)
                    .map(RegistryEntry::to_session)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let live_ids = sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<std::collections::HashSet<_>>();
        let owner_token = owner.session_token();
        if let Some(journal) = &self.journal {
            if let Ok(rows) = journal.list() {
                sessions.extend(rows.into_iter().filter_map(|row| {
                    if live_ids.contains(&row.id) {
                        return None;
                    }
                    let session_token = row.id.split('.').nth(1);
                    (row.owner == owner.user && session_token == Some(owner_token.as_str()))
                        .then(|| row.to_session())
                }));
            }
        }
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        sessions
            .into_iter()
            .map(|session| SessionStateSnapshot {
                id: session.id,
                title: session.title,
                state: session.state,
                elapsed_ms: session.elapsed_ms,
            })
            .collect()
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

    pub fn journal_usage(&self) -> Result<crate::journal::JournalUsage, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .usage()
            .map_err(Into::into)
    }

    pub fn journal_retention_get(&self) -> Result<JournalRetention, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .retention_get()
            .map_err(Into::into)
    }

    pub fn journal_retention_set(
        &self,
        patch: RetentionPatch,
    ) -> Result<JournalRetention, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .retention_set(patch)
            .map_err(Into::into)
    }

    pub fn delete_session(&self, session_id: &str, owner: &OwnerId) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let transcript_in_registry = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            match map.get(session_id) {
                Some(entry) => {
                    check_owner(entry, owner)?;
                    if entry.as_live().is_some() {
                        return Err(WireError::new(
                            ErrorCode::InvalidRequest,
                            "Close the session before deleting it.",
                        ));
                    }
                    true
                }
                None => false,
            }
        };
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        if !transcript_in_registry {
            let record = journal
                .list()
                .map_err(WireError::from)?
                .into_iter()
                .find(|record| record.id == session_id)
                .ok_or_else(not_found)?;
            let session_owner = owner_from_session_id(session_id, &record.owner)?;
            if &session_owner != owner {
                return Err(unauthorized());
            }
        }
        journal
            .delete_session(session_id)
            .map_err(WireError::from)?;
        if transcript_in_registry {
            if let Ok(mut map) = self.inner.lock() {
                map.remove(session_id);
            }
            journal.unpin(session_id);
        }
        self.notify_transition(owner);
        Ok(())
    }

    pub fn create(
        &self,
        state: &Arc<ServerState>,
        owner: &OwnerId,
        workspace_id: Option<String>,
        kind: SessionKind,
        command: Option<PtyCommand>,
    ) -> Result<Session, WireError> {
        let unique = format!("{:08x}", SESSION_COUNTER.fetch_add(1, Ordering::Relaxed));
        let id = compose_session_id(&owner.session_token(), &unique)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let metadata = Session {
            id: id.clone(),
            workspace_id,
            kind: kind.clone(),
            title: match kind {
                SessionKind::Terminal => "Terminal",
                SessionKind::Acp => "Agent",
            }
            .to_string(),
            state: SessionState::Live { generation: 1 },
            elapsed_ms: Some(0),
        };
        let command = match command {
            Some(command) => command,
            None if metadata.kind == SessionKind::Acp => acp_client::resolve_command(&self.paths)?,
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
        typed_permissions: bool,
    ) -> Result<(), WireError> {
        let runtime = match self.runtime_for_owner(session_id, owner) {
            Ok(runtime) => runtime,
            Err(error) if error.code == ErrorCode::SessionNotFound => {
                self.hydrate_transcript(session_id, from_cursor, owner)?
            }
            Err(error) => return Err(error),
        };
        let generation = runtime.try_attach(from_cursor, conn, typed_permissions)?;
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

    pub fn permission_respond(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: PermissionOutcome,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        if request_id.is_empty() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Permission request id is required.",
            ));
        }
        let runtime = self.runtime_for_owner(session_id, owner)?;
        if runtime.attached_conn_id() != Some(conn.id) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session is not attached to this client.",
            ));
        }
        let broker = runtime.permission_broker().ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                "Session has no live ACP permission broker.",
            )
        })?;
        broker.respond(request_id, outcome).map_err(|error| {
            let code = match error {
                acp_client::PermissionResponseError::NotFound => ErrorCode::InvalidRequest,
                acp_client::PermissionResponseError::InvalidRequest(_) => ErrorCode::InvalidRequest,
                acp_client::PermissionResponseError::Io(_) => ErrorCode::Io,
            };
            WireError::new(code, error.to_string())
        })
    }

    /// Drop every subscription this connection holds. The processes stay.
    pub fn detach_conn(&self, conn: &ConnHandle) {
        let ids = conn.take_attached_ids();
        for id in ids {
            self.detach_runtime(&id, conn.id);
            self.drop_transcript_if_idle(&id);
        }
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
                if let Some(session) = entry.as_live() {
                    session
                        .runtime
                        .transition_ready
                        .store(false, Ordering::Release);
                }
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
                self.notify_transition(owner);
                Ok(true)
            }
            Some(RegistryEntry::Transcript(_)) => {
                if let Some(journal) = &self.journal {
                    journal.try_mark_closed(session_id);
                    journal.unpin(session_id);
                }
                self.notify_transition(owner);
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
                        self.notify_transition(owner);
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
        killer.kill();
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
            (Arc::clone(&session.runtime), session.master.clone())
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
            // ACP sessions are structured streams and deliberately have no
            // terminal dimensions. Resize is already kind-agnostic at the
            // RPC seam; it is simply a no-op for this transport.
            return Ok(());
        };
        let (previous_cols, previous_rows) = screen.dimensions();
        screen.resize(cols.max(1), rows.max(1));
        let Some(master) = master else {
            return Ok(());
        };
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
    if metadata.kind == SessionKind::Acp {
        return start_spawned_session(
            state,
            registry,
            metadata,
            owner,
            acp_client::spawn_process(state, command)?,
        );
    }

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

    let spawned = SpawnedSession {
        process_job,
        master: Some(Arc::new(Mutex::new(pair.master))),
        killer: Box::new(PtyKiller { inner: killer }),
        child: Box::new(PtyWaitableChild { child }),
        writer: Arc::new(Mutex::new(writer)),
        reader,
        reader_dispatch: None,
        stderr: None,
        permission_broker: None,
    };
    start_spawned_session(state, registry, metadata, owner, spawned)
}

fn start_spawned_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    spawned: SpawnedSession,
) -> Result<(), WireError> {
    let SpawnedSession {
        process_job,
        master,
        killer,
        child,
        writer,
        reader,
        reader_dispatch,
        stderr,
        permission_broker,
    } = spawned;
    let runtime = if metadata.kind == SessionKind::Acp {
        SessionRuntime::for_acp(
            metadata.id.clone(),
            registry.journal.clone(),
            permission_broker.expect("ACP sessions have a permission broker"),
        )
    } else {
        Arc::new(SessionRuntime::with_journal(
            metadata.id.clone(),
            registry.journal.clone(),
        ))
    };
    let exited = Arc::new(AtomicBool::new(false));
    // Register before the reader thread starts: ConPTY's startup DSR can be
    // read within milliseconds, and the reply path needs the writer.
    if metadata.kind == SessionKind::Terminal {
        runtime
            .pty_writer
            .set(Arc::clone(&writer))
            .ok()
            .expect("pty writer registered exactly once");
    }
    let id = metadata.id.clone();
    let wait_runtime = Arc::clone(&runtime);
    let wait_registry = registry.clone();
    let wait_owner = owner.clone();
    let child_wait = std::thread::Builder::new()
        .name(format!("session-wait-{id}"))
        .spawn(move || {
            let code = child.wait();
            wait_runtime
                .child_reaped
                .store(code.is_some(), Ordering::Release);
            wait_runtime.mark_exited(code);
            if wait_runtime.should_publish_exit_transition() {
                wait_registry.notify_transition(&wait_owner);
            }
            code
        })
        .ok();
    let monitor_runtime = Arc::clone(&runtime);
    let monitor_registry = registry.clone();
    let monitor_owner = owner.clone();
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
                if monitor_runtime.mark_silent_if_due(Instant::now()).is_some()
                    && monitor_runtime.transition_ready()
                {
                    monitor_registry.notify_transition(&monitor_owner);
                }
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
        stderr_handle: None,
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

    let (coalesce_handle, reader_dispatch) = match reader_dispatch {
        Some(dispatch) => (None, dispatch),
        None => {
            let (coalesce_tx, coalesce_rx) = mpsc::channel::<Vec<u8>>();
            let coalesce_runtime = Arc::clone(&runtime);
            let coalesce_registry = registry.clone();
            let coalesce_owner = owner.clone();
            let coalesce_handle = match std::thread::Builder::new()
                .name(format!("session-coalesce-{id}"))
                .spawn(move || {
                    coalesce_loop(
                        coalesce_rx,
                        coalesce_runtime,
                        coalesce_registry,
                        coalesce_owner,
                    )
                }) {
                Ok(handle) => Some(handle),
                Err(_) => {
                    let _ = registry.close(&id, &owner);
                    return Err(WireError::new(
                        ErrorCode::Internal,
                        "Could not start the terminal reader.",
                    ));
                }
            };
            (
                coalesce_handle,
                Box::new(TerminalReaderDispatch {
                    tx: Some(coalesce_tx),
                }) as Box<dyn ReaderDispatch>,
            )
        }
    };

    let stderr_handle = stderr.and_then(|source| match source.spawn(Arc::clone(&runtime)) {
        Ok(handle) => Some(handle),
        Err(error) => {
            runtime.publish_agent_event(
                SessionEvent::AgentError {
                    message: format!("Could not drain agent stderr: {error}"),
                },
                None,
            );
            None
        }
    });
    if let Ok(mut map) = registry.inner.lock() {
        if let Some(session) = map.get_mut(&id).and_then(RegistryEntry::as_live_mut) {
            session.coalesce_handle = coalesce_handle;
            session.stderr_handle = stderr_handle;
        }
    }

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
                reader_dispatch,
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
    let mut orphaned_coalesce = None;
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
    // A child can die before the create transition is published. Mark that
    // exit as covered by this first snapshot; the second check catches an
    // exit racing the publication without allowing the wait thread to report
    // the same transition twice.
    if runtime.process_exited() {
        runtime.exit_transition_sent.store(true, Ordering::Release);
    }
    registry.notify_transition(&owner);
    runtime.transition_ready.store(true, Ordering::Release);
    if runtime.process_exited() && runtime.should_publish_exit_transition() {
        registry.notify_transition(&owner);
    }
    Ok(())
}

fn reader_loop(
    registry: SessionRegistry,
    state: Weak<ServerState>,
    id: String,
    mut reader: Box<dyn Read + Send>,
    runtime: Arc<SessionRuntime>,
    mut reader_dispatch: Box<dyn ReaderDispatch>,
) {
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if let Err(error) = reader_dispatch.feed(&buf[..n], &runtime) {
                    runtime.record_output_loss();
                    eprintln!("session {id} stopped reading child output: {error}");
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
    reader_dispatch.finish(&runtime);

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

fn coalesce_loop(
    rx: mpsc::Receiver<Vec<u8>>,
    runtime: Arc<SessionRuntime>,
    registry: SessionRegistry,
    owner: OwnerId,
) {
    let mut pending = Vec::new();
    loop {
        let received = if pending.is_empty() {
            rx.recv().ok()
        } else {
            match rx.recv_timeout(COALESCE_FLUSH) {
                Ok(bytes) => Some(bytes),
                Err(RecvTimeoutError::Timeout) => {
                    flush_coalesced(&mut pending, &runtime, &registry, &owner);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => None,
            }
        };
        match received {
            Some(bytes) => {
                pending.extend_from_slice(&bytes);
                if pending.len() >= COALESCE_MAX_BYTES || pending.len() == COALESCE_EAGER_BYTES {
                    flush_coalesced(&mut pending, &runtime, &registry, &owner);
                }
            }
            None => {
                flush_coalesced(&mut pending, &runtime, &registry, &owner);
                break;
            }
        }
    }
}

fn flush_coalesced(
    pending: &mut Vec<u8>,
    runtime: &SessionRuntime,
    registry: &SessionRegistry,
    owner: &OwnerId,
) {
    if pending.is_empty() {
        return;
    }
    let data = String::from_utf8_lossy(pending).into_owned();
    pending.clear();
    if runtime.publish_output(&data) && runtime.transition_ready() {
        registry.notify_transition(owner);
    }
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
    let stderr = session.stderr_handle.take();
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
    bounded_join(stderr);
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
        stderr_handle,
        runtime,
        exited: _,
        owner: _,
        metadata: _,
        preserve_on_exit: _,
    } = session;

    // 1) Kill first. The killer is separate so this cannot race with wait().
    killer.kill();
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
    bounded_join(stderr_handle);
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

fn not_found() -> WireError {
    WireError::new(ErrorCode::SessionNotFound, "No session with that id.")
}

fn journal_unavailable() -> WireError {
    WireError::new(
        ErrorCode::Journal,
        "The conversation journal is unavailable.",
    )
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
        let generation = runtime.try_attach(None, conn, false).expect("attach");
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
            runtime
                .try_attach(None, &conn, false)
                .expect("attach under flood");
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
    fn attach_rejects_a_second_connection() {
        let runtime = SessionRuntime::new();
        let first = ConnHandle::new(1);
        let second = ConnHandle::new(2);
        runtime.try_attach(None, &first, false).expect("first");
        let err = runtime.try_attach(None, &second, false).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRequest);
        assert!(err.message.contains("already attached"));
        assert_eq!(runtime.attached_conn_id(), Some(1));
    }

    #[test]
    fn same_connection_can_reattach() {
        let runtime = SessionRuntime::new();
        let conn = ConnHandle::new(7);
        runtime.try_attach(None, &conn, false).expect("first");
        runtime
            .try_attach(
                Some(Cursor {
                    generation: 1,
                    seq: 0,
                }),
                &conn,
                false,
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
                false,
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::SessionGenerationMismatch);
    }

    #[test]
    fn detach_clears_only_this_connection() {
        let runtime = SessionRuntime::new();
        let conn = ConnHandle::new(3);
        runtime.try_attach(None, &conn, false).expect("attach");
        runtime.detach_if_conn(3);
        assert_eq!(runtime.attached_conn_id(), None);
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
