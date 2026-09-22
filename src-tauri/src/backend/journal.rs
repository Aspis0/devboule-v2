//! Tauri commands for measuring and configuring journal retention.

use tauri::State;

use devboule_protocol::{validate_session_id, JournalRetention, JournalUsage, RetentionPatch};

use crate::client::DaemonBridge;

use super::blocking::off_main_thread;
use super::error::CommandError;

#[tauri::command]
pub async fn journal_usage(bridge: State<'_, DaemonBridge>) -> Result<JournalUsage, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.journal_usage()).await
}

#[tauri::command]
pub async fn journal_retention_get(
    bridge: State<'_, DaemonBridge>,
) -> Result<JournalRetention, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.journal_retention_get()).await
}

#[tauri::command]
pub async fn journal_retention_set(
    bridge: State<'_, DaemonBridge>,
    max_age_ms: Option<i64>,
    max_bytes: Option<i64>,
    max_sessions: Option<i64>,
    session_max_bytes: Option<i64>,
) -> Result<JournalRetention, CommandError> {
    let patch = RetentionPatch {
        max_age_ms,
        max_bytes,
        max_sessions,
        session_max_bytes,
    };
    let client = require_client(&bridge)?;
    off_main_thread(move || client.journal_retention_set(patch)).await
}

#[tauri::command]
pub async fn session_delete(
    bridge: State<'_, DaemonBridge>,
    id: String,
) -> Result<(), CommandError> {
    validate_session_id(&id).map_err(|message| {
        CommandError::new(devboule_protocol::ErrorCode::InvalidRequest, message)
    })?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.session_delete(&id)).await
}

fn require_client(
    bridge: &DaemonBridge,
) -> Result<std::sync::Arc<devboule_daemon::DaemonClient>, CommandError> {
    bridge
        .client()
        .map_err(|message| CommandError::new(devboule_protocol::ErrorCode::Io, message))
}
