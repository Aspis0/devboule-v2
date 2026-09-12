//! Session wire types shared by the daemon and its clients. Changes to these
//! types are protocol changes and must be reflected in the negotiated dialect.

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, ErrorDetails, WireError};
use crate::messages::PeerRole;

/// Identifies one live observer of a session. It is scoped by the daemon
/// connection and must be retained by the client until that observer detaches.
pub type SubscriptionId = u64;

/// M2 implements Terminal; agent transports are additive serialized variants
/// without changing the command signatures or existing wire values.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Terminal,
    Acp,
    Claude,
    Pi,
    Codex,
}

impl SessionKind {
    /// ACP, Claude stream-json, Pi RPC, and Codex app-server are live agent
    /// sessions.
    pub fn is_agent(&self) -> bool {
        match self {
            Self::Terminal => false,
            Self::Acp | Self::Claude | Self::Pi | Self::Codex => true,
        }
    }
}

/// Where a session came from. Local is the person at this machine; Peer is a
/// paired device, identified by its device id and the role it was paired as.
///
/// Set once, at the create that made the session, and read-only afterwards:
/// a session's origin is a fact about who asked for it, and every later
/// decision (a `Daemon` peer's ownership scope, a permission card's
/// provenance line, an attachment budget) reads it rather than re-deriving it
/// (`DESIGN-remote-agents.md` §8 R2, §8b A3/A14). Every pre-origin row is
/// `local`, which is what the field's `Default` is.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionOrigin {
    pub kind: SessionOriginKind,
    /// The paired device that asked for this session. `None` for `Local`, and
    /// for a peer row that predates the column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// The role that device was paired as. `None` for `Local`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<PeerRole>,
}

impl SessionOrigin {
    /// The origin of a session the person at this machine created.
    pub fn local() -> Self {
        Self {
            kind: SessionOriginKind::Local,
            device_id: None,
            role: None,
        }
    }

    /// The origin of a session a paired device created.
    pub fn peer(device_id: impl Into<String>, role: PeerRole) -> Self {
        Self {
            kind: SessionOriginKind::Peer,
            device_id: Some(device_id.into()),
            role: Some(role),
        }
    }

    /// The origin of a session nobody measured.
    ///
    /// Deliberately **not** `local`: `local` is a fact the registry installs
    /// for a session this machine created, so it is only ever *measured*. This
    /// is what a provider client writes as a placeholder before the daemon
    /// stamps the session's stored origin on the way out, and what
    /// `SessionRuntime::origin()` falls back to before the registry has
    /// installed one — the two places that used to invent `local` and so made
    /// "this machine's own" the answer to a question nobody had asked.
    pub fn unknown() -> Self {
        Self {
            kind: SessionOriginKind::Unknown,
            device_id: None,
            role: None,
        }
    }

    pub fn is_local(&self) -> bool {
        self.kind == SessionOriginKind::Local
    }
}

/// `"local"`, `"peer"` or `"unknown"`, lowercase on the wire.
///
/// `Unknown` is what a *stored* row reads as when its origin columns say
/// neither of the two facts the daemon writes — a `NULL` `origin_kind`, or a
/// spelling some newer daemon invented (`journal.rs::origin_from_columns`).
/// The session exists; where it came from does not. It is deliberately **not**
/// `Local`: the `Daemon` ownership arm opens only a session whose origin names
/// *that* device (`session.rs::check_user_owner`), so an unreadable origin
/// refuses a paired peer rather than promoting it to the person at this
/// machine (`DESIGN-remote-agents.md` §8 R2).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionOriginKind {
    #[default]
    Local,
    Peer,
    Unknown,
}

/// Activity the agent (or its hook) last reported. Wire names match herdr's
/// `pane.report_agent` states so a hook payload can be forwarded unchanged.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivityState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

/// Why a session is asking for the user's attention. This is runtime state,
/// not transcript history.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    Finished,
    Error,
    Permission,
}

impl AttentionReason {
    pub fn priority(self) -> u8 {
        match self {
            Self::Finished => 1,
            Self::Error => 2,
            Self::Permission => 3,
        }
    }
}

/// Runtime attention state for a live session.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Attention {
    pub reason: AttentionReason,
    pub at_ms: u64,
}

/// Public session metadata returned by `session_create` and `sessions_list`.
///
/// `workspace_id` is optional in M2 because workspace lookup is not
/// implemented yet; the terminal starts in the app process's current
/// directory.
///
/// `state` is the type-system distinction between a live process and a
/// recovered transcript. A recovered session is not a live one with a
/// comment: it cannot accept input, and attaching to it replays a journal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub workspace_id: Option<String>,
    /// Working directory the daemon actually handed the spawned process.
    /// `Some` only as an echo of that directory; `None` means the daemon does
    /// not know — never a path re-derived from the workspace row. Lossy
    /// (unpaired surrogates become U+FFFD) and for DISPLAY ONLY: never a
    /// filesystem key, never compared against a real path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub kind: SessionKind,
    pub title: String,
    /// Catalog provider id for ACP sessions, when one was persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Provider-side session id used by the ACP resume/load handshake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_session_id: Option<String>,
    pub state: SessionState,
    /// Monotonic age of the last observed sign of life. It is unavailable
    /// for journal-only transcripts because their monotonic clock died with
    /// the previous daemon.
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    /// Unix time in milliseconds when this session was first created.
    /// Stable across resume: a stored `(id, created_at_ms)` pair tells a
    /// caller whether a later session with the same id is the same session
    /// or a reissued id.
    pub created_at_ms: u64,
    /// Who asked for this session, set once at create. `#[serde(default)]`
    /// so a client that speaks an older dialect still parses a frame from a
    /// daemon that carries one.
    ///
    /// Two absences are not the same fact, and this field is the one place
    /// they meet. A frame with **no** `origin` key at all (a peer journal row
    /// written before v9, an older dialect) deserializes as `Local` — the
    /// `Default`. A row whose *stored* `origin_kind` column is `NULL` or
    /// unrecognised reads back as [`SessionOriginKind::Unknown`], which is not
    /// local and grants a peer nothing. The asymmetry is deliberate: the wire
    /// cannot express "absent" without breaking 1b clients, so absence stays
    /// the historical `local`; the journal can, so it says what it means.
    #[serde(default)]
    pub origin: SessionOrigin,
}

/// The connection-scoped roster update. It carries the fields the tab strip
/// needs for each session, including its workspace and kind identity. Identity
/// is authoritative on the wire and must never be inferred by the client.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionStateSnapshot {
    pub id: String,
    pub workspace_id: Option<String>,
    pub kind: SessionKind,
    pub title: String,
    pub state: SessionState,
    pub elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
    /// The session's origin. Carried on every push, because the tab strip
    /// names the device a peer-created session came from and a push that
    /// omitted it would leave that badge to the next full list.
    #[serde(default)]
    pub origin: SessionOrigin,
}

/// What the journal can honestly say about a finished transcript.
///
/// Three values, because there are three states of knowledge, not two.
/// `Unverifiable` is not a softer `Truncated`: it means the record cannot be
/// trusted to be complete because the daemon died before closing the journal.
/// The counters are what happened to get recorded before the death; zero
/// means "nothing was written down", never "nothing was lost".
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum TranscriptIntegrity {
    /// The writer committed a terminator and recorded no loss.
    Complete,
    /// The writer committed a terminator, so the tail is certified, and a
    /// loss was observed and measured before it.
    Truncated {
        dropped_frames: u64,
        dropped_bytes: u64,
        trimmed_bytes: u64,
    },
    /// The daemon died without closing the journal, so the tail cannot be
    /// checked. The counters preserve any measured loss that was recorded.
    Unverifiable {
        dropped_frames: u64,
        dropped_bytes: u64,
        trimmed_bytes: u64,
    },
}

/// How this session currently exists. Live and recovered are different
/// kinds of thing: one has a process, the other is a transcript of a
/// process the daemon can no longer see.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SessionState {
    /// A process is running. Output is live.
    Live { generation: u64 },
    /// A process is still running, but has produced no output for the
    /// configured silence threshold. This is never an exit or an idle
    /// transcript.
    Silent { generation: u64 },
    /// The process exited while this daemon was alive. `code` is the
    /// observed exit status (`None` if the child did not report one).
    Ended {
        generation: u64,
        code: Option<u32>,
        integrity: TranscriptIntegrity,
    },
    /// The daemon that owned the process is gone (kill, crash, update).
    /// Replay only.
    ///
    /// The journal was not closed orderly, so whatever was still
    /// uncommitted in the dying process's writer queue left no record
    /// anywhere. The transcript tail is always unverifiable; the counters
    /// preserve any measured loss that was recorded before the death.
    Recovered {
        generation: u64,
        integrity: TranscriptIntegrity,
    },
}

impl SessionState {
    pub fn generation(&self) -> u64 {
        match *self {
            Self::Live { generation }
            | Self::Silent { generation }
            | Self::Ended { generation, .. }
            | Self::Recovered { generation, .. } => generation,
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live { .. } | Self::Silent { .. })
    }
}

/// Events sent over the Tauri Channel supplied to `session_attach`.
///
/// [`SessionEvent::SessionsSnapshot`] is the exception: it is carried in the
/// same daemon event envelope but consumed by the daemon client connection
/// watcher before attachment events reach the Tauri channel.
///
/// `seq` starts at 1 and is contiguous for output chunks in one
/// *generation* of a session. A slow client is resynchronized with a
/// [`SessionEvent::Snapshot`], not with a declared missing range. A cursor
/// means "the last output sequence received"; replay therefore sends chunks
/// whose sequence is strictly greater than `from_cursor`.
///
/// Permission variants are additive for consumers that ignore unknown event
/// types, so older clients can continue to consume ordinary session events.
/// M3.5 uses that freedom: [`SessionEvent::Snapshot`] delivers the
/// current screen state on attach instead of a replay of past frames.
///
/// Attachment variants in this enum are the TypeScript `SessionEvent`
/// contract. Alignment is enforced by the committed snapshot
/// `session-event-samples.generated.json` and the tests in
/// `session_event_guard.rs`; the TypeScript handler must accept every sample.
/// Generation is **not**
/// a field here: it lives on [`Cursor`] and on [`super::SessionEventEnvelope`]
/// so a reconnecting client can tell a recreated process from the stream it
/// left. Putting generation on every output chunk would change the Channel
/// payload the frontend already parses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SessionEvent {
    Output {
        seq: u64,
        data: String,
    },
    /// A daemon-originated non-fatal transcript notice.
    SessionNotice {
        text: String,
        severity: NoticeSeverity,
    },
    /// Text emitted by an ACP agent message chunk.
    AgentMessage {
        message_id: Option<String>,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn_depth: Option<u32>,
    },
    /// Echo of the user prompt, one ACP `user_message_chunk` at a time.
    AgentUserMessage {
        message_id: Option<String>,
        text: String,
    },
    /// Agent reasoning, one ACP `agent_thought_chunk` at a time.
    AgentThought {
        message_id: Option<String>,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn_depth: Option<u32>,
    },
    /// Slash commands advertised by `available_commands_update`.
    AvailableCommands {
        commands: Vec<AvailableCommandView>,
    },
    /// An agent tool call announced by the agent. A separate permission request
    /// event carries the user-facing authorization conversation.
    ///
    /// `kind` is the ACP `ToolKind` snake_case name (`read`, `edit`, `execute`,
    /// `search`, `fetch`, `think`, `other`, …). `locations` are paths the
    /// daemon has already relativized against the session cwd.
    AgentToolCall {
        tool_call_id: String,
        title: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<ToolLocation>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn_depth: Option<u32>,
    },
    /// An agent tool-call status update. The optional text is the textual part
    /// of any content the agent supplied with the update.
    ///
    /// `locations`, when present, replace the previous list wholesale. They
    /// are never merged — ACP schema: "Collections are overwritten, not
    /// extended".
    AgentToolUpdate {
        tool_call_id: String,
        status: Option<String>,
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<ToolLocation>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn_depth: Option<u32>,
    },
    /// The response to one `session/prompt` request.
    AgentFinished {
        stop_reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TurnUsage>,
    },
    /// A Claude stream-json subagent birth. The absence of status and summary
    /// is intentional: Claude supplies those only in task_notification.
    AgentTaskStarted {
        task_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_backgrounded: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn_depth: Option<u32>,
    },
    /// A Claude stream-json subagent terminal notification.
    AgentTaskNotification {
        task_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        status: AgentTaskStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// The current set of background tasks. This is replacement state, not a
    /// lifecycle event; its entries do not carry subagent type or status.
    AgentBackgroundTasksChanged {
        tasks: Vec<AgentBackgroundTask>,
    },
    /// A valid ACP error response or a transport/decoding error surfaced to
    /// the attached session instead of being turned into a silent hang.
    AgentError {
        message: String,
    },
    /// Agent stderr is a separate stream and remains visible to the caller.
    AgentStderr {
        data: String,
    },
    /// An ACP agent is waiting for the user to authorize a tool call.
    ///
    /// `tool_call_id` is the ACP tool-call correlation key. The daemon's
    /// `session_permission_respond` request uses this same value; the ACP
    /// JSON-RPC request id remains private to the daemon transport.
    PermissionRequest {
        tool_call_id: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        /// Argument vector that will be spawned, when the host is asking
        /// about a concrete `terminal/create`. Agent-initiated prompts omit it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Environment applied to the spawn. Agent-initiated prompts omit it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<Vec<PermissionEnvVar>>,
        options: Vec<PermissionOption>,
        /// The origin of the session this request belongs to. Always on the
        /// wire, and deliberately **not** `Option`: an absent origin would be
        /// read as local by every consumer, so "absent" must not be
        /// expressible. The provider clients write `local` as a placeholder
        /// and the daemon overwrites it with the session's stored origin at the
        /// one place a request leaves for a subscriber — so a peer session's
        /// card always carries the peer origin (`DESIGN-remote-agents.md` §8b
        /// A14). The card renders a `peer` origin as its own first line, in
        /// its own element: the request's own text must never be able to
        /// imitate it.
        origin: SessionOrigin,
    },
    /// The pending permission is no longer waiting (allow, deny, timeout, or
    /// cancel). `tool_call_id` matches the request the UI is displaying.
    PermissionResolved {
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_option_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_option_kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_option_name: Option<String>,
    },
    /// Models, thinking, and modes the live ACP session has declared.
    ///
    /// The UI renders only what this event carries. It is emitted after
    /// handshake and whenever the agent publishes a models update, and
    /// re-emitted on attach so a remounted surface does not invent values.
    SessionManifest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current_model_id: Option<String>,
        models: Vec<SessionModel>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        modes: Option<SessionModeStateView>,
    },
    /// An external process (agent hook or our stub) announced itself on
    /// the daemon pipe. `seq` is the journal/stream sequence of this
    /// record; `report_seq` is the hook's own monotonic counter, which
    /// must not go backwards.
    AgentReported {
        seq: u64,
        source: String,
        agent: String,
        state: AgentActivityState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report_seq: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_session_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_start_source: Option<String>,
    },
    /// Process was observed to exit while the daemon was alive.
    Exit {
        code: Option<u32>,
    },
    /// The process remains alive but has crossed the observed silence
    /// threshold. The event is emitted once per silent transition.
    Silent {
        #[serde(rename = "elapsedMs")]
        elapsed_ms: u64,
    },
    /// Journal replay of a session whose process died with the daemon.
    /// Distinct from [`SessionEvent::Exit`]: Exit means the process was
    /// seen to die; Recovered means it was not, and this is a transcript.
    ///
    /// A `Recovered` marker never certifies a complete transcript. The
    /// journal of a process that died was not closed orderly, so whatever
    /// was still uncommitted in its writer queue is gone without a trace:
    /// the tail is unverifiable. The counters preserve any measured loss.
    Recovered {
        integrity: TranscriptIntegrity,
    },
    /// The journal has started dropping output for this live session. The
    /// counters measure what was noticed, never everything that was lost.
    JournalDegraded {
        dropped_frames: u64,
        dropped_bytes: u64,
    },
    /// Connection-scoped session roster update. Unlike the attachment events
    /// above, this event is not tied to a session attachment or generation.
    SessionsSnapshot {
        sessions: Vec<SessionStateSnapshot>,
    },
    /// Current screen state, delivered on attach instead of a replay of
    /// past frames (M3.5). The daemon holds a headless terminal emulator,
    /// applies every output chunk to it in sequence order, and renders the
    /// visible grid to a canonical ANSI string; the client writes `data`
    /// into its terminal emulator, then restores the cursor and all state from
    /// the explicit metadata below, and only then releases input. Output chunks
    /// that arrive after the snapshot are ordinary live events with sequences
    /// strictly greater than `as_of_seq`.
    Snapshot {
        /// Sequence boundary of this snapshot.
        ///
        /// A snapshot carrying `as_of_seq = N` is exactly the emulator
        /// state after every output chunk with sequence `<= N` has been
        /// applied, and before any chunk with sequence `> N`. Every live
        /// event after it carries a sequence strictly greater than `N`.
        ///
        /// The boundary is on **application to the emulator** — not on the
        /// write to the pipe, not on the journal commit, not on receipt by
        /// the client. The previous design advanced its cursor at
        /// pipe-write time, which let it claim delivered what was only
        /// queued; reconnections then produced both duplicates (queued
        /// chunks replayed) and gaps (queued chunks counted as seen and
        /// never sent). On the daemon side, capturing this state and
        /// registering a new attachment must happen under the same lock,
        /// or output applied in between lands in neither the snapshot nor
        /// the queued stream.
        #[serde(rename = "asOfSeq")]
        as_of_seq: u64,
        /// Screen width, in columns, the snapshot was taken at.
        cols: u16,
        /// Screen height, in rows, the snapshot was taken at.
        rows: u16,
        /// Reconstructed ANSI/VT string that reproduces the visible screen
        /// at `cols` x `rows`. Deliberately a rendered string, not a cell
        /// grid: a typical 200x50 screen is 8-30 KiB as ANSI against
        /// 470-850 KiB as JSON cells. The worst case still serialises under
        /// the 1 MiB NDJSON frame cap (`MAX_FRAME_BYTES`), but it is orders
        /// of magnitude larger than an ordinary output frame — see the
        /// frame-cap test in this module before assuming snapshots are
        /// small.
        data: String,
        /// Screen cursor to restore after writing `data`.
        cursor: ScreenCursor,
        /// The alternate screen buffer was active at capture. The client
        /// must restore this mode before releasing input.
        #[serde(rename = "alternateScreen")]
        alternate_screen: bool,
        /// Bracketed paste (DECSET 2004) was enabled at capture. The
        /// client must restore this mode before releasing input.
        #[serde(rename = "bracketedPaste")]
        bracketed_paste: bool,
        /// Whether line wrapping was enabled at capture.
        #[serde(rename = "lineWrap")]
        line_wrap: bool,
        /// Window title at capture, when the daemon saw one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

/// Shape of the screen cursor carried by [`SessionEvent::Snapshot`].
///
/// The wire values are the cursor styles xterm.js accepts, so the client
/// can apply the shape without translating it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

/// Cursor state of the captured screen.
///
/// Zero-based: row 0 is the first row of the visible screen, col 0 the
/// first column. Not a [`Cursor`]: that one is a replay position in the
/// output sequence, this one is a place on the screen.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScreenCursor {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
    pub shape: CursorShape,
    pub blinking: bool,
}

/// Replay position for a reconnecting client.
///
/// `seq` is the last output sequence the client has accounted for **for
/// `generation`**. Sequences in a generation are contiguous; a slow client
/// is resynchronized with a [`SessionEvent::Snapshot`], not with a declared
/// missing range.
/// A session whose process died and was recreated MUST bump `generation`
/// so a client holding an old cursor cannot silently consume a different
/// stream as if it were a continuation.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub generation: u64,
    pub seq: u64,
}

/// Outcome of a typed permission prompt. The wire name is fixed so
/// permission-response idempotency remains stable across clients.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOutcome {
    AllowOnce,
    Deny,
}

/// One ACP option displayed with a [`SessionEvent::PermissionRequest`].
///
/// `kind` stays a string because ACP deliberately has an open set of option
/// kinds. The daemon only interprets the four standard names when translating
/// the two Devboule outcomes back to ACP.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

/// One environment variable shown with a host-initiated permission prompt.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionEnvVar {
    pub name: String,
    pub value: String,
}

/// One thinking/effort level a model declared.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionModelEffort {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

/// One model in a live ACP session's declared catalog.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionModel {
    pub model_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<SessionModelEffort>>,
}

/// One mutually exclusive ACP session mode.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionModeView {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Standard ACP `SessionModeState` as shown to the UI.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionModeStateView {
    pub current_mode_id: String,
    pub available_modes: Vec<SessionModeView>,
}

/// One slash command advertised by an ACP agent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AvailableCommandView {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// A file the agent is reading or editing. Paths are relativized against the
/// session cwd in the daemon before this struct is published; a path that is
/// not under cwd is forwarded absolute.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolLocation {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// The only terminal statuses Claude exposes for a task notification.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskStatus {
    Completed,
    Failed,
    Stopped,
}

/// One entry in Claude's replacement set of background tasks.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackgroundTask {
    pub task_id: String,
    pub task_type: String,
    pub title: String,
}

/// Token usage attached to a prompt turn, when the agent supplied it.
///
/// Schema 1.5.0 keeps `usage` behind an unstable flag; grok sends the
/// counters on `session/prompt` result `_meta`. Optional fields keep both.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_tokens: Option<u64>,
}

/// ACP persistence handle. Terminal sessions always use [`PersistenceKind::None`].
///
/// The protocol carries an explicit "resume not supported" result because
/// "ACP is spoken" does not imply "resume is spoken".
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Persistence {
    pub kind: PersistenceKind,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistenceKind {
    None,
    Acp { handle: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResumeResult {
    /// Boxed because this variant carries the whole session metadata — the
    /// reply is a thin wrapper around it, and an unboxed payload makes the
    /// largest variant an order of magnitude bigger than its siblings
    /// (`clippy::large_enum_variant`). The wire shape is unchanged: a `Box`
    /// serializes as the value it points at.
    Resumed {
        session: Box<Session>,
    },
    NotSupported,
    Failed {
        message: String,
    },
}

/// Decide whether `cursor` may replay against `current_generation`.
///
/// Same generation: replay chunks with `seq > cursor.seq`.
/// Different generation: error. The client must treat the stream as new
/// (typically `Cursor { generation: current, seq: 0 }`).
pub fn cursor_replay_ok(current_generation: u64, cursor: Cursor) -> Result<(), WireError> {
    if cursor.generation == current_generation {
        Ok(())
    } else {
        Err(WireError {
            id: None,
            code: ErrorCode::SessionGenerationMismatch,
            message: format!(
                "session generation is {}, client cursor is {}",
                current_generation, cursor.generation
            ),
            details: Some(ErrorDetails::GenerationMismatch {
                current: current_generation,
                requested: cursor.generation,
            }),
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoticeSeverity {
    Info,
    Warning,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_integrity_variants_round_trip_with_exact_wire_shape() {
        let cases = [
            (TranscriptIntegrity::Complete, r#"{"kind":"complete"}"#),
            (
                TranscriptIntegrity::Truncated {
                    dropped_frames: 3,
                    dropped_bytes: 4096,
                    trimmed_bytes: 2048,
                },
                r#"{"kind":"truncated","droppedFrames":3,"droppedBytes":4096,"trimmedBytes":2048}"#,
            ),
            (
                TranscriptIntegrity::Unverifiable {
                    dropped_frames: 3,
                    dropped_bytes: 4096,
                    trimmed_bytes: 2048,
                },
                r#"{"kind":"unverifiable","droppedFrames":3,"droppedBytes":4096,"trimmedBytes":2048}"#,
            ),
        ];

        for (value, expected_json) in cases {
            let encoded = serde_json::to_string(&value).expect("json");
            assert_eq!(encoded, expected_json);
            let decoded: TranscriptIntegrity = serde_json::from_str(&encoded).expect("round trip");
            assert_eq!(decoded, value);
        }
    }

    #[test]
    fn session_event_uses_a_type_tag() {
        let output = serde_json::to_value(SessionEvent::Output {
            seq: 7,
            data: "hi".to_string(),
        })
        .expect("json");
        assert_eq!(output["type"], "output");
        assert_eq!(output["seq"], 7);
        let exit = serde_json::to_value(SessionEvent::Exit { code: Some(0) }).expect("json");
        assert_eq!(exit["type"], "exit");
        assert_eq!(exit["code"], 0);
    }

    #[test]
    fn attention_uses_camel_case_and_omits_absent_snapshot_value() {
        let attention = Attention {
            reason: AttentionReason::Permission,
            at_ms: 42,
        };
        let encoded = serde_json::to_value(attention).expect("attention json");
        assert_eq!(encoded["reason"], "permission");
        assert_eq!(encoded["atMs"], 42);

        let snapshot = SessionStateSnapshot {
            id: "s.client.1".to_string(),
            workspace_id: Some("ws-1".to_string()),
            kind: SessionKind::Acp,
            title: "Agent".to_string(),
            state: SessionState::Live { generation: 1 },
            elapsed_ms: None,
            attention: None,
            origin: SessionOrigin::local(),
        };
        let encoded = serde_json::to_value(snapshot).expect("snapshot json");
        assert_eq!(encoded["workspaceId"], "ws-1");
        assert_eq!(encoded["kind"], "acp");
        assert!(encoded.get("attention").is_none());
        assert_eq!(encoded["origin"]["kind"], "local");
        // A local origin names no device: the two optional fields stay off the
        // wire rather than travelling as `null`.
        assert!(encoded["origin"].get("deviceId").is_none());
        assert!(encoded["origin"].get("role").is_none());
    }

    #[test]
    fn session_uses_camel_case_workspace_id() {
        let session = Session {
            id: "session-1-1".to_string(),
            workspace_id: Some("ws-1".to_string()),
            cwd: None,
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
            state: SessionState::Live { generation: 1 },
            elapsed_ms: None,
            provider: None,
            peer_session_id: None,
            created_at_ms: 1,
            origin: SessionOrigin::local(),
        };
        let value = serde_json::to_value(&session).expect("json");
        assert_eq!(value["workspaceId"], "ws-1");
        assert_eq!(value["createdAtMs"], 1);
        assert_eq!(value["kind"], "terminal");
        assert!(value.get("generation").is_none());
        assert_eq!(value["state"]["type"], "live");
        assert_eq!(value["state"]["generation"], 1);
    }

    /// The app reads `session.origin?.kind === "peer"` to badge a session and
    /// names the device from `deviceId`. The wire spelling is the contract.
    #[test]
    fn a_peer_origin_session_round_trips_and_a_missing_origin_reads_as_local() {
        let session = Session {
            id: "s.peer.1".to_string(),
            workspace_id: None,
            cwd: None,
            kind: SessionKind::Claude,
            title: "Agent".to_string(),
            state: SessionState::Live { generation: 1 },
            elapsed_ms: None,
            provider: Some("claude".to_string()),
            peer_session_id: None,
            created_at_ms: 1,
            origin: SessionOrigin::peer("device-phone", PeerRole::Client),
        };
        let value = serde_json::to_value(&session).expect("json");
        assert_eq!(value["origin"]["kind"], "peer");
        assert_eq!(value["origin"]["deviceId"], "device-phone");
        assert_eq!(value["origin"]["role"], "client");
        let decoded: Session = serde_json::from_value(value).expect("session");
        assert_eq!(decoded.origin, session.origin);

        // Every row written before the origin existed is the local person's.
        let legacy = serde_json::json!({
            "id": "s.client.1",
            "workspaceId": null,
            "kind": "terminal",
            "title": "Terminal",
            "state": { "type": "live", "generation": 1 },
            "createdAtMs": 1
        });
        let decoded: Session = serde_json::from_value(legacy).expect("legacy session");
        assert_eq!(decoded.origin, SessionOrigin::local());
        assert!(decoded.origin.is_local());
    }

    #[test]
    fn session_json_without_created_at_ms_fails_to_deserialize() {
        let session = Session {
            id: "session-1-1".to_string(),
            workspace_id: None,
            cwd: None,
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
            state: SessionState::Live { generation: 1 },
            elapsed_ms: None,
            provider: None,
            peer_session_id: None,
            created_at_ms: 1,
            origin: SessionOrigin::local(),
        };
        let mut value = serde_json::to_value(&session).expect("json");
        value
            .as_object_mut()
            .expect("session object")
            .remove("createdAtMs");
        let error = serde_json::from_value::<Session>(value)
            .expect_err("missing createdAtMs must not default to 0");
        let message = error.to_string();
        assert!(
            message.contains("createdAtMs") || message.contains("created_at_ms"),
            "error must name the missing field, got {message}"
        );
    }

    #[test]
    fn silent_session_carries_elapsed_age_and_event_is_distinct() {
        let session = Session {
            id: "session-1-1".to_string(),
            workspace_id: None,
            cwd: None,
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
            state: SessionState::Silent { generation: 1 },
            elapsed_ms: Some(300_042),
            provider: None,
            peer_session_id: None,
            created_at_ms: 1,
            origin: SessionOrigin::local(),
        };
        let encoded = serde_json::to_value(&session).expect("session json");
        assert_eq!(encoded["state"]["type"], "silent");
        assert_eq!(encoded["elapsedMs"], 300_042);

        let event = serde_json::to_value(SessionEvent::Silent {
            elapsed_ms: 300_042,
        })
        .expect("event json");
        assert_eq!(event["type"], "silent");
        assert_eq!(event["elapsedMs"], 300_042);
    }

    #[test]
    fn resumable_session_metadata_uses_camel_case_wire_names() {
        let session = Session {
            id: "session-1-1".to_string(),
            workspace_id: None,
            cwd: None,
            kind: SessionKind::Acp,
            title: "Agent".to_string(),
            state: SessionState::Recovered {
                generation: 1,
                integrity: TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    trimmed_bytes: 0,
                },
            },
            elapsed_ms: None,
            provider: Some("grok".to_string()),
            peer_session_id: Some("peer-session-1".to_string()),
            created_at_ms: 1,
            origin: SessionOrigin::local(),
        };
        let value = serde_json::to_value(&session).expect("json");
        assert_eq!(value["provider"], "grok");
        assert_eq!(value["peerSessionId"], "peer-session-1");
        let decoded: Session = serde_json::from_value(value).expect("round trip");
        assert_eq!(decoded, session);
    }

    #[test]
    fn recovered_is_a_different_wire_type_from_live_and_ended() {
        let live = serde_json::to_value(SessionState::Live { generation: 1 }).expect("json");
        let ended = serde_json::to_value(SessionState::Ended {
            generation: 1,
            code: Some(0),
            integrity: TranscriptIntegrity::Complete,
        })
        .expect("json");
        let recovered = serde_json::to_value(SessionState::Recovered {
            generation: 1,
            integrity: TranscriptIntegrity::Unverifiable {
                dropped_frames: 0,
                dropped_bytes: 0,
                trimmed_bytes: 0,
            },
        })
        .expect("json");
        assert_eq!(live["type"], "live");
        assert_eq!(ended["type"], "ended");
        assert_eq!(recovered["type"], "recovered");
        assert_eq!(recovered["integrity"]["kind"], "unverifiable");
        assert_ne!(live["type"], recovered["type"]);
        assert_ne!(ended["type"], recovered["type"]);
    }

    #[test]
    fn recovered_event_is_distinct_from_exit() {
        let recovered = serde_json::to_value(SessionEvent::Recovered {
            integrity: TranscriptIntegrity::Unverifiable {
                dropped_frames: 2,
                dropped_bytes: 12,
                trimmed_bytes: 0,
            },
        })
        .expect("json");
        let exit = serde_json::to_value(SessionEvent::Exit { code: None }).expect("json");
        assert_eq!(recovered["type"], "recovered");
        assert_eq!(recovered["integrity"]["kind"], "unverifiable");
        assert_eq!(recovered["integrity"]["droppedBytes"], 12);
        assert_eq!(exit["type"], "exit");
        assert_ne!(recovered["type"], exit["type"]);
    }

    #[test]
    fn journal_degraded_event_round_trips() {
        let event = SessionEvent::JournalDegraded {
            dropped_frames: 3,
            dropped_bytes: 4096,
        };
        let encoded = serde_json::to_value(&event).expect("json");
        assert_eq!(encoded["type"], "journal_degraded");
        assert_eq!(encoded["droppedFrames"], 3);
        assert_eq!(encoded["droppedBytes"], 4096);
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn session_notice_round_trips_with_severity() {
        let event = SessionEvent::SessionNotice {
            text: "Codex declined an out-of-scope request.".to_string(),
            severity: NoticeSeverity::Warning,
        };
        let encoded = serde_json::to_value(&event).expect("event json");
        assert_eq!(
            encoded,
            serde_json::json!({
                "type": "session_notice",
                "text": "Codex declined an out-of-scope request.",
                "severity": "warning"
            })
        );
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn permission_request_round_trips_with_tool_call_correlation() {
        let event = SessionEvent::PermissionRequest {
            tool_call_id: "call-17".to_string(),
            title: "Run command".to_string(),
            description: Some("The agent wants to run a build.".to_string()),
            command: Some("cargo test".to_string()),
            args: Some(vec!["--offline".to_string(), "gate".to_string()]),
            cwd: Some("C:\\worktree".to_string()),
            env: Some(vec![PermissionEnvVar {
                name: "DB_GATE".to_string(),
                value: "SAFE".to_string(),
            }]),
            options: vec![PermissionOption {
                option_id: "allow".to_string(),
                name: "Allow once".to_string(),
                kind: "allow_once".to_string(),
            }],
            origin: SessionOrigin::peer("device-phone", PeerRole::Client),
        };
        let encoded = serde_json::to_value(&event).expect("json");
        assert_eq!(encoded["type"], "permission_request");
        assert_eq!(encoded["toolCallId"], "call-17");
        assert_eq!(encoded["args"][0], "--offline");
        assert_eq!(encoded["args"][1], "gate");
        assert_eq!(encoded["env"][0]["name"], "DB_GATE");
        assert_eq!(encoded["env"][0]["value"], "SAFE");
        assert_eq!(encoded["options"][0]["optionId"], "allow");
        assert_eq!(encoded["options"][0]["name"], "Allow once");
        assert_eq!(encoded["options"][0]["kind"], "allow_once");
        assert_eq!(encoded["origin"]["kind"], "peer");
        assert_eq!(encoded["origin"]["deviceId"], "device-phone");
        assert_eq!(encoded["origin"]["role"], "client");
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
    }

    /// The two shapes the app's `SessionOrigin` type names, in the protocol's
    /// own words, plus the one thing the field must refuse: absence.
    #[test]
    fn a_permission_request_origin_round_trips_and_absence_is_a_wire_error() {
        let local: SessionOrigin =
            serde_json::from_str(r#"{"kind":"local"}"#).expect("local origin");
        assert_eq!(local, SessionOrigin::local());
        assert_eq!(
            serde_json::to_string(&local).expect("json"),
            r#"{"kind":"local"}"#
        );

        let peer: SessionOrigin =
            serde_json::from_str(r#"{"kind":"peer","deviceId":"device-phone","role":"client"}"#)
                .expect("peer origin");
        assert_eq!(peer, SessionOrigin::peer("device-phone", PeerRole::Client));
        assert_eq!(
            serde_json::to_string(&peer).expect("json"),
            r#"{"kind":"peer","deviceId":"device-phone","role":"client"}"#
        );

        let request = SessionEvent::PermissionRequest {
            tool_call_id: "call-18".to_string(),
            title: "Run command".to_string(),
            description: None,
            command: None,
            args: None,
            cwd: None,
            env: None,
            options: Vec::new(),
            origin: SessionOrigin::local(),
        };
        let encoded = serde_json::to_value(&request).expect("json");
        assert_eq!(encoded["origin"]["kind"], "local");
        assert!(encoded["origin"].get("deviceId").is_none());
        assert!(encoded["origin"].get("role").is_none());
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, request);

        // Absence is not "local": a card that does not say where it came from
        // is a wire error, because "absent" would be read as this machine's own
        // session on a device that asked for it.
        let absent = serde_json::json!({
            "type": "permission_request",
            "toolCallId": "call-19",
            "title": "Run command",
            "options": []
        });
        let error = serde_json::from_value::<SessionEvent>(absent)
            .expect_err("a permission request without an origin must not parse");
        assert!(
            error.to_string().contains("origin"),
            "the error must name the missing field: {error}"
        );
    }

    #[test]
    fn session_manifest_round_trips_with_camel_case_wire_names() {
        let event = SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: Some("grok-4.6".to_string()),
            models: vec![SessionModel {
                model_id: "grok-4.6".to_string(),
                name: "Grok 4.6".to_string(),
                description: Some("SpaceXAI's latest frontier model".to_string()),
                context_tokens: Some(500_000),
                current_effort: Some("xhigh".to_string()),
                efforts: Some(vec![SessionModelEffort {
                    id: "xhigh".to_string(),
                    label: "Extra High Effort".to_string(),
                    description: Some("Highest effort and reasoning level".to_string()),
                    default: Some(false),
                }]),
            }],
            modes: Some(SessionModeStateView {
                current_mode_id: "ask".to_string(),
                available_modes: vec![SessionModeView {
                    id: "ask".to_string(),
                    name: "Always ask".to_string(),
                    description: Some("Ask before every tool call.".to_string()),
                }],
            }),
        };
        let encoded = serde_json::to_value(&event).expect("json");
        assert_eq!(encoded["type"], "session_manifest");
        assert_eq!(encoded["providerId"], "grok");
        assert_eq!(encoded["currentModelId"], "grok-4.6");
        assert_eq!(encoded["models"][0]["modelId"], "grok-4.6");
        assert_eq!(encoded["models"][0]["contextTokens"], 500_000);
        assert_eq!(encoded["models"][0]["currentEffort"], "xhigh");
        assert_eq!(encoded["models"][0]["efforts"][0]["id"], "xhigh");
        assert_eq!(encoded["modes"]["currentModeId"], "ask");
        assert_eq!(encoded["modes"]["availableModes"][0]["name"], "Always ask");
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn snapshot_event_round_trips_with_camel_case_wire_names() {
        let event = SessionEvent::Snapshot {
            as_of_seq: 41,
            cols: 200,
            rows: 50,
            data: "\u{1b}[2J\u{1b}[H$ ls\r\nsrc  target\r\n".to_string(),
            cursor: ScreenCursor {
                row: 12,
                col: 34,
                visible: true,
                shape: CursorShape::Block,
                blinking: true,
            },
            alternate_screen: false,
            bracketed_paste: true,
            line_wrap: true,
            title: Some("devboule - pwsh".to_string()),
        };
        let encoded = serde_json::to_value(&event).expect("json");
        assert_eq!(encoded["type"], "snapshot");
        assert_eq!(encoded["asOfSeq"], 41);
        assert_eq!(encoded["cols"], 200);
        assert_eq!(encoded["rows"], 50);
        assert_eq!(encoded["cursor"]["row"], 12);
        assert_eq!(encoded["cursor"]["col"], 34);
        assert_eq!(encoded["cursor"]["visible"], true);
        assert_eq!(encoded["cursor"]["shape"], "block");
        assert_eq!(encoded["cursor"]["blinking"], true);
        assert_eq!(encoded["alternateScreen"], false);
        assert_eq!(encoded["bracketedPaste"], true);
        assert_eq!(encoded["lineWrap"], true);
        assert_eq!(encoded["title"], "devboule - pwsh");
        // A rename here silently breaks the client, so pin the exact key
        // set, not just the individual names.
        let mut keys: Vec<&str> = encoded
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "alternateScreen",
                "asOfSeq",
                "bracketedPaste",
                "cols",
                "cursor",
                "data",
                "lineWrap",
                "rows",
                "title",
                "type"
            ]
        );
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn snapshot_title_is_absent_when_none() {
        let event = SessionEvent::Snapshot {
            as_of_seq: 0,
            cols: 80,
            rows: 24,
            data: "\u{1b}[Hready".to_string(),
            cursor: ScreenCursor {
                row: 0,
                col: 5,
                visible: true,
                shape: CursorShape::Underline,
                blinking: false,
            },
            alternate_screen: false,
            bracketed_paste: false,
            line_wrap: false,
            title: None,
        };
        let encoded = serde_json::to_value(&event).expect("json");
        assert!(encoded.get("title").is_none());
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn cursor_shapes_have_stable_wire_values() {
        assert_eq!(
            serde_json::to_value(CursorShape::Block).expect("json"),
            "block"
        );
        assert_eq!(
            serde_json::to_value(CursorShape::Underline).expect("json"),
            "underline"
        );
        assert_eq!(serde_json::to_value(CursorShape::Bar).expect("json"), "bar");
        for shape in [CursorShape::Block, CursorShape::Underline, CursorShape::Bar] {
            let decoded: CursorShape =
                serde_json::from_value(serde_json::to_value(shape).expect("json")).expect("shape");
            assert_eq!(decoded, shape);
        }
    }

    #[test]
    fn snapshot_screen_data_is_ndjson_safe() {
        let event = SessionEvent::Snapshot {
            as_of_seq: 7,
            cols: 80,
            rows: 24,
            data: "row1\r\nrow2\n\u{1b}[K".to_string(),
            cursor: ScreenCursor {
                row: 1,
                col: 0,
                visible: true,
                shape: CursorShape::Block,
                blinking: true,
            },
            alternate_screen: false,
            bracketed_paste: false,
            line_wrap: true,
            title: None,
        };
        let encoded = serde_json::to_string(&event).expect("json");
        assert!(
            !encoded.contains('\n'),
            "compact JSON must not contain a raw newline or NDJSON framing splits the event"
        );
        let decoded: SessionEvent = serde_json::from_str(&encoded).expect("parse");
        assert_eq!(decoded, event);
    }

    #[test]
    fn dense_worst_case_snapshot_still_fits_the_frame_cap() {
        // Heaviest plausible snapshot: a 200x50 screen where every cell
        // repaints foreground and background in its own 24-bit colour, so
        // no pair of SGR sequences can collapse. Each cell costs 49 bytes
        // of escaped JSON (each ESC becomes \u001b), about 490 KiB total -
        // heavier than the ~410 KiB worst case measured during design, and
        // still under the 1 MiB NDJSON frame cap. A typical
        // snapshot is 8-30 KiB: never assume snapshots are always small.
        let mut data = String::with_capacity(200 * 50 * 39);
        for cell in 0..(200u32 * 50) {
            // 100..=255 so every colour component is three digits long.
            let fg = (100 + (cell % 156)) as u8;
            let bg = (100 + (cell * 7 % 156)) as u8;
            data.push_str(&format!(
                "\u{1b}[38;2;{fg};{fg};{fg}m\u{1b}[48;2;{bg};{bg};{bg}mX"
            ));
        }
        let event = SessionEvent::Snapshot {
            as_of_seq: 4096,
            cols: 200,
            rows: 50,
            data,
            cursor: ScreenCursor {
                row: 49,
                col: 199,
                visible: true,
                shape: CursorShape::Bar,
                blinking: false,
            },
            alternate_screen: true,
            bracketed_paste: true,
            line_wrap: true,
            title: None,
        };
        let encoded = serde_json::to_vec(&event).expect("json");
        assert!(
            encoded.len() > 400_000,
            "expected a dense worst-case payload, got {} bytes",
            encoded.len()
        );
        assert!(
            encoded.len() < crate::MAX_FRAME_BYTES,
            "worst-case snapshot serialises to {} bytes, frame cap is {}",
            encoded.len(),
            crate::MAX_FRAME_BYTES
        );
    }

    #[test]
    fn cursor_generation_mismatch_is_an_error() {
        let err = cursor_replay_ok(
            2,
            Cursor {
                generation: 1,
                seq: 9,
            },
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::SessionGenerationMismatch);
        cursor_replay_ok(
            2,
            Cursor {
                generation: 2,
                seq: 9,
            },
        )
        .expect("same generation");
    }

    #[test]
    fn resume_not_supported_is_an_explicit_variant() {
        let value = serde_json::to_value(ResumeResult::NotSupported).expect("json");
        assert_eq!(value["type"], "not_supported");
    }

    #[test]
    fn resume_resumed_and_failed_round_trip_on_the_wire() {
        let resumed = ResumeResult::Resumed {
            session: Box::new(Session {
                id: "s.client.1".to_string(),
                workspace_id: None,
                cwd: None,
                kind: SessionKind::Acp,
                title: "t".to_string(),
                state: SessionState::Live { generation: 2 },
                elapsed_ms: None,
                provider: Some("grok".to_string()),
                peer_session_id: Some("peer-1".to_string()),
                created_at_ms: 1,
                origin: SessionOrigin::local(),
            }),
        };
        let value = serde_json::to_value(&resumed).expect("json");
        assert_eq!(value["type"], "resumed");
        assert_eq!(value["session"]["id"], "s.client.1");
        assert_eq!(value["session"]["peerSessionId"], "peer-1");
        let back: ResumeResult = serde_json::from_value(value).expect("round trip");
        assert!(matches!(back, ResumeResult::Resumed { .. }));

        let failed = ResumeResult::Failed {
            message: "no".to_string(),
        };
        let value = serde_json::to_value(&failed).expect("json");
        assert_eq!(value["type"], "failed");
        assert_eq!(value["message"], "no");
        let back: ResumeResult = serde_json::from_value(value).expect("round trip");
        assert!(matches!(back, ResumeResult::Failed { message } if message == "no"));
    }

    #[test]
    fn agent_reported_round_trips_with_herdr_shaped_fields() {
        let event = SessionEvent::AgentReported {
            seq: 4,
            source: "devboule:stub".to_string(),
            agent: "stub".to_string(),
            state: AgentActivityState::Working,
            message: Some("turn started".to_string()),
            report_seq: Some(7),
            agent_session_id: Some("agent-session-1".to_string()),
            agent_session_path: Some(r"C:\tmp\session.json".to_string()),
            session_start_source: Some("startup".to_string()),
        };
        let encoded = serde_json::to_value(&event).expect("json");
        assert_eq!(encoded["type"], "agent_reported");
        assert_eq!(encoded["seq"], 4);
        assert_eq!(encoded["source"], "devboule:stub");
        assert_eq!(encoded["agent"], "stub");
        assert_eq!(encoded["state"], "working");
        assert_eq!(encoded["message"], "turn started");
        assert_eq!(encoded["reportSeq"], 7);
        assert_eq!(encoded["agentSessionId"], "agent-session-1");
        assert_eq!(encoded["agentSessionPath"], r"C:\tmp\session.json");
        assert_eq!(encoded["sessionStartSource"], "startup");
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
        assert_eq!(
            serde_json::to_value(AgentActivityState::Idle).expect("json"),
            "idle"
        );
        assert_eq!(
            serde_json::to_value(AgentActivityState::Blocked).expect("json"),
            "blocked"
        );
        assert_eq!(
            serde_json::to_value(AgentActivityState::Unknown).expect("json"),
            "unknown"
        );
    }

    #[test]
    fn claude_session_kind_is_the_wire_string_claude() {
        assert_eq!(
            serde_json::to_value(SessionKind::Claude).expect("json"),
            "claude"
        );
        let decoded: SessionKind = serde_json::from_str("\"claude\"").expect("kind");
        assert_eq!(decoded, SessionKind::Claude);
        assert!(SessionKind::Claude.is_agent());
        assert!(SessionKind::Acp.is_agent());
        assert!(SessionKind::Pi.is_agent());
        assert!(SessionKind::Codex.is_agent());
        assert!(!SessionKind::Terminal.is_agent());
    }

    #[test]
    fn pi_session_kind_is_the_wire_string_pi() {
        assert_eq!(serde_json::to_value(SessionKind::Pi).expect("json"), "pi");
        let decoded: SessionKind = serde_json::from_str("\"pi\"").expect("kind");
        assert_eq!(decoded, SessionKind::Pi);
    }

    #[test]
    fn codex_session_kind_is_the_wire_string_codex() {
        assert_eq!(
            serde_json::to_value(SessionKind::Codex).expect("json"),
            "codex"
        );
        let decoded: SessionKind = serde_json::from_str("\"codex\"").expect("kind");
        assert_eq!(decoded, SessionKind::Codex);
    }

    #[test]
    fn tool_call_kind_and_locations_are_camel_case_and_optional() {
        let with = SessionEvent::AgentToolCall {
            tool_call_id: "toolu_1".to_string(),
            title: "Read src/lib.rs".to_string(),
            status: "pending".to_string(),
            kind: Some("read".to_string()),
            locations: Some(vec![ToolLocation {
                path: "src/lib.rs".to_string(),
                line: Some(12),
            }]),
            subagent_type: None,
            parent_tool_use_id: None,
            spawn_depth: None,
        };
        let encoded = serde_json::to_value(&with).expect("json");
        assert_eq!(encoded["type"], "agent_tool_call");
        assert_eq!(encoded["toolCallId"], "toolu_1");
        assert_eq!(encoded["kind"], "read");
        assert_eq!(encoded["locations"][0]["path"], "src/lib.rs");
        assert_eq!(encoded["locations"][0]["line"], 12);
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, with);

        let without = SessionEvent::AgentToolCall {
            tool_call_id: "t".to_string(),
            title: "x".to_string(),
            status: "pending".to_string(),
            kind: None,
            locations: None,
            subagent_type: None,
            parent_tool_use_id: None,
            spawn_depth: None,
        };
        let encoded = serde_json::to_value(&without).expect("json");
        assert!(encoded.get("kind").is_none());
        assert!(encoded.get("locations").is_none());
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, without);

        let update = SessionEvent::AgentToolUpdate {
            tool_call_id: "t".to_string(),
            status: Some("completed".to_string()),
            text: Some("ok".to_string()),
            title: None,
            kind: Some("edit".to_string()),
            locations: Some(vec![ToolLocation {
                path: "src/main.rs".to_string(),
                line: None,
            }]),
            parent_tool_use_id: None,
            spawn_depth: None,
        };
        let encoded = serde_json::to_value(&update).expect("json");
        assert_eq!(encoded["type"], "agent_tool_update");
        assert_eq!(encoded["kind"], "edit");
        assert_eq!(encoded["locations"][0]["path"], "src/main.rs");
        assert!(encoded["locations"][0].get("line").is_none());
    }
}
