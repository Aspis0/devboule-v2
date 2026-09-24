//! Client and daemon frames. Every message has a `type` tag so a human with a
//! pipe client can read a line and know what it is.

use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::error::WireError;
use crate::handshake::{ClientHello, DaemonHello};
use crate::project::{Project, Workspace, WorkspaceIsolation};
use crate::session::{
    ActiveTurnBehavior, AgentActivityState, AgentTaskState, Cursor, PermissionOutcome, Persistence,
    ResumeResult, Session, SessionEvent, SessionKind, SessionModeView, SessionModel,
    SubscriptionId,
};

/// The role a device is paired as, on the wire as `"client"` or `"daemon"`.
///
/// One definition for the wire and for the daemon's policy check
/// (`devboule-daemon/src/peer_policy.rs` re-exports this type) so a rename
/// cannot leave the two disagreeing.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PeerRole {
    Client,
    Daemon,
}

impl PeerRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Daemon => "daemon",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "client" => Some(Self::Client),
            "daemon" => Some(Self::Daemon),
            _ => None,
        }
    }
}

impl std::fmt::Display for PeerRole {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The capability names a paired peer may hold. The set is closed on the wire:
/// an unknown name is an error, never a silently dropped entry. The first five
/// name acts; `search` names the Oracle semantic search's per-device grant (the
/// decision of 2026-09-22: source snippets of this machine travel only to the
/// devices whose switch says so); `admin` names the rest of this device's
/// surface — the administrative acts no act-name covers (the decision of
/// 2026-09-21: a paired device is a full client, and only the permission model
/// itself stays local).
pub const PEER_CAPS: [&str; 7] = [
    "view",
    "send",
    "answer_permissions",
    "create_sessions",
    "roster",
    "search",
    "admin",
];
/// Every new pairing starts here: the whole set, which is the same decision as
/// the one above — "the phone is mine". A person restricts a device afterwards,
/// per device; `validate_caps` is what still refuses to leave a `Client` with
/// no `view`. `search` enters this default deliberately (owner's decision,
/// 2026-09-22): a new device may search this machine's code out of the box and
/// the Devices-panel switch is how that is taken back.
pub const PEER_DEFAULT_CAPS: [&str; 7] = [
    "view",
    "send",
    "answer_permissions",
    "create_sessions",
    "roster",
    "search",
    "admin",
];

/// One pairing code, with its `Debug` redacted.
///
/// A one-time secret that travels in two messages and through generic error
/// paths (`unexpected daemon frame: {message:?}`); the manual `Debug` is what
/// makes every one of those paths safe by construction rather than by review.
/// The buffer is overwritten on drop for the same reason.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct PairingSecret(String);

impl PairingSecret {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PairingSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("\"<redacted>\"")
    }
}

impl Drop for PairingSecret {
    fn drop(&mut self) {
        // SAFETY: every byte is replaced by NUL, which is valid UTF-8, so the
        // `String` invariant holds throughout. This is a best-effort wipe of
        // the buffer before it is freed; it deliberately avoids pulling a
        // cryptographic dependency into the protocol crate for eight bytes.
        unsafe {
            for byte in self.0.as_bytes_mut() {
                *byte = 0;
            }
        }
    }
}

/// One file the user attached to a prompt, carried as bytes.
///
/// `data` holds the bytes themselves, base64, and never a path. The reason is
/// the next slice of this feature: two daemons on two devices will relay a
/// request to each other, and a local file path does not survive that trip.
/// Keeping the bytes in the message means this field can be forwarded exactly
/// as it arrives; the file is written to disk by the daemon that is about to
/// talk to the provider, and never earlier.
///
/// `name` is the user's file name and is display metadata only. It may contain
/// `..`, a path separator, or a drive letter, so it is never used to build a
/// path — see `attachment_store` in the daemon for the name that is used.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptAttachment {
    pub name: String,
    pub mime_type: String,
    /// The bytes, base64. Never a path.
    pub data: String,
}

/// Hand-written, and it must stay hand-written: `data` is a whole image.
///
/// `ClientMessage` derives `Debug`, and the daemon formats whole frames into
/// error text — `dispatch_session`'s fallback arm says
/// `format!("unexpected session frame {other:?}")`, and that string is sent
/// back over the wire. A derived `Debug` here would put the full base64 of
/// every attached image into that reply, and into any log line or panic that
/// ever formats a frame. One rendered PDF page is ~128 KiB of base64 and a
/// deck is forty of them.
///
/// What someone debugging a frame needs is which attachment and how big; the
/// bytes have never once been the answer. The same treatment is applied to
/// `AcpImageBlock` in the daemon, for the same reason.
impl std::fmt::Debug for PromptAttachment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PromptAttachment")
            .field("name", &self.name)
            .field("mime_type", &self.mime_type)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// One stored attachment a prompt refers to, by digest and by session.
///
/// # Which digest this is
///
/// `digest` is the SHA-256, lowercase hex, of the bytes **as stored** — the
/// bytes `materialize` wrote after the metadata strip. It is the value the
/// stored file is named after, and it is the only digest that names a stored
/// file.
///
/// It is *not* the daemon's internal `attachment_digest`. That one hashes the
/// **decoded wire bytes** before the strip, exists only as the idempotency
/// fingerprint, and falls back to hashing the base64 text when the data does
/// not decode. A client cannot compute the stored digest — it cannot know what
/// the strip removed — which is why [`ClientMessage::SessionDeposit`] answers
/// with it.
///
/// # Why `session_id` is here and not merely implied by the frame
///
/// A digest resolves only inside the session it was deposited to, so a
/// reference that did not name its session would be a value with nowhere to
/// resolve. Carrying the session on every reference keeps a digest from ever
/// being handled on its own; `validate_attachment_references` refuses a
/// reference whose session is not the request's, and the store enforces the
/// same rule against the directory layout.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentReference {
    /// The session the deposit was made to.
    pub session_id: String,
    /// SHA-256, lowercase hex, of the stored bytes (64 characters). Never the
    /// daemon's wire-byte `attachment_digest`.
    pub digest: String,
    /// The size of the stored bytes, in bytes.
    ///
    /// [`DaemonMessage::SessionDeposited`] reports it so the app can refuse an
    /// over-budget import before depositing anything. Advisory only: the
    /// daemon re-stats the file on disk and never uses this number for the
    /// budget it enforces.
    pub stored_bytes: u64,
}

/// The bytes of one stored attachment, as the daemon hands them back.
///
/// `data` is base64, like [`PromptAttachment::data`]: bytes do not survive
/// the frame trip any other way, and a local path would not survive a
/// two-daemon relay. `mime_type` is the store's own statement from its
/// extension table — not the child's report from the finish event, which is
/// a claim about the same file, not the file.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredAttachment {
    /// The stored file's type, from the store's extension table.
    pub mime_type: String,
    /// The stored bytes, base64.
    pub data: String,
}

/// Messages the client writes.
///
/// # Session operations that cannot be collapsed
///
/// - [`ClientMessage::SessionDetach`]: drop **this client's** live
///   subscription. The process, reader, registry entry, and scrollback stay.
///   Other clients are unaffected. A later attach on the same id replays.
/// - [`ClientMessage::SessionClose`]: destroy the session. Kill the process
///   if any, drop in-memory state, invalidate the id. Unrecoverable except
///   by loading a *new* session from the journal (M3c).
/// - [`ClientMessage::SessionStop`]: terminate the running process (PTY child
///   / ACP agent) **and its descendants** — the session's job object is
///   terminated, not just its root, so an agent's children do not outlive the
///   stop — but **keep** the session object (id, scrollback, metadata).
///   Emits `exit`. Generation is unchanged — the instance died, it was not
///   replaced. Recreating a process under the same id is a different call
///   and MUST bump generation so a reconnecting client cannot treat the new
///   stream as the old one.
///
/// M2 already implements detach vs close with this meaning in-process. `stop`
/// is specified here so M3b does not have to change the protocol's meaning.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ClientMessage {
    Hello(ClientHello),
    Ping {
        id: u64,
    },
    Status {
        id: u64,
    },
    DaemonDiagnostics {
        id: u64,
    },
    Shutdown {
        id: u64,
    },
    SessionCreate {
        id: u64,
        workspace_id: Option<String>,
        kind: SessionKind,
        /// Catalog provider id (`grok`, `qwen`, `claude`, …). Single-word so
        /// Tauri v2's camelCase conversion cannot rename it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        /// The name the session is shown under, when the caller wants to choose
        /// one. Trimmed, then required to be 1..=[`crate::MAX_DISPLAY_NAME_CHARS`]
        /// characters; an absent field asks for the daemon's fallback title
        /// (recorded nowhere but the `Session.title` the session already had).
        /// A caller may not rename an existing session through this field: there
        /// is no rename frame and this is create-only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
        /// Deliberately absent: the creator of a session is the daemon's fact,
        /// written from the authenticated MCP bearer or from the creating frame
        /// itself, and never a field a client fills in. A claim to be a child of
        /// some other session would otherwise be one string away.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    SessionAttach {
        id: u64,
        session_id: String,
        subscription_id: SubscriptionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_cursor: Option<Cursor>,
    },
    SessionDetach {
        id: u64,
        session_id: String,
        subscription_id: SubscriptionId,
    },
    /// Claim the session's exclusive resize right for this subscription.
    SessionClaim {
        id: u64,
        session_id: String,
        subscription_id: SubscriptionId,
    },
    SessionClose {
        id: u64,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    SessionStop {
        id: u64,
        session_id: String,
        subscription_id: SubscriptionId,
    },
    SessionSend {
        id: u64,
        session_id: String,
        subscription_id: SubscriptionId,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<PromptAttachment>,
        #[serde(
            rename = "activeTurnBehavior",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        active_turn_behavior: Option<ActiveTurnBehavior>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
        /// References to attachments deposited earlier, resolved inside
        /// `session_id`.
        ///
        /// `#[serde(default)]` keeps recorded journal frames readable without
        /// moving the journal version. The inline `attachments` field above is
        /// untouched: one to four small images still travel in one round trip.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachment_references: Vec<AttachmentReference>,
    },
    /// Deliver text from one live agent session to another.
    AgentMessageSend {
        id: u64,
        from_session: String,
        to_session: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Store one attachment for a session and answer with a reference to the
    /// bytes **as stored**.
    ///
    /// One attachment per frame, deliberately. A rendered deck is several
    /// [`crate::MAX_FRAME_BYTES`] of base64, and the frame cap is also the
    /// per-connection buffer ceiling on the tailnet, so the bytes leave the
    /// frame here and [`ClientMessage::SessionSend`] later refers to them by
    /// digest. A single attachment is bounded by
    /// [`crate::MAX_ATTACHMENT_DATA_BYTES`], so every deposit frame is well
    /// under the ceiling.
    ///
    /// State-changing: it writes a file under the session, which is why
    /// [`ClientMessage::is_state_changing`] says `true` and the audit trail
    /// carries it. The reply is [`DaemonMessage::SessionDeposited`]; there is
    /// no notification form, because only the daemon can compute the stored
    /// digest.
    SessionDeposit {
        id: u64,
        session_id: String,
        attachment: PromptAttachment,
    },
    /// Read back the bytes of one deposited attachment.
    ///
    /// The reference is the value a deposit answered with (or the finish
    /// report carried): session, digest and claimed size together, so a
    /// digest is never handled without the session it resolves in. The
    /// daemon compares the claimed size against the store's and refuses a
    /// disagreement, exactly as the send path does, and refuses a file over
    /// the artifact cap. The reply is [`DaemonMessage::SessionAttachment`].
    SessionAttachmentRead {
        id: u64,
        reference: AttachmentReference,
    },
    SessionResize {
        id: u64,
        session_id: String,
        subscription_id: SubscriptionId,
        cols: u16,
        rows: u16,
    },
    SessionInterrupt {
        id: u64,
        session_id: String,
        subscription_id: SubscriptionId,
    },
    SessionSetModel {
        id: u64,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    SessionSetMode {
        id: u64,
        session_id: String,
        mode_id: String,
    },
    SessionPermissionRespond {
        id: u64,
        session_id: String,
        subscription_id: SubscriptionId,
        request_id: String,
        outcome: PermissionOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        option_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// External announcement from an agent hook (or our stub). The
    /// `session_id` in the frame is a claim, not a proof: the daemon
    /// verifies the named-pipe peer before accepting it.
    SessionReportAgent {
        id: u64,
        session_id: String,
        source: String,
        agent: String,
        state: AgentActivityState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_session_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_start_source: Option<String>,
    },
    SessionsList {
        id: u64,
    },
    SessionsWatch {
        id: u64,
    },
    SessionsUnwatch {
        id: u64,
    },
    /// Per-connection foreground presence. `focused_session_id` is only
    /// meaningful while `app_visible` is true.
    SessionsPresence {
        id: u64,
        focused_session_id: Option<String>,
        app_visible: bool,
    },
    SessionResume {
        id: u64,
        persistence: Persistence,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    JournalUsage {
        id: u64,
    },
    JournalRetentionGet {
        id: u64,
    },
    JournalRetentionSet {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age_ms: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_sessions: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_max_bytes: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    SessionDelete {
        id: u64,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    ProjectsList {
        id: u64,
    },
    ProjectAdd {
        id: u64,
        path: String,
    },
    WorkspacesList {
        id: u64,
        project_id: String,
    },
    /// The uncommitted working-tree state of one workspace, for the Changes
    /// panel. A read: the daemon resolves the directory from `workspace_id`
    /// and the caller's `path` field — the one every `Workspace` carries — is
    /// never consulted, because it is declared display-only
    /// (`src/types/ipc.ts`). The reply is [`DaemonMessage::WorkspaceGit`].
    WorkspaceGitStatus {
        id: u64,
        workspace_id: String,
    },
    /// The uncommitted diff of one workspace file, for the Changes panel's
    /// detail view. A read like [`Self::WorkspaceGitStatus`]: the daemon
    /// resolves the directory from `workspace_id`, and `path` is confined to
    /// a relative path inside it — refused when absolute, when it climbs out
    /// with `..`, or when it resolves outside — because the `path` every
    /// `Workspace` carries is declared display-only (`src/types/ipc.ts`).
    /// The reply is [`DaemonMessage::WorkspaceGitFile`].
    WorkspaceGitDiff {
        id: u64,
        workspace_id: String,
        /// Path relative to the workspace folder, spelled the way
        /// `git status` printed it.
        path: String,
    },
    /// Stage paths in the Changes panel's index — `git add` over a
    /// confined selection (modified, new, or a tracked file's deletion),
    /// literal pathspecs and `--` so a file named `-f` is a file. A
    /// **write** like [`Self::WorkspaceFileRename`]: the daemon resolves
    /// the folder from `workspace_id`, confines every path of `paths`
    /// through the same two layers the reads use **before spawning
    /// anything**, refuses the repository's metadata in any spelling and
    /// the workspace's own folder, and runs under the workspace's write
    /// mutex (two of this daemon's writes never cross; the owner's own
    /// git in a terminal stays `index.lock`'s arbitrage, answered with a
    /// static sentence). No confirmation: staging loses nothing. The cap
    /// is **500 paths per frame**, refused before any spawn. The reply is
    /// [`DaemonMessage::WorkspaceGitWrite`] — one reply for this frame
    /// and the three git writes below, because only the caller knows
    /// which act it sent.
    WorkspaceGitStage {
        id: u64,
        workspace_id: String,
        /// Paths relative to the repository root, spelled the way `git
        /// status` printed them; at most 500 per frame.
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Unstage paths — the index entry goes back to `HEAD` and the
    /// worktree keeps its bytes, with the declared fallback for an `HEAD`
    /// that does not resolve (the paths leave the index directly,
    /// Paseo's own step). A **write** like [`Self::WorkspaceGitStage`],
    /// same guards, same cap, same mutex, no confirmation: this act
    /// loses nothing. The reply is [`DaemonMessage::WorkspaceGitWrite`].
    WorkspaceGitUnstage {
        id: u64,
        workspace_id: String,
        /// Paths relative to the repository root; at most 500 per frame.
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Discard paths — the act of this group that **loses data**: the
    /// selection returns to `HEAD` and untracked paths are deleted. The
    /// wire carries **no confirmation**, like [`Self::WorkspaceFileDelete`]:
    /// the asking screen is the local Changes panel's own gate (it
    /// confirms through a native dialog before it sends), and a peer
    /// holding the admin capability acts under that capability as behind
    /// every administrative door. Paseo's sequence, pathspec-scoped at
    /// every step. A **write** like [`Self::WorkspaceGitStage`], same
    /// guards, same cap, same mutex. The reply is
    /// [`DaemonMessage::WorkspaceGitWrite`].
    WorkspaceGitDiscard {
        id: u64,
        workspace_id: String,
        /// Paths relative to the repository root; at most 500 per frame.
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Commit **what is staged and nothing else** — no `add -A` exists on
    /// this frame (the divergence from Paseo's `commitChanges`, which
    /// stages everything because it has no separate stage; this panel
    /// does, `DECISIONS-write.md` §2), and the message is the caller's
    /// own: empty after trimming is refused before anything spawns —
    /// no message is ever generated. A **write**: same folder resolution,
    /// same probe, same mutex; a hook that dies answers as operation plus
    /// exit code, never git's stderr. The reply is
    /// [`DaemonMessage::WorkspaceGitWrite`].
    WorkspaceGitCommit {
        id: u64,
        workspace_id: String,
        /// The commit message, written by hand; must be non-empty after
        /// trimming.
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// The entries of one workspace folder, for the Files panel's tree. A read
    /// like [`Self::WorkspaceGitStatus`]: the daemon resolves the directory
    /// from `workspace_id`, and `path` is a relative path inside it — empty
    /// names the folder itself — confined before anything is opened, because
    /// the `path` every `Workspace` carries is declared display-only
    /// (`src/types/ipc.ts`). One directory per request: no recursion, no tree.
    /// The reply is [`DaemonMessage::WorkspaceFiles`].
    WorkspaceFilesList {
        id: u64,
        workspace_id: String,
        /// Path relative to the workspace folder; the empty string is the
        /// folder's own top level.
        path: String,
    },
    /// The content of one workspace file, for the Files panel's preview. A read
    /// like [`Self::WorkspaceFilesList`]: the daemon resolves the directory
    /// from `workspace_id` and confines `path` to a relative path inside it
    /// before anything is opened — and nothing is written behind this frame.
    /// One **window** per request: `from_line` and `line_count` address it,
    /// both absent being the first window — the whole frame a caller that
    /// never heard of windows still sends, unchanged. The reply is
    /// [`DaemonMessage::WorkspaceFileContent`].
    WorkspaceFileRead {
        id: u64,
        workspace_id: String,
        /// Path relative to the workspace folder, of a file — the spelling a
        /// listing entry already handed back.
        path: String,
        /// First line of the window, 1-based; absent is line 1. A line the
        /// file does not have answers with no lines and `has_more: false` —
        /// past the end is a window, not a failure.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_line: Option<u64>,
        /// How many lines the window may hold; absent lets the frame's byte
        /// cap alone decide. Asked with 0 it still takes one line — a
        /// window of nothing is not a request this frame answers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_count: Option<u64>,
    },
    /// Rename one entry inside a workspace — the Files panel's inline rename.
    /// A **write**: the daemon resolves the folder from `workspace_id`,
    /// confines `path` like [`Self::WorkspaceFileRead`] does, validates `name`
    /// as one name with no separator (and never `.git` in any spelling),
    /// refuses a target name that is already taken (a rename that changes
    /// only the case of the same entry is allowed), and — for an entry git
    /// tracks — performs the rename with `git mv`, so the act arrives in the
    /// Changes panel as a staged rename rather than as a deletion beside a
    /// new file. No confirmation exists on this frame: a rename loses no
    /// data. The reply is [`DaemonMessage::WorkspaceFileRenamed`].
    WorkspaceFileRename {
        id: u64,
        workspace_id: String,
        /// Path relative to the workspace folder of the entry being renamed —
        /// the spelling a listing entry already handed back.
        path: String,
        /// The entry's new name: one name, judged on its trimmed spelling.
        /// The frontend checks it too as a courtesy; this daemon's check is
        /// the rule.
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Duplicate one entry inside a workspace — the Files panel's Duplicate.
    /// A **write** like [`Self::WorkspaceFileRename`], with the same
    /// confinement and the same guards. The new name is chosen by the
    /// daemon (`a copy.txt`, then `a copy 2.txt`, …) and the copy is created
    /// exclusive: an existing entry is never overwritten, and a folder is
    /// copied whole — refusing (and removing what it made) if it meets a
    /// link, because nothing in this tree is ever copied *through* a link.
    /// Git is not consulted: a duplicate is not a rename and stages nothing.
    /// The reply is [`DaemonMessage::WorkspaceFileDuplicated`].
    WorkspaceFileDuplicate {
        id: u64,
        workspace_id: String,
        /// Path relative to the workspace folder of the entry to duplicate.
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Delete one entry inside a workspace — the Files panel's Delete, the
    /// one act of the group that **loses data**. The wire carries **no
    /// confirmation**: this frame performs no confirmation and checks none.
    /// The confirmation is the local Files screen's own gate (the panel
    /// asks through a native dialog before it sends); a peer holding the
    /// admin capability can send this frame and the daemon acts under that
    /// capability, as behind every administrative door — whether a paired
    /// device may delete without a dialog is an open product question
    /// (`DECISIONS-write.md`, for the owner). The daemon re-judges the path
    /// with the same guards the reads use — the workspace's own folder
    /// refused, the repository's metadata refused in every spelling, and a
    /// link named as the act's target refused, never followed (where
    /// Paseo's delete unlinks the link, one rule for the whole tree). A
    /// link **inside** a deleted folder goes with the folder — removed as
    /// an entry, never followed — and the folder goes whole, its children
    /// with it. The act is irreversible: no undo exists on this frame. The
    /// reply is [`DaemonMessage::WorkspaceFileDeleted`].
    WorkspaceFileDelete {
        id: u64,
        workspace_id: String,
        /// Path relative to the workspace folder of the entry to delete.
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// Stage one workspace file as the Files panel's preview: the daemon
    /// confines `path` like [`Self::WorkspaceFileRead`] and refuses what
    /// that read refuses — plus any extension the panel never shows as
    /// media — then copies the bytes into the runtime directory's
    /// `previews` folder, the one folder the app concedes to Tauri's asset
    /// protocol (the workspace itself is never in that scope). The reply is
    /// [`DaemonMessage::WorkspaceFilePreviewStaged`], carrying the copy's
    /// absolute path; a stage also clears the copies of earlier stages, so
    /// at most one copy rests in the folder at a time.
    WorkspaceFilePreviewStage {
        id: u64,
        workspace_id: String,
        /// Path relative to the workspace folder, of a file — the spelling
        /// a listing entry already handed back.
        path: String,
    },
    /// Delete every copy [`Self::WorkspaceFilePreviewStage`] left — the
    /// preview's revoke. The panel sends it when the selection leaves a
    /// staged file and when the panel closes: revoking is deleting the
    /// copy, never withdrawing a scope (a Tauri asset concession cannot be
    /// taken back until the process restarts). The reply is
    /// [`DaemonMessage::Ok`].
    WorkspaceFilePreviewUnstage {
        id: u64,
    },
    WorkspaceCreate {
        id: u64,
        project_id: String,
        isolation: WorkspaceIsolation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    /// Remove a worktree workspace. Never deletes the branch. A dirty
    /// checkout fails unless `force` is true.
    WorkspaceDelete {
        id: u64,
        workspace_id: String,
        #[serde(default)]
        force: bool,
    },
    ProvidersList {
        id: u64,
    },
    ProvidersRefresh {
        id: u64,
    },
    ProviderUpdate {
        id: u64,
        provider_id: String,
    },
    /// Plugin-backend tenant. `method` is a capability name (`workspace.root`
    /// today). The daemon returns [`ErrorCode::Unimplemented`]; it is not a
    /// plugin backend.
    Invoke {
        id: u64,
        method: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },
    /// This device's peers, the pending Client-role confirmations, and this
    /// device's own advertised identity.
    DevicesList {
        id: u64,
    },
    /// Ask the responding daemon which agents it is running **right now**,
    /// for the user who approved this pairing. Answered live from the
    /// responder's registry — never from journal rows — and nothing is
    /// stored: a reply is a snapshot that is stale the moment it is read,
    /// and a session id in it is unique only within the responding daemon.
    PeerAgentsList {
        id: u64,
    },
    /// Show a pairing code on **this** device. `role` is the role this device
    /// will have in the pairing.
    PairingStart {
        id: u64,
        role: PeerRole,
    },
    /// Type a code shown by another device. `role` is this device's role, and
    /// `address` is the other device's `ip:port`.
    PairingComplete {
        id: u64,
        address: String,
        code: PairingSecret,
        role: PeerRole,
    },
    /// Answer a Client-role pairing parked on this device.
    PairingConfirm {
        id: u64,
        device_id: String,
        accept: bool,
    },
    PeerRevoke {
        id: u64,
        device_id: String,
    },
    /// Replace a peer's capability set with exactly the names in `caps`.
    PeerSetCaps {
        id: u64,
        device_id: String,
        caps: Vec<String>,
    },
    /// Read every stored per-provider tool policy. This device's own settings:
    /// a paired device needs the administrative capability to read or change
    /// this device's tool gates.
    ToolPolicyGet {
        id: u64,
    },
    /// Replace one provider's tool policy. An absent `enabled` means enabled;
    /// so does `true`. `disabled_tools` is the complete per-tool set, never a
    /// delta.
    ToolPolicySet {
        id: u64,
        provider_id: String,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        disabled_tools: Vec<String>,
    },
    /// Read the whole agent-profile document: the ordered profile list and the
    /// standing instructions. This device's own settings: profiles carry the
    /// modes and tool overlays this machine's agents are created in, so a paired
    /// device needs the administrative capability to read or change them.
    AgentProfilesGet {
        id: u64,
    },
    /// Replace the whole agent-profile document — the list **and** the standing
    /// instructions, never one half. The order is the human's and is kept as
    /// sent; nothing sorts it.
    ///
    /// `id` on a profile is the caller's when it has one and absent when it is
    /// new, in which case the daemon mints one. A request that names a provider
    /// the catalog does not publish, an id already used twice, a name outside
    /// 1..60 characters, a `note` over 2 KiB or `standingInstructions` over
    /// 8 KiB is refused by name and nothing is written.
    AgentProfilesSet {
        id: u64,
        document: AgentProfilesDocument,
    },
    /// Ask what one provider offers — its models and its modes — so the
    /// profile form can be authored from real vocabulary instead of free
    /// text. `provider` is a catalog provider id or alias, canonicalised the
    /// way the profile store canonicalises one — trimmed first, then
    /// resolved by the catalog's own walk — and the reply carries the
    /// canonical id back. `refresh: false` is a cached read; `refresh: true`
    /// re-probes now, which for most providers briefly starts the provider's
    /// process (Claude usually costs a file scan; the one process it can
    /// start is the native version probe, and only while its installed
    /// version is still unknown).
    ///
    /// The profile store's companion, and the same rule: a paired device needs
    /// the administrative capability to read it. The handshake
    /// capability `provider_vocabulary` is the feature gate: a daemon without
    /// it predates this query, which is a different fact from the query
    /// answering `absent`, and the two must never be collapsed.
    ProviderVocabularyGet {
        id: u64,
        provider: String,
        refresh: bool,
    },
    /// Read the permission-delegation switch: whether an agent that created a
    /// child may answer that child's permission cards. The reply carries
    /// `source` beside `enabled`, because "off" is three different facts the
    /// app renders differently — the human turned it off (`file`), nobody ever
    /// configured it (`default`), or the settings file was damaged and the
    /// daemon is reading off until it is repaired (`quarantined`).
    ///
    /// The profile store's companion, read: the switch decides what
    /// this machine's agents may answer on their children's behalf, so a
    /// paired device needs the administrative capability to read it. The
    /// handshake capability `permission_delegation` is the feature
    /// gate, the same pairing the profiles pair uses.
    DelegationGet {
        id: u64,
    },
    /// Set the permission-delegation switch. One boolean for the whole daemon:
    /// there are no per-session grants, no pause and no cap anywhere in this
    /// slice — a session id can name a stranger's session after a daemon
    /// restart, so nothing per-session may exist on disk or in memory to
    /// revoke. `false` is immediate: every delegated answer arriving after it
    /// is refused, and a card already surfaced to a creator simply stays what
    /// it always was — pending for the human.
    DelegationSet {
        id: u64,
        enabled: bool,
    },
}

/// Trim a requested display name and check it, or say why it cannot be used.
///
/// One function for the two halves because they are one rule: the length is
/// judged on the trimmed value, and the trimmed value is what the caller stores;
/// a daemon that validated one string and stored another would cap a name it did
/// not keep. The rules are [`crate::MAX_DISPLAY_NAME_CHARS`] characters and at
/// least one, and neither sentence echoes the name back — it is a string the
/// caller sent with nothing bounding its length, and repeating it would move a
/// flood out of the frame and into an error the app renders.
///
/// Refusing an empty name instead of treating it as absent is deliberate:
/// `None` is how a caller says "no name", and a caller that sent `""` (or only
/// whitespace) meant to name the session something it did not manage to say.
pub fn validate_display_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("A session display name is required; it was empty.".to_string());
    }
    let length = trimmed.chars().count();
    if length > crate::MAX_DISPLAY_NAME_CHARS {
        return Err(format!(
            "A session display name is {length} characters; the limit is {}.",
            crate::MAX_DISPLAY_NAME_CHARS
        ));
    }
    Ok(trimmed.to_string())
}

impl ClientMessage {
    pub fn request_id(&self) -> Option<u64> {
        match self {
            Self::Hello(_) => None,
            Self::Ping { id }
            | Self::Status { id }
            | Self::DaemonDiagnostics { id }
            | Self::Shutdown { id }
            | Self::SessionCreate { id, .. }
            | Self::SessionAttach { id, .. }
            | Self::SessionDetach { id, .. }
            | Self::SessionClaim { id, .. }
            | Self::SessionClose { id, .. }
            | Self::SessionStop { id, .. }
            | Self::SessionSend { id, .. }
            | Self::AgentMessageSend { id, .. }
            | Self::SessionDeposit { id, .. }
            | Self::SessionAttachmentRead { id, .. }
            | Self::SessionResize { id, .. }
            | Self::SessionInterrupt { id, .. }
            | Self::SessionSetModel { id, .. }
            | Self::SessionSetMode { id, .. }
            | Self::SessionPermissionRespond { id, .. }
            | Self::SessionReportAgent { id, .. }
            | Self::SessionsList { id }
            | Self::SessionsWatch { id }
            | Self::SessionsUnwatch { id }
            | Self::SessionsPresence { id, .. }
            | Self::SessionResume { id, .. }
            | Self::JournalUsage { id }
            | Self::JournalRetentionGet { id }
            | Self::JournalRetentionSet { id, .. }
            | Self::SessionDelete { id, .. }
            | Self::ProjectsList { id }
            | Self::ProjectAdd { id, .. }
            | Self::WorkspacesList { id, .. }
            | Self::WorkspaceGitStatus { id, .. }
            | Self::WorkspaceGitDiff { id, .. }
            | Self::WorkspaceGitStage { id, .. }
            | Self::WorkspaceGitUnstage { id, .. }
            | Self::WorkspaceGitDiscard { id, .. }
            | Self::WorkspaceGitCommit { id, .. }
            | Self::WorkspaceFilesList { id, .. }
            | Self::WorkspaceFileRead { id, .. }
            | Self::WorkspaceFileRename { id, .. }
            | Self::WorkspaceFileDuplicate { id, .. }
            | Self::WorkspaceFileDelete { id, .. }
            | Self::WorkspaceFilePreviewStage { id, .. }
            | Self::WorkspaceFilePreviewUnstage { id }
            | Self::WorkspaceCreate { id, .. }
            | Self::WorkspaceDelete { id, .. }
            | Self::ProvidersList { id }
            | Self::ProvidersRefresh { id }
            | Self::ProviderUpdate { id, .. }
            | Self::Invoke { id, .. }
            | Self::DevicesList { id }
            | Self::PeerAgentsList { id }
            | Self::PairingStart { id, .. }
            | Self::PairingComplete { id, .. }
            | Self::PairingConfirm { id, .. }
            | Self::PeerRevoke { id, .. }
            | Self::PeerSetCaps { id, .. }
            | Self::ToolPolicyGet { id }
            | Self::ToolPolicySet { id, .. }
            | Self::AgentProfilesGet { id }
            | Self::AgentProfilesSet { id, .. }
            | Self::ProviderVocabularyGet { id, .. }
            | Self::DelegationGet { id }
            | Self::DelegationSet { id, .. } => Some(*id),
        }
    }

    pub fn idempotency_key(&self) -> Option<&str> {
        match self {
            Self::SessionCreate {
                idempotency_key, ..
            }
            | Self::SessionSend {
                idempotency_key, ..
            }
            | Self::AgentMessageSend {
                idempotency_key, ..
            }
            | Self::SessionPermissionRespond {
                idempotency_key, ..
            }
            | Self::SessionResume {
                idempotency_key, ..
            }
            | Self::SessionClose {
                idempotency_key, ..
            }
            | Self::JournalRetentionSet {
                idempotency_key, ..
            }
            | Self::SessionDelete {
                idempotency_key, ..
            }
            | Self::WorkspaceFileRename {
                idempotency_key, ..
            }
            | Self::WorkspaceFileDuplicate {
                idempotency_key, ..
            }
            | Self::WorkspaceFileDelete {
                idempotency_key, ..
            }
            | Self::WorkspaceGitStage {
                idempotency_key, ..
            }
            | Self::WorkspaceGitUnstage {
                idempotency_key, ..
            }
            | Self::WorkspaceGitDiscard {
                idempotency_key, ..
            }
            | Self::WorkspaceGitCommit {
                idempotency_key, ..
            } => idempotency_key.as_deref(),
            Self::Hello(_)
            | Self::Ping { .. }
            | Self::Status { .. }
            | Self::DaemonDiagnostics { .. }
            | Self::Shutdown { .. }
            | Self::SessionAttach { .. }
            | Self::SessionDetach { .. }
            | Self::SessionClaim { .. }
            | Self::SessionStop { .. }
            | Self::SessionDeposit { .. }
            | Self::SessionAttachmentRead { .. }
            | Self::SessionResize { .. }
            | Self::SessionInterrupt { .. }
            | Self::SessionSetModel { .. }
            | Self::SessionSetMode { .. }
            | Self::SessionReportAgent { .. }
            | Self::SessionsList { .. }
            | Self::SessionsWatch { .. }
            | Self::SessionsUnwatch { .. }
            | Self::SessionsPresence { .. }
            | Self::JournalUsage { .. }
            | Self::JournalRetentionGet { .. }
            | Self::ProvidersList { .. }
            | Self::ProvidersRefresh { .. }
            | Self::ProviderUpdate { .. }
            | Self::ProjectsList { .. }
            | Self::ProjectAdd { .. }
            | Self::WorkspacesList { .. }
            | Self::WorkspaceGitStatus { .. }
            | Self::WorkspaceGitDiff { .. }
            | Self::WorkspaceFilesList { .. }
            | Self::WorkspaceFileRead { .. }
            | Self::WorkspaceFilePreviewStage { .. }
            | Self::WorkspaceFilePreviewUnstage { .. }
            | Self::WorkspaceCreate { .. }
            | Self::WorkspaceDelete { .. }
            | Self::Invoke { .. }
            | Self::DevicesList { .. }
            | Self::PeerAgentsList { .. }
            | Self::PairingStart { .. }
            | Self::PairingComplete { .. }
            | Self::PairingConfirm { .. }
            | Self::PeerRevoke { .. }
            | Self::PeerSetCaps { .. }
            | Self::ToolPolicyGet { .. }
            | Self::ToolPolicySet { .. }
            | Self::AgentProfilesGet { .. }
            | Self::AgentProfilesSet { .. }
            | Self::ProviderVocabularyGet { .. }
            | Self::DelegationGet { .. }
            | Self::DelegationSet { .. } => None,
        }
    }

    /// ASCII variant name, for audit rows. A closed match: a new variant must
    /// name itself here as well as decide its peer policy in `peer_policy`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello(_) => "Hello",
            Self::Ping { .. } => "Ping",
            Self::Status { .. } => "Status",
            Self::DaemonDiagnostics { .. } => "DaemonDiagnostics",
            Self::Shutdown { .. } => "Shutdown",
            Self::SessionCreate { .. } => "SessionCreate",
            Self::SessionAttach { .. } => "SessionAttach",
            Self::SessionDetach { .. } => "SessionDetach",
            Self::SessionClaim { .. } => "SessionClaim",
            Self::SessionClose { .. } => "SessionClose",
            Self::SessionStop { .. } => "SessionStop",
            Self::SessionSend { .. } => "SessionSend",
            Self::AgentMessageSend { .. } => "AgentMessageSend",
            Self::SessionDeposit { .. } => "SessionDeposit",
            Self::SessionAttachmentRead { .. } => "SessionAttachmentRead",
            Self::SessionResize { .. } => "SessionResize",
            Self::SessionInterrupt { .. } => "SessionInterrupt",
            Self::SessionSetModel { .. } => "SessionSetModel",
            Self::SessionSetMode { .. } => "SessionSetMode",
            Self::SessionPermissionRespond { .. } => "SessionPermissionRespond",
            Self::SessionReportAgent { .. } => "SessionReportAgent",
            Self::SessionsList { .. } => "SessionsList",
            Self::SessionsWatch { .. } => "SessionsWatch",
            Self::SessionsUnwatch { .. } => "SessionsUnwatch",
            Self::SessionsPresence { .. } => "SessionsPresence",
            Self::SessionResume { .. } => "SessionResume",
            Self::JournalUsage { .. } => "JournalUsage",
            Self::JournalRetentionGet { .. } => "JournalRetentionGet",
            Self::JournalRetentionSet { .. } => "JournalRetentionSet",
            Self::SessionDelete { .. } => "SessionDelete",
            Self::ProjectsList { .. } => "ProjectsList",
            Self::ProjectAdd { .. } => "ProjectAdd",
            Self::WorkspacesList { .. } => "WorkspacesList",
            Self::WorkspaceGitStatus { .. } => "WorkspaceGitStatus",
            Self::WorkspaceGitDiff { .. } => "WorkspaceGitDiff",
            Self::WorkspaceGitStage { .. } => "WorkspaceGitStage",
            Self::WorkspaceGitUnstage { .. } => "WorkspaceGitUnstage",
            Self::WorkspaceGitDiscard { .. } => "WorkspaceGitDiscard",
            Self::WorkspaceGitCommit { .. } => "WorkspaceGitCommit",
            Self::WorkspaceFilesList { .. } => "WorkspaceFilesList",
            Self::WorkspaceFileRead { .. } => "WorkspaceFileRead",
            Self::WorkspaceFileRename { .. } => "WorkspaceFileRename",
            Self::WorkspaceFileDuplicate { .. } => "WorkspaceFileDuplicate",
            Self::WorkspaceFileDelete { .. } => "WorkspaceFileDelete",
            Self::WorkspaceFilePreviewStage { .. } => "WorkspaceFilePreviewStage",
            Self::WorkspaceFilePreviewUnstage { .. } => "WorkspaceFilePreviewUnstage",
            Self::WorkspaceCreate { .. } => "WorkspaceCreate",
            Self::WorkspaceDelete { .. } => "WorkspaceDelete",
            Self::ProvidersList { .. } => "ProvidersList",
            Self::ProvidersRefresh { .. } => "ProvidersRefresh",
            Self::ProviderUpdate { .. } => "ProviderUpdate",
            Self::Invoke { .. } => "Invoke",
            Self::DevicesList { .. } => "DevicesList",
            Self::PeerAgentsList { .. } => "PeerAgentsList",
            Self::PairingStart { .. } => "PairingStart",
            Self::PairingComplete { .. } => "PairingComplete",
            Self::PairingConfirm { .. } => "PairingConfirm",
            Self::PeerRevoke { .. } => "PeerRevoke",
            Self::PeerSetCaps { .. } => "PeerSetCaps",
            Self::ToolPolicyGet { .. } => "ToolPolicyGet",
            Self::ToolPolicySet { .. } => "ToolPolicySet",
            Self::AgentProfilesGet { .. } => "AgentProfilesGet",
            Self::AgentProfilesSet { .. } => "AgentProfilesSet",
            Self::DelegationGet { .. } => "DelegationGet",
            Self::DelegationSet { .. } => "DelegationSet",
            Self::ProviderVocabularyGet { .. } => "ProviderVocabularyGet",
        }
    }

    /// Whether this request may change durable state, and therefore may
    /// produce an audit row.
    ///
    /// A closed match, because the two failure modes are asymmetric: an audit
    /// row for a read is a disk sink a `Ping` loop can drive (muse M1), and a
    /// missing row for a write is an untraceable remote action. Listing every
    /// variant forces the next one to pick a side.
    pub fn is_state_changing(&self) -> bool {
        match self {
            Self::Hello(_)
            | Self::Ping { .. }
            | Self::Status { .. }
            | Self::DaemonDiagnostics { .. }
            | Self::SessionsList { .. }
            | Self::SessionsWatch { .. }
            | Self::JournalUsage { .. }
            | Self::JournalRetentionGet { .. }
            | Self::ProjectsList { .. }
            | Self::WorkspacesList { .. }
            | Self::WorkspaceGitStatus { .. }
            | Self::WorkspaceGitDiff { .. }
            | Self::WorkspaceFilesList { .. }
            | Self::WorkspaceFileRead { .. }
            | Self::ProvidersList { .. }
            | Self::DevicesList { .. }
            | Self::PeerAgentsList { .. }
            | Self::SessionAttachmentRead { .. }
            | Self::ToolPolicyGet { .. }
            | Self::AgentProfilesGet { .. }
            | Self::ProviderVocabularyGet { .. }
            | Self::DelegationGet { .. } => false,

            Self::Shutdown { .. }
            | Self::SessionCreate { .. }
            | Self::SessionAttach { .. }
            | Self::SessionDetach { .. }
            | Self::SessionClaim { .. }
            | Self::SessionClose { .. }
            | Self::SessionStop { .. }
            | Self::SessionSend { .. }
            | Self::AgentMessageSend { .. }
            | Self::SessionDeposit { .. }
            | Self::SessionResize { .. }
            | Self::SessionInterrupt { .. }
            | Self::SessionSetModel { .. }
            | Self::SessionSetMode { .. }
            | Self::SessionPermissionRespond { .. }
            | Self::SessionReportAgent { .. }
            | Self::SessionsUnwatch { .. }
            | Self::SessionsPresence { .. }
            | Self::SessionResume { .. }
            | Self::JournalRetentionSet { .. }
            | Self::SessionDelete { .. }
            | Self::ProjectAdd { .. }
            | Self::WorkspaceCreate { .. }
            | Self::WorkspaceDelete { .. }
            | Self::WorkspaceFileRename { .. }
            | Self::WorkspaceFileDuplicate { .. }
            // The one act that destroys data — audited like the two writes
            // above, and the reason its frame exists at all.
            | Self::WorkspaceFileDelete { .. }
            // The four git writes: they move paths through the index and
            // (commit) into history — a disk sink an audit row must cover,
            // exactly like the file writes above them.
            | Self::WorkspaceGitStage { .. }
            | Self::WorkspaceGitUnstage { .. }
            | Self::WorkspaceGitDiscard { .. }
            | Self::WorkspaceGitCommit { .. }
            // Both write the runtime directory's `previews` folder — a
            // stage creates a copy, an unstage deletes it — so both earn an
            // audit row like the two writes above them.
            | Self::WorkspaceFilePreviewStage { .. }
            | Self::WorkspaceFilePreviewUnstage { .. }
            | Self::ProvidersRefresh { .. }
            | Self::ProviderUpdate { .. }
            | Self::Invoke { .. }
            | Self::PairingStart { .. }
            | Self::PairingComplete { .. }
            | Self::PairingConfirm { .. }
            | Self::PeerRevoke { .. }
            | Self::PeerSetCaps { .. }
            | Self::ToolPolicySet { .. }
            | Self::AgentProfilesSet { .. }
            | Self::DelegationSet { .. } => true,
        }
    }
}

/// Lifecycle state reported for an inter-agent message.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMessageState {
    Accepted,
    Queued,
    Delivered,
    Started,
    Completed,
    RejectedAbsent,
    RejectedUnpaired,
    /// The caller was authenticated — a paired device, or the person at this
    /// machine — and the message was refused anyway: the session exists and is
    /// the caller's to reach, but it will not take this message (A2-07). A
    /// refusal by *identity* is `RejectedUnpaired`; this one is a refusal by
    /// policy, which is what the daemon answers with `ErrorCode::Unauthorized`
    /// for a steer a paired device may not turn into an interrupt.
    RejectedDenied,
    Expired,
    Failed,
}

/// Messages the daemon writes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum DaemonMessage {
    Hello(DaemonHello),
    /// Handshake-level or request-level error. `error.id` is `None` for a
    /// handshake failure; the connection is then closed.
    Error(WireError),
    Pong {
        id: u64,
        ts_ms: u64,
    },
    Status {
        id: u64,
        #[serde(flatten)]
        body: DaemonStatusBody,
    },
    Diagnostics {
        id: u64,
        report: serde_json::Value,
    },
    Shutdown {
        id: u64,
        accepted: bool,
        /// Why a request was refused, present only when `accepted` is false.
        /// The daemon refuses a `Shutdown` that would stop it out from under
        /// another local app client; the reason is for that caller's log or
        /// dialog, never for a peer's screen.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Session {
        id: u64,
        session: Session,
    },
    Sessions {
        id: u64,
        sessions: Vec<Session>,
    },
    Projects {
        id: u64,
        projects: Vec<Project>,
    },
    Project {
        id: u64,
        project: Project,
    },
    Workspaces {
        id: u64,
        workspaces: Vec<Workspace>,
    },
    /// The reply to [`ClientMessage::WorkspaceGitStatus`].
    WorkspaceGit {
        id: u64,
        status: WorkspaceGitStatus,
    },
    /// The reply to [`ClientMessage::WorkspaceGitDiff`]: the diff of one
    /// file, or a refusal of it.
    WorkspaceGitFile {
        id: u64,
        file: WorkspaceGitFileDiff,
    },
    /// The reply to [`ClientMessage::WorkspaceGitStage`],
    /// [`ClientMessage::WorkspaceGitUnstage`],
    /// [`ClientMessage::WorkspaceGitDiscard`] and
    /// [`ClientMessage::WorkspaceGitCommit`] — one reply for the four
    /// git writes, because only the caller knows which act it sent:
    /// `error` is `null` when the act landed and the refusing sentence
    /// otherwise. Like `WorkspaceGitStatus.error` on these frames the
    /// sentence is written without any path and without git's stderr —
    /// `error` here does not pass the redaction seam.
    WorkspaceGitWrite {
        id: u64,
        error: Option<String>,
    },
    /// The reply to [`ClientMessage::WorkspaceFilesList`]: the entries of one
    /// folder, or a refusal of it.
    WorkspaceFiles {
        id: u64,
        directory: WorkspaceDirectory,
    },
    /// The reply to [`ClientMessage::WorkspaceFileRead`]: one window of the
    /// file's content, a deliberate withholding of it (`too_large`,
    /// `binary`), or a refusal of it. Flattened, so the wire is one flat object beside
    /// `id` — the same shape [`DaemonMessage::Status`] gives its body.
    WorkspaceFileContent {
        id: u64,
        #[serde(flatten)]
        file: WorkspaceFileContent,
    },
    /// The reply to [`ClientMessage::WorkspaceFilePreviewStage`]: the copy's
    /// absolute path beside the source file's stat, or the sentence the
    /// refusal stopped on. Flattened beside `id` the same way
    /// [`DaemonMessage::WorkspaceFileContent`] flattens its body.
    WorkspaceFilePreviewStaged {
        id: u64,
        #[serde(flatten)]
        staged: WorkspaceFilePreview,
    },
    /// The reply to [`ClientMessage::WorkspaceFileRename`]: the entry's new
    /// spelling, or the sentence the refusal stopped on. Flattened beside
    /// `id` the same way [`DaemonMessage::WorkspaceFileContent`] flattens
    /// its body.
    WorkspaceFileRenamed {
        id: u64,
        #[serde(flatten)]
        change: WorkspaceFileMutation,
    },
    /// The reply to [`ClientMessage::WorkspaceFileDuplicate`]: the copy's
    /// spelling (the daemon chose the name), or the sentence the refusal
    /// stopped on.
    WorkspaceFileDuplicated {
        id: u64,
        #[serde(flatten)]
        change: WorkspaceFileMutation,
    },
    /// The reply to [`ClientMessage::WorkspaceFileDelete`]: the sentence
    /// the refusal stopped on, or — the one success shape that carries
    /// neither field — the silence that says the entry is gone.
    WorkspaceFileDeleted {
        id: u64,
        #[serde(flatten)]
        change: WorkspaceFileMutation,
    },
    Workspace {
        id: u64,
        workspace: Workspace,
    },
    SessionAttached {
        id: u64,
        subscription_id: SubscriptionId,
    },
    Ok {
        id: u64,
    },
    AgentMessageReceipt {
        id: u64,
        state: AgentMessageState,
    },
    Resume {
        id: u64,
        result: ResumeResult,
    },
    /// The reply to [`ClientMessage::SessionDeposit`]: the reference to the
    /// bytes that were stored.
    ///
    /// `reference.digest` is of the **stored** bytes, after the metadata
    /// strip — not of the wire bytes the request carried. See
    /// [`AttachmentReference`] for why the client cannot compute it and why
    /// the session travels with it.
    SessionDeposited {
        id: u64,
        reference: AttachmentReference,
    },
    /// The reply to [`ClientMessage::SessionAttachmentRead`]: the stored
    /// bytes, base64, with the store's own MIME type for them.
    SessionAttachment {
        id: u64,
        attachment: StoredAttachment,
    },
    InvokeResult {
        id: u64,
        value: serde_json::Value,
    },
    JournalUsage {
        id: u64,
        usage: JournalUsage,
    },
    JournalRetention {
        id: u64,
        retention: JournalRetention,
    },
    Providers {
        id: u64,
        providers: Vec<ProviderInfo>,
        /// PATH directories the catalog could not list. Zero when every
        /// entry was readable or simply missing.
        #[serde(default)]
        unreadable_dirs: u32,
    },
    ProviderUpdated {
        id: u64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        log: String,
    },
    Event(SessionEventEnvelope),
    SubscriptionEvent {
        subscription_id: SubscriptionId,
        envelope: SessionEventEnvelope,
    },
    /// Everything the Devices panel needs in one reply, already projected for
    /// the connection's role: a local client sees the full rows, a remote peer
    /// sees a subset (`devboule-daemon/src/server.rs`).
    Devices {
        id: u64,
        self_info: SelfInfo,
        peers: Vec<PeerRow>,
        /// Client-role pairings parked on this device waiting for
        /// [`ClientMessage::PairingConfirm`]. This is how a far-side
        /// `PairingPending` reaches the owner: the panel polls `DevicesList`.
        #[serde(default)]
        pending: Vec<PendingPairing>,
    },
    /// The reply to [`ClientMessage::PeerAgentsList`]: the agents the
    /// responder is running **now**, and `scope` — whose roster this is.
    /// An empty `agents` list with scope `pairing_user` or `local_user`
    /// means that user has no live agents. Scope `unscoped` means the
    /// responder's pairing row recorded no user (a platform without user
    /// ids, or a pairing that predates the recording), so it cannot say
    /// whose roster it would be exposing and refuses to guess: `agents` is
    /// empty there too, and the two absences must never be collapsed —
    /// "no agents" and "cannot scope" are different facts about the far
    /// machine. A reply is a liveness snapshot, never a stored object.
    PeerAgents {
        id: u64,
        agents: Vec<PeerAgent>,
        scope: PeerRosterScope,
    },
    PairingCode {
        id: u64,
        code: PairingSecret,
        /// Unix milliseconds.
        expires_at: i64,
        address: String,
    },
    /// The reply to `PairingComplete` when the far side must confirm locally.
    PairingPending {
        id: u64,
        peer: PendingPairing,
    },
    /// The reply to `PairingComplete` when the pairing completed at once.
    PairingDone {
        id: u64,
        peer: PeerRow,
    },
    /// The reply to a `PairingConfirm` that declined. A decline is a success,
    /// not a failure: the panel must not render it as an error, and there is
    /// no row to report (a `PeerRow` that is not in the table would be a lie).
    PairingDeclined {
        id: u64,
        device_id: String,
    },
    /// The reply to `PairingConfirm`, `PeerRevoke` and `PeerSetCaps`.
    PeerUpdated {
        id: u64,
        peer: PeerRow,
    },
    /// The reply to `ToolPolicyGet`: every stored policy, ordered by provider
    /// id. A provider with no stored policy is absent here and reads as
    /// enabled with nothing disabled, so an empty list is not an error.
    ToolPolicy {
        id: u64,
        policies: Vec<ToolPolicyEntry>,
    },
    /// The reply to `ToolPolicySet` once the store is on disk.
    ToolPolicySetOk {
        id: u64,
    },
    /// The reply to `AgentProfilesGet`: the whole stored document, the ordered
    /// profile list and the standing instructions. An empty document is not an
    /// error — it is a first run, or a file the store had to quarantine, and it
    /// means the same thing in both cases: no profiles and no standing
    /// instructions.
    AgentProfiles {
        id: u64,
        document: AgentProfilesDocument,
    },
    /// The reply to `AgentProfilesSet` once the whole document is on disk. The
    /// reply carries no document: the ids the daemon minted for new profiles
    /// are read back with `AgentProfilesGet`, exactly as a tool policy toggle is
    /// read back with `ToolPolicyGet`.
    AgentProfilesSetOk {
        id: u64,
    },
    /// The reply to `ProviderVocabularyGet`: what one provider offers, both
    /// axes, and how this answer was produced. The item shapes are the live
    /// manifest's ([`SessionModel`], [`SessionModeView`]) reused unchanged —
    /// the vocabulary is the same shape everywhere and only its origin
    /// differs, which is carried explicitly rather than flattened.
    ///
    /// `source` says whether THIS reply came from the cache or from a fresh
    /// probe. `probed_at_ms` is when the cache entry was filled and is
    /// therefore a cache fact: a probe reply is fresh by definition and omits
    /// it. Both optional fields are absent from the wire — never an explicit
    /// `null` — and `origin` on an axis follows one biconditional: it is set
    /// if and only if that axis's state is `Present`.
    ProviderVocabulary {
        id: u64,
        /// The canonical provider id the reply answers for, as the profile
        /// store would store it.
        provider: String,
        models: VocabularyModels,
        modes: VocabularyModes,
        source: VocabularySource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        probed_at_ms: Option<u64>,
    },
    /// The reply to `DelegationGet`: the switch and where the answer came
    /// from. `source` is a wire value, not a Rust detail: the app renders a
    /// quarantined file as damaged ("delegation reads off") and a missing one
    /// as never configured, and collapsing either into plain "off" would turn
    /// a fact the human needs into a state they cannot distinguish.
    DelegationState {
        id: u64,
        enabled: bool,
        source: DelegationSource,
    },
    /// The reply to `DelegationSet`, carrying what the daemon **stored** —
    /// not an echo of the request. The value is the same boolean today, but
    /// the reply is the one acknowledgement a write gets, so it names the
    /// stored truth: a client that trusts its own request instead would hold
    /// a value the daemon does not, and nothing would reveal the disagreement
    /// until a second client's answer refused (`NOTE-a-write-that-does-not-
    /// say-what-it-stored.md`, the class, applied here from birth).
    DelegationSetOk {
        id: u64,
        enabled: bool,
        source: DelegationSource,
    },
    /// The daemon pushed the switch. Server-initiated and id-less, like
    /// `Event`: it answers no request, so the client's pending-request table
    /// must never consume it. The setting is global and read once by the app
    /// at mount, so a write from any surface — the Settings switch, the
    /// roster's take-back — has to reach every connected client or a stale
    /// OFF hides the very control that stops delegation. Delivered to the
    /// daemon's session watchers, which is every local app connection that
    /// asked to watch — and a paired device's too, when it holds the
    /// administrative capability that opens the watch frame.
    DelegationChanged {
        enabled: bool,
        source: DelegationSource,
    },
}

/// One workspace's working-tree state, as the Changes panel reads it.
///
/// `is_git` and `error` answer different questions and must never collapse:
/// a folder that is simply not a repository answers `is_git: false` with
/// `error: null`, while an `error` says *this reply* is incomplete — the
/// workspace folder is gone, git did not run, or `git status` produced more
/// bytes than the reply cap allows and the daemon refused to hand back a
/// list it had cut short. `error` beside `is_git: true` is a caveat on an
/// answer that is otherwise real; `error` beside `is_git: false` is a
/// refusal to claim either way.
///
/// **Debt, recorded in the slice-1 fix round:** `error` is free text on a
/// frame that does **not** pass `redact_for_conn` — that seam rewrites only
/// `DaemonMessage::Error`, never this variant. Today the reply is behind the
/// `admin` capability and every sentence is written without a path and
/// without git's stderr, so nothing leaks; a future lowering of that
/// capability would let this machine's paths out in silence, and the fix then
/// belongs in the redaction seam, not in the message writers.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGitStatus {
    pub is_git: bool,
    /// Whether anything is uncommitted. Normally `!rows.is_empty()`, so the
    /// two agree; the one deliberate exception is the withheld list: `git
    /// status` passed the reply cap, so `rows` is empty while `dirty` still
    /// says the tree is dirty — a cut-short list is not an empty tree.
    pub dirty: bool,
    /// `# branch.head` verbatim, including git's own `(detached)`. `null`
    /// when there is no repository or no answer.
    pub branch: Option<String>,
    pub totals: WorkspaceGitTotals,
    pub rows: Vec<WorkspaceGitRow>,
    pub error: Option<String>,
}

/// Added and removed lines over every row of one reply.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGitTotals {
    pub additions: u64,
    pub deletions: u64,
}

/// One changed file. `status` is derived from the `XY` pair of
/// `git status --porcelain=v2` (or from the record kind for `?` and `u`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGitRow {
    /// Path relative to the repository root, as git printed it.
    pub path: String,
    /// The original path of a rename (or copy) record: `-z` writes it as
    /// the bare token right after the record, and this field is where that
    /// token lands instead of vanishing. The panel needs it to act on a
    /// renamed row **with both of its paths** — discarding (or unstaging)
    /// only the new path leaves the old side's deletion staged, a half
    /// operation that answers success (measured on git 2.54.0; the row is
    /// keyed on the new path because that is what the `2` record carries).
    /// `null`/absent for every other row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    pub status: WorkspaceGitFileStatus,
    /// Whether the two counts are **not** the file's exact line counts. Set,
    /// never implied, and by every path that can make them inexact: the
    /// untracked reader refused the file (over the byte cap, unreadable or
    /// gone) or stopped inside it; the file carries a NUL byte; git printed
    /// `-` for it; the path is unmerged, so git's numstat is stage
    /// bookkeeping rather than a delta; or the whole numstat round was
    /// degraded — one dump failed or was cut — in which case every number
    /// that came from it is a floor. An untracked row counts its own file and
    /// is unaffected by a degraded round.
    pub capped: bool,
}

/// The six words the Changes panel renders, all derived from `porcelain=v2`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceGitFileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// Why this reply does or does not carry lines. `ok` and `binary` are
/// complete answers; `too_large` says the lines exist and were withheld
/// rather than cut short (the sentence in `error` names which cap); `error`
/// is a refusal to answer at all (the sentence says what stopped it).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceGitDiffStatus {
    Ok,
    Binary,
    TooLarge,
    Error,
}

/// One line's role in the diff. Line-level, never word-level: that is the
/// choice the working diff makes (`plan.md` §4b), the word-level form being
/// reserved for an agent tool's own diff elsewhere.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceGitDiffLineKind {
    Add,
    Remove,
    Context,
    Header,
}

/// One line of a diff. `header` is a hunk header (`@@ …`), kept whole so the
/// panel can label and navigate; the other three are file content.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGitDiffLine {
    pub kind: WorkspaceGitDiffLineKind,
    /// Without the `+`/`-`/space marker for content lines; the whole `@@ …`
    /// for a header.
    pub text: String,
}

/// One file's diff of the working tree, as the Changes panel renders it.
///
/// `status` and `error` answer different questions and must never collapse,
/// like the pair on `WorkspaceGitStatus`: `binary` and `too_large` are
/// complete answers about a file this reply deliberately carries no lines
/// for, while `error` with `status: "error"` is a refusal — the folder is
/// not a repository, the path is outside it, git did not run. An unchanged
/// file is `ok` with no lines: an empty diff is an answer too.
///
/// **Debt, recorded with `WorkspaceGitStatus` in the slice-1 fix round and
/// true here too:** `error` is free text on a frame that does **not** pass
/// `redact_for_conn` — that seam rewrites only `DaemonMessage::Error`. Every
/// sentence is written without an absolute path and without git's stderr,
/// and this reply's only caller-supplied text (`path`) is echoed only in its
/// own field, never in `error`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGitFileDiff {
    /// The path this reply is about, echoed **verbatim**: the caller's own
    /// text coming back — including in a refusal that rejects it, where the
    /// rejected string (possibly absolute) travels only to the sender that
    /// supplied it. This side never replaces it with a path of its own.
    pub path: String,
    /// The path is not in `HEAD` — untracked, staged new, or the surviving
    /// side of a rename. Carve-out, the same one `additions` has: `false`
    /// whenever `status` is not `ok` — a `binary`, `too_large` or `error`
    /// reply zeroes the flags with the lines, and about such a reply the
    /// flags claim nothing.
    pub is_new: bool,
    /// The path is in `HEAD` and gone from the working tree, from git's own
    /// `deleted file mode`. Carve-out, the same one `additions` has: `false`
    /// whenever `status` is not `ok` — a `binary`, `too_large` or `error`
    /// reply zeroes the flags with the lines, and about such a reply the
    /// flags claim nothing.
    pub is_deleted: bool,
    /// Added and removed lines of `lines`. `0` whenever `status` is not
    /// `ok`: a count of lines this reply does not carry would be a guess.
    pub additions: u64,
    pub deletions: u64,
    pub lines: Vec<WorkspaceGitDiffLine>,
    pub status: WorkspaceGitDiffStatus,
    /// Why no lines came back, in one synthetic sentence: no absolute path
    /// and no git stderr (see the debt note above). `null` exactly when
    /// `status` is `ok` or `binary` — those two are answers, not failures.
    pub error: Option<String>,
}

/// The entries of one workspace folder, as the Files panel's tree renders
/// them — one directory per reply, never a subtree.
///
/// `entries` and `error` answer different questions and must never collapse,
/// like the pair on `WorkspaceGitStatus`: a folder's entries with `error:
/// null` is the answer, an empty list with `error: null` is a folder that
/// holds nothing, and `error` with a sentence is a refusal — the path left
/// the workspace, it is not a folder, the folder could not be listed. A
/// refusal carries no entries, so the panel may not claim anything about the
/// folder behind it.
///
/// **Debt, recorded with `WorkspaceGitStatus` in the slice-1 fix round and
/// true here too:** `error` is free text on a frame that does **not** pass
/// `redact_for_conn`. Every sentence is written without an absolute path and
/// without an OS error string, and this reply's only caller-supplied text
/// (`path`) is echoed only in its own field, never in `error`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDirectory {
    /// The path this reply is about, echoed **verbatim**: the caller's own
    /// text coming back — including in a refusal that rejects it, exactly as
    /// `WorkspaceGitFileDiff::path` does. The empty string is the folder
    /// itself.
    pub path: String,
    /// The entries of this single directory, already ordered by the daemon:
    /// folders first, then by name in byte order (never a locale collation).
    pub entries: Vec<WorkspaceFileEntry>,
    /// Whether the entry cap dropped entries of this folder. Set, never
    /// implied: a partial list says so instead of passing for the whole
    /// folder.
    pub capped: bool,
    /// Entries this directory had that the reply does **not** carry because
    /// they failed the survival test — a link (never classified; its target
    /// is not read) or an entry that would not stat. Set, never implied: a
    /// folder holding a link says so instead of looking complete. Two
    /// carve-outs, both deliberate: `.git` is the tree's declared policy
    /// exclusion (DECISIONS §6), not a hidden entry; and entries past
    /// `capped` are `capped`'s own confession, not this count's.
    pub skipped: u64,
    /// Why no entries came back, in one synthetic sentence: no absolute
    /// path, no OS error text. `null` exactly when the reply is an answer —
    /// a listed folder, empty or not.
    pub error: Option<String>,
}

/// One directory entry: an ordinary folder or an ordinary file of the
/// workspace. A link is never an entry — the daemon skips it rather than
/// classifying what it points at, so `kind` never has to guess.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileEntry {
    /// Path relative to the workspace folder, `/`-separated — the key the
    /// panel expands and collapses by.
    pub path: String,
    /// The entry's own name, for the row's label.
    pub name: String,
    pub kind: WorkspaceFileKind,
    /// File size in bytes as `stat` reported it; `null` for a folder (a
    /// folder has no size to show) and never a guess.
    pub size: Option<u64>,
}

/// What an entry is, decided without following it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceFileKind {
    Dir,
    File,
}

/// What one file-content reply says happened. The four answer different
/// questions and must never collapse, like the pair on
/// [`WorkspaceDirectory`]: `binary` and `too_large` are complete answers
/// about a file deliberately carried without content, while `refused` with
/// a sentence in `error` is a failure.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceFileContentStatus {
    /// The bytes came back; `kind` says how to read `content`.
    Ok,
    /// Past the content cap: refused whole with the measure in `error`,
    /// never cut short.
    TooLarge,
    /// Read and sniffed as binary: no content, by decision, not by loss.
    Binary,
    /// Refused before any byte was handed back; `error` says why, in the
    /// sentences the rest of the panel already shows.
    Refused,
}

/// How to read `content`: UTF-8 text, base64 for an image recognized by its
/// extension, or the bytes' own class beside a `binary` status. `null`
/// whenever the bytes were never read (`too_large`, `refused`).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceFileContentKind {
    Text,
    Image,
    Binary,
}

/// The content of one workspace file, as the Files panel's preview renders
/// it. Carve-outs, each stated rather than implied: `error` is `Some`
/// exactly for `refused` and `too_large` (the cap's own sentence carries
/// the measure) and `null` for `ok` and `binary`, which are answers;
/// `content` is `Some` only for `ok` — base64 when `kind` is `image`, UTF-8
/// otherwise — and `null` for every other status, never a decoded binary;
/// `size` and `modified_at` come from the stat and are `null` exactly when
/// the status is `refused`, because a refusal claims nothing about the
/// file it rejected; `kind` is `Some` exactly when the bytes were read, an
/// over-cap file never being opened at all; and the five window fields
/// (`from_line`, `lines`, `has_more`, `truncated`, `note`) are `Some`
/// exactly for a text `ok` reply — an answer about one window, where
/// `lines` counts the lines this reply carries and never the file's own
/// total (which would be a whole read behind one number). `truncated` and
/// `note` travel together: the first says the cap cut a line too big for
/// one window, the second is the sentence that says its rest — and with
/// `has_more: false` everything after it — cannot be read this way.
///
/// **Debt, the same one `WorkspaceDirectory` records:** `error` is free text
/// on a frame that does **not** pass `redact_for_conn`. The sentences this
/// module composes are static or built from an operation name and a number —
/// never a path, never an OS error string — but the registry's own failure
/// rides this field too, and that sentence carries the `workspace_id` the
/// caller sent (`session_workspaces.rs`, `Workspace '{workspace_id}' is
/// unavailable…`), plus the journal's reason text a local writer chose:
/// the R4 debt `WorkspaceDirectory` already records, nominated here rather
/// than denied. The only caller text in an echo is that id — never a path.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileContent {
    pub status: WorkspaceFileContentStatus,
    pub kind: Option<WorkspaceFileContentKind>,
    pub content: Option<String>,
    /// Bytes, as `stat` reported them before the read. `null` on `refused`.
    pub size: Option<u64>,
    /// Milliseconds since the Unix epoch, as `stat` reported them; `null`
    /// when the filesystem gave no stamp, and on `refused`.
    pub modified_at: Option<i64>,
    pub error: Option<String>,
    /// The 1-based line this window starts at; `Some` exactly for a text
    /// `ok` reply (the four window fields share that carve-out).
    pub from_line: Option<u64>,
    /// How many lines this window carries: 0 is a window past the file's
    /// end, and no reply counts the file whole.
    pub lines: Option<u64>,
    /// Whether another window follows this one; `false` on the last.
    pub has_more: Option<bool>,
    /// Whether the byte cap cut this window's last line short — declared
    /// rather than hidden, because the file goes on inside that line.
    pub truncated: Option<bool>,
    /// The sentence that cut carries: `Some` exactly when `truncated` is
    /// `Some(true)`, saying the line exceeds one window and the rest of it
    /// cannot be read this way — static words, never a path.
    pub note: Option<String>,
}

/// The outcome of one Files-panel write — a rename, a duplicate, or the
/// delete — as the three replies carry it. A rename or duplicate success
/// carries `new_path` — the entry's new spelling relative to the workspace
/// folder, `/`-joined the way a listing builds its entries — and no
/// sentence; a delete success carries **neither** field, because there is
/// no spelling to name and nothing left to say it about; a refusal carries
/// a static `error` sentence and `new_path: null`, so a refusal claims
/// nothing about where anything is. The same pair discipline
/// [`WorkspaceDirectory`] and [`WorkspaceFileContent`] keep, and the same
/// debt: `error` is free text on a frame the redaction seam does not touch,
/// so its sentences are static or the registry's own (which echoes the
/// `workspace_id` the caller sent — never a path).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileMutation {
    /// The entry's new spelling on success; `null` on a refusal.
    pub new_path: Option<String>,
    /// Why the act was refused; `null` on a success.
    pub error: Option<String>,
}

/// What one preview-stage reply says happened. The two answer different
/// questions and must never collapse, like [`WorkspaceFileContentStatus`]:
/// `ok` means a copy of the file rests in the runtime directory's
/// `previews` folder, while `refused` with a sentence in `error` means the
/// daemon copied nothing and claims nothing about the file.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceFilePreviewStatus {
    /// The copy exists; `path` names it and `size`/`modified_at` are the
    /// source file's own stat.
    Ok,
    /// Refused before anything was copied; `error` says why, in the
    /// sentences the rest of the panel already shows.
    Refused,
}

/// The staged copy of one workspace file, as the Files panel's preview
/// renders it. Carve-outs stated, the same pair discipline as
/// [`WorkspaceFileContent`]: `path`, `size` and `modified_at` are `Some`
/// exactly when the status is `ok`, and `error` is `Some` exactly when it
/// is `refused` — a refusal claims nothing about a file it never copied.
/// `path` is absolute and inside the `previews` folder by construction;
/// the frontend turns it into an asset URL and never draws it as content.
///
/// **Debt, the same one `WorkspaceFileContent` records:** `error` is free
/// text on a frame that does not pass `redact_for_conn`, so every sentence
/// it carries is static or the registry's own (which echoes the
/// `workspace_id` the caller sent — never a path).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFilePreview {
    pub status: WorkspaceFilePreviewStatus,
    /// Absolute path of the copy under the `previews` folder; `null` on a
    /// refusal.
    pub path: Option<String>,
    /// Bytes, as `stat` reported them on the source file. `null` on a
    /// refusal.
    pub size: Option<u64>,
    /// Milliseconds since the Unix epoch, as `stat` reported them on the
    /// source file; `null` when the filesystem gave no stamp, and on a
    /// refusal.
    pub modified_at: Option<i64>,
    /// Why the stage was refused; `null` on a success.
    pub error: Option<String>,
}

/// The three-valued answer to "what does this provider offer". The three are
/// distinct wire values on purpose and must never collapse: `present` — a
/// source answered with a list (never with empty items); `none` — the source
/// can answer and answered "I have none"; `absent` — no source could answer
/// (the agent declared no model shape, the probe failed, the provider is not
/// installed). "The provider published nothing" and "nobody could ask" are
/// different facts.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VocabularyState {
    Present,
    None,
    Absent,
}

/// Who authored a `present` vocabulary list: the provider's own answer on its
/// wire, or the daemon's own mapping (Claude's, Codex's and pi's modes are the
/// launcher's vocabulary — the provider cannot report them). Set only when
/// the state is [`VocabularyState::Present`], in both directions.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VocabularyOrigin {
    Provider,
    Daemon,
}

/// How a `ProviderVocabulary` reply was produced: served from the daemon's
/// cache, or probed fresh for this request.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VocabularySource {
    Cache,
    Probe,
}

/// Where a delegation-switch answer came from. Three values on purpose and
/// never collapsed: `file` — the human wrote the switch; `default` — no file
/// exists, which reads off but is "never configured", not "turned off";
/// `quarantined` — the file existed and was damaged, so the daemon reads off
/// while holding neither of the other two facts. The missing file reading as
/// off is the safe direction — it withholds power and invents no knowledge —
/// and the three answers stay distinct so the app can name which one it got.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DelegationSource {
    File,
    Default,
    Quarantined,
}

/// The models axis of a `ProviderVocabulary` reply. Items are the live
/// manifest's shape, reused. `origin` is Some exactly when `state` is
/// [`VocabularyState::Present`]; a `present` with no items is a collapsed
/// absence and is never sent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VocabularyModels {
    pub state: VocabularyState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<VocabularyOrigin>,
    pub items: Vec<SessionModel>,
}

impl VocabularyModels {
    /// The way every axis builder constructs this struct: it refuses the two
    /// pairs the biconditional forbids — an `origin` on a `none`/`absent`
    /// axis, and a `present` axis without one — so an illegal combination is
    /// rejected at construction rather than merely never built. The fields
    /// stay public for the serializer's derives and for tests that pin the
    /// wire encoding itself.
    pub fn new(
        state: VocabularyState,
        origin: Option<VocabularyOrigin>,
        items: Vec<SessionModel>,
    ) -> Result<Self, String> {
        if origin.is_some() != matches!(state, VocabularyState::Present) {
            return Err(format!(
                "origin is present exactly when the state is present: got {state:?} with {} origin",
                if origin.is_some() { "an" } else { "no" }
            ));
        }
        Ok(Self {
            state,
            origin,
            items,
        })
    }
}

/// The modes axis of a `ProviderVocabulary` reply. Same shape discipline as
/// [`VocabularyModels`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VocabularyModes {
    pub state: VocabularyState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<VocabularyOrigin>,
    pub items: Vec<SessionModeView>,
}

impl VocabularyModes {
    /// Same discipline as [`VocabularyModels::new`]: the biconditional as
    /// code, refusing the pairs no builder may emit.
    pub fn new(
        state: VocabularyState,
        origin: Option<VocabularyOrigin>,
        items: Vec<SessionModeView>,
    ) -> Result<Self, String> {
        if origin.is_some() != matches!(state, VocabularyState::Present) {
            return Err(format!(
                "origin is present exactly when the state is present: got {state:?} with {} origin",
                if origin.is_some() { "an" } else { "no" }
            ));
        }
        Ok(Self {
            state,
            origin,
            items,
        })
    }
}

/// This device's own advertised identity. `remote` deliberately carries only
/// the state and reason: the addresses and the port are right here, because
/// they describe *where* this node can be reached and the wire `remote` object
/// answers only *whether* it is reachable.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SelfInfo {
    pub device_id: String,
    pub display_name: String,
    /// The Noise static public key, base64.
    ///
    /// Always present, even when a projection has nothing to put in it (the
    /// `Daemon`-role projection sends `""`). The 1b wire contract types every
    /// one of these fields as required, and it is consumed by TypeScript, which
    /// has no way to check a key that the daemon chose to omit: the panel reads
    /// `self.addresses.length` unconditionally, so an omitted key is a crash in
    /// the Devices tab rather than a missing value. Withholding is by **value**
    /// (empty string, empty array, zero), never by key presence.
    pub public_key: String,
    /// Hex of the first 16 bytes of SHA-256 of that key: the value a person
    /// reads aloud when confirming a pairing. Always present, like
    /// [`SelfInfo::public_key`].
    pub key_fingerprint: String,
    /// Where this device can be reached. Empty when the listener is down, or
    /// when a projection withholds the network position. Always present.
    pub addresses: Vec<String>,
    /// The peer port; `0` when there is no listener. Always present.
    pub port: u16,
    pub daemon_version: String,
    pub protocol_version: u32,
    /// Whether the tailnet listener is up, and why not when it is not. Local
    /// information; a `Daemon` peer is told `Ping` is the liveness answer and
    /// nothing about this device's network position.
    ///
    /// This is the one field that stays optional, because its absence is what
    /// the `Daemon` projection uses to withhold the *state* rather than a value:
    /// `RemoteState` has no "unknown" variant to send instead. See the note in
    /// the `Daemon` arm of `server.rs::devices_reply`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteState>,
}

/// One paired device as the Devices panel sees it.
///
/// `role` is the *peer's* role. `binding_kind` is `"tailnet"` today.
/// `address` is the `ip:port` recorded at pairing (design §8 R4, F-15); the
/// `whois` check at connect time is the backstop, not the source of truth.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PeerRow {
    pub device_id: String,
    pub display_name: String,
    pub role: PeerRole,
    /// The pinned Noise static public key, base64.
    pub public_key: String,
    pub key_fingerprint: String,
    pub binding_kind: String,
    pub binding_node_name: Option<String>,
    pub binding_login_name: Option<String>,
    pub address: String,
    /// Unix milliseconds.
    pub paired_at: i64,
    /// Unix milliseconds; `None` while the pairing stands.
    pub revoked_at: Option<i64>,
    pub caps: Vec<String>,
    /// The SID this daemon paired the peer from, or `None` on a platform with
    /// no SID or for a peer paired before the value was recorded.
    pub paired_by_user: Option<String>,
    /// Whether this peer has a live connection right now.
    pub online: bool,
}

/// Whose roster a `PeerAgents` reply answers for — or whether the responder
/// could scope it at all. Three values on purpose, never collapsed:
/// `pairing_user` — the user at the responding machine who approved the
/// pairing; `local_user` — a local pipe's own user; `unscoped` — the pairing
/// row recorded no user (a platform without user ids, or a pairing that
/// predates the recording), so the responder cannot say whose roster it
/// would be exposing and answers with an **empty** list. `unscoped` is not
/// "no agents"; it is the third state, and the dialer renders it as its own
/// sentence.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PeerRosterScope {
    PairingUser,
    LocalUser,
    Unscoped,
}

/// One agent running now on the responding device, as the peer roster
/// carries it.
///
/// Deliberately narrow, and deliberately **not** protocol [`Session`]: a
/// `Session` carries `cwd` and workspace ids, and a filesystem path is a
/// disclosure that has nothing to do with naming an agent. An entry is
/// identified by the pair (the responder's device id, `session_id`) — a
/// session id is unique only within one daemon, so the device half of the
/// pair is not optional. `state` is the A2A word, the same vocabulary the
/// local roster answers with.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PeerAgent {
    pub session_id: String,
    /// The name the agent is shown under; a session a person started falls
    /// back to its title.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub state: AgentTaskState,
    /// How far the agent is from a human root: 0 for a session a person
    /// started, 1 for its child, 2 for a grandchild.
    pub depth: u32,
}

/// A Client-role pairing parked on this device, awaiting a local decision.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingPairing {
    pub device_id: String,
    pub display_name: String,
    pub role: PeerRole,
    pub key_fingerprint: String,
    pub address: String,
    /// Unix milliseconds: when the parked socket is closed and the pairing is
    /// answered `accepted: false`.
    pub expires_at: i64,
}

/// One CLI agent the daemon found on PATH. Authentication is never probed:
/// an executable on PATH is "installed, status unknown".
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: String,
    pub executable: String,
    pub acp_available: bool,
    pub authentication: String,
    /// Chat launch dialect. `"acp"` or `"stream-json"` when the catalog
    /// entry can start a session; omitted when the CLI is installed but
    /// not chat-capable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// How this row was obtained. `"user-binary"` is a locally installed CLI;
    /// `"npx-wrapper"` comes from the ACP registry. Omitted for older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Registry-supplied arguments appended after `npx -y <package>`. Present
    /// only for npx-wrapper rows so consent can show the complete trusted
    /// launch line without exposing local npx plumbing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_args: Option<Vec<String>>,
    /// Explicit picker policy. `Some(false)` means a covered registry wrapper
    /// remains visible in Settings but is omitted from the workspace picker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pickable: Option<bool>,
    /// Installed CLI version: read from package.json for an npm shim, or
    /// obtained from a native executable's --version probe on refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    /// Newest known version from the npm registry or ACP registry feed,
    /// depending on the installation channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    /// Version declared by the running ACP process during its last live
    /// initialize handshake. This may be the adapter version, not the
    /// underlying CLI version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    /// Installation source: `npm`, `npx-registry`, or `native`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_channel: Option<String>,
    /// False only for a known npm provider that is not currently installed.
    /// The field is omitted for installed rows so older clients keep their
    /// existing absent-means-installed interpretation.
    #[serde(
        default = "default_provider_installed",
        skip_serializing_if = "is_true"
    )]
    pub installed: bool,
    /// Known npm package for this provider, including not-installed rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm_package: Option<String>,
    /// Tools the daemon's MCP broker serves to this provider's sessions, in
    /// the broker's catalog order. Omitted when empty, so a provider without
    /// an MCP channel keeps the older row's shape and the panel hides the
    /// tool section.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDescriptor>,
}

/// One tool a provider's sessions can be served by the daemon's MCP broker.
///
/// The name is the tool's identity in `tools/list` and in a tool policy's
/// `disabled_tools`; the description is display text and may be reworded
/// without breaking a stored policy.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
}

/// One provider's tool policy, as stored by the daemon and as listed by
/// `ToolPolicy`.
///
/// An absent policy, and `enabled: null`, both mean enabled; only
/// `enabled: false` turns every tool off. This mirrors the app's
/// `enabled: boolean | null`, so a client cannot express "enabled" and
/// "disabled" with two different spellings.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicyEntry {
    pub provider_id: String,
    /// `None` or `Some(true)` = enabled. `Some(false)` = every tool disabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Tools disabled one by one. The always-on roster tool is never read
    /// from here: `is_tool_enabled` answers for it first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_tools: Vec<String>,
}

/// One agent profile, as the Settings → Agents form saves it and as the
/// ordered list in `agent-profiles.json` holds it.
///
/// `id` is the profile's identity and the only key anything downstream uses.
/// The daemon mints one when a caller leaves it empty, a human renaming a
/// profile changes `name` and never `id`, and a session records the `id` it was
/// started from — so a rename cannot make a running child report a profile that
/// no longer exists, and two profiles may share a `name` without one shadowing
/// the other. Nothing looks a profile up by name in this type.
///
/// `model`, `mode_id` and `thinking_option_id` are the provider's own
/// vocabulary, stored verbatim and bounded by length. The daemon's catalog
/// answers which providers exist — and `agent_profiles.rs` uses exactly that
/// predicate — but it publishes no per-provider list of models or modes at this
/// commit (`peer_policy.rs` says so for modes in its own words: "ACP modes are
/// defined by the agent at runtime"), so a membership test here would be a
/// second list that could refuse a profile the provider really offers. The
/// provider refuses an unknown mode or model itself when the creation path asks
/// it to spawn (`claude_client.rs`, `codex_view.rs`, `pi_client.rs`,
/// `session.rs`).
///
/// `tool_overlay` can only ever *remove* tools, for the reason `ToolOverlay`
/// states in `provider_catalog.rs`: a profile that widened a session's tools
/// would be a second policy authority beside the stored `ToolPolicyEntry`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub note: String,
    pub provider: String,
    pub model: String,
    pub mode_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_option_id: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub features: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_overlay: Vec<String>,
    pub enabled_for_agents: bool,
}

/// The whole stored document: the **ordered** profile list, in the human's
/// order, plus the standing instructions.
///
/// One document rather than two files, so a creation reads both halves at one
/// moment and one write cannot leave them disagreeing. An empty document — the
/// first run, or a file the store had to quarantine — means no profiles **and**
/// no standing instructions, never "the last good ones".
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentProfilesDocument {
    #[serde(default)]
    pub profiles: Vec<AgentProfile>,
    #[serde(default)]
    pub standing_instructions: String,
}

fn default_provider_installed() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JournalUsage {
    pub total_bytes: u64,
    pub session_count: usize,
    pub deleted_by_user: usize,
    pub deleted_by_retention: usize,
    pub unreclaimable: Unreclaimable,
    pub limits: JournalLimits,
    pub per_session: Vec<JournalSessionUsage>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JournalLimits {
    pub snapshot_every_bytes: u64,
    pub session_max_bytes: u64,
    pub max_bytes: u64,
    pub max_sessions: usize,
    pub max_age_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JournalSessionUsage {
    pub id: String,
    pub title: String,
    /// The name a created agent is shown under (protocol `Session.displayName`,
    /// read back from the journal's own `display_name` column), so History can
    /// name a row the way the tab strip already names it. `Option` with a serde
    /// default, exactly like `Session.display_name`: a client that speaks an
    /// older dialect still parses a frame carrying it, and a row written before
    /// the column existed reads back as `None` — which the app renders as its
    /// fallback name, never as an empty one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub kind: SessionKind,
    pub bytes: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Unreclaimable {
    pub bytes_over: u64,
    pub sessions_over: usize,
    pub aged_out: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RetentionSource {
    Default,
    User,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetentionLimit {
    pub value: u64,
    pub source: RetentionSource,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JournalRetention {
    pub session_max_bytes: RetentionLimit,
    pub max_bytes: RetentionLimit,
    pub max_sessions: RetentionLimit,
    pub max_age_ms: RetentionLimit,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_max_bytes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sessions: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_ms: Option<i64>,
}

/// Live counters of the conversation journal writer, from the `status`
/// frame.
///
/// They exist to separate two failure modes that a per-session degraded
/// flag alone conflates:
///
/// - `failedFrames > 0`: the daemon REJECTED output while it was alive
///   (journal queue full or a write error). It dropped those frames
///   knowing it, recorded the per-session degradation, and a recovered
///   transcript preserves that loss in its integrity counters.
/// - `committedFrames < acceptedFrames` with `failedFrames == 0`: frames
///   were accepted into the bounded queue but not committed yet. If the
///   process dies in this state the queue dies with it and no record of
///   those frames ever reaches the database — the loss is real but
///   nothing after the fact can observe that it happened.
///
/// Even both conditions clean do not certify a complete transcript:
/// output the daemon produced but never accepted is invisible here by
/// construction. Completeness is only ever claimed by an orderly close
/// (`Exit`), never by these counters.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JournalStats {
    /// Output frames the journal queue accepted. A frame is counted here
    /// when it enters the queue, not when it is on disk.
    #[serde(default)]
    pub accepted_frames: u64,
    /// Payload bytes of the accepted frames.
    #[serde(default)]
    pub accepted_bytes: u64,
    /// Accepted frames whose SQLite transaction has committed. At most
    /// `acceptedFrames`; the difference is the queue that dies with the
    /// process.
    #[serde(default)]
    pub committed_frames: u64,
    /// Payload bytes of the committed frames.
    #[serde(default)]
    pub committed_bytes: u64,
    /// Output frames the journal rejected while the daemon was alive:
    /// queue full or a write error. Every one of these is output the
    /// daemon dropped knowing it.
    #[serde(default)]
    pub failed_frames: u64,
}

/// Status fields flattened into the `status` frame so a pipe client sees
/// them next to `type`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatusBody {
    pub instance_id: String,
    pub protocol_version: u32,
    pub daemon_version: String,
    pub pid: u32,
    pub uptime_ms: u64,
    pub clients: u32,
    /// How many of `clients` are local app connections (the named pipe). A
    /// quitting app asks for this: a `Shutdown` is refused while another
    /// local app client would lose the daemon out from under it. Peers do
    /// not count — a paired device neither blocks nor performs a local
    /// quit.
    #[serde(default)]
    pub local_clients: u32,
    pub sessions: u32,
    /// How many of `sessions` are agents (provider-driven), as opposed to
    /// terminals — present from the daemon that distinguishes them, so an
    /// older status reads as "unknown", never as "zero agents".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<u32>,
    pub capabilities: Vec<Capability>,
    /// Highest live-session scrollback occupancy observed by the daemon.
    #[serde(default)]
    pub peak_ring_bytes: u64,
    /// Aggregate live-session output evictions since those sessions started.
    #[serde(default)]
    pub ring_evicted_bytes: u64,
    /// Aggregate live-session output frames evicted from scrollback.
    #[serde(default)]
    pub ring_dropped_frames: u64,
    /// Present when the conversation journal could not be opened or a live
    /// session has lost journal writes. Live sessions continue; recovery
    /// reports observed losses through the per-session integrity counters.
    /// Losses that were never observed (the uncommitted writer queue dying
    /// with the process) leave no flag anywhere — the recovered state
    /// itself is what carries that doubt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_error: Option<String>,
    /// Live counters of the journal writer, present when the journal was
    /// opened. `None` means the journal is unavailable (see `journalError`):
    /// there is no writer whose behaviour could be counted.
    ///
    /// Boxed only to keep this frame small: `serde` treats `Box<T>`
    /// transparently, so the JSON is identical to an inline field, while the
    /// Tauri client holds the whole body by value inside a request/response
    /// enum and `clippy::large_enum_variant` measures that enum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_stats: Option<Box<JournalStats>>,
    /// Which store holds the Noise static private key: `"keyring"` or
    /// `"file"`. Part of the status body, which a peer reads only with the
    /// administrative capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_store: Option<String>,
    /// Remote-listener state. Part of the status body, which a peer reads only
    /// with the administrative capability; `DevicesList.self_info` carries the
    /// identity to a peer without it.
    /// Boxed for the same size reason as `journal_stats`; the wire is
    /// unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<Box<RemoteState>>,
}

/// Whether the tailnet listener is up, and why it is not when it is not.
///
/// The addresses and the port are **not** here: they describe where this node
/// can be reached, which is `SelfInfo`'s business (brief 1b, wire contract).
/// This type answers one question — is the daemon reachable — and carries the
/// daemon's own reason string when the answer is no.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteState {
    pub state: RemoteStateKind,
    /// Present as `null` when the state is `enabled`. Always serialised, so a
    /// client can distinguish "no reason" from "field absent".
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteStateKind {
    Enabled,
    Disabled,
    KeyMissing,
}

impl RemoteState {
    pub fn enabled() -> Self {
        Self {
            state: RemoteStateKind::Enabled,
            reason: None,
        }
    }

    pub fn disabled(reason: impl Into<String>) -> Self {
        Self {
            state: RemoteStateKind::Disabled,
            reason: Some(reason.into()),
        }
    }

    pub fn key_missing() -> Self {
        Self {
            state: RemoteStateKind::KeyMissing,
            reason: Some(
                "the Noise static key is missing from the secret store; the remote listener \
                 is not started"
                    .to_string(),
            ),
        }
    }
}

/// Live session event on the daemon pipe. `generation` is here, not inside
/// [`SessionEvent`], so the TypeScript Channel contract stays unchanged
/// while a reconnecting daemon client can still detect a recreated process.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventEnvelope {
    pub session_id: String,
    pub generation: u64,
    /// Journal seq of `event` inside `generation`. Absent on a frame that
    /// holds no position in the stream — a marker, a daemon-local card, a row
    /// from another generation's numbering — so a reader treats absence as
    /// "no position", never as position zero.
    #[serde(default)]
    pub transcript_seq: Option<u64>,
    pub event: SessionEvent,
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
