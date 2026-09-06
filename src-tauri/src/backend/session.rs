//! Tauri session commands. These are forwarders: they validate, translate
//! to a protocol request, send it over the daemon pipe, and translate the
//! reply. The app owns no PTY. Output arrives as `SessionEventEnvelope`
//! frames and is fanned into the `Channel<SessionEvent>` the frontend
//! already consumes — that Channel contract is unchanged.

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use devboule_daemon::{DaemonClient, EventHandler, SessionStateHandler};
use devboule_protocol::{
    Cursor, ErrorCode, PermissionOutcome, Persistence, PersistenceKind, ResumeResult,
};

use crate::client::DaemonBridge;

use super::error::CommandError;

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
/// frames that follow the Ok cannot land on a missing subscriber. Live
/// reader output on the daemon waits until that attach is registered
/// under the stream mutex; there is no subscribe/snapshot race.
#[tauri::command]
pub fn session_attach(
    bridge: State<'_, DaemonBridge>,
    id: String,
    from_cursor: Option<u64>,
    ch: Channel<SessionEvent>,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    let client = require_client(&bridge)?;
    let generation = bridge.generation_for(&id);
    let from_cursor = from_cursor.map(|seq| Cursor { generation, seq });
    let tracker = bridge.generation_tracker();
    let session_id = id.clone();
    let handler: EventHandler = Arc::new(move |envelope| {
        tracker.note_generation(&envelope.session_id, envelope.generation);
        let _ = ch.send(envelope.event);
    });
    Ok(client.session_attach(&session_id, from_cursor, handler)?)
}

/// Detach the current view without touching the process, reader, registry,
/// or scrollback. The daemon's idle-exit condition is clients==0 &&
/// sessions==0, so a detached-but-alive session keeps the daemon up.
#[tauri::command]
pub fn session_detach(bridge: State<'_, DaemonBridge>, id: String) -> Result<(), CommandError> {
    require_session_id(&id)?;
    Ok(require_client(&bridge)?.session_detach(&id)?)
}

#[tauri::command]
pub fn session_send(
    bridge: State<'_, DaemonBridge>,
    id: String,
    text: String,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    require_write_size(&text)?;
    Ok(require_client(&bridge)?.session_send(&id, &text)?)
}

#[tauri::command]
pub fn session_permission_respond(
    bridge: State<'_, DaemonBridge>,
    id: String,
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
    Ok(require_client(&bridge)?.session_permission_respond(&id, &request_id, outcome)?)
}

#[tauri::command]
pub fn session_resize(
    bridge: State<'_, DaemonBridge>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), CommandError> {
    require_session_id(&id)?;
    Ok(require_client(&bridge)?.session_resize(&id, cols, rows)?)
}

#[tauri::command]
pub fn session_interrupt(bridge: State<'_, DaemonBridge>, id: String) -> Result<(), CommandError> {
    require_session_id(&id)?;
    Ok(require_client(&bridge)?.session_interrupt(&id)?)
}

#[tauri::command]
pub fn session_close(bridge: State<'_, DaemonBridge>, id: String) -> Result<(), CommandError> {
    require_session_id(&id)?;
    bridge.forget_generation(&id);
    Ok(require_client(&bridge)?.session_close(&id)?)
}

#[tauri::command]
pub fn sessions_list(bridge: State<'_, DaemonBridge>) -> Result<Vec<Session>, CommandError> {
    Ok(require_client(&bridge)?.sessions_list()?)
}

#[tauri::command]
pub fn sessions_watch(
    bridge: State<'_, DaemonBridge>,
    ch: Channel<Vec<SessionStateSnapshot>>,
) -> Result<(), CommandError> {
    let handler: SessionStateHandler = Arc::new(move |snapshots| {
        let _ = ch.send(snapshots);
    });
    Ok(require_client(&bridge)?.sessions_watch(handler)?)
}

#[tauri::command]
pub fn sessions_unwatch(bridge: State<'_, DaemonBridge>) -> Result<(), CommandError> {
    Ok(require_client(&bridge)?.sessions_unwatch()?)
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
    fn lost_daemon_connection_is_io() {
        let error = disconnected("The daemon connection was lost.".to_string());
        assert_eq!(error.code, ErrorCode::Io);
        assert_eq!(error.message, "The daemon connection was lost.");
    }
}
