//! Devboule daemon: single-instance lock, named pipe, versioned handshake,
//! and (behind the `server` feature) ownership of PTY sessions.

use std::time::Duration;

#[cfg(feature = "server")]
mod acp_view;
#[cfg(feature = "server")]
mod agent_activity;
#[cfg(feature = "server")]
mod agent_env;
#[cfg(feature = "server")]
mod agent_profiles;
#[cfg(feature = "server")]
mod agent_report;
#[cfg(feature = "server")]
mod atomic;
#[cfg(feature = "server")]
mod attachment_store;
#[cfg(feature = "server")]
mod claude_catalog;
#[cfg(feature = "server")]
mod claude_view;
mod client;
#[cfg(feature = "server")]
mod codex_view;
mod daemon_record;
#[cfg(feature = "server")]
mod delegation_store;
#[cfg(feature = "server")]
mod device_identity;
mod diagnostics;
mod error;
mod framing;
#[cfg(feature = "server")]
mod git;
#[cfg(feature = "server")]
mod idempotency;
#[cfg(feature = "server")]
mod journal;
mod lock;
mod login_shell_env;
#[cfg(feature = "server")]
mod mcp_broker;
#[cfg(feature = "server")]
mod mcp_device_roster;
#[cfg(feature = "server")]
mod mcp_peer_agents;
#[cfg(feature = "server")]
mod mcp_project_graph;
mod oracle_app_record;
#[cfg(feature = "server")]
mod oracle_forward;
#[cfg(feature = "server")]
mod outbound;
#[cfg(feature = "server")]
mod pairing;
mod paths;
#[cfg(feature = "server")]
mod peer_policy;
#[cfg(feature = "server")]
mod peer_transport;
#[cfg(feature = "server")]
mod pi_view;
mod process_tree;
#[cfg(feature = "server")]
mod profile_delivery;
pub mod provider_catalog;
#[cfg(feature = "server")]
mod provider_update;
#[cfg(feature = "server")]
mod provider_vocabulary;
#[cfg(feature = "server")]
mod raster_metadata;
#[cfg(feature = "server")]
mod registry;
#[cfg(feature = "server")]
mod release_guard;
mod rpc_trace;
#[cfg(feature = "server")]
mod screen;
#[cfg(feature = "server")]
mod secret_store;
#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
mod session;
#[cfg(feature = "server")]
mod shell_unwrap;
mod spawn;
#[cfg(feature = "server")]
mod tailscale_localapi;
#[cfg(test)]
mod temp_dir_guard_tests;
#[cfg(test)]
mod test_dirs;
#[cfg(all(test, feature = "server"))]
mod test_support;
mod text_safety;
#[cfg(feature = "server")]
mod tool_paths;
#[cfg(feature = "server")]
mod tool_policy;
mod transport;
#[cfg(feature = "server")]
mod user_providers;
#[cfg(windows)]
mod windows_path_env;
#[cfg(feature = "server")]
mod wire_json;
#[cfg(feature = "server")]
mod workspace;
#[cfg(feature = "server")]
mod workspace_file_mutations;
#[cfg(feature = "server")]
mod workspace_file_preview;
#[cfg(feature = "server")]
mod workspace_file_read;
#[cfg(feature = "server")]
mod workspace_files;
#[cfg(feature = "server")]
mod workspace_git_diff;
#[cfg(feature = "server")]
mod workspace_git_status;
#[cfg(feature = "server")]
mod workspace_git_support;
#[cfg(feature = "server")]
mod workspace_git_write;
#[cfg(feature = "server")]
mod worktree;

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
    connect, connect_or_spawn, handshake, test_owner, DaemonClient, DelegationChangedHandler,
    EventHandler, SessionStateHandler,
};
// Neither the daemon's record nor its reader is behind `server`: the GUI
// process is the reader, and it links this crate with `default-features = false`.
pub use daemon_record::{
    DaemonRecord, DaemonState, ExitReason, Heartbeat, GOODBYE_TRUSTED_FOR, HEARTBEAT_INTERVAL,
    RECORD_CAPACITY, STALE_AFTER, STALE_BEATS,
};
#[cfg(feature = "server")]
pub use diagnostics::DiagnosticsInput;
pub use diagnostics::{
    DaemonDiagnostics, DiagnosticsReport, EnvironmentDiagnostics, HealthDiagnostics,
    ProviderDiagnostics, SafeText, SessionDiagnostics,
};
pub use error::DaemonError;
pub use framing::Framed;
#[cfg(feature = "server")]
pub use journal::{
    Journal, JournalError, JournalLimits, Replay, JOURNAL_MAX_AGE_MS, JOURNAL_MAX_BYTES,
    JOURNAL_MAX_SESSIONS, JOURNAL_QUEUE_CAP, JOURNAL_SCHEMA_VERSION, JOURNAL_SESSION_MAX_BYTES,
    SNAPSHOT_EVERY_BYTES,
};
pub use lock::SingleInstanceLock;
pub use login_shell_env::{
    initialize_login_shell_environment, login_shell_capture_outcome, LoginShellCaptureOutcome,
    LoginShellCaptureState,
};
// Same rule as the record above: the app writes this one, the daemon reads
// it, and neither side runs behind `server`.
pub use oracle_app_record::{
    oracle_app_lock_path, OracleAppRecord, OracleAppState, ORACLE_APP_LOCK_FILE_NAME,
};
pub use paths::RuntimePaths;
#[cfg(feature = "server")]
pub use peer_transport::{
    initiator_handshake, split_session, NoiseReader, NoiseWriter, PeerTransport, Tailnet,
    PEER_NOISE_PATTERN, PEER_PROLOGUE,
};
pub use process_tree::JobObject;
#[cfg(feature = "server")]
pub use provider_update::{NpmInstallResult, NpmInstallRunner, ProcessNpmInstallRunner};
#[cfg(feature = "server")]
pub use screen::{
    render_ansi, Screen, ScreenSnapshot, SnapshotCursor, SnapshotCursorShape, MAX_TITLE_CHARS,
};
#[cfg(feature = "server")]
pub use server::{call_peer, dial_peer, run, DialError, ServerState};
#[cfg(feature = "server")]
pub use session::{
    write_test_pty_command, PtyCommand, COALESCE_FLUSH, COALESCE_MAX_BYTES,
    PENDING_OUTPUT_BUDGET_BYTES, PENDING_OUTPUT_BUDGET_FRAMES, SESSION_OS_SWEEP_INTERVAL,
    SESSION_SILENCE_THRESHOLD,
};
pub use spawn::{daemon_file_name, resolve_daemon_binary, spawn_daemon};
// Test support, not product (audit S5B-10): absent from a release build.
#[cfg(any(test, feature = "test-support"))]
pub use spawn::spawn_daemon_with_env;

/// How long an otherwise idle daemon waits before beginning shutdown.
///
/// Keep this long enough for a client reconnect caused by a transient app or
/// pipe interruption, while still releasing the daemon binary promptly after
/// the app has really gone away.
pub const IDLE_SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

#[cfg(windows)]
pub use security::{
    apply_current_user_dacl, current_user_sid, dacl_is_current_user_only, dacl_sddl_for_path,
    user_only_sddl,
};
#[cfg(windows)]
pub use transport::{connect_pipe, inspect_pipe_dacl};
