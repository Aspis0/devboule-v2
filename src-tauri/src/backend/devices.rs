//! Devices panel commands: this device's identity, every paired or pending
//! peer, and the pairing and revocation actions the panel offers.
//!
//! The daemon owns all of it — its own identity, the `peers` table, the pairing
//! state machine — so every command here forwards one request and re-shapes the
//! reply into what the panel reads. Nothing on this side writes device state.
//!
//! The pairing code is a five-minute secret. It travels from the daemon to this
//! layer and straight out to the panel: it is never formatted into an error
//! string, and [`unexpected_reply`] deliberately names no frame.

use std::sync::Arc;

use devboule_daemon::DaemonClient;
use devboule_protocol::{
    DaemonMessage, ErrorCode, PairingSecret, PeerRole, PeerRow, PendingPairing, SelfInfo,
};
use tauri::State;

use super::error::CommandError;
use crate::client::DaemonBridge;

fn require_client(bridge: &DaemonBridge) -> Result<Arc<DaemonClient>, CommandError> {
    bridge
        .client()
        .map_err(|message| CommandError::new(ErrorCode::Io, message))
}

/// A reply of the wrong variant. It names no frame on purpose: a `pairing_code`
/// frame carries the live code, and formatting the frame into a message is how
/// a secret ends up in a log.
fn unexpected_reply() -> CommandError {
    CommandError::new(ErrorCode::Internal, "unexpected daemon reply")
}

/// Read the panel's role string into the daemon's enum.
///
/// The frontend sends `"client" | "daemon"`, which is what `PeerRole`
/// serialises to; an unknown value is refused **here**, with a sentence a
/// person can act on, rather than being passed to the daemon as a string it
/// would have to reinterpret. The message names both accepted values because
/// the only realistic cause is a UI bug or a stale frontend.
fn parse_role(role: &str) -> Result<PeerRole, CommandError> {
    PeerRole::parse(role).ok_or_else(|| {
        CommandError::new(ErrorCode::InvalidRequest, "role must be client or daemon")
    })
}

/// The `devices_list` reply: the daemon's `devices` frame without its request
/// id. `serde` spells the fields the way the panel reads them.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicesReply {
    pub self_info: SelfInfo,
    pub peers: Vec<PeerRow>,
    pub pending: Vec<PendingPairing>,
}

/// The `pairing_start` reply: the daemon's `pairing_code` frame without its
/// request id.
///
/// `expires_at` is the daemon's `i64` unix-millisecond stamp, and `code` is a
/// plain string for the panel; `PairingSecret` is the daemon's redacting
/// wrapper and its `Debug` is not what the frontend needs.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCode {
    pub code: String,
    pub expires_at: i64,
    pub address: String,
}

/// The two ways `pairing_complete` can land. Tagged with the daemon frame's own
/// names (`pairing_pending` / `pairing_done`) so the frontend discriminates on
/// the reply variant instead of on which fields are present.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PairingOutcome {
    PairingPending { peer: PendingPairing },
    PairingDone { peer: PeerRow },
}

#[tauri::command]
pub fn devices_list(bridge: State<'_, DaemonBridge>) -> Result<DevicesReply, CommandError> {
    match require_client(&bridge)?.devices_list()? {
        DaemonMessage::Devices {
            self_info,
            peers,
            pending,
            ..
        } => Ok(DevicesReply {
            self_info,
            peers,
            pending,
        }),
        _ => Err(unexpected_reply()),
    }
}

#[tauri::command]
pub fn pairing_start(
    bridge: State<'_, DaemonBridge>,
    role: String,
) -> Result<PairingCode, CommandError> {
    let role = parse_role(&role)?;
    match require_client(&bridge)?.pairing_start(role)? {
        DaemonMessage::PairingCode {
            code,
            expires_at,
            address,
            ..
        } => Ok(PairingCode {
            // The panel shows this string and never logs it; unwrapping the
            // wrapper here is the one place it leaves its envelope.
            code: code.as_str().to_string(),
            expires_at,
            address,
        }),
        _ => Err(unexpected_reply()),
    }
}

#[tauri::command]
pub fn pairing_complete(
    bridge: State<'_, DaemonBridge>,
    address: String,
    code: String,
    role: String,
) -> Result<PairingOutcome, CommandError> {
    let role = parse_role(&role)?;
    match require_client(&bridge)?.pairing_complete(
        &address,
        // The wrapper keeps the code out of `Debug` output for the whole trip
        // through the daemon client.
        PairingSecret::new(code),
        role,
    )? {
        DaemonMessage::PairingPending { peer, .. } => Ok(PairingOutcome::PairingPending { peer }),
        DaemonMessage::PairingDone { peer, .. } => Ok(PairingOutcome::PairingDone { peer }),
        _ => Err(unexpected_reply()),
    }
}

#[tauri::command]
pub fn pairing_confirm(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    accept: bool,
) -> Result<Option<PeerRow>, CommandError> {
    // `None` is a declined pairing, which the daemon reports as success; it
    // reaches the frontend as `null`.
    Ok(require_client(&bridge)?.pairing_confirm(&device_id, accept)?)
}

#[tauri::command]
pub fn peer_revoke(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
) -> Result<PeerRow, CommandError> {
    Ok(require_client(&bridge)?.peer_revoke(&device_id)?)
}

#[tauri::command]
pub fn peer_set_caps(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    caps: Vec<String>,
) -> Result<PeerRow, CommandError> {
    Ok(require_client(&bridge)?.peer_set_caps(&device_id, caps)?)
}
