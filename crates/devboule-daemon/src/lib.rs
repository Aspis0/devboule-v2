//! Devboule daemon: single-instance lock, named pipe, versioned handshake,
//! and (behind the `server` feature) ownership of PTY sessions.

use std::time::Duration;

#[cfg(feature = "server")]
mod acp_tool_content;
#[cfg(feature = "server")]
mod acp_view;
#[cfg(feature = "server")]
mod agent_activity;
#[cfg(feature = "server")]
mod agent_env;
#[cfg(feature = "server")]
mod agent_image;
#[cfg(feature = "server")]
mod agent_profiles;
#[cfg(feature = "server")]
mod agent_report;
#[cfg(feature = "server")]
mod atomic;
#[cfg(feature = "server")]
mod attachment_store;
#[cfg(feature = "server")]
mod attachment_upload;
#[cfg(feature = "server")]
mod browser_affinity;
#[cfg(feature = "server")]
mod browser_broker;
#[cfg(feature = "server")]
mod browser_registry;
#[cfg(feature = "server")]
mod browser_tool_title;
#[cfg(feature = "server")]
mod ci_gh;
#[cfg(feature = "server")]
mod ci_pages;
#[cfg(feature = "server")]
mod ci_pass;
#[cfg(feature = "server")]
mod ci_summary;
#[cfg(all(test, feature = "server"))]
mod ci_test_support;
#[cfg(feature = "server")]
mod ci_wake;
#[cfg(feature = "server")]
mod ci_watch;
#[cfg(feature = "server")]
mod ci_watch_quota;
#[cfg(feature = "server")]
mod ci_watch_store;
#[cfg(feature = "server")]
mod claude_abort;
#[cfg(feature = "server")]
mod claude_catalog;
#[cfg(feature = "server")]
mod claude_cost_baseline;
#[cfg(feature = "server")]
mod claude_task_state;
#[cfg(feature = "server")]
mod claude_view;
mod client;
#[cfg(feature = "server")]
mod codex_command_catalog;
#[cfg(feature = "server")]
mod codex_commands;
#[cfg(feature = "server")]
mod codex_compaction;
#[cfg(feature = "server")]
mod codex_goals;
#[cfg(feature = "server")]
mod codex_plan_marks;
#[cfg(feature = "server")]
mod codex_prompt_expand;
#[cfg(feature = "server")]
mod codex_view;
#[cfg(feature = "server")]
mod config_read;
mod daemon_log;
mod daemon_record;
#[cfg(feature = "server")]
mod delegation_store;
#[cfg(feature = "server")]
mod device_identity;
#[cfg(feature = "server")]
mod device_recovery;
mod diagnostics;
#[cfg(feature = "server")]
mod egress_client;
#[cfg(feature = "server")]
mod egress_policy;
mod error;
#[cfg(feature = "server")]
mod file_collisions;
mod framing;
#[cfg(feature = "server")]
mod git;
#[cfg(feature = "server")]
mod idempotency;
#[cfg(feature = "server")]
mod journal;
#[cfg(feature = "server")]
mod journal_lookback;
#[cfg(feature = "server")]
mod journal_resume;
mod lock;
#[cfg(all(windows, feature = "server"))]
mod log_pipeline;
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
mod origin_chain;
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
mod pi_task_adapters;
#[cfg(feature = "server")]
mod pi_view;
#[cfg(feature = "server")]
mod plan_text;
#[cfg(feature = "server")]
mod plan_usage_cache;
#[cfg(all(test, feature = "server"))]
mod portable_pty_tests;
#[cfg(all(test, unix, feature = "server"))]
mod portable_pty_unix_tests;
#[cfg(feature = "server")]
mod process_argv_redact;
#[cfg(feature = "server")]
mod process_index;
#[cfg(feature = "server")]
mod process_plan;
#[cfg(feature = "server")]
mod process_terminate;
mod process_tree;
#[cfg(feature = "server")]
mod profile_delivery;
#[cfg(feature = "server")]
mod provider_auth;
pub mod provider_catalog;
#[cfg(feature = "server")]
mod provider_feature_probe;
#[cfg(feature = "server")]
mod provider_features;
#[cfg(feature = "server")]
mod provider_switches;
#[cfg(feature = "server")]
mod provider_update;
#[cfg(feature = "server")]
mod provider_vocabulary;
mod quota_key;
mod quota_live;
mod quota_opencode_go;
mod quota_poller;
mod quota_source;
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
mod session_tasks;
#[cfg(feature = "server")]
mod shell_unwrap;
mod spawn;
#[cfg(feature = "server")]
mod tailscale_localapi;
#[cfg(test)]
mod temp_dir_guard_tests;
#[cfg(test)]
mod test_dirs_link_tests;
// The temp-dir helper the integration tests call: a test API, absent from a
// release build (audit S5B-10).
#[cfg(any(test, feature = "test-support"))]
pub mod test_dirs;
#[cfg(all(test, feature = "server"))]
mod test_support;
#[cfg(feature = "server")]
mod text_cap;
#[cfg(feature = "server")]
mod tool_paths;
#[cfg(feature = "server")]
mod tool_policy;
mod transport;
#[cfg(unix)]
mod unix_modes;
#[cfg(feature = "server")]
mod untrusted_frame;
#[cfg(feature = "server")]
mod usage_cost;
#[cfg(feature = "server")]
mod user_providers;
// Only the server and the Windows PATH snapshot call plain_path: everywhere
// else (the client-only macOS/Linux build) the module would warn as dead.
#[cfg_attr(not(any(windows, feature = "server")), allow(dead_code))]
mod verbatim_path;
#[cfg(feature = "server")]
mod visible_text;
#[cfg(windows)]
mod windows_path_env;
#[cfg(windows)]
mod windows_registry_path;
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
mod workspace_git_log;
#[cfg(feature = "server")]
mod workspace_git_status;
#[cfg(feature = "server")]
mod workspace_git_support;
#[cfg(feature = "server")]
mod workspace_git_write;
#[cfg(feature = "server")]
mod worktree;
#[cfg(feature = "server")]
mod write_evidence;

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
/// The Unix restart's verified fallback kill; shared with the Unix end-to-end
/// test's cleanup guard.
#[cfg(unix)]
pub use client::kill_verified_daemon;
pub use client::{
    connect, connect_or_spawn, connect_within, handshake, test_owner, DaemonClient,
    DelegationChangedHandler, EventHandler, RemoteHostStatusHandler, SessionResetHandler,
    SessionStateHandler, ShutdownAnswer,
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
// The daemon binary's entrypoint into the log sink; the binary is only built
// with the server feature, so anything less would be dead code in the
// client-only build the app produces.
#[cfg(all(windows, feature = "server"))]
pub use daemon_log::take_over_stderr;
// The binary's exit paths flush the log the same way run_windows does.
#[cfg(all(windows, feature = "server"))]
pub use log_pipeline::shutdown_log;
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
/// The owner name Unix clients present and the server derives. Exported
/// for the app's client hello; the daemon side uses it through transport.
#[cfg(unix)]
pub use transport::local_uid;
#[cfg(windows)]
pub use transport::{connect_pipe, inspect_pipe_dacl};
