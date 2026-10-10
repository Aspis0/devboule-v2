//! The `workspace_archive` and `project_remove` forwarders on their source:
//! the seam no unit test can exercise without a live daemon behind the bridge.

use tauri::State;

use super::super::error::CommandError;
use super::super::workspace::{project_remove, workspace_archive};
use crate::client::DaemonBridge;

/// The Tauri boundary `src/lib/tauri.ts` is written against. Tauri derives the
/// JS-side key names from these parameters, so a rename silently changes the
/// command's argument shape: `{ workspaceId }` and `{ projectId }`, nothing out.
#[test]
fn archive_and_removal_forwarders_have_the_frozen_tauri_signatures() {
    fn frozen<Fut: std::future::Future<Output = Result<(), CommandError>>>(
        _: fn(State<'static, DaemonBridge>, String) -> Fut,
    ) {
    }
    frozen(workspace_archive);
    frozen(project_remove);
}

/// Each body sends its own id to the daemon, and nothing else: an archive never
/// names a folder, and a removal never names a workspace.
#[test]
fn archive_and_removal_forward_their_own_ids() {
    let source = include_str!("../workspace.rs");
    assert!(
        source.contains("client.workspace_archive(&workspace_id)"),
        "backend/workspace.rs must send the workspace id to the archive"
    );
    assert!(
        source.contains("client.project_remove(&project_id)"),
        "backend/workspace.rs must send the project id to the removal"
    );
}
