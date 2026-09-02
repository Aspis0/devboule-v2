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
    SessionsWatch {
        id: u64,
    },
    SessionsUnwatch {
        id: u64,
    },
    SessionResume {
        id: u64,
        persistence: Persistence,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
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
            | Self::SessionPermissionRespond { id, .. }
            | Self::SessionsList { id }
            | Self::SessionsWatch { id }
            | Self::SessionsUnwatch { id }
            | Self::SessionResume { id, .. }
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
            } => idempotency_key.as_deref(),
            _ => None,
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
    Event(SessionEventEnvelope),
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
}
