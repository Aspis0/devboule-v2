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

/// What a send does when the target agent already has a turn running.
/// Omitting this field preserves the interrupt-and-replace behavior; only
/// steering is an explicit alternative in this protocol revision.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActiveTurnBehavior {
    Steer,
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

/// Whether a session can pass a permission moment with no human answering.
///
/// A closed wire enum with three values, because there are three states of
/// knowledge, not two. The daemon's marker derives from the session's
/// **delivered** mode, judged by the vocabulary that authored it: the daemon's
/// own mode dictionaries answer `yes` or `no`, and a mode whose vocabulary is
/// the provider's own (an ACP agent's `{id, name, description}` modes are
/// prose) is `unknown` — deriving a permission fact from prose is a defect,
/// not a solution. `unknown` is a value, never an absence dressed up: it says
/// "the daemon was not told or cannot establish", which is the one answer a
/// boolean could not carry, and it is the default a frame without the key
/// reads as — a session nobody said anything about is not a session a human
/// is watching.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UnattendedState {
    /// The daemon knows the session asks: its delivered mode is one the
    /// daemon itself authored and knows stops at a human.
    No,
    /// The daemon cannot establish the answer. The mode's vocabulary is the
    /// provider's own, or no mode was ever said.
    #[default]
    Unknown,
    /// The daemon knows the session answers its own permission prompts: the
    /// delivered mode is one its own broker honours, or one the daemon
    /// authored the knob for (a launch flag, a turn parameter, an extension
    /// it wrote).
    Yes,
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
    /// Catalog provider id for agent sessions, when one was persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Provider-side session id used by the family resume handshake
    /// (ACP session/load, Claude `--resume`).
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
    /// The name a created agent is shown under (S5-09). Set once at creation
    /// and never renamable in v1, so it travels with the session row and not
    /// with the creation request that named it. `#[serde(default)]` for the
    /// same reason `origin` has it: a client that speaks an older dialect must
    /// still parse a frame carrying it, and a row written before the field
    /// existed reads back as `None` — which the app renders as its fallback
    /// name, never as an empty one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The session id of the agent that created this one (S5-04). Written by
    /// the daemon only: it is deliberately absent from
    /// [`crate::ClientMessage::SessionCreate`], so no client can claim a
    /// parent. `None` for every session a human started, and for rows written
    /// before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// The profile this session was created from, by its **stable id** and
    /// never by its name.
    ///
    /// A profile can be renamed (the id is what survives) and two profiles may
    /// share a name, so the name a creation was asked for is not a fact about
    /// the session that came out of it; the id is. `None` for a session a human
    /// started from the provider picker — that path resolves no profile — and
    /// for every row written before the column existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// The context this session belongs to: **its own id**, unless another
    /// session created it, in which case it is that creator's `context_id`.
    ///
    /// One value for a creator and everything it commissions, at any depth, so
    /// a caller can name its whole family without keeping a map of its own
    /// (A2A's `contextId`). Written once at create from a fact the daemon
    /// already had; a client that reads a frame without it derives the same
    /// value from the session's own id, which is the rule the field states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Whether this session can pass a permission moment with no human
    /// answering, as [`UnattendedState`] knows it.
    ///
    /// A fact of the session's **birth**, derived once from the mode the
    /// daemon delivered and never re-derived: a human who later un-ticks that
    /// profile, or edits its mode, does not change what this child already
    /// is — it did run that way. Written on **every** frame the daemon emits
    /// for a child; `#[serde(default)]` so a frame that predates the field
    /// (or a journal row written before the tri-state existed) reads back as
    /// `unknown` — the honest reading, since nobody has said — and never as
    /// `no`, which would claim a human is watching when the truth is that
    /// nobody knows. The old `bool` shape could not carry the third value:
    /// `default` + `skip_serializing_if` made absent and `false` the same
    /// value, which is the collapse this field exists to remove.
    #[serde(default)]
    pub unattended: UnattendedState,
    /// The labels this session carries: the caller's own free-form map with the
    /// four `devboule.` keys (`created-by`, `depth`, `origin`, `profile`) the
    /// daemon stamped into it.
    ///
    /// For humans and for display, and for nothing else: no code in the daemon
    /// reads a label to decide anything. The `devboule.` prefix is reserved —
    /// the daemon's own facts are the ones it writes, and a caller cannot set
    /// or overwrite one.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
    /// Whether the daemon would accept a resume for this session right now:
    /// not live, a resumable family, provider and peer id persisted.
    ///
    /// Computed by the daemon from `Provider::resumable()`; the app renders
    /// it and never re-derives it from kind, state, or columns. `#[serde(default)]`
    /// so a frame from a daemon that predates the field reads back as false —
    /// the button stays hidden rather than offered on a guess.
    #[serde(default)]
    pub resumable: bool,
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
    /// The name a created agent is shown under (S5-09). Carried on every push,
    /// because a child created while the app is open arrives as a push-only row
    /// and one that omitted it would stay nameless until the next full list —
    /// which is a list nothing may run again. Absent means "no name of its
    /// own", which the app renders as its fallback, never as an empty name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The session id of the agent that created this one (S5-04). Carried on
    /// every push for the same reason as the name: the row is what the human
    /// sees, and "created by" is part of it. `None` for every session a human
    /// started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// The profile id this session was created from, when a profile made it.
    /// Carried on every push for the same reason as the name: a child created
    /// while the app is open arrives as a push-only row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// The context this session belongs to (its own id, or its creator's). On
    /// every push, like the name and the creator, because a push-only row has
    /// only what the push carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Whether this session can pass a permission moment with no human
    /// answering (`UnattendedState`). Carried on **every** push, like the
    /// name and the creator, because a push-only row has only what the push
    /// carries — and a row that arrives without the marker must not be read
    /// as `no`, which is what the collapsed `bool` this field replaces made
    /// every absent row say.
    #[serde(default)]
    pub unattended: UnattendedState,
    /// The session's labels, stamped by the daemon and readable by a human.
    /// Carried on every push for the same reason as the name.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
    /// The delegation facts for this session, when it is an agent-created
    /// child. Carried on **every** push, like the name and the creator, so
    /// the count never goes stale and an `active → off` transition lands.
    /// Absent means **not an agent-created child** — a state of its own,
    /// never to be read as `off`, which is a child whose switch a human
    /// turned off. On the snapshot only: the protocol `Session` (the
    /// `sessions_list` struct) does not carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation: Option<DelegationState>,
}

/// The delegation ledger for one agent-created child (snapshot only).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DelegationState {
    /// How many permission cards of this session were resolved, counted from
    /// what the journal survived — every resolution, whoever answered.
    pub answered: u32,
    /// Whether delegated answers may happen for this child right now
    /// ([`DelegationRunState::Active`]), may not
    /// ([`DelegationRunState::Off`]), or whether the child was created in an
    /// auto-accepting profile and can run without asking at all
    /// ([`DelegationRunState::Unattended`]) — that last one is a birth fact
    /// read from the journal's ratcheted column, never recomputed from the
    /// live switch: after a human turns delegation off, an unattended child
    /// keeps running without asking, and its row is the only thing telling
    /// the human which sessions those are.
    pub state: DelegationRunState,
}

/// Whether delegated answers may reach this child's cards. The three are
/// distinct wire values and never collapse: `off` is a child the switch
/// governs, `unattended` is a child nothing needs to govern.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DelegationRunState {
    Off,
    Active,
    Unattended,
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
    /// Replay always; resume when the family is resumable.
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

    /// The A2A word for what this session is doing (`S5-08`). `turn_running` is
    /// the live runtime's own answer and is only consulted for a live process: a
    /// transcript has no turn to be in the middle of.
    ///
    /// `Live` with no turn is `submitted` and not `working` — the vocabulary
    /// distinguishes "accepted, nothing happening yet" from "in progress", and a
    /// live session with nothing running is the former. `Recovered` is
    /// `canceled`: the daemon that owned it died, so the run ended without
    /// reporting, and `completed` would be a claim nothing supports.
    pub fn task_state(&self, turn_running: bool) -> AgentTaskState {
        match self {
            Self::Live { .. } | Self::Silent { .. } => {
                if turn_running {
                    AgentTaskState::Working
                } else {
                    AgentTaskState::Submitted
                }
            }
            Self::Ended { code, .. } => match code {
                Some(0) => AgentTaskState::Completed,
                _ => AgentTaskState::Failed,
            },
            Self::Recovered { .. } => AgentTaskState::Canceled,
        }
    }
}

/// The vocabulary a created agent's lifecycle is reported in (`S5-08`).
///
/// These are A2A's `TaskState` words, reserved here so the daemon never grows a
/// second name for one fact: `submitted` is accepted-and-not-yet-started,
/// `working` is running, `input_required` is parked on a card only a human can
/// answer, and the last three are terminal. The finish report uses the terminal
/// three today; the roster uses the first three; `rejected` is reserved for a
/// creation that was refused, which this slice reports as an error sentence and
/// not as a session state.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskState {
    Submitted,
    Working,
    Completed,
    Failed,
    Canceled,
    InputRequired,
    Rejected,
}

impl AgentTaskState {
    /// The word this state is written as, on the wire and in the finish
    /// envelope's `state:` line. One spelling, from the same list serde is
    /// generated from: a second table is a second thing to keep in step.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Working => "working",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::InputRequired => "input_required",
            Self::Rejected => "rejected",
        }
    }
}

/// The caps one creation is admitted under, as the creation card states them
/// (`S5` decision 5).
///
/// Every number is what the daemon holds at the moment the card is composed, so
/// the human decides against the budget that is actually about to be spent
/// rather than against a configuration claim.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentCaps {
    pub live_children: u32,
    pub max_live_children: u32,
    pub creations_this_hour: u32,
    pub max_creations_per_hour: u32,
    pub depth: u32,
    pub max_depth: u32,
    pub live_agent_sessions: u32,
    pub max_live_agent_sessions: u32,
}

/// The `create_agent` payload a creation card carries on top of the ordinary
/// permission card's fields (`S5` §1).
///
/// The card is a [`SessionEvent::PermissionRequest`] and not a variant of its
/// own: it needs exactly what that variant already provides — a pending entry
/// the permission broker can answer through `SessionPermissionRespond`, two
/// options (allow once / deny), an origin stamp, the per-device card budget and
/// a journaled decision — and it adds only the facts being decided about. A
/// second variant would have to re-implement all of that, and the decision
/// leaves the gate closed on a refusal, which is a property of that entry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentCard {
    /// The session that asked. It is also the session the card is published on,
    /// so this is a restatement on the wire rather than a lookup for the app.
    pub creator_session_id: String,
    pub provider: String,
    /// The **name** of the profile the child would be created from: the card is
    /// the human's sentence, and the name is the word the human ticked. What the
    /// profile resolves to is on the card's description; the child's session row
    /// records the profile's stable id, not this.
    ///
    /// The field was named `preset` until `d5c72a3` renamed it, and creation
    /// cards are journalled — the alias keeps a card journaled under the old
    /// word hydrating on replay instead of dropping the whole permission row,
    /// the same repair the `AgentCreated` sibling carries for the same rename:
    /// the journal keeps saying what it actually said.
    #[serde(alias = "preset")]
    pub profile: String,
    /// The display name the child would be created with.
    pub title: String,
    /// The tools state the child will start in, as the S1 wire word
    /// (`hosted`/`unavailable`/`unverified`). The card promises verification;
    /// the result and roster report it (S2/S8 precedence rule): a card for an
    /// MCP-capable family reads `hosted` with "will be verified at start" in
    /// the description, never bare "has tools"; a card for pi/codex reads
    /// `unavailable` with the no-tools sentence until S9 flips the gate.
    ///
    /// Additive (P1): an older peer's frame without this key decodes to the
    /// tri-state's not-established value — a daemon that never heard of
    /// `ToolsState` has established nothing, so absent renders as the unknown,
    /// never as the benign "no tools".
    #[serde(default = "default_create_card_tools")]
    pub tools: String,
    pub caps: CreateAgentCaps,
}

/// The tools word for a card frame that predates it (P1): the tri-state's
/// not-established value. Lives beside the struct because the protocol crate
/// owns the wire contract; the daemon's `ToolsState::Unverified.as_str()`
/// spells the same word, pinned on both sides (daemon walk test + the decode
/// test below).
fn default_create_card_tools() -> String {
    "unverified".to_string()
}

/// One part of a finish artifact (`S5` decision 10, A2A §3 `Part`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FinishArtifactPart {
    /// `devboule-attachment:<sessionId>/<digest>` and never a path: the app
    /// resolves a reference through the daemon the same way it resolves a
    /// prompt attachment.
    pub url: String,
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<FinishArtifactPartMetadata>,
}

/// What the daemon knows about a stored part without re-reading it.
///
/// A cheap refusal, never *the* size: a caller compares this against the size
/// the store reports when it resolves the reference, and the store's number is
/// the one that counts.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FinishArtifactPartMetadata {
    pub stored_bytes: u64,
}

/// One artifact a child's finish delivered (`S5` decision 10): the child's whole
/// last `AgentMessage`, deposited in the **creator's** folder.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FinishArtifact {
    /// The reference's own spelling, so the app can key the two records that
    /// describe one artifact (the text message and
    /// [`SessionEvent::ChildFinished`]) on the same value.
    pub artifact_id: String,
    pub parts: Vec<FinishArtifactPart>,
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
    ///
    /// `author` names who spoke, while `message_kind` names the part this text
    /// plays in this session. The two facts are deliberately independent:
    /// `agent` covers both an outgoing echo and a received envelope.
    AgentUserMessage {
        message_id: Option<String>,
        text: String,
        #[serde(default)]
        author: UserMessageAuthor,
        /// Absent on stored rows written before this field existed.
        #[serde(default)]
        message_kind: UserMessageKind,
    },
    /// A prompt accepted by a running turn. This is journaled for audit but
    /// intentionally not pushed to live observers; the normal user-message
    /// echo is the transcript event.
    Steered {
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
    /// How full the context window is, in the provider's own words — the
    /// meter's number, never a guess. A provider that sent no number gets no
    /// event; the app shows nothing rather than a stand-in zero.
    ///
    /// `live` names the source: Codex pushes it during the turn; Claude, pi
    /// and ACP report at the turn's end, and the popover labels those
    /// "as of the last turn". `max_tokens` is absent when the view's frame
    /// carried no window — the app may then take the window from the
    /// manifest entry of the SAME `model_id`, never from another model.
    ContextUsage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        used_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_tokens: Option<u64>,
        live: bool,
    },
    /// The account's plan consumption as the provider pushed it — today only
    /// Codex `account/rateLimits/updated`, which the wire already carried
    /// before this variant existed. Account-scoped, not session-scoped: the
    /// app keeps the latest event per provider id.
    ///
    /// Carries no token counter and no account id — only what the popover
    /// shows: the plan's own label, one window per window the frame actually
    /// sent, and the credits block when the frame had one. A window the
    /// frame did not send is never added.
    PlanUsage {
        provider_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan_label: Option<String>,
        windows: Vec<PlanWindow>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credits: Option<PlanCredits>,
    },
    /// An agent asked for, and got, a child session (`S5` §1).
    ///
    /// Published and journaled on the **creator's** session, never on the
    /// child: the child's transcript begins with its `initialPrompt`, and the
    /// creator's record is the one that has to explain where the session came
    /// from. `message_id` is an ordinary transcript id, optional for the same
    /// reason [`Self::AgentMessage`]'s is.
    AgentCreated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        child_session_id: String,
        display_name: String,
        /// The catalog provider id the child was created with.
        provider: String,
        /// The **name** of the profile the child was created from, as it was
        /// called at that moment. A record of a birth: the child's session row
        /// carries the profile's stable id (`Session.profile_id`), because a
        /// rename must not make a running child misreport what it was started
        /// from, while this event is the sentence the creator's transcript
        /// shows.
        ///
        /// This field was written as `preset` before journal v11 and this
        /// pass's rename, and rows with the old spelling are still on disk, so
        /// the reader accepts both. A row read back through the alias keeps the
        /// value it was written with, which is the **preset** the child was
        /// created under (`worker`, `design`) — a name from the old built-in
        /// vocabulary, not a profile a human saved. Nothing is rewritten: the
        /// journal keeps saying what it actually said.
        #[serde(alias = "preset")]
        profile: String,
    },
    /// A created child finished, in structured form, published on the creator
    /// beside the `<devboule-system>` text message that carries the same facts
    /// (`S5` decision 7 + 10, rev 4).
    ///
    /// Two records, one delivery: the text message is what an agent reads, and
    /// this event is what a surface consumes — the app has no parser for the
    /// envelope and must not grow one, so it copies the artifact into its own
    /// store when this arrives (the daemon's copy dies with the creator session
    /// and after [`crate::ATTACHMENT_RETENTION`] idle). `message_id` is the
    /// **text message's** id, which is what lets a client tell the two records
    /// apart from two separate finishes.
    ChildFinished {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        child_session_id: String,
        /// The child's display name at the moment it finished. Copied rather
        /// than looked up: the row can be gone by the time a client reads this.
        display_name: String,
        state: AgentTaskState,
        /// Why `artifacts` is empty, when it is (the deposit failed, or the
        /// message was over the 32 KiB cap). Absent when there is an artifact.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        artifacts: Vec<FinishArtifact>,
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
        /// The daemon's chooser verdict, computed from `options` by the same
        /// rule the broker's auto-answer uses (`options_form_a_chooser`): the
        /// same kind offered twice — allow or reject — means the agent is
        /// asking which one to use, so the app renders one control per option
        /// instead of the ordinary pair. `Some(true)` exactly when the rule
        /// fired; absent otherwise — an ordinary permission, or a frame from
        /// a daemon older than this field, which the app renders as the
        /// ordinary card. The app never re-derives the rule from the option
        /// list.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_chooser: Option<bool>,
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
        /// Present only on a creation card (`S5` §1): the ordinary card fields
        /// say what is being asked, `options` says allow/deny, and this says
        /// *what* is being created. Absent on every other permission request,
        /// which is why it is an extension of this variant and not a variant of
        /// its own (see [`CreateAgentCard`]).
        ///
        /// Boxed after measurement: this variant is the wire enum's largest —
        /// 377 bytes against `AgentToolCall`'s 176 (the split clippy flagged
        /// the enum for), with this payload alone `size_of` 152 of the enum's
        /// 384. `Option<Box<_>>` holds the payload at 8 and the enum at 240,
        /// which clears the lint without an `#[allow]` that would silence it
        /// for every variant added later.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        create_agent: Option<Box<CreateAgentCard>>,
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
        /// Who answered, when it was not a person at this machine: the
        /// session id of the agent that created the card's session and
        /// answered for it under the delegation switch. Absent (and `null`)
        /// means a person — the card's default history, so the app renders
        /// attribution only when delegation actually answered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answered_by: Option<String>,
    },
    /// The durable record of a resolution, emitted beside
    /// [`SessionEvent::PermissionResolved`] on **every** resolution — a
    /// person's answer, a delegated one, an auto-answer and a cancel alike —
    /// and journalled, because the snapshot's delegation count is read back
    /// from what survived, not from live state. `answered_by` is absent for a
    /// human and names the creator session for a delegated answer; `outcome`
    /// is the journal's own vocabulary (`allow_once`, `deny`, `timeout`,
    /// `cancelled`, …), the same string the `permissions` table records.
    PermissionAnswered {
        card_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answered_by: Option<String>,
        outcome: String,
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
    /// This observer's attachment was removed because another client took
    /// the session over: a resume replaced the generation it was watching.
    /// The session itself lives on — this names the observer's view, not
    /// the session, and reattaching observes the new generation. Distinct
    /// from [`SessionEvent::Recovered`], which says the process died
    /// unobserved and this is a transcript.
    Detached,
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

/// The seq value of a cursor that means "nothing is owed": the reader asks
/// to be sent no rows at all, whatever generations they span. It is a
/// sentinel, not a position — no seq is past every row once history is
/// owed regardless of the cursor.
pub const NOTHING_OWED_CURSOR: u64 = u64::MAX;

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

/// One rate-limit window a plan-usage frame actually carried.
///
/// `duration_mins` identifies the window and labels it: Codex sends 300 for
/// the 5-hour window and 10080 for the weekly one. `resets_at` is Unix
/// **seconds** — the unit measured in
/// `fixtures/wire/codex/E1-step1-handshake.jsonl`, where `emittedAtMs`
/// 1789053466049 precedes `resetsAt` 1789057213 by ~62 min inside a
/// 300-minute window at 82 % used.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanWindow {
    pub duration_mins: u64,
    /// Absent when the frame named the window but not its consumption —
    /// the app then shows the window and its reset, never a stand-in 0 %.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

/// The credits block of a plan-usage frame, when the frame had one.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanCredits {
    /// The balance exactly as the provider spelled it (Codex sends a decimal
    /// string), carried only when the frame sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
    /// Whether the balance is unlimited — `None` when the frame did not say,
    /// the wire's third state: a missing field is never rendered as `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlimited: Option<bool>,
}

/// One family's resume handle. Terminal sessions always use [`PersistenceKind::None`].
///
/// The protocol carries an explicit "resume not supported" result because
/// "the family is spoken" does not imply "resume is spoken".
///
/// The variant names the family that wrote the row; the daemon re-derives
/// provider and peer id from the journal and admits through
/// `Provider::resumable()`, so two variants unwrap to the same handling and
/// the tag never decides.
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
    Claude { handle: String },
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

/// Who authored one `AgentUserMessage` echo. Not `SessionOrigin` (where a
/// session came from) nor the envelope's `role`/`from_agent` (the delivery's
/// connection facts): this names whose words the echo carries. `creation` is
/// its own value even when a human wrote the initial text, because the line
/// is daemon-composed (standing instructions plus preamble plus prompt).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserMessageAuthor {
    #[default]
    Human,
    Agent,
    Creation,
}

/// What one `AgentUserMessage` means in the session that displays it. This is
/// separate from [`UserMessageAuthor`]: an agent can author an outgoing echo,
/// an incoming relay, or a daemon notice.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserMessageKind {
    /// A stored row predating this field; retain the legacy text classifier.
    #[default]
    Unknown,
    Composer,
    OutgoingA2a,
    IncomingA2a,
    SystemNotice,
    Creation,
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
