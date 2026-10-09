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
    AttachmentReference, ErrorCode, PermissionOutcome, PromptAttachment, RemoteHostList,
    RemoteHostListBody, RemoteHostStatus, RemoteRelayMessage, Session, SessionKind, SubscriptionId,
};

use super::blocking::off_main_thread;
use super::error::CommandError;
use super::session::{
    parse_active_turn_behavior, require_attachment_limits, require_attachment_reference_limits,
    require_idempotency_key, require_session_id, require_write_size,
};
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

/// Create one session in a workspace on a paired host. The owning daemon
/// runs the provider and the process; this side only carries the ask.
///
/// `idempotency_key` is the caller's retry identity: it travels with the
/// host's own create, so an explicit retry after a lost reply answers with
/// the same session. The UI mints one key per user intent and keeps it
/// across the retry affordance — never resending on its own, because after
/// a transport failure the create outcome is unknown.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn remote_host_create(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    workspace_id: Option<String>,
    kind: SessionKind,
    provider: Option<String>,
    mode: Option<String>,
    display_name: Option<String>,
    idempotency_key: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<Session, CommandError> {
    require_idempotency_key(idempotency_key.as_deref())?;
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        client.remote_host_create(
            &device_id,
            workspace_id,
            kind,
            provider,
            mode,
            display_name,
            idempotency_key,
            cols,
            rows,
        )
    })
    .await
}

/// Send text into one session on a paired host. The stream must already be
/// attached: the subscription is the one `remote_session_attach` opened,
/// and the host's scope decides the answer exactly as for a local send.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn remote_host_send(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    subscription_id: SubscriptionId,
    text: String,
    attachments: Option<Vec<PromptAttachment>>,
    active_turn_behavior: Option<String>,
    attachment_references: Option<Vec<AttachmentReference>>,
    idempotency_key: Option<String>,
) -> Result<bool, CommandError> {
    require_session_id(&session_id)?;
    require_write_size(&text)?;
    let attachments = attachments.unwrap_or_default();
    require_attachment_limits(&attachments)?;
    let attachment_references = attachment_references.unwrap_or_default();
    require_attachment_reference_limits(&session_id, &attachment_references)?;
    require_idempotency_key(idempotency_key.as_deref())?;
    let active_turn_behavior = parse_active_turn_behavior(active_turn_behavior.as_deref())?;
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        client.remote_host_send(
            &device_id,
            &session_id,
            subscription_id,
            &text,
            attachments,
            active_turn_behavior,
            idempotency_key,
            attachment_references,
        )
    })
    .await
}

/// Resize one terminal on a paired host.
#[tauri::command]
pub async fn remote_host_resize(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    subscription_id: SubscriptionId,
    cols: u16,
    rows: u16,
) -> Result<(), CommandError> {
    require_session_id(&session_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        client.remote_host_resize(&device_id, &session_id, subscription_id, cols, rows)
    })
    .await
}

/// Claim one terminal's resize right on a paired host.
#[tauri::command]
pub async fn remote_host_claim(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    require_session_id(&session_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.remote_host_claim(&device_id, &session_id, subscription_id))
        .await
}

/// Interrupt one session on a paired host.
#[tauri::command]
pub async fn remote_host_interrupt(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    require_session_id(&session_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.remote_host_interrupt(&device_id, &session_id, subscription_id))
        .await
}

/// Answer one permission card on a paired host. The card was created and is
/// resolved there; this side cannot auto-approve it, and the human's answer
/// travels as the human's — no confirmation card of its own.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn remote_host_permission_respond(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    subscription_id: SubscriptionId,
    request_id: String,
    outcome: PermissionOutcome,
    option_id: Option<String>,
    answer: Option<String>,
    idempotency_key: Option<String>,
) -> Result<(), CommandError> {
    require_session_id(&session_id)?;
    if request_id.is_empty() {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "Permission request id is required.",
        ));
    }
    require_idempotency_key(idempotency_key.as_deref())?;
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        client.remote_host_permission_respond(
            &device_id,
            &session_id,
            subscription_id,
            &request_id,
            outcome,
            option_id,
            answer,
            idempotency_key,
        )
    })
    .await
}

/// Close one session on a paired host.
#[tauri::command]
pub async fn remote_host_close(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    idempotency_key: Option<String>,
) -> Result<(), CommandError> {
    require_session_id(&session_id)?;
    require_idempotency_key(idempotency_key.as_deref())?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.remote_host_close(&device_id, &session_id, idempotency_key))
        .await
}

/// Stop one session's process on a paired host, keeping the session.
#[tauri::command]
pub async fn remote_host_stop(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    subscription_id: SubscriptionId,
) -> Result<(), CommandError> {
    require_session_id(&session_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.remote_host_stop(&device_id, &session_id, subscription_id)).await
}

/// Read a paired host's provider catalog, for the remote create picker:
/// what that machine offers, not this one's.
#[tauri::command]
pub async fn remote_host_providers(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
) -> Result<super::providers::ProviderCatalog, CommandError> {
    let client = require_client(&bridge)?;
    let (providers, unreadable_dirs) =
        off_main_thread(move || client.remote_host_providers(&device_id)).await?;
    Ok(super::providers::ProviderCatalog {
        providers,
        unreadable_dirs,
    })
}

/// Switch the model of one session on a paired host.
#[tauri::command]
pub async fn remote_host_set_model(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    model_id: Option<String>,
    effort: Option<String>,
) -> Result<(), CommandError> {
    require_session_id(&session_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.remote_host_set_model(&device_id, &session_id, model_id, effort))
        .await
}

/// Switch the mode of one session on a paired host.
#[tauri::command]
pub async fn remote_host_set_mode(
    bridge: State<'_, DaemonBridge>,
    device_id: String,
    session_id: String,
    mode_id: String,
) -> Result<(), CommandError> {
    require_session_id(&session_id)?;
    let client = require_client(&bridge)?;
    off_main_thread(move || client.remote_host_set_mode(&device_id, &session_id, &mode_id)).await
}
