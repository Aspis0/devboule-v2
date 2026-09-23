use std::sync::Arc;

use devboule_daemon::DaemonClient;
use devboule_protocol::{
    Project, Workspace, WorkspaceDirectory, WorkspaceFileContent, WorkspaceGitFileDiff,
    WorkspaceGitStatus, WorkspaceIsolation,
};
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
pub async fn projects_list(bridge: State<'_, DaemonBridge>) -> Result<Vec<Project>, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.projects_list()).await
}

#[tauri::command]
pub async fn project_add(
    bridge: State<'_, DaemonBridge>,
    path: String,
) -> Result<Project, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.project_add(&path)).await
}

#[tauri::command]
pub async fn workspaces_list(
    bridge: State<'_, DaemonBridge>,
    project_id: String,
) -> Result<Vec<Workspace>, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.workspaces_list(&project_id)).await
}

#[tauri::command]
pub async fn workspace_create(
    bridge: State<'_, DaemonBridge>,
    project_id: String,
    isolation: WorkspaceIsolation,
    branch: Option<String>,
) -> Result<Workspace, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.workspace_create(&project_id, isolation, branch)).await
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

/// The diff of one workspace file. `workspace_id` names the folder and
/// `path` is relative to it; the daemon confines the path before it opens
/// anything, so the frontend cannot name a directory it was never vouched
/// for.
///
/// Bounded exactly like the status road above: `RPC_TIMEOUT` (30 s) is what
/// this caller feels, against a daemon worst case of **190 s** — the 10 s
/// probe (`GIT_PROBE_TIMEOUT`, `crates/devboule-daemon/src/git.rs:15`)
/// plus three commands at 60 s each (`git status` for the path, `git diff
/// HEAD`, and the one declared `--cached` fallback in a repository with no
/// commit). Outside those four calls sits the one wait nobody bounds: the
/// synthesis of an untracked file reads the filesystem with **no timeout at
/// all** — inherited from slice 1, declared there (its review, §4.1). On a
/// checkout that slow or a file that hangs, the caller times out at 30 s
/// while the daemon finishes, and the residual is accepted and stated
/// rather than hidden. The wait leaves the window's thread the way the
/// other long roads do.
#[tauri::command]
pub async fn workspace_git_diff(
    bridge: State<'_, DaemonBridge>,
    workspace_id: String,
    path: String,
) -> Result<WorkspaceGitFileDiff, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.workspace_git_diff(&workspace_id, &path)).await
}

/// The entries of one workspace folder — the Files panel's tree, one
/// directory per request. `workspace_id` names the folder and `path` is
/// relative to it (empty = the folder itself); the daemon confines the path
/// before it opens anything, and only reads: no rename, no delete, no write
/// exists behind this road.
///
/// The wait is a `read_dir` plus one stat per entry over a folder whose
/// entry count nobody chose. The daemon's entry cap bounds what the reply
/// **carries**, not this scan: entries the listing skips never reach the
/// cap, so a folder full of links is read to its end. What bounds this
/// caller — and with it the scan — is `RPC_TIMEOUT` (30 s), the same bound
/// as the two git roads above; the residual on a folder that slow is
/// accepted and stated the same way there. Either way the wait leaves the
/// window's thread the way the other long roads do.
#[tauri::command]
pub async fn workspace_files_list(
    bridge: State<'_, DaemonBridge>,
    workspace_id: String,
    path: String,
) -> Result<WorkspaceDirectory, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.workspace_files_list(&workspace_id, &path)).await
}

/// The content of one workspace file — the Files panel's preview behind a
/// clicked file row. `workspace_id` names the folder and `path` is relative
/// to it; the daemon confines the path, refuses links and the repository's
/// metadata, caps the read at 128 KiB before opening anything, and only
/// reads: no write exists behind this road.
///
/// Bounded like the other three workspace roads: `RPC_TIMEOUT` (30 s) is
/// what this caller feels, and the daemon's own work is one stat plus a read
/// of at most 128 KiB — no process is spawned for this frame at all. The
/// wait leaves the window's thread the way the other long roads do.
#[tauri::command]
pub async fn workspace_file_read(
    bridge: State<'_, DaemonBridge>,
    workspace_id: String,
    path: String,
) -> Result<WorkspaceFileContent, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || client.workspace_file_read(&workspace_id, &path)).await
}
