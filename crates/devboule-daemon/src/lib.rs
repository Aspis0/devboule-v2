//! Devboule daemon: single-instance lock, named pipe, versioned handshake.
//!
//! Sessions still run in-process in the Tauri app (M3b moves them here).
//! This crate is a library so the app can spawn, connect, and handshake
//! using the same code the integration tests use.

mod client;
mod error;
mod framing;
mod idempotency;
mod lock;
mod paths;
mod server;
mod spawn;
mod transport;

#[cfg(windows)]
mod security;

pub use client::{connect, connect_or_spawn, handshake, test_owner, DaemonClient};
pub use error::DaemonError;
pub use paths::RuntimePaths;
pub use server::run;
pub use spawn::{daemon_file_name, resolve_daemon_binary, spawn_daemon};

#[cfg(windows)]
pub use security::{current_user_sid, dacl_is_current_user_only, user_only_sddl};
#[cfg(windows)]
pub use transport::inspect_pipe_dacl;
