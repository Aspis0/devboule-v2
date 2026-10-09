//! Remote-host commands: take a lease on a paired daemon, read one of its
//! three lists, and forward that host's state changes to the webview.
//!
//! Everything the daemon owns — the link, the lease, the keepalive, the
//! budgets — stays on the daemon side. These commands forward one request each
//! and hand back what the daemon (or, through it, the paired machine) says.
//! No key, address or tailnet name crosses this boundary: the only handle the
//! app ever names is the device id it already knows from the Devices panel.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::ipc::Channel;
use tauri::State;

use devboule_daemon::DaemonClient;
use devboule_protocol::{
    ErrorCode, RemoteHostList, RemoteHostListBody, RemoteHostStatus, RemoteRelayMessage,
};

use super::blocking::off_main_thread;
use super::error::CommandError;
use crate::client::DaemonBridge;

fn require_client(bridge: &DaemonBridge) -> Result<Arc<DaemonClient>, CommandError> {
    bridge
        .client()
        .map_err(|message| CommandError::new(ErrorCode::Io, message))
}

/// Every open remote stream's channel, keyed by `device:subscription`. One
/// dispatcher is installed on the client for the process, so a second stream
/// never replaces the first stream's handler; the key routes an event to the
/// surface that opened it, even when two surfaces watch the same host.
fn relay_channels() -> &'static Mutex<HashMap<String, Channel<RemoteRelayMessage>>> {
    static CHANNELS: OnceLock<Mutex<HashMap<String, Channel<RemoteRelayMessage>>>> =
        OnceLock::new();
    CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn relay_key(device_id: &str, subscription_id: u64) -> String {
    format!("{device_id}:{subscription_id}")
}

/// Whether a registry key belongs to one host, whoever subscribed.
fn relay_key_is_for_device(key: &str, device_id: &str) -> bool {
    key.starts_with(&format!("{device_id}:"))
}

fn forget_relay(device_id: &str, subscription_id: u64) {
    relay_channels()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&relay_key(device_id, subscription_id));
}

/// Drop every stream route of one host: a window that gives the lease back
/// cannot still want the streams it opened under it, and a closed window must
/// not leave its channels in the process registry.
fn forget_relays_for_device(device_id: &str) {
    relay_channels()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|key, _| !relay_key_is_for_device(key, device_id));
}

/// Install the one dispatcher, once. Called before a channel is registered so
/// no event can arrive before its route exists.
fn install_relay_dispatcher(client: &Arc<DaemonClient>) {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.get().is_some() {
        return;
    }
    client.on_remote_host_event(Arc::new(|message: RemoteRelayMessage| {
        let (device_id, subscription_id) = match &message {
            RemoteRelayMessage::Event(event) => (&event.device_id, event.subscription_id),
            RemoteRelayMessage::Gap(gap) => (&gap.device_id, gap.subscription_id),
        };
        let channel = relay_channels()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&relay_key(device_id, subscription_id))
            .cloned();
        if let Some(channel) = channel {
            let _ = channel.send(message);
        }
    }));
    let _ = INSTALLED.set(());
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
    let host = device_id.clone();
    let result = off_main_thread(move || client.remote_host_unwatch(&device_id)).await;
    // The lease is gone: its stream routes go with it, so a closed window's
    // channels do not linger in the process registry.
    forget_relays_for_device(&host);
    result
}

/// Read one of the three allowlisted lists from that host.
///
/// The reply is the remote machine's own rows. A refusal is the remote's own
/// typed error and reaches the app as `CommandError`, so the empty state can
/// say what the far side said instead of what this machine guessed.
/// Open one session's live stream on a paired host and forward its events to
/// the webview. Registration happens before the attach leaves, exactly like
/// the watch: an event that overtakes the reply would otherwise be lost.
#[tauri::command]
pub async fn remote_session_attach(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    subscription_id: u64,
    ch: Channel<RemoteRelayMessage>,
) -> Result<(), CommandError> {
    let client = require_client(&bridge)?;
    install_relay_dispatcher(&client);
    // The route exists before the attach leaves, so an event that overtakes
    // the reply still lands on this surface's channel.
    relay_channels()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(relay_key(&device_id, subscription_id), ch);
    let route = (device_id.clone(), subscription_id);
    let result = off_main_thread(move || {
        client.remote_host_attach(&device_id, &session_id, subscription_id)
    })
    .await;
    if result.is_err() {
        forget_relay(&route.0, route.1);
    }
    result
}

/// Close one session's live stream. The daemon drops the local subscription
/// regardless of what the peer answers, so this is idempotent.
#[tauri::command]
pub async fn remote_session_detach(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    subscription_id: u64,
) -> Result<(), CommandError> {
    let client = require_client(&bridge)?;
    let route = (device_id.clone(), subscription_id);
    let result = off_main_thread(move || {
        client.remote_host_detach(&device_id, &session_id, subscription_id)
    })
    .await;
    // The route goes whether or not the daemon answered: a closed stream must
    // not keep a channel alive, which is also what an unmount reaches.
    forget_relay(&route.0, route.1);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two streams of the same host and two hosts with the same subscription
    /// id are different routes; one process-wide handler never mixes them.
    #[test]
    fn a_relay_route_is_keyed_by_device_and_subscription() {
        assert_eq!(relay_key("device-a", 1), "device-a:1");
        assert_ne!(relay_key("device-a", 1), relay_key("device-a", 2));
        assert_ne!(relay_key("device-a", 1), relay_key("device-b", 1));
        assert!(relay_key_is_for_device("device-a:7", "device-a"));
        assert!(!relay_key_is_for_device("device-ab:7", "device-a"));
    }
}

#[tauri::command]
pub async fn remote_host_list(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    list: RemoteHostList,
) -> Result<RemoteHostListBody, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.remote_host_list(&device_id, list)).await
}
