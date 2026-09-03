//! Tauri commands for measuring and configuring journal retention.

use tauri::State;

use devboule_protocol::{validate_session_id, JournalRetention, JournalUsage, RetentionPatch};

use crate::client::DaemonBridge;

use super::error::CommandError;

#[tauri::command]
pub fn journal_usage(bridge: State<'_, DaemonBridge>) -> Result<JournalUsage, CommandError> {
    Ok(require_client(&bridge)?.journal_usage()?)
}

#[tauri::command]
pub fn journal_retention_get(
    bridge: State<'_, DaemonBridge>,
) -> Result<JournalRetention, CommandError> {
    Ok(require_client(&bridge)?.journal_retention_get()?)
}

#[tauri::command]
pub fn journal_retention_set(
    bridge: State<'_, DaemonBridge>,
    max_age_ms: Option<i64>,
    max_bytes: Option<i64>,
    max_sessions: Option<i64>,
    session_max_bytes: Option<i64>,
) -> Result<JournalRetention, CommandError> {
    Ok(
        require_client(&bridge)?.journal_retention_set(RetentionPatch {
            max_age_ms,
            max_bytes,
            max_sessions,
            session_max_bytes,
        })?,
    )
}

#[tauri::command]
pub fn session_delete(bridge: State<'_, DaemonBridge>, id: String) -> Result<(), CommandError> {
    validate_session_id(&id).map_err(|message| {
        CommandError::new(devboule_protocol::ErrorCode::InvalidRequest, message)
    })?;
    Ok(require_client(&bridge)?.session_delete(&id)?)
}

fn require_client(
    bridge: &DaemonBridge,
) -> Result<std::sync::Arc<devboule_daemon::DaemonClient>, CommandError> {
    bridge
        .client()
        .map_err(|message| CommandError::new(devboule_protocol::ErrorCode::Io, message))
}
