//! One place where work that blocks leaves the window's thread.

use devboule_protocol::ErrorCode;

use super::error::CommandError;

/// Run one blocking piece of work away from the thread that draws.
///
/// A `#[tauri::command]` that is not `async` is called inline by the IPC
/// dispatcher — the window's own thread — so a reply that can take minutes
/// (the resume budget is 300 s) freezes the window; the measured case is
/// Reopen, `Not Responding` for tens of seconds
/// (`scout/user-pass/f08b.png`). An `async` command is spawned on the async
/// runtime instead, and the blocking work itself goes to `spawn_blocking` so
/// no runtime worker is the one that waits. The form is the plugin bridge's
/// (`plugins/rpc.rs`, `plugin_backend_ensure`), where this project already
/// waits off the window's thread.
///
/// The error is the caller's, mapped at this boundary: a daemon round trip
/// answers `DaemonError`, the artifact write and the plugin install answer
/// [`CommandError`] already, and both become the one the command returns.
pub(crate) async fn off_main_thread<T, E, F>(work: F) -> Result<T, CommandError>
where
    T: Send + 'static,
    E: Into<CommandError> + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?
        .map_err(Into::into)
}
