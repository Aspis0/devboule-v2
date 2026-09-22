//! Agent profile commands: the Settings → Agents panel's read and write of the
//! daemon's profile store.
//!
//! The daemon owns all of it — the `runtime_dir/agent-profiles.json` file, the
//! validation caps, the quarantine of a file it cannot admit. Every command
//! here forwards one request and re-shapes the reply into what the panel
//! reads. Nothing on this side writes profile state, truncates anything, or
//! checks a cap: a document over a cap is refused by the daemon with the size
//! named, and this layer passes that sentence through verbatim.
//!
//! The daemon answers a get as `DaemonMessage::AgentProfiles { id, document }`
//! and a set as `DaemonMessage::AgentProfilesSetOk { id }`. An empty document
//! is not an error: it is a first run, or a quarantined file, and it means
//! agents create nothing — the panel renders that as the off state it is.

use std::sync::Arc;

use devboule_daemon::DaemonClient;
use devboule_protocol::{AgentProfilesDocument, DaemonMessage, ErrorCode};
use tauri::State;

use super::blocking::off_main_thread;
use super::error::CommandError;
use crate::client::DaemonBridge;

fn require_client(bridge: &DaemonBridge) -> Result<Arc<DaemonClient>, CommandError> {
    bridge
        .client()
        .map_err(|message| CommandError::new(ErrorCode::Io, message))
}

/// A reply of the wrong variant. It names no frame on purpose.
fn unexpected_reply() -> CommandError {
    CommandError::new(ErrorCode::Internal, "unexpected daemon reply")
}

/// The `agent_profiles_get` reply: the daemon's `AgentProfiles` frame without
/// its request id. `serde` spells the fields the way the panel reads them.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfilesReply {
    pub document: AgentProfilesDocument,
}

#[tauri::command]
pub async fn agent_profiles_get(
    bridge: State<'_, DaemonBridge>,
) -> Result<AgentProfilesReply, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || match client.agent_profiles_get()? {
        DaemonMessage::AgentProfiles { document, .. } => Ok(AgentProfilesReply { document }),
        _ => Err(unexpected_reply()),
    })
    .await
}

#[tauri::command]
pub async fn agent_profiles_set(
    bridge: State<'_, DaemonBridge>,
    document: AgentProfilesDocument,
) -> Result<(), CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || match client.agent_profiles_set(document)? {
        DaemonMessage::AgentProfilesSetOk { .. } => Ok(()),
        _ => Err(unexpected_reply()),
    })
    .await
}
