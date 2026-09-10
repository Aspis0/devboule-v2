//! Tauri session commands. These are forwarders: they validate, translate
//! to a protocol request, send it over the daemon pipe, and translate the
//! reply. The app owns no PTY. Output arrives as `SessionEventEnvelope`
//! frames are delivered to the `Channel<SessionEvent>` the frontend already
//! consumes — that Channel contract is unchanged.

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use devboule_daemon::{DaemonClient, DiagnosticsReport, SessionStateHandler};
use devboule_protocol::{
    ErrorCode, PermissionOutcome, Persistence, PersistenceKind, ResumeResult, SubscriptionId,
};

use crate::client::DaemonBridge;

use super::error::CommandError;

#[cfg(test)]
use devboule_daemon::SafeText;

const MAX_WRITE_BYTES: usize = 64 * 1024;

pub use devboule_protocol::{
    validate_session_id, Session, SessionEvent, SessionKind, SessionStateSnapshot,
};

#[tauri::command]
pub fn session_create(
    bridge: State<'_, DaemonBridge>,
    workspace_id: Option<String>,
    kind: SessionKind,
    provider: Option<String>,
) -> Result<Session, CommandError> {
    require_terminal_kind(&kind)?;
    Ok(require_client(&bridge)?.session_create_with(workspace_id, kind, provider, None)?)
}

#[tauri::command]
pub fn session_resume(
    bridge: State<'_, DaemonBridge>,
    session_id: String,
) -> Result<ResumeResult, CommandError> {
    require_session_id(&session_id)?;
    Ok(require_client(&bridge)?.session_resume(
        Persistence {
            kind: PersistenceKind::Acp { handle: session_id },
        },
        None,
    )?)
}

/// IMPORTANT STARTUP ORDER: the client registers the Channel as the
/// session's event handler *before* it sends `session_attach`, so replay
/// frames that follow the attach reply cannot land on a missing subscriber. Live
/// reader output on the daemon waits until that attach is registered
/// under the stream mutex; there is no subscribe/snapshot race.
#[tauri::command]
pub fn session_attach(
    bridge: State<'_, DaemonBridge>,
    id: String,
    from_cursor: Option<u64>,
    ch: Channel<SessionEvent>,
) -> Result<SubscriptionId, CommandError> {
    require_session_id(&id)?;
    let sink = Arc::new(move |event| {
        let _ = ch.send(event);
    });
    Ok(bridge.session_attach(&id, from_cursor, sink)?)
}

/// Detach the current view without touching the process, reader, registry,
/// or scrollback. The daemon's idle-exit condition is clients==0 &&
/// sessions==0, so a detached-but-alive session keeps the daemon up.
#[tauri::command]
pub fn session_detach(
    bridge: State<'_, DaemonBridge>,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    Ok(bridge.session_detach(subscription_id)?)
}

#[tauri::command]
pub fn session_claim(
    bridge: State<'_, DaemonBridge>,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    Ok(bridge.session_claim(subscription_id)?)
}

#[tauri::command]
pub fn session_presence(
    bridge: State<'_, DaemonBridge>,
    focused_session_id: Option<String>,
    app_visible: bool,
) -> Result<(), CommandError> {
    // Presence is best-effort UI state: preserve errors for observability, while a lost hint only leaves a transiently stale badge.
    Ok(require_client(&bridge)?.session_presence(focused_session_id.as_deref(), app_visible)?)
}

#[tauri::command]
pub fn session_send(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
    text: String,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    require_write_size(&text)?;
    bridge.ensure_subscription_attached(subscription_id)?;
    Ok(require_client(&bridge)?.session_send_with_subscription(&id, subscription_id, &text)?)
}

#[tauri::command]
pub fn session_permission_respond(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
    request_id: String,
    outcome: PermissionOutcome,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    if request_id.is_empty() {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "Permission request id is required.",
        ));
    }
    bridge.ensure_subscription_attached(subscription_id)?;
    Ok(
        require_client(&bridge)?.session_permission_respond_with_subscription(
            &id,
            subscription_id,
            &request_id,
            outcome,
        )?,
    )
}

#[tauri::command]
pub fn session_resize(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
    cols: u16,
    rows: u16,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    bridge.ensure_subscription_attached(subscription_id)?;
    Ok(require_client(&bridge)?.session_resize_with_subscription(
        &id,
        subscription_id,
        cols,
        rows,
    )?)
}

#[tauri::command]
pub fn session_interrupt(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    bridge.ensure_subscription_attached(subscription_id)?;
    Ok(require_client(&bridge)?.session_interrupt_with_subscription(&id, subscription_id)?)
}

#[tauri::command]
pub fn session_set_model(
    bridge: State<'_, DaemonBridge>,
    id: String,
    model_id: Option<String>,
    effort: Option<String>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    Ok(require_client(&bridge)?.session_set_model(&id, model_id.as_deref(), effort.as_deref())?)
}

/// Destroys the session. The subscription is optional: the wire `SessionClose`
/// frame carries only the session id, and the daemon authenticates the caller
/// as the session owner, so a session that never produced a subscription (a
/// startup that failed after `session_create`) can still be closed. Without
/// this, such a session stays alive in the daemon with nothing able to close
/// it, because every other teardown path is keyed on a subscription.
#[tauri::command]
pub fn session_close(
    bridge: State<'_, DaemonBridge>,
    id: String,
    subscription_id: Option<SubscriptionId>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    bridge.session_close(&id, subscription_id)?;
    bridge.forget_generation(&id);
    Ok(())
}

#[tauri::command]
pub fn sessions_list(bridge: State<'_, DaemonBridge>) -> Result<Vec<Session>, CommandError> {
    Ok(require_client(&bridge)?.sessions_list()?)
}

#[tauri::command]
pub fn daemon_diagnostics(
    bridge: State<'_, DaemonBridge>,
) -> Result<DiagnosticsReport, CommandError> {
    Ok(require_client(&bridge)?.daemon_diagnostics()?)
}

#[tauri::command]
pub fn sessions_watch(
    bridge: State<'_, DaemonBridge>,
    ch: Channel<Vec<SessionStateSnapshot>>,
) -> Result<(), CommandError> {
    let handler: SessionStateHandler = Arc::new(move |snapshots| {
        let _ = ch.send(snapshots);
    });
    Ok(bridge.sessions_watch(handler)?)
}

#[tauri::command]
pub fn sessions_unwatch(bridge: State<'_, DaemonBridge>) -> Result<(), CommandError> {
    Ok(bridge.sessions_unwatch()?)
}

fn require_client(bridge: &DaemonBridge) -> Result<Arc<DaemonClient>, CommandError> {
    bridge.client().map_err(disconnected)
}

fn disconnected(message: String) -> CommandError {
    CommandError::new(ErrorCode::Io, message)
}

fn require_session_id(id: &str) -> Result<(), CommandError> {
    validate_session_id(id).map_err(|message| CommandError::new(ErrorCode::InvalidRequest, message))
}

fn require_write_size(text: &str) -> Result<(), CommandError> {
    if text.len() > MAX_WRITE_BYTES {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "Session input is too large.",
        ));
    }
    Ok(())
}

fn require_terminal_kind(kind: &SessionKind) -> Result<(), CommandError> {
    match kind {
        SessionKind::Terminal | SessionKind::Acp | SessionKind::Claude => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_session_id_is_invalid_request() {
        let error = require_session_id("../other").expect_err("rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "Invalid session id.");
    }

    #[test]
    fn oversized_write_is_invalid_request() {
        require_write_size(&"x".repeat(MAX_WRITE_BYTES)).expect("at cap");
        let error = require_write_size(&"x".repeat(MAX_WRITE_BYTES + 1)).expect_err("rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "Session input is too large.");
    }

    #[test]
    fn supported_session_kinds_are_accepted() {
        require_terminal_kind(&SessionKind::Terminal).expect("terminal");
        require_terminal_kind(&SessionKind::Acp).expect("acp");
        require_terminal_kind(&SessionKind::Claude).expect("claude");
    }

    #[test]
    fn session_presence_forwarder_has_the_frozen_tauri_signature() {
        let _: fn(State<'_, DaemonBridge>, Option<String>, bool) -> Result<(), CommandError> =
            session_presence;
    }

    #[test]
    fn lost_daemon_connection_is_io() {
        let error = disconnected("The daemon connection was lost.".to_string());
        assert_eq!(error.code, ErrorCode::Io);
        assert_eq!(error.message, "The daemon connection was lost.");
    }

    #[test]
    fn safe_text_agrees_with_oracle_and_extends_it() {
        struct OracleCase {
            name: &'static str,
            input: &'static str,
            removed_literal: &'static str,
        }

        let oracle_cases = [
            OracleCase {
                name: "github token",
                input: "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
                removed_literal: "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            },
            OracleCase {
                name: "slack token",
                input: "xoxb-1234567890-1234567890-1234567890",
                removed_literal: "xoxb-1234567890-1234567890-1234567890",
            },
            OracleCase {
                name: "aws access key",
                input: "AKIA1234567890ABCDEF",
                removed_literal: "AKIA1234567890ABCDEF",
            },
            OracleCase {
                name: "bearer token",
                input: "Bearer abcdefghijklmnopqrstuvwxyz0123456789",
                removed_literal: "abcdefghijklmnopqrstuvwxyz0123456789",
            },
            OracleCase {
                name: "jwt",
                input: "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
                removed_literal: "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
            },
            OracleCase {
                name: "api key assignment",
                input: "api_key=super_secret_value_123",
                removed_literal: "super_secret_value_123",
            },
            OracleCase {
                name: "password assignment",
                input: "password = \"hunter2\"",
                removed_literal: "hunter2",
            },
            OracleCase {
                name: "high entropy base64",
                input: "Aa0Bb1Cc2Dd3Ee4Ff5Gg6Hh7Ii8Jj9Kk0Ll1Mm2Nn3Oo4Pp5",
                removed_literal: "Aa0Bb1Cc2Dd3Ee4Ff5Gg6Hh7Ii8Jj9Kk0Ll1Mm2Nn3Oo4Pp5",
            },
            OracleCase {
                name: "long hex",
                input: "0123456789abcdef0123456789abcdef01234567",
                removed_literal: "0123456789abcdef0123456789abcdef01234567",
            },
        ];

        for case in oracle_cases {
            let oracle = oracle_core::redact_secret_tokens(case.input);
            assert!(
                !oracle.contains(case.removed_literal),
                "corpus case no longer exercises oracle-core: {} -> {oracle:?}",
                case.name
            );
            let safe = SafeText::new(case.input);
            assert!(
                !safe.as_str().contains(case.removed_literal),
                "diagnostics redactor drift on {}: {:?}",
                case.name,
                safe.as_str()
            );
        }

        for (name, input, removed_literal) in [
            (
                "Windows home path",
                r"C:\Users\alice\secret-project",
                r"C:\Users\alice",
            ),
            (
                "Windows SID",
                "S-1-5-21-111-222-333-1001",
                "S-1-5-21-111-222-333-1001",
            ),
        ] {
            let oracle = oracle_core::redact_secret_tokens(input);
            assert!(
                oracle.contains(removed_literal),
                "diagnostics-only case unexpectedly belongs to oracle-core: {name}"
            );
            let safe = SafeText::new(input);
            assert!(
                !safe.as_str().contains(removed_literal),
                "diagnostics-only identifier survived: {name} -> {:?}",
                safe.as_str()
            );
        }
    }
}
