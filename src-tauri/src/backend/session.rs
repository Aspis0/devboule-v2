//! Tauri session commands. These are forwarders: they validate, translate
//! to a protocol request, send it over the daemon pipe, and translate the
//! reply. The app owns no PTY. Output arrives as `SessionEventEnvelope`
//! frames and is fanned into the `Channel<SessionEvent>` the frontend
//! already consumes — that Channel contract is unchanged.

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use devboule_daemon::{DaemonError, EventHandler};
use devboule_protocol::{Cursor, ErrorCode};

use crate::client::DaemonBridge;

const MAX_WRITE_BYTES: usize = 64 * 1024;

pub use devboule_protocol::{validate_session_id, Session, SessionEvent, SessionKind};

#[tauri::command]
pub fn session_create(
    bridge: State<'_, DaemonBridge>,
    workspace_id: Option<String>,
    kind: SessionKind,
) -> Result<Session, String> {
    if kind != SessionKind::Terminal {
        return Err("Only terminal sessions are available.".to_string());
    }
    let client = bridge.client()?;
    client
        .session_create(workspace_id, kind, None)
        .map_err(map_error)
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
) -> Result<(), String> {
    validate_session_id(&id)?;
    let client = bridge.client()?;
    let generation = bridge.generation_for(&id);
    let from_cursor = from_cursor.map(|seq| Cursor { generation, seq });
    let tracker = bridge.generation_tracker();
    let session_id = id.clone();
    let handler: EventHandler = Arc::new(move |envelope| {
        tracker.note_generation(&envelope.session_id, envelope.generation);
        let _ = ch.send(envelope.event);
    });
    client
        .session_attach(&session_id, from_cursor, handler)
        .map_err(map_error)
}

/// Detach the current view without touching the process, reader, registry,
/// or scrollback. The daemon's idle-exit condition is clients==0 &&
/// sessions==0, so a detached-but-alive session keeps the daemon up.
#[tauri::command]
pub fn session_detach(bridge: State<'_, DaemonBridge>, id: String) -> Result<(), String> {
    validate_session_id(&id)?;
    bridge.client()?.session_detach(&id).map_err(map_error)
}

#[tauri::command]
pub fn session_send(
    bridge: State<'_, DaemonBridge>,
    id: String,
    text: String,
) -> Result<(), String> {
    validate_session_id(&id)?;
    if text.len() > MAX_WRITE_BYTES {
        return Err("Session input is too large.".to_string());
    }
    bridge.client()?.session_send(&id, &text).map_err(map_error)
}

#[tauri::command]
pub fn session_resize(
    bridge: State<'_, DaemonBridge>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    validate_session_id(&id)?;
    bridge
        .client()?
        .session_resize(&id, cols, rows)
        .map_err(map_error)
}

#[tauri::command]
pub fn session_close(bridge: State<'_, DaemonBridge>, id: String) -> Result<(), String> {
    validate_session_id(&id)?;
    bridge.forget_generation(&id);
    bridge.client()?.session_close(&id).map_err(map_error)
}

#[tauri::command]
pub fn sessions_list(bridge: State<'_, DaemonBridge>) -> Result<Vec<Session>, String> {
    bridge.client()?.sessions_list().map_err(map_error)
}

fn map_error(error: DaemonError) -> String {
    match error {
        DaemonError::Handshake(wire) if wire.code == ErrorCode::SessionNotFound => {
            "No session with that id.".to_string()
        }
        DaemonError::Handshake(wire) => wire.message,
        other => other.to_string(),
    }
}
