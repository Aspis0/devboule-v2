//! The delegation switch at the app boundary: the Settings switch's read and
//! write of the one stored answer.
//!
//! The daemon owns all of it — the `delegation.json` file, its quarantine,
//! the push of every change to every watching connection. Each command here
//! forwards one request and hands back what the daemon says it **holds**, not
//! what the caller sent: `delegation_set` returns the stored value plus where
//! it came from, which is the rule
//! `NOTE-a-write-that-does-not-say-what-it-stored.md` argues for — the writer
//! can hold the value the daemon actually has instead of guessing its own
//! argument landed. Nothing on this side reads or writes the file.

use std::sync::Arc;

use devboule_daemon::DaemonClient;
use devboule_protocol::{DaemonMessage, DelegationSource, ErrorCode};
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

/// The reply both commands carry: the stored answer plus where it came from.
/// `source` is the daemon's own three words — a human's `file`, a first-run
/// `default`, a damaged `quarantined` — and the panel must keep them three
/// sentences; collapsing either of the last two into plain "off" turns a fact
/// the human needs into a state they cannot distinguish.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationReply {
    pub enabled: bool,
    pub source: DelegationSource,
}

#[tauri::command]
pub fn delegation_get(bridge: State<'_, DaemonBridge>) -> Result<DelegationReply, CommandError> {
    match require_client(&bridge)?.delegation_get()? {
        DaemonMessage::DelegationState {
            enabled, source, ..
        } => Ok(DelegationReply { enabled, source }),
        _ => Err(unexpected_reply()),
    }
}

#[tauri::command]
pub fn delegation_set(
    bridge: State<'_, DaemonBridge>,
    enabled: bool,
) -> Result<DelegationReply, CommandError> {
    match require_client(&bridge)?.delegation_set(enabled)? {
        DaemonMessage::DelegationSetOk {
            enabled, source, ..
        } => Ok(DelegationReply { enabled, source }),
        _ => Err(unexpected_reply()),
    }
}
