//! Remote-host commands: take a lease on a paired daemon, read one of its
//! three lists, and forward that host's state changes to the webview.
//!
//! Everything the daemon owns — the link, the lease, the keepalive, the
//! budgets — stays on the daemon side. These commands forward one request each
//! and hand back what the daemon (or, through it, the paired machine) says.
//! No key, address or tailnet name crosses this boundary: the only handle the
//! app ever names is the device id it already knows from the Devices panel.

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use devboule_daemon::DaemonClient;
use devboule_protocol::{ErrorCode, RemoteHostList, RemoteHostListBody, RemoteHostStatus};

use super::blocking::off_main_thread;
use super::error::CommandError;
use crate::client::DaemonBridge;

fn require_client(bridge: &DaemonBridge) -> Result<Arc<DaemonClient>, CommandError> {
    bridge
        .client()
        .map_err(|message| CommandError::new(ErrorCode::Io, message))
}

/// IMPORTANT STARTUP ORDER: the Channel is registered as the client's
/// `remote_host_status` handler **before** the watch is sent, exactly as
/// `session_attach` registers its Channel before `session_attach` leaves. A
/// link that is already up publishes its state the moment it is watched, and a
/// handler installed afterwards would miss that first edge.
///
/// The handler is one slot for the whole process, like the delegation switch
/// and the session-state snapshot: there is one sidebar here, and a window
/// that mounts replaces the previous window's handler.
#[tauri::command]
pub async fn remote_host_watch(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    ch: Channel<RemoteHostStatus>,
) -> Result<(), CommandError> {
    let client = require_client(&bridge)?;
    let sink = Arc::new(move |status: RemoteHostStatus| {
        let _ = ch.send(status);
    });
    client.on_remote_host_status(sink);
    off_main_thread(move || client.remote_host_watch(&device_id)).await
}

/// Give back this window's lease. The daemon keeps the link for its grace, so
/// a host that reopens immediately does not re-handshake.
#[tauri::command]
pub async fn remote_host_unwatch(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
) -> Result<(), CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.remote_host_unwatch(&device_id)).await
}

/// Read one of the three allowlisted lists from that host.
///
/// The reply is the remote machine's own rows. A refusal is the remote's own
/// typed error and reaches the app as `CommandError`, so the empty state can
/// say what the far side said instead of what this machine guessed.
#[tauri::command]
pub async fn remote_host_list(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    list: RemoteHostList,
) -> Result<RemoteHostListBody, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.remote_host_list(&device_id, list)).await
}
