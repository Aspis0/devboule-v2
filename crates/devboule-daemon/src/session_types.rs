use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use portable_pty::CommandBuilder;

use devboule_protocol::{
    OwnerId, Session, SessionEvent, SessionEventEnvelope, SessionOrigin, TranscriptIntegrity,
};

use crate::agent_report::AgentReportState;
use crate::outbound::ConnOut;
use crate::screen::{Screen, ScreenSnapshot};

/// Everything needed to spawn one PTY child, independent of the session kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub provider_id: Option<String>,
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
            provider_id: None,
        }
    }

    pub fn with_provider_id(mut self, id: impl Into<String>) -> Self {
        self.provider_id = Some(id.into());
        self
    }

    pub(super) fn to_command_builder(&self) -> CommandBuilder {
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
pub(super) struct SequencedChunk {
    pub(super) seq: u64,
    pub(super) data: Vec<u8>,
}

/// Transcript replay buffer for a recovered session.
///
/// A recovered session loads its journal records here once at hydration and
/// serves cursor-based replays from the union of these chunks and a fresh
/// journal read. It is NOT the live screen mechanism: a live attach receives
/// a screen snapshot, never this buffer.
#[derive(Debug, Default)]
pub(super) struct Scrollback {
    pub(super) chunks: VecDeque<SequencedChunk>,
}

impl Scrollback {
    pub(super) fn push(&mut self, seq: u64, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.chunks.push_back(SequencedChunk {
            seq,
            data: data.to_vec(),
        });
    }

    pub(super) fn needs_journal_replay(&self, from_cursor: Option<u64>, next_seq: u64) -> bool {
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

    pub(super) fn replay_after_with_journal(
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AttachmentKey {
    pub(crate) conn_id: u64,
    pub(crate) subscription_id: u64,
}

pub(super) struct Attachment {
    pub(super) outbound: Arc<ConnOut>,
    pub(super) typed_permissions: bool,
    pub(super) pending: VecDeque<PendingItem>,
    pub(super) pending_bytes: usize,
    pub(super) pending_frames: u64,
    pub(super) pending_silences: VecDeque<u64>,
}

/// One item queued for an observer, in wire order.
///
/// `SessionEvent` is held inline, and the creation card and the finish
/// artifacts grew it: the largest variant is now a few hundred bytes. Boxing it
/// is the right fix and a change of its own (every construction and every
/// replay path touches it); the queue this sits in is bounded per observer, so
/// the cost is fixed rather than open-ended. Recorded here so the next pass
/// finds it rather than re-deriving it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub(super) enum PendingItem {
    /// Screen state at `as_of_seq`. Always the first item of an attachment;
    /// also the replacement emitted when a slow viewer's queue exceeds the
    /// budget. The owned grid is rendered to ANSI outside every lock.
    Snapshot {
        as_of_seq: u64,
        screen: ScreenSnapshot,
    },
    /// An applied output chunk, forwarded verbatim.
    Output { seq: u64, data: String },
    /// A structured ACP event. It travels through the same bounded live
    /// attachment queue as terminal output, but has no screen representation.
    Agent {
        /// Journal sequence of the envelope that produced this event. None
        /// is used for daemon-local events such as stderr and permission
        /// resolution, which have no replay row to de-duplicate against.
        seq: Option<u64>,
        event: SessionEvent,
        bytes: usize,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Disposition {
    Running,
    Silent,
    Exited { integrity: TranscriptIntegrity },
    Recovered { integrity: TranscriptIntegrity },
}

pub(crate) struct StreamState {
    /// Session-wide monotonic output counter. Labels journal records and
    /// live events; it is NOT a replay cursor and never advances because a
    /// frame was written to a pipe.
    pub(super) next_seq: u64,
    /// Greatest sequence whose complete chunk has been applied to the
    /// emulator. This is the snapshot boundary (`as_of_seq`).
    pub(super) last_applied_seq: u64,
    pub(super) generation: u64,
    /// The headless emulator. `None` for a recovered transcript, which has
    /// no live process and serves cursor-based journal replays instead.
    pub(super) screen: Option<Screen>,
    /// Recovered transcripts have no screen; live ACP sessions also have no
    /// screen, so this explicit bit keeps those two contracts distinct.
    pub(super) transcript: bool,
    /// The single subscription allowed to resize the session, if any.
    pub(super) resize_owner: Option<AttachmentKey>,
    /// All live observers. Their queues and notification handles are
    /// independent so one slow or departing view cannot replace another.
    pub(super) observers: HashMap<AttachmentKey, Attachment>,
    /// Structured ACP events observed before an attachment exists. Unlike a
    /// terminal, a headless live session has no screen snapshot that can
    /// represent these events for a later attach.
    pub(super) agent_backlog: VecDeque<PendingItem>,
    pub(super) agent_backlog_bytes: usize,
    pub(super) agent_backlog_frames: u64,
    /// Transcript replay buffer. Unused by live sessions, which never
    /// replay bytes to synchronise a screen.
    pub(super) scrollback: Scrollback,
    /// Reader has seen EOF. Further publish_output is dropped.
    pub(super) output_closed: bool,
    /// Child::wait returned. Output may still be in the ConPTY buffer.
    pub(super) process_exited: bool,
    pub(super) exit_code: Option<u32>,
    pub(super) last_publish: Option<Instant>,
    pub(super) exit_at: Option<Instant>,
    pub(super) disposition: Disposition,
    /// Last accepted hook report per source. Seq is checked under the
    /// stream lock so two concurrent announcements cannot both apply.
    pub(super) agent_reports: AgentReportState,
    /// Journaled agent reports for a recovered transcript, keyed by the
    /// stream sequence so attach replay can interleave them with output.
    pub(super) transcript_agent_reports: BTreeMap<u64, SessionEvent>,
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

pub(super) struct PullState {
    pub(super) runtime: Arc<super::SessionRuntime>,
    pub(super) attachment_key: AttachmentKey,
    /// Whether this pull follows the transcript replay contract (recovered
    /// session) or the live snapshot contract.
    pub(super) transcript: bool,
    /// Transcript-only: last replay sequence the client accounted for.
    /// Live terminal sessions keep no replay cursor; their screen boundary is
    /// the snapshot's `as_of_seq`. Live headless agents use `agent_replay`.
    pub(super) transcript_cursor: Option<u64>,
    /// A live headless agent attach replays the durable prefix before its
    /// watermark. This is deliberately per-connection state rather than
    /// `StreamState.pending`, so a long journal remains page-bounded.
    pub(super) agent_replay: Option<AgentReplay>,
    pub(super) exit_sent: bool,
    pub(super) journal_degraded_sent: bool,
    pub(super) generation: u64,
    pub(super) attachment_generation: u64,
}

pub(super) struct AgentReplay {
    pub(super) from_seq: u64,
    pub(super) cursor: u64,
    pub(super) watermark: u64,
    pub(super) generation: u64,
    pub(super) pending: VecDeque<(u64, SessionEvent)>,
    pub(super) replayed_seqs: HashSet<u64>,
    pub(super) claude_view: Option<crate::claude_view::ClaudeView>,
    pub(super) codex_view: Option<crate::codex_view::CodexView>,
    pub(super) is_pi: bool,
    pub(super) is_codex: bool,
    pub(super) manifest_emitted: bool,
    /// Number of times a page boundary has extended the replay watermark to
    /// catch live journal rows published during the replay.
    pub(super) catch_up_extensions: u8,
    pub(super) durable_done: bool,
    pub(super) journal_lagged: bool,
    /// Stop extending after the bounded catch-up policy gives up. The
    /// replay still drains in order, then emits JournalDegraded before live
    /// items so a permanent producer outrunning SQLite is never a spin loop.
    pub(super) force_finish: bool,
}

#[derive(Debug)]
pub(crate) struct PendingEvent {
    pub(crate) session_id: String,
    pub(crate) subscription_id: u64,
    pub(crate) attachment_generation: u64,
    pub(crate) envelope: SessionEventEnvelope,
    /// Transcript-only: journal seq of this envelope, including ACP views
    /// that do not carry seq on the event itself.
    pub(crate) transcript_seq: Option<u64>,
}

pub(super) struct TranscriptSession {
    pub(super) metadata: Session,
    pub(super) owner: OwnerId,
    pub(super) runtime: Arc<super::SessionRuntime>,
}

pub(super) enum RegistryEntry {
    Live(Box<super::PtySession>),
    /// The child is spawned, the reader may even be running, but the
    /// profile's delivery has not landed yet — the session exists for the
    /// daemon's own teardown paths and for nobody else (the re-audit's
    /// P2-1). A `Configuring` entry is invisible to every roster read and
    /// refused by every id-addressed peer call through the one door such a
    /// call resolves its id through (`peer_entry`/`peer_entry_mut` in
    /// `session.rs` — a `Configuring` entry answers `SessionNotFound`
    /// there, so a new peer path cannot forget the window by resolving
    /// through it), while a child that is live but not yet configured
    /// cannot be found, prompted, or closed from the outside; the
    /// delivery's own refusal path closes it by id because teardown is
    /// exactly what the variant still permits.
    Configuring(Box<super::PtySession>),
    Transcript(Box<TranscriptSession>),
}

impl RegistryEntry {
    /// Whether this entry is a session its peers must not see yet.
    pub(super) fn is_configuring(&self) -> bool {
        matches!(self, Self::Configuring(_))
    }

    /// The entry's child process slot — `Live` and `Configuring` both hold
    /// one; `Transcript` does not. This answers the **daemon's** question,
    /// *"is there a child here?"* — the resume guard, handle
    /// storage, EOF reaping, teardown — and it deliberately reaches through
    /// the delivery window: a `Configuring` child is exactly the child a
    /// refused delivery must tear down. It never answers the peer's
    /// question, *"does this session exist yet?"* — peers ask
    /// [`Self::as_peer_visible`], which stops at the window.
    pub(super) fn as_child_process(&self) -> Option<&super::PtySession> {
        match self {
            Self::Live(session) | Self::Configuring(session) => Some(session),
            Self::Transcript(_) => None,
        }
    }

    /// The mutable half of [`Self::as_child_process`].
    pub(super) fn as_child_process_mut(&mut self) -> Option<&mut super::PtySession> {
        match self {
            Self::Live(session) | Self::Configuring(session) => Some(session),
            Self::Transcript(_) => None,
        }
    }

    pub(super) fn owner(&self) -> &OwnerId {
        match self {
            Self::Live(session) | Self::Configuring(session) => &session.owner,
            Self::Transcript(session) => &session.owner,
        }
    }

    /// The wire metadata of this entry, borrowed rather than cloned: the
    /// ownership, origin and kind checks all read one field of it.
    pub(super) fn metadata(&self) -> &Session {
        match self {
            Self::Live(session) | Self::Configuring(session) => &session.metadata,
            Self::Transcript(session) => &session.metadata,
        }
    }

    /// Where this session came from (§8 R2). Read by the peer gate — the
    /// `Daemon` role's whole scope — and never written after the create that
    /// made the row.
    pub(super) fn origin(&self) -> SessionOrigin {
        self.metadata().origin.clone()
    }

    pub(super) fn runtime(&self) -> Arc<super::SessionRuntime> {
        match self {
            Self::Live(session) | Self::Configuring(session) => Arc::clone(&session.runtime),
            Self::Transcript(session) => Arc::clone(&session.runtime),
        }
    }

    /// The wire view. Callers that serve rosters filter [`Self::Configuring`]
    /// out before reaching this; the arm exists so the internal readers of a
    /// session's own metadata (a finish report, a resume that just inserted
    /// the entry) never need to care about the window.
    pub(super) fn to_session(&self) -> Session {
        match self {
            Self::Live(session) | Self::Configuring(session) => super::live_session_view(session),
            Self::Transcript(session) => session.metadata.clone(),
        }
    }

    /// The session a **peer** sees — the answer to *"does this session exist
    /// for the outside yet?"*: a configuring session does not exist yet, and
    /// a transcript-only entry holds no child. The daemon's own question,
    /// *"is there a child process here?"*, is [`Self::as_child_process`],
    /// which reaches through the delivery window; the two are deliberately
    /// different predicates over the same enum, and the names are not
    /// interchangeable.
    pub(super) fn as_peer_visible(&self) -> Option<&super::PtySession> {
        match self {
            Self::Live(session) => Some(session),
            Self::Configuring(_) | Self::Transcript(_) => None,
        }
    }

    /// The mutable half of [`Self::as_peer_visible`].
    pub(super) fn as_peer_visible_mut(&mut self) -> Option<&mut super::PtySession> {
        match self {
            Self::Live(session) => Some(session),
            Self::Configuring(_) | Self::Transcript(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    data: "two".to_string(),
                },
                SessionEvent::Output {
                    seq: 3,
                    data: "three".to_string(),
                },
            ]
        );
        assert_eq!(scrollback.replay_after(None).len(), 3);
    }
}
