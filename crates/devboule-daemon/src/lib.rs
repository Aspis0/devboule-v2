//! Devboule daemon: single-instance lock, named pipe, versioned handshake,
//! and (behind the `server` feature) ownership of PTY sessions.

use std::time::Duration;

#[cfg(feature = "server")]
mod atomic;
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
#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
mod session;
mod spawn;
mod transport;

#[cfg(windows)]
mod security;

#[cfg(feature = "server")]
pub use atomic::atomic_write;
pub use client::{connect, connect_or_spawn, handshake, test_owner, DaemonClient, EventHandler};
pub use error::DaemonError;
#[cfg(feature = "server")]
pub use journal::{
    Journal, JournalError, JournalLimits, JOURNAL_MAX_AGE_MS, JOURNAL_MAX_BYTES,
    JOURNAL_MAX_SESSIONS, JOURNAL_QUEUE_CAP, JOURNAL_SCHEMA_VERSION, JOURNAL_SESSION_MAX_BYTES,
    SNAPSHOT_EVERY_BYTES,
};
pub use paths::RuntimePaths;
#[cfg(feature = "server")]
pub use server::run;
#[cfg(feature = "server")]
pub use session::{
    write_test_pty_command, PtyCommand, COALESCE_FLUSH, COALESCE_MAX_BYTES, RING_CAPACITY,
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
pub use transport::inspect_pipe_dacl;
