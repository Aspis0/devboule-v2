//! The `workspace_delete` forwarder on its source: the seam no unit test can
//! exercise without a live daemon behind the bridge.

use tauri::State;

use super::super::error::CommandError;
use super::super::workspace::workspace_delete;
use crate::client::DaemonBridge;

/// The Tauri boundary `src/lib/tauri.ts` is written against: `{ workspaceId }`
/// in, nothing out. Tauri derives the JS-side key names from these
/// parameters, so a rename or a new parameter here silently changes the
/// command's argument shape.
#[test]
fn workspace_delete_forwarder_has_the_frozen_tauri_signature() {
    fn frozen<Fut: std::future::Future<Output = Result<(), CommandError>>>(
        _: fn(State<'static, DaemonBridge>, String) -> Fut,
    ) {
    }
    frozen(workspace_delete);
}

/// What the command body sends, pinned where it is written: this workspace's
/// id and the daemon's own default — no caller of this command can force a
/// dirty checkout away.
#[test]
fn workspace_delete_forwards_the_id_and_never_forces() {
    let source = include_str!("../workspace.rs");
    assert!(
        source.contains("client.workspace_delete(&workspace_id, false)"),
        "backend/workspace.rs must send this workspace id with force=false: the app has no force"
    );
}
