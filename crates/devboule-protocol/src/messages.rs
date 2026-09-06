//! Client and daemon frames. Every message has a `type` tag so a human with a
//! pipe client can read a line and know what it is.

use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::error::WireError;
use crate::handshake::{ClientHello, DaemonHello};
use crate::session::{
    AgentActivityState, Cursor, PermissionOutcome, Persistence, ResumeResult, Session,
    SessionEvent, SessionKind,
};

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
        idempotency_key: Option<String>,
    },
    SessionAttach {
        id: u64,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_cursor: Option<Cursor>,
    },
    SessionDetach {
        id: u64,
        session_id: String,
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
    },
    SessionSend {
        id: u64,
        session_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    SessionResize {
        id: u64,
        session_id: String,
        cols: u16,
        rows: u16,
    },
    SessionInterrupt {
        id: u64,
        session_id: String,
    },
    SessionSetModel {
        id: u64,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    SessionPermissionRespond {
        id: u64,
        session_id: String,
        request_id: String,
        outcome: PermissionOutcome,
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
}

impl ClientMessage {
    pub fn request_id(&self) -> Option<u64> {
        match self {
            Self::Hello(_) => None,
            Self::Ping { id }
            | Self::Status { id }
            | Self::Shutdown { id }
            | Self::SessionCreate { id, .. }
            | Self::SessionAttach { id, .. }
            | Self::SessionDetach { id, .. }
            | Self::SessionClose { id, .. }
            | Self::SessionStop { id, .. }
            | Self::SessionSend { id, .. }
            | Self::SessionResize { id, .. }
            | Self::SessionInterrupt { id, .. }
            | Self::SessionSetModel { id, .. }
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
            | Self::ProvidersList { id }
            | Self::ProvidersRefresh { id }
            | Self::ProviderUpdate { id, .. }
            | Self::Invoke { id, .. } => Some(*id),
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
            | Self::Shutdown { .. }
            | Self::SessionAttach { .. }
            | Self::SessionDetach { .. }
            | Self::SessionStop { .. }
            | Self::SessionResize { .. }
            | Self::SessionInterrupt { .. }
            | Self::SessionSetModel { .. }
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
            | Self::Invoke { .. } => None,
        }
    }
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
    Ok {
        id: u64,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_stats: Option<JournalStats>,
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
    fn detach_close_stop_are_three_type_tags() {
        let detach = serde_json::to_value(ClientMessage::SessionDetach {
            id: 1,
            session_id: "s.a.1".to_string(),
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
    fn session_state_broadcast_is_a_compact_event_snapshot() {
        let message = DaemonMessage::Event(SessionEventEnvelope {
            session_id: String::new(),
            generation: 0,
            event: SessionEvent::SessionsSnapshot {
                sessions: vec![SessionStateSnapshot {
                    id: "s.client.1".to_string(),
                    title: "Terminal".to_string(),
                    state: SessionState::Silent { generation: 3 },
                    elapsed_ms: Some(300_001),
                    attention: None,
                }],
            },
        });
        let value = serde_json::to_value(message).expect("json");
        assert_eq!(value["event"]["type"], "sessions_snapshot");
        assert_eq!(value["event"]["sessions"][0]["id"], "s.client.1");
        assert_eq!(value["event"]["sessions"][0]["title"], "Terminal");
        assert_eq!(value["event"]["sessions"][0]["state"]["type"], "silent");
        assert_eq!(value["event"]["sessions"][0]["elapsedMs"], 300_001);
        assert!(value["event"]["sessions"][0].get("workspaceId").is_none());
        assert!(value["event"]["sessions"][0].get("kind").is_none());
    }

    #[test]
    fn attach_cursor_carries_generation_and_seq() {
        let value = serde_json::to_value(ClientMessage::SessionAttach {
            id: 3,
            session_id: "s.a.1".to_string(),
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
    fn create_send_permission_carry_idempotency_key() {
        let create = ClientMessage::SessionCreate {
            id: 1,
            workspace_id: None,
            kind: SessionKind::Terminal,
            provider: None,
            idempotency_key: Some("k1".to_string()),
        };
        let send = ClientMessage::SessionSend {
            id: 2,
            session_id: "s.a.1".to_string(),
            text: "x".to_string(),
            idempotency_key: Some("k2".to_string()),
        };
        let perm = ClientMessage::SessionPermissionRespond {
            id: 3,
            session_id: "s.a.1".to_string(),
            request_id: "r1".to_string(),
            outcome: PermissionOutcome::AllowOnce,
            idempotency_key: Some("k3".to_string()),
        };
        assert_eq!(create.idempotency_key(), Some("k1"));
        assert_eq!(send.idempotency_key(), Some("k2"));
        assert_eq!(perm.idempotency_key(), Some("k3"));
        assert_eq!(
            serde_json::to_value(&perm).expect("json")["outcome"],
            "allow_once"
        );
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
            "protocolVersion": 1,
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
