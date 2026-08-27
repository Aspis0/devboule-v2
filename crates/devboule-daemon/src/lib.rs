//! Devboule daemon: single-instance lock, named pipe, versioned handshake.
//!
//! Sessions still run in-process in the Tauri app (M3b moves them here).
//! This crate is a library so the app can spawn, connect, and handshake
//! using the same code the integration tests use.

use std::time::Duration;

mod client;
mod error;
mod framing;
#[cfg(feature = "server")]
mod idempotency;
#[cfg(feature = "server")]
mod lock;
mod paths;
#[cfg(feature = "server")]
mod server;
mod spawn;
mod transport;

#[cfg(windows)]
mod security;

pub use client::{connect, connect_or_spawn, handshake, test_owner, DaemonClient};
pub use error::DaemonError;
pub use paths::RuntimePaths;
#[cfg(feature = "server")]
pub use server::run;
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
