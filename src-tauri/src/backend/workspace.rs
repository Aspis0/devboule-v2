use std::sync::Arc;

use devboule_daemon::DaemonClient;
use devboule_protocol::{Project, Workspace, WorkspaceGitStatus, WorkspaceIsolation};
use tauri::State;

use super::blocking::off_main_thread;
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

/// The uncommitted working-tree state of one workspace. `workspace_id` is the
/// whole argument: the daemon resolves the directory from it, because the
/// `path` the frontend holds is declared display-only.
///
/// The wait is a `git status` on the user's checkout. What bounds **this
/// caller** is `RPC_TIMEOUT` — 30 s, `crates/devboule-daemon/src/client.rs`
/// — which expires well before the daemon's own worst case (probe 10 s +
/// status 60 s + two numstat calls at 60 s each). The residual is accepted
/// and stated rather than hidden: on a checkout that slow the caller gets a
/// timeout while the daemon finishes work it already started, and the next
/// refresh starts over. Either way the wait leaves the window's thread the
/// way the other long roads do.
#[tauri::command]
pub async fn workspace_git_status(
    bridge: State<'_, DaemonBridge>,
    workspace_id: String,
) -> Result<WorkspaceGitStatus, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.workspace_git_status(&workspace_id)).await
}
