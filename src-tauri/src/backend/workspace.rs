use std::sync::Arc;

use devboule_daemon::DaemonClient;
use devboule_protocol::{Project, Workspace, WorkspaceIsolation};
use tauri::State;

use super::error::CommandError;
use crate::client::DaemonBridge;

fn require_client(bridge: &DaemonBridge) -> Result<Arc<DaemonClient>, CommandError> {
    bridge
        .client()
        .map_err(|message| CommandError::new(devboule_protocol::ErrorCode::Io, message))
}

#[tauri::command]
pub fn projects_list(bridge: State<'_, DaemonBridge>) -> Result<Vec<Project>, CommandError> {
    Ok(require_client(&bridge)?.projects_list()?)
}

#[tauri::command]
pub fn project_add(bridge: State<'_, DaemonBridge>, path: String) -> Result<Project, CommandError> {
    Ok(require_client(&bridge)?.project_add(&path)?)
}

#[tauri::command]
pub fn workspaces_list(
    bridge: State<'_, DaemonBridge>,
    project_id: String,
) -> Result<Vec<Workspace>, CommandError> {
    Ok(require_client(&bridge)?.workspaces_list(&project_id)?)
}

#[tauri::command]
pub fn workspace_create(
    bridge: State<'_, DaemonBridge>,
    project_id: String,
    isolation: WorkspaceIsolation,
    branch: Option<String>,
) -> Result<Workspace, CommandError> {
    Ok(require_client(&bridge)?.workspace_create(&project_id, isolation, branch)?)
}
