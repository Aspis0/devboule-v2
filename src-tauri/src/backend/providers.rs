//! Tauri command for the honest PATH provider catalog.

use serde::Serialize;
use tauri::State;

use devboule_protocol::ProviderInfo;

use crate::client::DaemonBridge;

use super::blocking::off_main_thread;
use super::error::CommandError;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalog {
    pub providers: Vec<ProviderInfo>,
    pub unreadable_dirs: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUpdateResult {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub log: String,
}

#[tauri::command]
pub async fn providers_list(
    bridge: State<'_, DaemonBridge>,
) -> Result<ProviderCatalog, CommandError> {
    let client = require_client(&bridge)?;
    let (providers, unreadable_dirs) = off_main_thread(move || client.providers_list()).await?;
    Ok(ProviderCatalog {
        providers,
        unreadable_dirs,
    })
}

#[tauri::command]
pub async fn providers_refresh(
    bridge: State<'_, DaemonBridge>,
) -> Result<ProviderCatalog, CommandError> {
    let client = require_client(&bridge)?;
    let (providers, unreadable_dirs) = off_main_thread(move || client.providers_refresh()).await?;
    Ok(ProviderCatalog {
        providers,
        unreadable_dirs,
    })
}

/// The window must not wait on this call either: the daemon runs the
/// provider's package install inline, and the client's budget for it is
/// `PROVIDER_UPDATE_RPC_TIMEOUT` (240 s).
#[tauri::command]
pub async fn provider_update(
    bridge: State<'_, DaemonBridge>,
    provider_id: String,
) -> Result<ProviderUpdateResult, CommandError> {
    let client = require_client(&bridge)?;
    let (ok, exit_code, log) =
        off_main_thread(move || client.provider_update(&provider_id)).await?;
    Ok(ProviderUpdateResult { ok, exit_code, log })
}

fn require_client(
    bridge: &DaemonBridge,
) -> Result<std::sync::Arc<devboule_daemon::DaemonClient>, CommandError> {
    bridge
        .client()
        .map_err(|message| CommandError::new(devboule_protocol::ErrorCode::Io, message))
}
