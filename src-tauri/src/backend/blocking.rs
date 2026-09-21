//! One place where a daemon round trip that can take minutes leaves the
//! window's thread.

use devboule_daemon::DaemonError;
use devboule_protocol::ErrorCode;

use super::error::CommandError;

/// Run one blocking daemon round trip away from the thread that draws.
///
/// A `#[tauri::command]` that is not `async` is called inline by the IPC
/// dispatcher — the window's own thread — so a reply that can take minutes
/// (the resume budget is 300 s) freezes the window; the measured case is
/// Reopen, `Not Responding` for tens of seconds
/// (`scout/user-pass/f08b.png`). An `async` command is spawned on the async
/// runtime instead, and the blocking call itself goes to `spawn_blocking` so
/// no runtime worker is the one that waits. The form is the plugin bridge's
/// (`plugins/rpc.rs`, `plugin_backend_ensure`), where this project already
/// waits off the window's thread.
pub(crate) async fn off_main_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, DaemonError> + Send + 'static,
) -> Result<T, CommandError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?
        .map_err(CommandError::from)
}
