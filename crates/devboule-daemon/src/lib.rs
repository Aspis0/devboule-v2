//! Devboule daemon: single-instance lock, named pipe, versioned handshake,
//! and (behind the `server` feature) ownership of PTY sessions.

use std::time::Duration;

#[cfg(feature = "server")]
mod acp_view;
#[cfg(feature = "server")]
mod agent_env;
#[cfg(feature = "server")]
mod agent_report;
#[cfg(feature = "server")]
mod atomic;
#[cfg(feature = "server")]
mod claude_view;
mod client;
mod error;
mod framing;
#[cfg(feature = "server")]
mod idempotency;
#[cfg(feature = "server")]
mod journal;
#[cfg(feature = "server")]
mod lock;
#[cfg(feature = "server")]
mod outbound;
mod paths;
mod process_tree;
pub mod provider_catalog;
#[cfg(feature = "server")]
mod provider_update;
#[cfg(feature = "server")]
mod registry;
#[cfg(feature = "server")]
mod screen;
#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
mod session;
mod spawn;
mod transport;

#[cfg(windows)]
mod security;

#[cfg(feature = "server")]
pub use agent_env::{
    BIN_PATH as DEVBOULE_BIN_PATH, ENV_MARKER as DEVBOULE_ENV,
    ENV_MARKER_VALUE as DEVBOULE_ENV_VALUE, SESSION_ID as DEVBOULE_SESSION_ID,
    SOCKET_PATH as DEVBOULE_SOCKET_PATH, WORKSPACE_ID as DEVBOULE_WORKSPACE_ID,
};
#[cfg(feature = "server")]
pub use atomic::atomic_write;
pub use client::{
    connect, connect_or_spawn, handshake, test_owner, DaemonClient, EventHandler,
    SessionStateHandler,
};
pub use error::DaemonError;
pub use framing::Framed;
#[cfg(feature = "server")]
pub use journal::{
    Journal, JournalError, JournalLimits, Replay, JOURNAL_MAX_AGE_MS, JOURNAL_MAX_BYTES,
    JOURNAL_MAX_SESSIONS, JOURNAL_QUEUE_CAP, JOURNAL_SCHEMA_VERSION, JOURNAL_SESSION_MAX_BYTES,
    SNAPSHOT_EVERY_BYTES,
};
pub use paths::RuntimePaths;
pub use process_tree::JobObject;
#[cfg(feature = "server")]
pub use provider_update::{NpmInstallResult, NpmInstallRunner, ProcessNpmInstallRunner};
#[cfg(feature = "server")]
pub use screen::{
    render_ansi, Screen, ScreenSnapshot, SnapshotCursor, SnapshotCursorShape, MAX_TITLE_CHARS,
};
#[cfg(feature = "server")]
pub use server::{run, ServerState};
#[cfg(feature = "server")]
pub use session::{
    write_test_pty_command, PtyCommand, COALESCE_FLUSH, COALESCE_MAX_BYTES,
    PENDING_OUTPUT_BUDGET_BYTES, PENDING_OUTPUT_BUDGET_FRAMES, SESSION_OS_SWEEP_INTERVAL,
    SESSION_SILENCE_THRESHOLD,
};
pub use spawn::{daemon_file_name, resolve_daemon_binary, spawn_daemon};

/// How long an otherwise idle daemon waits before beginning shutdown.
///
/// Keep this long enough for a client reconnect caused by a transient app or
/// pipe interruption, while still releasing the daemon binary promptly after
/// the app has really gone away.
pub const IDLE_SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

#[cfg(windows)]
pub use security::{current_user_sid, dacl_is_current_user_only, user_only_sddl};
#[cfg(windows)]
pub use transport::{connect_pipe, inspect_pipe_dacl};
