//! Session wire types. `SessionKind`, `Session`, and `SessionEvent` are the
//! M2 contract; moving them here must not change the JSON the TypeScript
//! frontend already consumes.

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, ErrorDetails, WireError};

/// M2 implements Terminal; ACP/Agent can be added as another serialized
/// variant without changing the command signatures or the existing
/// `terminal` wire value.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Terminal,
    Acp,
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
    pub kind: SessionKind,
    pub title: String,
    pub state: SessionState,
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
    /// The process exited while this daemon was alive. `code` is the
    /// observed exit status (`None` if the child did not report one).
    Ended { generation: u64, code: Option<u32> },
    /// The daemon that owned the process is gone (kill, crash, update).
    /// Replay only. `truncated` is set when the journal dropped frames
    /// (slow or full disk) and the transcript is a prefix, not a lie.
    Recovered { generation: u64, truncated: bool },
}

impl SessionState {
    pub fn generation(&self) -> u64 {
        match *self {
            Self::Live { generation }
            | Self::Ended { generation, .. }
            | Self::Recovered { generation, .. } => generation,
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live { .. })
    }
}

/// Events sent over the Tauri Channel supplied to `session_attach`.
///
/// `seq` starts at 1 and is contiguous for output chunks in one
/// *generation* of a session. A cursor means "the last output sequence
/// already received"; replay therefore sends chunks whose sequence is
/// strictly greater than `from_cursor`.
///
/// Permission and ACP variants are intentionally reserved for M6: adding
/// new tagged variants is additive for consumers that ignore unknown event
/// types.
///
/// This enum is the TypeScript `SessionEvent` contract. Generation is **not**
/// a field here: it lives on [`Cursor`] and on [`super::SessionEventEnvelope`]
/// so a reconnecting client can tell a recreated process from the stream it
/// left. Putting generation on every output chunk would change the Channel
/// payload the frontend already parses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    Output {
        seq: u64,
        data: String,
    },
    /// Process was observed to exit while the daemon was alive.
    Exit {
        code: Option<u32>,
    },
    /// Journal replay of a session whose process died with the daemon.
    /// Distinct from [`SessionEvent::Exit`]: Exit means the process was
    /// seen to die; Recovered means it was not, and this is a transcript.
    Recovered {
        truncated: bool,
    },
}

/// Replay position for a reconnecting client.
///
/// `seq` is the last output sequence the client has **for `generation`**.
/// A session whose process died and was recreated MUST bump `generation`
/// so a client holding an old cursor cannot silently consume a different
/// stream as if it were a continuation.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub generation: u64,
    pub seq: u64,
}

/// Outcome of a typed permission prompt. Reserved for M6; the wire name is
/// fixed so permission-response idempotency can be specified now.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOutcome {
    AllowOnce,
    Deny,
}

/// ACP persistence handle. Terminal sessions always use [`PersistenceKind::None`].
///
/// The protocol carries an explicit "resume not supported" result because
/// "ACP is spoken" does not imply "resume is spoken" (ARCHITETTURA §1.6).
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
    Resumed { session: Session },
    NotSupported,
    Failed { message: String },
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn session_uses_camel_case_workspace_id() {
        let session = Session {
            id: "session-1-1".to_string(),
            workspace_id: Some("ws-1".to_string()),
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
            state: SessionState::Live { generation: 1 },
        };
        let value = serde_json::to_value(&session).expect("json");
        assert_eq!(value["workspaceId"], "ws-1");
        assert_eq!(value["kind"], "terminal");
        assert!(value.get("generation").is_none());
        assert_eq!(value["state"]["type"], "live");
        assert_eq!(value["state"]["generation"], 1);
    }

    #[test]
    fn recovered_is_a_different_wire_type_from_live_and_ended() {
        let live = serde_json::to_value(SessionState::Live { generation: 1 }).expect("json");
        let ended = serde_json::to_value(SessionState::Ended {
            generation: 1,
            code: Some(0),
        })
        .expect("json");
        let recovered = serde_json::to_value(SessionState::Recovered {
            generation: 1,
            truncated: false,
        })
        .expect("json");
        assert_eq!(live["type"], "live");
        assert_eq!(ended["type"], "ended");
        assert_eq!(recovered["type"], "recovered");
        assert_ne!(live["type"], recovered["type"]);
        assert_ne!(ended["type"], recovered["type"]);
    }

    #[test]
    fn recovered_event_is_distinct_from_exit() {
        let recovered =
            serde_json::to_value(SessionEvent::Recovered { truncated: true }).expect("json");
        let exit = serde_json::to_value(SessionEvent::Exit { code: None }).expect("json");
        assert_eq!(recovered["type"], "recovered");
        assert_eq!(recovered["truncated"], true);
        assert_eq!(exit["type"], "exit");
        assert_ne!(recovered["type"], exit["type"]);
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
}
