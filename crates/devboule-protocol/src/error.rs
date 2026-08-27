//! Protocol error codes. Stable names; new codes are additive.

use serde::{Deserialize, Serialize};

/// Machine-readable failure. Serialized as a snake_case string.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Handshake version ranges do not overlap. Actionable: update app or daemon.
    ProtocolVersionMismatch,
    /// The peer is not the pipe owner. Should not happen if the DACL is set.
    Unauthorized,
    /// The RPC exists in the protocol but this daemon build does not serve it.
    Unimplemented,
    /// The RPC exists but was not in the negotiated capability set.
    CapabilityNotSupported,
    /// Malformed frame, bad id, bad idempotency key, etc.
    InvalidRequest,
    SessionNotFound,
    /// Cursor.generation is not the live instance. Replay would lie.
    SessionGenerationMismatch,
    /// Same idempotency key, different payload.
    IdempotencyConflict,
    /// Daemon is exiting; the client should not retry against this instance.
    ShuttingDown,
    Internal,
    Io,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ErrorDetails {
    VersionMismatch {
        client: u32,
        client_min: u32,
        daemon: u32,
        daemon_min: u32,
    },
    GenerationMismatch {
        current: u64,
        requested: u64,
    },
}

/// Error payload used both as a handshake-level first frame (`id` is `None`)
/// and as a request response (`id` is the client's request id).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WireError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ErrorDetails>,
}

impl WireError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            id: None,
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_id(mut self, id: u64) -> Self {
        self.id = Some(id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_is_snake_case() {
        let value = serde_json::to_value(ErrorCode::ProtocolVersionMismatch).expect("json");
        assert_eq!(value, "protocol_version_mismatch");
        let value = serde_json::to_value(ErrorCode::SessionGenerationMismatch).expect("json");
        assert_eq!(value, "session_generation_mismatch");
        let value = serde_json::to_value(ErrorCode::IdempotencyConflict).expect("json");
        assert_eq!(value, "idempotency_conflict");
    }
}
