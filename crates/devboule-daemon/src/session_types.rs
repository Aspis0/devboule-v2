use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use portable_pty::CommandBuilder;

use devboule_protocol::{
    OwnerId, Session, SessionEvent, SessionEventEnvelope, TranscriptIntegrity,
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

pub(super) struct Attachment {
    pub(super) conn_id: u64,
    pub(super) outbound: Arc<ConnOut>,
}

/// One item queued for the attached viewer, in wire order.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// The single attached viewer, if any.
    pub(super) attached: Option<Attachment>,
    /// Unsent items for the attachment, in wire order. Bounded: when the
    /// Output extent exceeds the budget, the whole queue is replaced by one
    /// fresh snapshot.
    pub(super) pending: VecDeque<PendingItem>,
    /// Byte extent of `pending`'s Output items (snapshots are not counted;
    /// a replacement resets this to zero).
    pub(super) pending_bytes: usize,
    /// Frame count of `pending`'s Output items.
    pub(super) pending_frames: u64,
    /// Structured ACP events observed before an attachment exists. Unlike a
    /// terminal, a headless live session has no screen snapshot that can
    /// represent these events for a later attach.
    pub(super) agent_backlog: VecDeque<PendingItem>,
    pub(super) agent_backlog_bytes: usize,
    pub(super) agent_backlog_frames: u64,
    /// Whether the attached client negotiated typed permission prompts.
    /// Detached sessions keep permission requests in `agent_backlog` until a
    /// capable client attaches.
    pub(super) typed_permissions: bool,
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
    pub(super) pending_silences: VecDeque<u64>,
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
    pub(super) agent_backlog_after_replay: bool,
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
    Live(super::PtySession),
    Transcript(TranscriptSession),
}

impl RegistryEntry {
    pub(super) fn owner(&self) -> &OwnerId {
        match self {
            Self::Live(session) => &session.owner,
            Self::Transcript(session) => &session.owner,
        }
    }

    pub(super) fn runtime(&self) -> Arc<super::SessionRuntime> {
        match self {
            Self::Live(session) => Arc::clone(&session.runtime),
            Self::Transcript(session) => Arc::clone(&session.runtime),
        }
    }

    pub(super) fn to_session(&self) -> Session {
        match self {
            Self::Live(session) => super::live_session_view(session),
            Self::Transcript(session) => session.metadata.clone(),
        }
    }

    pub(super) fn as_live(&self) -> Option<&super::PtySession> {
        match self {
            Self::Live(session) => Some(session),
            Self::Transcript(_) => None,
        }
    }

    pub(super) fn as_live_mut(&mut self) -> Option<&mut super::PtySession> {
        match self {
            Self::Live(session) => Some(session),
            Self::Transcript(_) => None,
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
