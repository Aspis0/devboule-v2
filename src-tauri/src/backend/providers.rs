//! Tauri command for the honest PATH provider catalog.

use serde::Serialize;
use tauri::State;

use devboule_protocol::ProviderInfo;

use crate::client::DaemonBridge;

use super::error::CommandError;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalog {
    pub providers: Vec<ProviderInfo>,
    pub unreadable_dirs: u32,
}

#[tauri::command]
pub fn providers_list(bridge: State<'_, DaemonBridge>) -> Result<ProviderCatalog, CommandError> {
    let (providers, unreadable_dirs) = require_client(&bridge)?.providers_list()?;
    Ok(ProviderCatalog {
        providers,
        unreadable_dirs,
    })
}

fn require_client(
    bridge: &DaemonBridge,
) -> Result<std::sync::Arc<devboule_daemon::DaemonClient>, CommandError> {
    bridge
        .client()
        .map_err(|message| CommandError::new(devboule_protocol::ErrorCode::Io, message))
}
