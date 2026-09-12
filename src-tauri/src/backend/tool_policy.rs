//! Tool policy commands: the per-provider tool toggles the Providers panel offers.
//!
//! The daemon owns all of it — the `runtime_dir/tool-policies.json` file, the
//! stored rows, the registration-time gate. Every command here forwards one
//! request and re-shapes the reply into what the panel reads. Nothing on this
//! side writes policy state.
//!
//! `ToolPolicyGet` returns only the STORED rows: a provider or tool with no
//! row is enabled by default, so the panel treats a missing entry as enabled,
//! never as an error. The daemon answers the stored rows as
//! `DaemonMessage::ToolPolicy { id, policies }` and a set as
//! `DaemonMessage::ToolPolicySetOk { id }`.

use std::sync::Arc;

use devboule_daemon::DaemonClient;
use devboule_protocol::{DaemonMessage, ErrorCode, ToolPolicyEntry};
use tauri::State;

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

/// The `tool_policy_get` reply: the daemon's `tool_policy` frame without its
/// request id. `serde` spells the fields the way the panel reads them.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicyReply {
    pub policies: Vec<ToolPolicyEntry>,
}

#[tauri::command]
pub fn tool_policy_get(bridge: State<'_, DaemonBridge>) -> Result<ToolPolicyReply, CommandError> {
    match require_client(&bridge)?.tool_policy_get()? {
        DaemonMessage::ToolPolicy { policies, .. } => Ok(ToolPolicyReply { policies }),
        _ => Err(unexpected_reply()),
    }
}

#[tauri::command]
pub fn tool_policy_set(
    bridge: State<'_, DaemonBridge>,
    provider_id: String,
    enabled: Option<bool>,
    disabled_tools: Vec<String>,
) -> Result<(), CommandError> {
    match require_client(&bridge)?.tool_policy_set(&provider_id, enabled, disabled_tools)? {
        DaemonMessage::ToolPolicySetOk { .. } => Ok(()),
        _ => Err(unexpected_reply()),
    }
}
