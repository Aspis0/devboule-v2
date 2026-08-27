//! Client and daemon frames. Every message has a `type` tag so a human with a
//! pipe client can read a line and know what it is.

use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::error::WireError;
use crate::handshake::{ClientHello, DaemonHello};
use crate::session::{
    Cursor, PermissionOutcome, Persistence, ResumeResult, Session, SessionEvent, SessionKind,
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
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
    SessionPermissionRespond {
        id: u64,
        session_id: String,
        request_id: String,
        outcome: PermissionOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    SessionsList {
        id: u64,
    },
    SessionResume {
        id: u64,
        persistence: Persistence,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
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
            | Self::SessionPermissionRespond { id, .. }
            | Self::SessionsList { id }
            | Self::SessionResume { id, .. } => Some(*id),
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
            } => idempotency_key.as_deref(),
            _ => None,
        }
    }
}

/// Messages the daemon writes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
    Event(SessionEventEnvelope),
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
    fn create_send_permission_carry_idempotency_key() {
        let create = ClientMessage::SessionCreate {
            id: 1,
            workspace_id: None,
            kind: SessionKind::Terminal,
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
    fn ping_roundtrip() {
        let msg = ClientMessage::Ping { id: 7 };
        let encoded = serde_json::to_string(&msg).expect("json");
        assert!(!encoded.contains('\n'));
        let decoded: ClientMessage = serde_json::from_str(&encoded).expect("parse");
        assert_eq!(msg, decoded);
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
}
