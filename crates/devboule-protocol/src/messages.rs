//! Client and daemon frames. Every message has a `type` tag so a human with a
//! pipe client can read a line and know what it is.

use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::error::WireError;
use crate::handshake::{ClientHello, DaemonHello};
use crate::project::{Project, Workspace, WorkspaceIsolation};
use crate::session::{
    ActiveTurnBehavior, AgentActivityState, Cursor, PermissionOutcome, Persistence, ResumeResult,
    Session, SessionEvent, SessionKind, SubscriptionId,
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
/// an unknown name is an error, never a silently dropped entry.
pub const PEER_CAPS: [&str; 4] = ["view", "send", "answer_permissions", "create_sessions"];
/// Every new pairing starts here (design §8b A11: only "view" is on).
pub const PEER_DEFAULT_CAPS: [&str; 1] = ["view"];

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
///   / ACP agent) but **keep** the session object (id, scrollback, metadata).
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
    /// Read every stored per-provider tool policy. Local-only: a paired
    /// device may not read or change this device's tool gates.
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
            | Self::WorkspaceCreate { id, .. }
            | Self::WorkspaceDelete { id, .. }
            | Self::ProvidersList { id }
            | Self::ProvidersRefresh { id }
            | Self::ProviderUpdate { id, .. }
            | Self::Invoke { id, .. }
            | Self::DevicesList { id }
            | Self::PairingStart { id, .. }
            | Self::PairingComplete { id, .. }
            | Self::PairingConfirm { id, .. }
            | Self::PeerRevoke { id, .. }
            | Self::PeerSetCaps { id, .. }
            | Self::ToolPolicyGet { id }
            | Self::ToolPolicySet { id, .. } => Some(*id),
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
            | Self::WorkspaceCreate { .. }
            | Self::WorkspaceDelete { .. }
            | Self::Invoke { .. }
            | Self::DevicesList { .. }
            | Self::PairingStart { .. }
            | Self::PairingComplete { .. }
            | Self::PairingConfirm { .. }
            | Self::PeerRevoke { .. }
            | Self::PeerSetCaps { .. }
            | Self::ToolPolicyGet { .. }
            | Self::ToolPolicySet { .. } => None,
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
            Self::WorkspaceCreate { .. } => "WorkspaceCreate",
            Self::WorkspaceDelete { .. } => "WorkspaceDelete",
            Self::ProvidersList { .. } => "ProvidersList",
            Self::ProvidersRefresh { .. } => "ProvidersRefresh",
            Self::ProviderUpdate { .. } => "ProviderUpdate",
            Self::Invoke { .. } => "Invoke",
            Self::DevicesList { .. } => "DevicesList",
            Self::PairingStart { .. } => "PairingStart",
            Self::PairingComplete { .. } => "PairingComplete",
            Self::PairingConfirm { .. } => "PairingConfirm",
            Self::PeerRevoke { .. } => "PeerRevoke",
            Self::PeerSetCaps { .. } => "PeerSetCaps",
            Self::ToolPolicyGet { .. } => "ToolPolicyGet",
            Self::ToolPolicySet { .. } => "ToolPolicySet",
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
            | Self::ProvidersList { .. }
            | Self::DevicesList { .. }
            | Self::ToolPolicyGet { .. } => false,

            Self::Shutdown { .. }
            | Self::SessionCreate { .. }
            | Self::SessionAttach { .. }
            | Self::SessionDetach { .. }
            | Self::SessionClaim { .. }
            | Self::SessionClose { .. }
            | Self::SessionStop { .. }
            | Self::SessionSend { .. }
            | Self::AgentMessageSend { .. }
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
            | Self::ProvidersRefresh { .. }
            | Self::ProviderUpdate { .. }
            | Self::Invoke { .. }
            | Self::PairingStart { .. }
            | Self::PairingComplete { .. }
            | Self::PairingConfirm { .. }
            | Self::PeerRevoke { .. }
            | Self::PeerSetCaps { .. }
            | Self::ToolPolicySet { .. } => true,
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
    pub sessions: u32,
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
    /// `"file"`. Local-only information; peers are denied `Status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_store: Option<String>,
    /// Remote-listener state. Local-only information; peers are denied
    /// `Status`, and `DevicesList.self_info` carries the identity instead.
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
    pub event: SessionEvent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionState, SessionStateSnapshot};

    #[test]
    fn the_pairing_code_is_never_debug_formatted() {
        let complete = ClientMessage::PairingComplete {
            id: 1,
            address: "100.64.0.2:47831".to_string(),
            code: PairingSecret::new("ABCD2345"),
            role: PeerRole::Client,
        };
        let rendered = format!("{complete:?}");
        assert!(
            !rendered.contains("ABCD2345"),
            "a pairing code must never reach a log or an error string: {rendered}"
        );
        assert!(rendered.contains("<redacted>"), "{rendered}");

        let code = DaemonMessage::PairingCode {
            id: 1,
            code: PairingSecret::new("ABCD2345"),
            expires_at: 1,
            address: "100.64.0.2:47831".to_string(),
        };
        let rendered = format!("{code:?}");
        assert!(!rendered.contains("ABCD2345"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");

        // The ordinary case: a generic error path formats the whole frame.
        let wrapped = format!("unexpected daemon frame {code:?}");
        assert!(!wrapped.contains("ABCD2345"), "{wrapped}");
    }

    #[test]
    fn the_devices_wire_contract_round_trips_with_its_exact_field_names() {
        // Brief 1b's wire contract is normative for the frontend, so the field
        // names are asserted on the serialised JSON, not on the struct.
        let pending = PendingPairing {
            device_id: "dev-2".to_string(),
            display_name: "Phone".to_string(),
            role: PeerRole::Client,
            key_fingerprint: "ab".repeat(16),
            address: "100.64.0.2:47831".to_string(),
            expires_at: 1_700_000_000_000,
        };
        let pending_json = serde_json::to_value(&pending).expect("json");
        for key in [
            "deviceId",
            "displayName",
            "role",
            "keyFingerprint",
            "address",
            "expiresAt",
        ] {
            assert!(
                pending_json.get(key).is_some(),
                "PendingPairing is missing {key}"
            );
        }
        assert_eq!(pending_json["role"], "client");
        assert_eq!(
            serde_json::from_value::<PendingPairing>(pending_json.clone()).expect("back"),
            pending
        );

        let row = PeerRow {
            device_id: "dev-1".to_string(),
            display_name: "MacBook".to_string(),
            role: PeerRole::Daemon,
            public_key: "AAAA".to_string(),
            key_fingerprint: "cd".repeat(16),
            binding_kind: "tailnet".to_string(),
            binding_node_name: Some("host.tailnet.ts.net.".to_string()),
            binding_login_name: Some("user@example.com".to_string()),
            address: "100.64.0.1:47831".to_string(),
            paired_at: 1_700_000_000_000,
            revoked_at: None,
            caps: vec!["view".to_string()],
            paired_by_user: Some("S-1-5-21-1".to_string()),
            online: true,
        };
        let row_json = serde_json::to_value(&row).expect("json");
        for key in [
            "deviceId",
            "displayName",
            "role",
            "publicKey",
            "keyFingerprint",
            "bindingKind",
            "bindingNodeName",
            "bindingLoginName",
            "address",
            "pairedAt",
            "revokedAt",
            "caps",
            "pairedByUser",
            "online",
        ] {
            assert!(row_json.get(key).is_some(), "PeerRow is missing {key}");
        }
        // Present as `null`, not absent: the panel distinguishes the two.
        assert!(row_json["revokedAt"].is_null());
        assert_eq!(row_json["role"], "daemon");
        assert_eq!(
            serde_json::from_value::<PeerRow>(row_json.clone()).expect("back"),
            row
        );

        let self_info = SelfInfo {
            device_id: "dev-1".to_string(),
            display_name: "MacBook".to_string(),
            public_key: "AAAA".to_string(),
            key_fingerprint: "cd".repeat(16),
            addresses: vec!["100.64.0.1".to_string()],
            port: 47831,
            daemon_version: "0.1.0".to_string(),
            protocol_version: crate::PROTOCOL_VERSION,
            remote: Some(RemoteState::enabled()),
        };
        let self_json = serde_json::to_value(&self_info).expect("json");
        for key in [
            "deviceId",
            "displayName",
            "publicKey",
            "keyFingerprint",
            "addresses",
            "port",
            "daemonVersion",
            "protocolVersion",
            "remote",
        ] {
            assert!(self_json.get(key).is_some(), "SelfInfo is missing {key}");
        }
        assert_eq!(
            self_json["remote"],
            serde_json::json!({ "state": "enabled", "reason": null })
        );

        // A projection withholds by **value**, never by key presence.
        //
        // This is the C4 fix: `addresses`, `port`, `publicKey` and
        // `keyFingerprint` are always in the frame, empty when the projection
        // has nothing to put in them, because the 1b contract types them as
        // required and TypeScript cannot check a key the daemon chose to omit.
        // The panel's identity card reads `self.addresses.length` with no
        // guard, so an omitted key was a crash in the Devices tab whenever
        // remote was off (the default first-run state).
        //
        // `remote` is the exception: it stays absent for the `Daemon`
        // projection, because `RemoteState` has no "unknown" variant, so its
        // only way to withhold the listener state is to omit the object.
        let withheld = SelfInfo {
            device_id: "dev-1".to_string(),
            display_name: "MacBook".to_string(),
            public_key: String::new(),
            key_fingerprint: String::new(),
            addresses: Vec::new(),
            port: 0,
            daemon_version: "0.1.0".to_string(),
            protocol_version: crate::PROTOCOL_VERSION,
            remote: None,
        };
        let withheld_json = serde_json::to_value(&withheld).expect("json");
        assert_eq!(
            withheld_json["addresses"],
            serde_json::json!([]),
            "an empty address list is the empty array, not an absent key: {withheld_json}"
        );
        assert_eq!(
            withheld_json["port"],
            serde_json::json!(0),
            "a listener-less self_info carries port 0: {withheld_json}"
        );
        assert_eq!(withheld_json["publicKey"], "");
        assert_eq!(withheld_json["keyFingerprint"], "");
        for key in ["publicKey", "keyFingerprint", "addresses", "port"] {
            assert!(
                withheld_json.get(key).is_some(),
                "a withheld SelfInfo must still carry {key}: {withheld_json}"
            );
        }
        assert!(
            withheld_json.get("remote").is_none(),
            "the Daemon projection withholds the listener state by omission: {withheld_json}"
        );
        for key in [
            "deviceId",
            "displayName",
            "daemonVersion",
            "protocolVersion",
        ] {
            assert!(
                withheld_json.get(key).is_some(),
                "a withheld SelfInfo keeps {key}: {withheld_json}"
            );
        }

        // The other direction, and the one the panel actually hits: a real
        // local projection with remote off serialises every contract key with
        // empty values, so `self.addresses.length` has something to read.
        let local_off = SelfInfo {
            remote: Some(RemoteState::disabled("no tailscale")),
            ..withheld.clone()
        };
        let local_json = serde_json::to_value(&local_off).expect("json");
        assert_eq!(local_json["addresses"], serde_json::json!([]));
        assert_eq!(local_json["port"], 0);
        assert_eq!(local_json["remote"]["state"], "disabled");
        assert!(
            local_json["addresses"].is_array() && local_json["port"].is_number(),
            "the panel can read these unconditionally: {local_json}"
        );

        // The reply variants carry exactly the contract's fields.
        let devices = serde_json::to_value(DaemonMessage::Devices {
            id: 4,
            self_info: self_info.clone(),
            peers: vec![row.clone()],
            pending: vec![pending.clone()],
        })
        .expect("json");
        assert_eq!(devices["type"], "devices");
        assert_eq!(devices["id"], 4);
        for key in ["selfInfo", "peers", "pending"] {
            assert!(devices.get(key).is_some(), "Devices is missing {key}");
        }
        assert_eq!(
            serde_json::to_value(DaemonMessage::PairingPending {
                id: 5,
                peer: pending.clone(),
            })
            .expect("json")["type"],
            "pairing_pending"
        );
        assert_eq!(
            serde_json::to_value(DaemonMessage::PairingDone {
                id: 6,
                peer: row.clone(),
            })
            .expect("json")["type"],
            "pairing_done"
        );
        assert_eq!(
            serde_json::to_value(DaemonMessage::PeerUpdated { id: 7, peer: row }).expect("json")
                ["type"],
            "peer_updated"
        );
        let declined = serde_json::to_value(DaemonMessage::PairingDeclined {
            id: 8,
            device_id: "dev-2".to_string(),
        })
        .expect("json");
        assert_eq!(declined["type"], "pairing_declined");
        assert_eq!(declined["id"], 8);
        assert_eq!(declined["deviceId"], "dev-2");

        // And the request variants, with their argument names.
        let requests: [(ClientMessage, &str, &[&str]); 6] = [
            (
                ClientMessage::DevicesList { id: 1 },
                "devices_list",
                &["id"],
            ),
            (
                ClientMessage::PairingStart {
                    id: 1,
                    role: PeerRole::Daemon,
                },
                "pairing_start",
                &["id", "role"],
            ),
            (
                ClientMessage::PairingComplete {
                    id: 1,
                    address: "100.64.0.2:47831".to_string(),
                    code: PairingSecret::new("ABCD2345"),
                    role: PeerRole::Client,
                },
                "pairing_complete",
                &["id", "address", "code", "role"],
            ),
            (
                ClientMessage::PairingConfirm {
                    id: 1,
                    device_id: "dev-2".to_string(),
                    accept: true,
                },
                "pairing_confirm",
                &["id", "deviceId", "accept"],
            ),
            (
                ClientMessage::PeerRevoke {
                    id: 1,
                    device_id: "dev-2".to_string(),
                },
                "peer_revoke",
                &["id", "deviceId"],
            ),
            (
                ClientMessage::PeerSetCaps {
                    id: 1,
                    device_id: "dev-2".to_string(),
                    caps: vec!["view".to_string(), "send".to_string()],
                },
                "peer_set_caps",
                &["id", "deviceId", "caps"],
            ),
        ];
        for (request, tag, keys) in requests {
            let json = serde_json::to_value(&request).expect("json");
            assert_eq!(json["type"], tag, "{request:?}");
            for key in keys {
                assert!(json.get(key).is_some(), "{tag} is missing {key}");
            }
            assert_eq!(
                serde_json::from_value::<ClientMessage>(json.clone()).expect("back"),
                request
            );
        }
    }

    #[test]
    fn devices_capability_is_advertised_and_the_peer_caps_are_the_agreed_set() {
        assert!(crate::m3a_daemon_capabilities()
            .iter()
            .any(|capability| capability.as_str() == crate::caps::DEVICES));
        assert_eq!(PEER_DEFAULT_CAPS, ["view"]);
        assert_eq!(
            PEER_CAPS,
            ["view", "send", "answer_permissions", "create_sessions"]
        );
    }

    #[test]
    fn remote_state_serialises_exactly_the_agreed_shape() {
        // Brief 1b's wire contract, pinned here because the frontend is built
        // against this JSON: `{ state, reason }`, both keys always present.
        let enabled = serde_json::to_value(RemoteState::enabled()).expect("json");
        assert_eq!(
            enabled,
            serde_json::json!({ "state": "enabled", "reason": null })
        );

        let disabled =
            serde_json::to_value(RemoteState::disabled("Tailscale is not running")).expect("json");
        assert_eq!(
            disabled,
            serde_json::json!({ "state": "disabled", "reason": "Tailscale is not running" })
        );

        let missing = serde_json::to_value(RemoteState::key_missing()).expect("json");
        assert_eq!(missing["state"], "key_missing");
        assert!(missing["reason"].is_string());

        // Round-trips, so a local client can echo it back in a test fixture.
        for state in [
            RemoteState::enabled(),
            RemoteState::disabled("why"),
            RemoteState::key_missing(),
        ] {
            let json = serde_json::to_string(&state).expect("serialize");
            assert_eq!(
                serde_json::from_str::<RemoteState>(&json).expect("deserialize"),
                state
            );
        }
    }

    #[test]
    fn only_writes_are_state_changing_and_every_variant_names_itself() {
        assert!(!ClientMessage::Ping { id: 1 }.is_state_changing());
        assert!(!ClientMessage::Status { id: 1 }.is_state_changing());
        assert!(!ClientMessage::SessionsList { id: 1 }.is_state_changing());
        assert!(!ClientMessage::JournalUsage { id: 1 }.is_state_changing());
        assert!(!ClientMessage::ProjectsList { id: 1 }.is_state_changing());
        assert!(!ClientMessage::JournalRetentionGet { id: 1 }.is_state_changing());
        assert!(ClientMessage::Shutdown { id: 1 }.is_state_changing());
        assert!(ClientMessage::JournalRetentionSet {
            id: 1,
            max_age_ms: None,
            max_bytes: None,
            max_sessions: None,
            session_max_bytes: None,
            idempotency_key: None,
        }
        .is_state_changing());
        assert!(ClientMessage::SessionSend {
            id: 1,
            session_id: "s.a.1".to_string(),
            subscription_id: 1,
            text: "hi".to_string(),
            attachments: Vec::new(),
            active_turn_behavior: None,
            idempotency_key: None,
        }
        .is_state_changing());
        assert!(ClientMessage::ProvidersRefresh { id: 1 }.is_state_changing());
        assert!(!ClientMessage::ToolPolicyGet { id: 1 }.is_state_changing());
        assert!(ClientMessage::ToolPolicySet {
            id: 1,
            provider_id: "claude".to_string(),
            enabled: None,
            disabled_tools: Vec::new(),
        }
        .is_state_changing());
        assert_eq!(
            ClientMessage::ToolPolicyGet { id: 1 }.name(),
            "ToolPolicyGet"
        );
        assert_eq!(
            ClientMessage::ToolPolicySet {
                id: 1,
                provider_id: "claude".to_string(),
                enabled: Some(true),
                disabled_tools: Vec::new(),
            }
            .name(),
            "ToolPolicySet"
        );

        assert_eq!(ClientMessage::Ping { id: 1 }.name(), "Ping");
        assert_eq!(
            ClientMessage::SessionSetMode {
                id: 1,
                session_id: "s.a.1".to_string(),
                mode_id: "acceptEdits".to_string(),
            }
            .name(),
            "SessionSetMode"
        );
        assert_eq!(
            ClientMessage::Hello(crate::ClientHello::m3a(
                crate::OwnerId::new("u", "c").expect("owner"),
                "test",
            ))
            .name(),
            "Hello"
        );
        assert!(!ClientMessage::Hello(crate::ClientHello::m3a(
            crate::OwnerId::new("u", "c").expect("owner"),
            "test",
        ))
        .is_state_changing());
    }

    #[test]
    fn detach_close_stop_are_three_type_tags() {
        let detach = serde_json::to_value(ClientMessage::SessionDetach {
            id: 1,
            session_id: "s.a.1".to_string(),
            subscription_id: 11,
        })
        .expect("json");
        let close = serde_json::to_value(ClientMessage::SessionClose {
            id: 1,
            session_id: "s.a.1".to_string(),
            idempotency_key: None,
        })
        .expect("json");
        let stop = serde_json::to_value(ClientMessage::SessionStop {
            id: 1,
            session_id: "s.a.1".to_string(),
            subscription_id: 11,
        })
        .expect("json");
        assert_eq!(detach["type"], "session_detach");
        assert_eq!(close["type"], "session_close");
        assert_eq!(stop["type"], "session_stop");
        assert_ne!(detach["type"], close["type"]);
        assert_ne!(close["type"], stop["type"]);
        assert_ne!(detach["type"], stop["type"]);
    }

    #[test]
    fn session_send_without_attachments_still_deserializes() {
        // An older client does not know the field at all. Dropping it here
        // would make `serde(default)` on the variant look like it worked while
        // every other builder in this crate still had to pass it: the frame
        // below is the one a v4 client sends today.
        let frame = r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello"}"#;
        let message: ClientMessage = serde_json::from_str(frame).expect("old frame");
        assert_eq!(
            message,
            ClientMessage::SessionSend {
                id: 7,
                session_id: "s.a.1".to_string(),
                subscription_id: 11,
                text: "hello".to_string(),
                attachments: Vec::new(),
                active_turn_behavior: None,
                idempotency_key: None,
            }
        );
    }

    #[test]
    fn session_send_with_attachments_round_trips() {
        let message = ClientMessage::SessionSend {
            id: 7,
            session_id: "s.a.1".to_string(),
            subscription_id: 11,
            text: "hello".to_string(),
            attachments: vec![PromptAttachment {
                name: "photo.png".to_string(),
                mime_type: "image/png".to_string(),
                data: "AA==".to_string(),
            }],
            active_turn_behavior: None,
            idempotency_key: None,
        };
        let value = serde_json::to_value(&message).expect("json");
        assert_eq!(value["attachments"][0]["mimeType"], "image/png");
        assert_eq!(value["attachments"][0]["name"], "photo.png");
        assert_eq!(value["attachments"][0]["data"], "AA==");
        let decoded: ClientMessage = serde_json::from_value(value).expect("round trip");
        assert_eq!(decoded, message);
    }

    #[test]
    fn session_send_accepts_only_the_steer_active_turn_behavior() {
        let steer: ClientMessage = serde_json::from_str(
            r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello","activeTurnBehavior":"steer"}"#,
        )
        .expect("steer frame");
        assert!(matches!(
            steer,
            ClientMessage::SessionSend {
                active_turn_behavior: Some(ActiveTurnBehavior::Steer),
                ..
            }
        ));
        assert!(serde_json::from_str::<ClientMessage>(
            r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello","activeTurnBehavior":"replace"}"#
        )
        .is_err());
        // The two shapes a hand-written frame gets wrong: an empty value (the
        // field is present, so `default` does not apply) and a differently-cased
        // spelling of the one behaviour. Both must be refused by the decoder,
        // which is where the daemon's own frame reader refuses them: a steer the
        // daemon read as "the default" would be an interrupt-and-replace the
        // caller never asked for.
        assert!(serde_json::from_str::<ClientMessage>(
            r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello","activeTurnBehavior":""}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ClientMessage>(
            r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello","activeTurnBehavior":"Steer"}"#
        )
        .is_err());
    }

    #[test]
    fn agent_message_receipt_round_trips_with_the_wire_state() {
        let message = DaemonMessage::AgentMessageReceipt {
            id: 9,
            state: AgentMessageState::RejectedAbsent,
        };
        let value = serde_json::to_value(&message).expect("json");
        assert_eq!(value["type"], "agent_message_receipt");
        assert_eq!(value["state"], "rejected_absent");
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(value).expect("decode"),
            message
        );
    }

    /// A2-07: the receipt that says a caller was *denied* has its own wire
    /// spelling, and it is not the one that blames the pairing.
    #[test]
    fn a_denied_agent_message_has_its_own_wire_state() {
        let message = DaemonMessage::AgentMessageReceipt {
            id: 10,
            state: AgentMessageState::RejectedDenied,
        };
        let value = serde_json::to_value(&message).expect("json");
        assert_eq!(value["state"], "rejected_denied");
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(value).expect("decode"),
            message
        );
        assert_ne!(
            serde_json::to_value(AgentMessageState::RejectedUnpaired).expect("json"),
            serde_json::to_value(AgentMessageState::RejectedDenied).expect("json"),
            "a denial is not an unpaired caller, and the wire must not say it is"
        );
    }

    #[test]
    fn session_send_with_no_attachments_omits_the_field() {
        let value = serde_json::to_value(ClientMessage::SessionSend {
            id: 7,
            session_id: "s.a.1".to_string(),
            subscription_id: 11,
            text: "hello".to_string(),
            attachments: Vec::new(),
            active_turn_behavior: None,
            idempotency_key: None,
        })
        .expect("json");
        assert!(
            value.get("attachments").is_none(),
            "an empty list must not add a field to every send frame"
        );
    }

    #[test]
    fn session_state_broadcast_is_a_compact_event_snapshot() {
        let message = DaemonMessage::Event(SessionEventEnvelope {
            session_id: String::new(),
            generation: 0,
            event: SessionEvent::SessionsSnapshot {
                sessions: vec![SessionStateSnapshot {
                    id: "s.client.1".to_string(),
                    workspace_id: Some("workspace-1".to_string()),
                    kind: SessionKind::Terminal,
                    title: "Terminal".to_string(),
                    state: SessionState::Silent { generation: 3 },
                    elapsed_ms: Some(300_001),
                    attention: None,
                    origin: crate::SessionOrigin::peer("device-phone", PeerRole::Client),
                }],
            },
        });
        let value = serde_json::to_value(message).expect("json");
        assert_eq!(value["event"]["type"], "sessions_snapshot");
        assert_eq!(value["event"]["sessions"][0]["id"], "s.client.1");
        assert_eq!(value["event"]["sessions"][0]["title"], "Terminal");
        assert_eq!(value["event"]["sessions"][0]["state"]["type"], "silent");
        assert_eq!(value["event"]["sessions"][0]["elapsedMs"], 300_001);
        assert_eq!(value["event"]["sessions"][0]["workspaceId"], "workspace-1");
        assert_eq!(value["event"]["sessions"][0]["kind"], "terminal");
    }

    #[test]
    fn attach_cursor_carries_generation_and_seq() {
        let value = serde_json::to_value(ClientMessage::SessionAttach {
            id: 3,
            session_id: "s.a.1".to_string(),
            subscription_id: 12,
            from_cursor: Some(Cursor {
                generation: 2,
                seq: 40,
            }),
        })
        .expect("json");
        assert_eq!(value["fromCursor"]["generation"], 2);
        assert_eq!(value["fromCursor"]["seq"], 40);
    }

    #[test]
    fn subscription_identity_is_explicit_in_attach_reply_claim_and_events() {
        let attach = ClientMessage::SessionAttach {
            id: 3,
            session_id: "s.a.1".to_string(),
            subscription_id: 12,
            from_cursor: None,
        };
        let attach_json = serde_json::to_value(&attach).expect("attach json");
        assert_eq!(attach_json["type"], "session_attach");
        assert_eq!(attach_json["subscriptionId"], 12);
        assert_eq!(attach.request_id(), Some(3));

        let claim = ClientMessage::SessionClaim {
            id: 4,
            session_id: "s.a.1".to_string(),
            subscription_id: 12,
        };
        let claim_json = serde_json::to_value(&claim).expect("claim json");
        assert_eq!(claim_json["type"], "session_claim");
        assert_eq!(claim_json["subscriptionId"], 12);
        assert_eq!(claim.request_id(), Some(4));

        let attached = DaemonMessage::SessionAttached {
            id: 4,
            subscription_id: 12,
        };
        let attached_json = serde_json::to_value(&attached).expect("attach reply json");
        assert_eq!(attached_json["type"], "session_attached");
        assert_eq!(attached_json["subscriptionId"], 12);

        let event = DaemonMessage::SubscriptionEvent {
            subscription_id: 12,
            envelope: SessionEventEnvelope {
                session_id: "s.a.1".to_string(),
                generation: 1,
                event: SessionEvent::AgentMessage {
                    message_id: None,
                    text: "hello".to_string(),
                    parent_tool_use_id: None,
                    spawn_depth: None,
                },
            },
        };
        let event_json = serde_json::to_value(&event).expect("subscription event json");
        assert_eq!(event_json["type"], "subscription_event");
        assert_eq!(event_json["subscriptionId"], 12);
        assert_eq!(event_json["envelope"]["sessionId"], "s.a.1");
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(event_json).expect("event round trip"),
            event
        );
    }

    #[test]
    fn presence_carries_focus_and_visibility_per_connection() {
        let message = ClientMessage::SessionsPresence {
            id: 4,
            focused_session_id: Some("s.a.1".to_string()),
            app_visible: true,
        };
        let value = serde_json::to_value(&message).expect("presence json");
        assert_eq!(value["type"], "sessions_presence");
        assert_eq!(value["focusedSessionId"], "s.a.1");
        assert_eq!(value["appVisible"], true);
        let decoded: ClientMessage = serde_json::from_value(value).expect("presence round trip");
        assert_eq!(decoded, message);
    }

    #[test]
    fn project_workspace_wire_fields_are_camel_case() {
        let request = ClientMessage::WorkspaceCreate {
            id: 7,
            project_id: "p.one".to_string(),
            isolation: WorkspaceIsolation::Local,
            branch: Some("main".to_string()),
        };
        let value = serde_json::to_value(&request).expect("workspace request json");
        assert_eq!(value["type"], "workspace_create");
        assert_eq!(value["projectId"], "p.one");
        assert_eq!(value["isolation"], "local");
        assert_eq!(value["branch"], "main");
        assert!(value.get("project_id").is_none());

        let workspace = Workspace {
            id: "w.one".to_string(),
            project_id: "p.one".to_string(),
            title: "Project".to_string(),
            isolation: WorkspaceIsolation::Local,
            path: r"C:\code\Project".to_string(),
        };
        let reply = serde_json::to_value(DaemonMessage::Workspace { id: 7, workspace })
            .expect("workspace reply json");
        assert_eq!(reply["workspace"]["projectId"], "p.one");
        assert_eq!(reply["workspace"]["path"], r"C:\code\Project");
        assert!(reply["workspace"].get("project_id").is_none());

        let delete = ClientMessage::WorkspaceDelete {
            id: 8,
            workspace_id: "w.one".to_string(),
            force: true,
        };
        let value = serde_json::to_value(&delete).expect("workspace delete json");
        assert_eq!(value["type"], "workspace_delete");
        assert_eq!(value["workspaceId"], "w.one");
        assert_eq!(value["force"], true);
    }

    #[test]
    fn create_send_permission_carry_idempotency_key() {
        let create = ClientMessage::SessionCreate {
            id: 1,
            workspace_id: None,
            kind: SessionKind::Terminal,
            provider: None,
            mode: None,
            idempotency_key: Some("k1".to_string()),
        };
        let send = ClientMessage::SessionSend {
            id: 2,
            session_id: "s.a.1".to_string(),
            subscription_id: 12,
            text: "x".to_string(),
            attachments: Vec::new(),
            active_turn_behavior: None,
            idempotency_key: Some("k2".to_string()),
        };
        let perm = ClientMessage::SessionPermissionRespond {
            id: 3,
            session_id: "s.a.1".to_string(),
            subscription_id: 12,
            request_id: "r1".to_string(),
            outcome: PermissionOutcome::AllowOnce,
            option_id: Some("allow-once".to_string()),
            idempotency_key: Some("k3".to_string()),
        };
        assert_eq!(create.idempotency_key(), Some("k1"));
        assert_eq!(send.idempotency_key(), Some("k2"));
        assert_eq!(perm.idempotency_key(), Some("k3"));
        assert_eq!(
            serde_json::to_value(&perm).expect("json")["outcome"],
            "allow_once"
        );
        assert_eq!(
            serde_json::to_value(&perm).expect("json")["optionId"],
            "allow-once"
        );
        let decoded: ClientMessage =
            serde_json::from_value(serde_json::to_value(&perm).expect("json"))
                .expect("permission response round trip");
        assert_eq!(decoded, perm);
    }

    #[test]
    fn permission_resolved_reports_the_selected_option() {
        let event = DaemonMessage::Event(SessionEventEnvelope {
            session_id: "s.a.1".to_string(),
            generation: 1,
            event: SessionEvent::PermissionResolved {
                tool_call_id: "tool-1".to_string(),
                selected_option_id: Some("allow-once".to_string()),
                selected_option_kind: Some("allow_once".to_string()),
                selected_option_name: Some("Allow once".to_string()),
            },
        });
        let value = serde_json::to_value(&event).expect("permission resolved json");
        assert_eq!(value["event"]["selectedOptionId"], "allow-once");
        assert_eq!(value["event"]["selectedOptionKind"], "allow_once");
        assert_eq!(value["event"]["selectedOptionName"], "Allow once");
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(value).expect("permission resolved round trip"),
            event
        );
    }

    #[test]
    fn legacy_permission_frames_without_option_fields_still_parse() {
        // Bytes, not Rust-to-Rust: an older client answers without
        // `optionId`, and an older daemon resolves without the option triple.
        let respond: ClientMessage = serde_json::from_str(
            r#"{"type":"session_permission_respond","id":3,"sessionId":"s.a.1","subscriptionId":12,"requestId":"r1","outcome":"allow_once"}"#,
        )
        .expect("legacy permission response parses");
        assert_eq!(
            respond,
            ClientMessage::SessionPermissionRespond {
                id: 3,
                session_id: "s.a.1".to_string(),
                subscription_id: 12,
                request_id: "r1".to_string(),
                outcome: PermissionOutcome::AllowOnce,
                option_id: None,
                idempotency_key: None,
            }
        );

        let resolved: DaemonMessage = serde_json::from_str(
            r#"{"type":"event","sessionId":"s.a.1","generation":1,"event":{"type":"permission_resolved","toolCallId":"tool-1"}}"#,
        )
        .expect("legacy permission resolved parses");
        assert_eq!(
            resolved,
            DaemonMessage::Event(SessionEventEnvelope {
                session_id: "s.a.1".to_string(),
                generation: 1,
                event: SessionEvent::PermissionResolved {
                    tool_call_id: "tool-1".to_string(),
                    selected_option_id: None,
                    selected_option_kind: None,
                    selected_option_name: None,
                },
            })
        );
        let value = serde_json::to_value(&resolved).expect("resolved json");
        assert!(value["event"].get("selectedOptionId").is_none());
        assert!(value["event"].get("selectedOptionKind").is_none());
        assert!(value["event"].get("selectedOptionName").is_none());
    }

    #[test]
    fn retention_mutations_carry_idempotency_keys() {
        let retention = ClientMessage::JournalRetentionSet {
            id: 4,
            max_age_ms: None,
            max_bytes: Some(10),
            max_sessions: None,
            session_max_bytes: None,
            idempotency_key: Some("retention-key".to_string()),
        };
        let delete = ClientMessage::SessionDelete {
            id: 5,
            session_id: "s.a.1".to_string(),
            idempotency_key: Some("delete-key".to_string()),
        };
        let close = ClientMessage::SessionClose {
            id: 6,
            session_id: "s.a.1".to_string(),
            idempotency_key: Some("close-key".to_string()),
        };
        assert_eq!(retention.idempotency_key(), Some("retention-key"));
        assert_eq!(delete.idempotency_key(), Some("delete-key"));
        assert_eq!(close.idempotency_key(), Some("close-key"));
        assert_eq!(
            serde_json::to_value(retention).expect("json")["idempotencyKey"],
            "retention-key"
        );
        assert_eq!(
            serde_json::to_value(delete).expect("json")["idempotencyKey"],
            "delete-key"
        );
        assert_eq!(
            serde_json::to_value(close).expect("json")["idempotencyKey"],
            "close-key"
        );
    }

    #[test]
    fn ping_roundtrip() {
        let msg = ClientMessage::Ping { id: 7 };
        let encoded = serde_json::to_string(&msg).expect("json");
        assert!(!encoded.contains('\n'));
        let decoded: ClientMessage = serde_json::from_str(&encoded).expect("parse");
        assert_eq!(msg, decoded);
    }

    #[test]
    fn session_set_model_round_trips_with_optional_fields() {
        let msg = ClientMessage::SessionSetModel {
            id: 8,
            session_id: "s.a.1".to_string(),
            model_id: Some("grok-4.5".to_string()),
            effort: Some("low".to_string()),
        };
        let value = serde_json::to_value(&msg).expect("json");
        assert_eq!(value["type"], "session_set_model");
        assert_eq!(value["sessionId"], "s.a.1");
        assert_eq!(value["modelId"], "grok-4.5");
        assert_eq!(value["effort"], "low");
        let encoded = serde_json::to_string(&msg).expect("json");
        let decoded: ClientMessage = serde_json::from_str(&encoded).expect("parse");
        assert_eq!(decoded, msg);

        let effort_only = ClientMessage::SessionSetModel {
            id: 9,
            session_id: "s.a.1".to_string(),
            model_id: None,
            effort: Some("high".to_string()),
        };
        let effort_only_value = serde_json::to_value(effort_only).expect("json");
        assert!(effort_only_value.get("modelId").is_none());
    }

    #[test]
    fn session_set_mode_round_trips_with_camel_case_fields() {
        let message = ClientMessage::SessionSetMode {
            id: 10,
            session_id: "s.a.1".to_string(),
            mode_id: "acceptEdits".to_string(),
        };
        let value = serde_json::to_value(&message).expect("json");
        assert_eq!(value["type"], "session_set_mode");
        assert_eq!(value["sessionId"], "s.a.1");
        assert_eq!(value["modeId"], "acceptEdits");
        assert_eq!(message.request_id(), Some(10));
        assert_eq!(message.idempotency_key(), None);
        assert_eq!(
            serde_json::from_value::<ClientMessage>(value).expect("round trip"),
            message
        );
    }

    #[test]
    fn session_create_round_trips_an_optional_mode() {
        let message = ClientMessage::SessionCreate {
            id: 11,
            workspace_id: None,
            kind: SessionKind::Claude,
            provider: None,
            mode: Some("plan".to_string()),
            idempotency_key: None,
        };
        let value = serde_json::to_value(&message).expect("json");
        assert_eq!(value["mode"], "plan");
        assert_eq!(
            serde_json::from_value::<ClientMessage>(value).expect("round trip"),
            message
        );
    }

    #[test]
    fn session_report_agent_round_trips_with_herdr_payload() {
        let msg = ClientMessage::SessionReportAgent {
            id: 11,
            session_id: "s.client.1".to_string(),
            source: "devboule:stub".to_string(),
            agent: "stub".to_string(),
            state: AgentActivityState::Working,
            message: None,
            seq: Some(3),
            agent_session_id: Some("agent-1".to_string()),
            agent_session_path: None,
            session_start_source: Some("startup".to_string()),
        };
        let value = serde_json::to_value(&msg).expect("json");
        assert_eq!(value["type"], "session_report_agent");
        assert_eq!(value["sessionId"], "s.client.1");
        assert_eq!(value["source"], "devboule:stub");
        assert_eq!(value["agent"], "stub");
        assert_eq!(value["state"], "working");
        assert_eq!(value["seq"], 3);
        assert_eq!(value["agentSessionId"], "agent-1");
        assert_eq!(value["sessionStartSource"], "startup");
        assert!(value.get("message").is_none());
        assert!(value.get("agentSessionPath").is_none());
        assert_eq!(msg.request_id(), Some(11));
        assert_eq!(msg.idempotency_key(), None);
        let encoded = serde_json::to_string(&msg).expect("json");
        assert!(!encoded.contains('\n'));
        let decoded: ClientMessage = serde_json::from_str(&encoded).expect("parse");
        assert_eq!(decoded, msg);
    }

    #[test]
    fn invoke_is_the_plugin_tenant_on_the_same_frames() {
        let msg = ClientMessage::Invoke {
            id: 11,
            method: crate::caps::WORKSPACE_ROOT.to_string(),
            payload: None,
        };
        let value = serde_json::to_value(&msg).expect("json");
        assert_eq!(value["type"], "invoke");
        assert_eq!(value["id"], 11);
        assert_eq!(value["method"], "workspace.root");
        assert!(value.get("payload").is_none());
        assert_eq!(msg.request_id(), Some(11));

        let reply = DaemonMessage::InvokeResult {
            id: 11,
            value: serde_json::json!({
                "root": r"C:\repo",
                "status": "ok"
            }),
        };
        let encoded = serde_json::to_string(&reply).expect("json");
        assert!(!encoded.contains('\n'));
        let decoded: DaemonMessage = serde_json::from_str(&encoded).expect("parse");
        assert_eq!(decoded, reply);
        let wire = serde_json::to_value(&reply).expect("json");
        assert_eq!(wire["type"], "invoke_result");
        assert_eq!(wire["value"]["root"], r"C:\repo");
        assert_eq!(wire["value"]["status"], "ok");
    }

    #[test]
    fn journal_stats_round_trips_with_camel_case_wire_names() {
        let stats = JournalStats {
            accepted_frames: 12,
            accepted_bytes: 4096,
            committed_frames: 10,
            committed_bytes: 3840,
            failed_frames: 2,
        };
        let encoded = serde_json::to_value(stats).expect("json");
        assert_eq!(encoded["acceptedFrames"], 12);
        assert_eq!(encoded["acceptedBytes"], 4096);
        assert_eq!(encoded["committedFrames"], 10);
        assert_eq!(encoded["committedBytes"], 3840);
        assert_eq!(encoded["failedFrames"], 2);
        let decoded: JournalStats = serde_json::from_value(encoded).expect("parse");
        assert_eq!(decoded, stats);
    }

    #[test]
    fn status_body_treats_journal_stats_as_optional_for_older_daemons() {
        // A daemon predating the field must still parse; the client must
        // read its absence as "no journal writer", not as a wire error.
        let older_daemon_frame = serde_json::json!({
            "type": "status",
            "id": 5,
            "instanceId": "i",
            "protocolVersion": 2,
            "daemonVersion": "0.0.0",
            "pid": 42,
            "uptimeMs": 7,
            "clients": 1,
            "sessions": 2,
            "capabilities": [],
            "peakRingBytes": 0,
            "ringEvictedBytes": 0,
            "ringDroppedFrames": 0
        });
        let decoded = serde_json::from_value::<DaemonStatusBody>(older_daemon_frame)
            .expect("a status frame without journalStats");
        assert!(decoded.journal_stats.is_none());
    }

    #[test]
    fn boxing_journal_stats_and_remote_does_not_change_the_wire() {
        // The two fields are `Box`ed purely to keep this frame small: the Tauri
        // client stores the whole body inside an enum and
        // `clippy::large_enum_variant` measures that enum. `serde` treats
        // `Box<T>` as transparent, so the JSON must be byte-identical to the
        // inline shape, including the camelCase names and the nested `remote`
        // object.
        let body = DaemonStatusBody {
            instance_id: "i".to_string(),
            protocol_version: 4,
            daemon_version: "0.0.0".to_string(),
            pid: 1,
            uptime_ms: 2,
            clients: 0,
            sessions: 0,
            capabilities: Vec::new(),
            peak_ring_bytes: 0,
            ring_evicted_bytes: 0,
            ring_dropped_frames: 0,
            journal_error: None,
            journal_stats: Some(Box::new(JournalStats {
                accepted_frames: 1,
                accepted_bytes: 2,
                committed_frames: 3,
                committed_bytes: 4,
                failed_frames: 5,
            })),
            secret_store: Some("file".to_string()),
            remote: Some(Box::new(RemoteState::disabled("no tailscale"))),
        };
        let json = serde_json::to_value(&body).expect("json");
        assert_eq!(json["journalStats"]["acceptedFrames"], 1);
        assert_eq!(json["journalStats"]["failedFrames"], 5);
        assert_eq!(json["secretStore"], "file");
        assert_eq!(json["remote"]["state"], "disabled");
        assert_eq!(json["remote"]["reason"], "no tailscale");

        let decoded: DaemonStatusBody = serde_json::from_value(json.clone()).expect("back");
        assert_eq!(
            decoded.journal_stats.as_deref(),
            body.journal_stats.as_deref()
        );
        assert_eq!(decoded.remote.as_deref(), body.remote.as_deref());
        assert_eq!(
            serde_json::to_value(&decoded).expect("json"),
            json,
            "the round trip must not move a byte"
        );
    }

    #[test]
    fn pty_output_newlines_are_escaped_in_compact_json() {
        let event = SessionEvent::Output {
            seq: 1,
            data: "line1\nline2".to_string(),
        };
        let encoded = serde_json::to_string(&event).expect("json");
        assert!(
            !encoded.contains('\n'),
            "compact JSON must not contain a raw newline or NDJSON framing splits the event"
        );
        assert!(encoded.contains("\\n"));
    }

    #[test]
    fn journal_commands_round_trip_the_amended_usage_shape() {
        let set = ClientMessage::JournalRetentionSet {
            id: 17,
            session_max_bytes: Some(0),
            max_bytes: Some(8_000),
            max_sessions: None,
            max_age_ms: Some(0),
            idempotency_key: None,
        };
        let wire = serde_json::to_value(&set).expect("json");
        assert_eq!(wire["type"], "journal_retention_set");
        assert_eq!(wire["sessionMaxBytes"], 0);
        assert_eq!(wire["maxBytes"], 8_000);
        assert!(wire.get("maxSessions").is_none());

        let usage = DaemonMessage::JournalUsage {
            id: 17,
            usage: JournalUsage {
                total_bytes: 10,
                session_count: 2,
                deleted_by_user: 1,
                deleted_by_retention: 4,
                unreclaimable: Unreclaimable {
                    bytes_over: 3,
                    sessions_over: 4,
                    aged_out: 5,
                },
                limits: JournalLimits {
                    snapshot_every_bytes: 1,
                    session_max_bytes: 2,
                    max_bytes: 3,
                    max_sessions: 4,
                    max_age_ms: 5,
                },
                per_session: vec![JournalSessionUsage {
                    id: "s.1".to_string(),
                    title: "Terminal".to_string(),
                    kind: SessionKind::Terminal,
                    bytes: 6,
                    updated_at_ms: 7,
                }],
            },
        };
        let encoded = serde_json::to_string(&usage).expect("json");
        assert!(encoded.contains("\"deletedByUser\":1"));
        assert!(encoded.contains("\"deletedByRetention\":4"));
        assert!(encoded.contains("\"unreclaimable\":{"));
        assert!(encoded.contains("\"bytesOver\":3"));
        assert!(encoded.contains("\"sessionsOver\":4"));
        assert!(encoded.contains("\"agedOut\":5"));
        assert_eq!(
            serde_json::from_str::<DaemonMessage>(&encoded).expect("round trip"),
            usage
        );
    }

    #[test]
    fn tool_policy_wire_contract_round_trips_with_its_exact_field_names() {
        // The Settings panel is built against this JSON, and a rename here is
        // a silently inert toggle there, so the names are asserted on the
        // serialised form rather than on the Rust fields.
        let get = serde_json::to_value(ClientMessage::ToolPolicyGet { id: 31 }).expect("json");
        assert_eq!(
            get,
            serde_json::json!({"type": "tool_policy_get", "id": 31})
        );

        let set = ClientMessage::ToolPolicySet {
            id: 32,
            provider_id: "claude".to_string(),
            enabled: Some(false),
            disabled_tools: vec!["devboule_list_agents".to_string()],
        };
        let set_json = serde_json::to_value(&set).expect("json");
        assert_eq!(set_json["type"], "tool_policy_set");
        assert_eq!(set_json["providerId"], "claude");
        assert_eq!(set_json["enabled"], false);
        assert_eq!(set_json["disabledTools"][0], "devboule_list_agents");
        assert_eq!(
            serde_json::from_value::<ClientMessage>(set_json).expect("back"),
            set
        );

        // An absent `enabled` is the app's `null` and means enabled; so does
        // an explicit `null`, because the field has a serde default.
        for omitted in [
            serde_json::json!({"type": "tool_policy_set", "id": 33, "providerId": "pi"}),
            serde_json::json!({
                "type": "tool_policy_set",
                "id": 33,
                "providerId": "pi",
                "enabled": null
            }),
        ] {
            assert_eq!(
                serde_json::from_value::<ClientMessage>(omitted).expect("absent enabled"),
                ClientMessage::ToolPolicySet {
                    id: 33,
                    provider_id: "pi".to_string(),
                    enabled: None,
                    disabled_tools: Vec::new(),
                }
            );
        }

        let reply = DaemonMessage::ToolPolicy {
            id: 34,
            policies: vec![ToolPolicyEntry {
                provider_id: "claude".to_string(),
                enabled: None,
                disabled_tools: Vec::new(),
            }],
        };
        let reply_json = serde_json::to_value(&reply).expect("json");
        assert_eq!(reply_json["type"], "tool_policy");
        assert_eq!(reply_json["policies"][0]["providerId"], "claude");
        assert_eq!(
            reply_json["policies"][0]["enabled"],
            serde_json::Value::Null
        );
        assert!(
            reply_json["policies"][0].get("disabledTools").is_none(),
            "an empty disabled list is omitted, not sent as []"
        );
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(reply_json).expect("back"),
            reply
        );

        assert_eq!(
            serde_json::to_value(DaemonMessage::ToolPolicySetOk { id: 35 }).expect("json"),
            serde_json::json!({"type": "tool_policy_set_ok", "id": 35})
        );
    }

    #[test]
    fn provider_tools_are_camel_case_and_omitted_when_empty() {
        let mut row: ProviderInfo = serde_json::from_value(serde_json::json!({
            "id": "grok",
            "executable": "grok.exe",
            "acpAvailable": true,
            "authentication": "unknown"
        }))
        .expect("older row without tools");
        assert!(row.tools.is_empty(), "an absent key means no tools");
        assert!(serde_json::to_value(&row)
            .expect("json")
            .get("tools")
            .is_none());

        row.tools.push(ToolDescriptor {
            name: "devboule_list_agents".to_string(),
            description: "Lists live agent sessions.".to_string(),
        });
        let json = serde_json::to_value(&row).expect("json");
        assert_eq!(json["tools"][0]["name"], "devboule_list_agents");
        assert_eq!(
            json["tools"][0]["description"],
            "Lists live agent sessions."
        );
        assert_eq!(
            serde_json::from_value::<ProviderInfo>(json).expect("back"),
            row
        );
    }

    #[test]
    fn providers_list_round_trips_with_camel_case_and_unknown_auth() {
        let request = ClientMessage::ProvidersList { id: 9 };
        let request_json = serde_json::to_value(&request).expect("json");
        assert_eq!(request_json["type"], "providers_list");
        assert_eq!(request_json["id"], 9);

        let reply = DaemonMessage::Providers {
            id: 9,
            providers: vec![ProviderInfo {
                id: "grok".to_string(),
                executable: r"C:\Users\gualt\AppData\Roaming\npm\grok.cmd".to_string(),
                acp_available: true,
                authentication: "unknown".to_string(),
                protocol: Some("acp".to_string()),
                origin: None,
                launch_args: None,
                pickable: None,
                installed_version: None,
                latest_version: None,
                agent_version: None,
                install_channel: None,
                installed: true,
                npm_package: None,
                tools: Vec::new(),
            }],
            unreadable_dirs: 2,
        };
        let encoded = serde_json::to_value(&reply).expect("json");
        assert_eq!(encoded["type"], "providers");
        assert_eq!(encoded["providers"][0]["id"], "grok");
        assert_eq!(encoded["providers"][0]["acpAvailable"], true);
        assert_eq!(encoded["providers"][0]["protocol"], "acp");
        assert_eq!(encoded["providers"][0]["authentication"], "unknown");
        assert_eq!(encoded["unreadableDirs"], 2);
        assert!(encoded["providers"][0].get("authenticated").is_none());
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(encoded).expect("round trip"),
            reply
        );
    }

    #[test]
    fn providers_refresh_round_trips_with_same_providers_shape() {
        let request = ClientMessage::ProvidersRefresh { id: 12 };
        let encoded = serde_json::to_value(&request).expect("json");
        assert_eq!(encoded["type"], "providers_refresh");
        assert_eq!(encoded["id"], 12);

        let reply = DaemonMessage::Providers {
            id: 12,
            providers: vec![
                ProviderInfo {
                    id: "grok".to_string(),
                    executable: "grok.exe".to_string(),
                    acp_available: true,
                    authentication: "unknown".to_string(),
                    protocol: Some("acp".to_string()),
                    origin: Some("user-binary".to_string()),
                    launch_args: None,
                    pickable: None,
                    installed_version: Some("1.2.3".to_string()),
                    latest_version: Some("1.2.4".to_string()),
                    agent_version: Some("adapter-1".to_string()),
                    install_channel: Some("native".to_string()),
                    installed: true,
                    npm_package: None,
                    tools: Vec::new(),
                },
                ProviderInfo {
                    id: "pi".to_string(),
                    executable: "pi.exe".to_string(),
                    acp_available: false,
                    authentication: "unknown".to_string(),
                    protocol: None,
                    origin: Some("user-binary".to_string()),
                    launch_args: None,
                    pickable: None,
                    installed_version: Some("0.1.0".to_string()),
                    latest_version: None,
                    agent_version: None,
                    install_channel: Some("native".to_string()),
                    installed: true,
                    npm_package: None,
                    tools: Vec::new(),
                },
            ],
            unreadable_dirs: 0,
        };
        let encoded = serde_json::to_value(&reply).expect("json");
        assert_eq!(encoded["providers"][0]["installedVersion"], "1.2.3");
        assert_eq!(encoded["providers"][0]["latestVersion"], "1.2.4");
        assert_eq!(encoded["providers"][0]["agentVersion"], "adapter-1");
        assert_eq!(encoded["providers"][0]["installChannel"], "native");
        assert_eq!(encoded["providers"][1]["installedVersion"], "0.1.0");
        assert!(encoded["providers"][1].get("latestVersion").is_none());
        assert!(encoded["providers"][1].get("agentVersion").is_none());
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(encoded).expect("round trip"),
            reply
        );
    }

    #[test]
    fn provider_origin_is_camel_case_on_the_wire() {
        let reply = DaemonMessage::Providers {
            id: 3,
            providers: vec![ProviderInfo {
                id: "codex-acp".to_string(),
                executable: "@agentclientprotocol/codex-acp@1.10.0".to_string(),
                acp_available: true,
                authentication: "unknown".to_string(),
                protocol: Some("acp".to_string()),
                origin: Some("npx-wrapper".to_string()),
                launch_args: None,
                pickable: None,
                installed_version: None,
                latest_version: None,
                agent_version: None,
                install_channel: None,
                installed: true,
                npm_package: None,
                tools: Vec::new(),
            }],
            unreadable_dirs: 0,
        };
        let encoded = serde_json::to_value(&reply).expect("json");
        assert_eq!(encoded["providers"][0]["origin"], "npx-wrapper");
        assert!(encoded["providers"][0].get("npx_wrapper").is_none());
        let native = ProviderInfo {
            id: "grok".to_string(),
            executable: r"C:\npm\grok.exe".to_string(),
            acp_available: true,
            authentication: "unknown".to_string(),
            protocol: Some("acp".to_string()),
            origin: Some("user-binary".to_string()),
            launch_args: None,
            pickable: None,
            installed_version: None,
            latest_version: None,
            agent_version: None,
            install_channel: None,
            installed: true,
            npm_package: None,
            tools: Vec::new(),
        };
        let native_json = serde_json::to_value(&native).expect("json");
        assert_eq!(native_json["origin"], "user-binary");
        assert_eq!(
            serde_json::from_value::<ProviderInfo>(native_json).expect("round trip"),
            native
        );
    }

    #[test]
    fn provider_launch_args_and_pickable_are_optional_camel_case_fields() {
        let wrapper = ProviderInfo {
            id: "codex-acp".to_string(),
            executable: "@agentclientprotocol/codex-acp@1.10.0".to_string(),
            acp_available: true,
            authentication: "unknown".to_string(),
            protocol: Some("acp".to_string()),
            origin: Some("npx-wrapper".to_string()),
            launch_args: Some(vec!["--registry=https://evil".to_string()]),
            pickable: Some(false),
            installed_version: None,
            latest_version: None,
            agent_version: None,
            install_channel: None,
            installed: true,
            npm_package: None,
            tools: Vec::new(),
        };
        let encoded = serde_json::to_value(&wrapper).expect("json");
        assert_eq!(encoded["launchArgs"][0], "--registry=https://evil");
        assert_eq!(encoded["pickable"], false);
        assert_eq!(
            serde_json::from_value::<ProviderInfo>(encoded).expect("round trip"),
            wrapper
        );

        let native = ProviderInfo {
            launch_args: None,
            pickable: None,
            ..wrapper
        };
        let native_json = serde_json::to_value(native).expect("json");
        assert!(native_json.get("launchArgs").is_none());
        assert!(native_json.get("pickable").is_none());
    }

    #[test]
    fn provider_update_request_and_reply_round_trip_with_camel_case_fields() {
        let request = ClientMessage::ProviderUpdate {
            id: 41,
            provider_id: "codex".to_string(),
        };
        let request_json = serde_json::to_value(&request).expect("json");
        assert_eq!(request_json["type"], "provider_update");
        assert_eq!(request_json["providerId"], "codex");

        let reply = DaemonMessage::ProviderUpdated {
            id: 41,
            ok: false,
            exit_code: Some(7),
            log: "npm output\nlast line".to_string(),
        };
        let reply_json = serde_json::to_value(&reply).expect("json");
        assert_eq!(reply_json["type"], "provider_updated");
        assert_eq!(reply_json["exitCode"], 7);
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(reply_json).expect("round trip"),
            reply
        );

        let no_exit_code = serde_json::json!({
            "type": "provider_updated",
            "id": 42,
            "ok": false,
            "log": "npm was not found on PATH"
        });
        assert!(serde_json::to_value(DaemonMessage::ProviderUpdated {
            id: 42,
            ok: false,
            exit_code: None,
            log: "npm was not found on PATH".to_string(),
        })
        .expect("missing exit code json")
        .get("exitCode")
        .is_none());
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(no_exit_code).expect("missing exit code"),
            DaemonMessage::ProviderUpdated {
                id: 42,
                ok: false,
                exit_code: None,
                log: "npm was not found on PATH".to_string(),
            }
        );
    }

    #[test]
    fn provider_info_installed_false_is_emitted_and_missing_means_true() {
        let not_installed = ProviderInfo {
            id: "codex".to_string(),
            executable: String::new(),
            acp_available: false,
            authentication: "unknown".to_string(),
            protocol: None,
            origin: Some("user-binary".to_string()),
            launch_args: None,
            pickable: Some(false),
            installed_version: None,
            latest_version: Some("1.2.3".to_string()),
            agent_version: None,
            install_channel: Some("npm".to_string()),
            installed: false,
            npm_package: Some("@openai/codex".to_string()),
            tools: Vec::new(),
        };
        let encoded = serde_json::to_value(&not_installed).expect("json");
        assert_eq!(encoded["installed"], false);
        assert_eq!(encoded["npmPackage"], "@openai/codex");
        let installed_wire = serde_json::to_value(ProviderInfo {
            installed: true,
            ..not_installed.clone()
        })
        .expect("installed json");
        assert!(installed_wire.get("installed").is_none());

        let installed: ProviderInfo = serde_json::from_value(serde_json::json!({
            "id": "codex",
            "executable": "codex.exe",
            "acpAvailable": false,
            "authentication": "unknown"
        }))
        .expect("older provider row");
        assert!(installed.installed);
        assert_eq!(installed.npm_package, None);
    }

    #[test]
    fn synthetic_provider_info_round_trips_installed_package_and_latest_version() {
        let synthetic = ProviderInfo {
            id: "qwen".to_string(),
            executable: String::new(),
            acp_available: false,
            authentication: "unknown".to_string(),
            protocol: None,
            origin: None,
            launch_args: None,
            pickable: Some(false),
            installed_version: None,
            latest_version: Some("0.23.0".to_string()),
            agent_version: None,
            install_channel: Some("npm".to_string()),
            installed: false,
            npm_package: Some("@qwen-code/qwen-code".to_string()),
            tools: Vec::new(),
        };
        let encoded = serde_json::to_value(&synthetic).expect("synthetic json");
        assert_eq!(encoded["installed"], false);
        assert_eq!(encoded["npmPackage"], "@qwen-code/qwen-code");
        assert_eq!(encoded["latestVersion"], "0.23.0");
        assert_eq!(
            serde_json::from_value::<ProviderInfo>(encoded).expect("synthetic round trip"),
            synthetic
        );
    }
}
