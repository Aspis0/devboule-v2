//! Structured error returned by Tauri commands. The daemon already produces
//! a typed [`WireError`]; this type is that payload at the app boundary,
//! without the daemon RPC `id`.

use serde::{Deserialize, Serialize};

use devboule_daemon::DaemonError;
use devboule_protocol::{ErrorCode, ErrorDetails, WireError};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ErrorDetails>,
}

impl CommandError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }
}

impl From<WireError> for CommandError {
    fn from(wire: WireError) -> Self {
        // Keep the user-facing SessionNotFound copy the app already showed
        // when this boundary returned a String.
        if wire.code == ErrorCode::SessionNotFound {
            return Self {
                code: ErrorCode::SessionNotFound,
                message: "No session with that id.".to_string(),
                details: wire.details,
            };
        }
        Self {
            code: wire.code,
            message: wire.message,
            details: wire.details,
        }
    }
}

impl From<DaemonError> for CommandError {
    fn from(error: DaemonError) -> Self {
        match error {
            DaemonError::Handshake(wire) => Self::from(wire),
            DaemonError::Io(error) => Self::new(ErrorCode::Io, error.to_string()),
            // Timeouts here are pipe/connect/reply waits, not a protocol code.
            DaemonError::TimedOut(what) => Self::new(ErrorCode::Io, format!("timed out: {what}")),
            DaemonError::ConnectionLost => Self::new(ErrorCode::Io, "daemon connection was lost"),
            // Framing, unexpected frames, serde: no more specific code exists.
            DaemonError::Protocol(message) => Self::new(ErrorCode::Internal, message),
            DaemonError::AlreadyRunning => Self::new(
                ErrorCode::Internal,
                "another Devboule daemon is already running",
            ),
            DaemonError::UnsupportedPlatform => Self::new(
                ErrorCode::Internal,
                "devboule-daemon M3a targets Windows only",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn session_not_found_keeps_its_code_on_the_tauri_boundary() {
        let wire = WireError::new(ErrorCode::SessionNotFound, "daemon-specific wording");
        let error = CommandError::from(DaemonError::Handshake(wire));
        assert_eq!(error.code, ErrorCode::SessionNotFound);
        assert_eq!(error.message, "No session with that id.");
        assert_eq!(error.details, None);

        let json = serde_json::to_value(&error).expect("json");
        assert_eq!(json["code"], "session_not_found");
        assert_eq!(json["message"], "No session with that id.");
        assert!(json.get("details").is_none());
        assert!(json.get("id").is_none());
    }

    #[test]
    fn generation_mismatch_keeps_code_and_details() {
        let wire = WireError {
            id: Some(9),
            code: ErrorCode::SessionGenerationMismatch,
            message: "session generation is 3, client cursor is 1".to_string(),
            details: Some(ErrorDetails::GenerationMismatch {
                current: 3,
                requested: 1,
            }),
        };
        let error = CommandError::from(DaemonError::Handshake(wire));
        assert_eq!(error.code, ErrorCode::SessionGenerationMismatch);
        assert_eq!(error.message, "session generation is 3, client cursor is 1");
        assert_eq!(
            error.details,
            Some(ErrorDetails::GenerationMismatch {
                current: 3,
                requested: 1,
            })
        );

        let json = serde_json::to_value(&error).expect("json");
        assert_eq!(json["code"], "session_generation_mismatch");
        assert_eq!(json["details"]["type"], "generation_mismatch");
        assert_eq!(json["details"]["current"], 3);
        assert_eq!(json["details"]["requested"], 1);
        assert!(json.get("id").is_none());
    }

    #[test]
    fn unauthorized_handshake_is_not_crushed_to_a_string() {
        let wire = WireError::new(ErrorCode::Unauthorized, "not the pipe owner");
        let error = CommandError::from(DaemonError::Handshake(wire));
        assert_eq!(error.code, ErrorCode::Unauthorized);
        assert_eq!(error.message, "not the pipe owner");
        let json = serde_json::to_value(&error).expect("json");
        assert_eq!(json["code"], "unauthorized");
        assert!(json.get("code").unwrap().is_string());
        assert!(json.get("message").unwrap().is_string());
    }

    #[test]
    fn shutting_down_keeps_its_code() {
        let wire = WireError::new(ErrorCode::ShuttingDown, "daemon is exiting");
        let error = CommandError::from(DaemonError::Handshake(wire));
        assert_eq!(error.code, ErrorCode::ShuttingDown);
        assert_eq!(error.message, "daemon is exiting");
    }

    #[test]
    fn io_and_timeout_use_the_io_code_and_keep_display_text() {
        let io = CommandError::from(DaemonError::Io(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "broken pipe",
        )));
        assert_eq!(io.code, ErrorCode::Io);
        assert_eq!(io.message, "broken pipe");

        let timeout = CommandError::from(DaemonError::timed_out("waiting for a daemon reply"));
        assert_eq!(timeout.code, ErrorCode::Io);
        assert_eq!(timeout.message, "timed out: waiting for a daemon reply");
    }

    #[test]
    fn protocol_and_lifecycle_errors_use_internal() {
        let protocol = CommandError::from(DaemonError::Protocol(
            "unexpected daemon frame: Ok".to_string(),
        ));
        assert_eq!(protocol.code, ErrorCode::Internal);
        assert_eq!(protocol.message, "unexpected daemon frame: Ok");

        let running = CommandError::from(DaemonError::AlreadyRunning);
        assert_eq!(running.code, ErrorCode::Internal);
        assert_eq!(
            running.message,
            "another Devboule daemon is already running"
        );

        let platform = CommandError::from(DaemonError::UnsupportedPlatform);
        assert_eq!(platform.code, ErrorCode::Internal);
        assert_eq!(platform.message, "devboule-daemon M3a targets Windows only");
    }

    #[test]
    fn command_error_serializes_camel_case_with_snake_case_code() {
        let error = CommandError::new(ErrorCode::InvalidRequest, "Invalid session id.");
        let json = serde_json::to_value(&error).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "code": "invalid_request",
                "message": "Invalid session id."
            })
        );
    }
}
