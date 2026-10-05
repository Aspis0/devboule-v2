//! Byte transport. The protocol crate never sees this. Windows speaks a
//! named pipe, Unix a domain socket; both surface as `File` streams.

use std::fs::File;
use std::io::{self, Read, Write};
#[cfg(feature = "server")]
use std::sync::atomic::AtomicBool;
#[cfg(feature = "server")]
use std::sync::Arc;
use std::time::Duration;

use crate::paths::RuntimePaths;

#[cfg(unix)]
mod unix_socket;
#[cfg(all(test, unix, feature = "server"))]
mod unix_socket_tests;
#[cfg(windows)]
mod windows_pipe;
#[cfg(all(unix, feature = "server"))]
pub use unix_socket::{peer_identity, ListenerShutdown, UnixListener};
#[cfg(windows)]
pub use windows_pipe::{
    connect_pipe, connect_pipe_within, inspect_pipe_dacl, server_process_id,
    terminate_server_process_if_identity_matches,
};
#[cfg(all(windows, feature = "server"))]
pub use windows_pipe::{peer_identity, ListenerShutdown, NamedPipeListener};

#[cfg(all(not(windows), not(unix), feature = "server"))]
#[derive(Clone)]
pub struct ListenerShutdown;

#[cfg(all(not(windows), not(unix), feature = "server"))]
impl ListenerShutdown {
    pub fn shutdown(&self) {}
}

/// Byte stream bound for listeners that do not yield files. Named pipes
/// and Unix sockets both convert into `std::fs::File`, which implements it.
#[allow(dead_code)]
pub trait ByteStream: Read + Write + Send {}
#[allow(dead_code)]
impl<T> ByteStream for T where T: Read + Write + Send {}

/// Accept loop. `shutdown` must unblock a thread stuck in [`Listener::accept`].
#[cfg(feature = "server")]
pub trait Listener {
    type Stream: Read + Write + Send + 'static;
    fn accept(&mut self) -> io::Result<Self::Stream>;
    fn shutdown(&mut self) -> io::Result<()>;
}

#[cfg(feature = "server")]
pub fn bind(
    paths: &RuntimePaths,
    stop: Arc<AtomicBool>,
) -> io::Result<(BoundListener, ListenerShutdown)> {
    #[cfg(windows)]
    {
        let inner = NamedPipeListener::bind(paths, stop)?;
        let shutdown = inner.shutdown_handle();
        Ok((BoundListener::Windows(inner), shutdown))
    }
    #[cfg(unix)]
    {
        let inner = UnixListener::bind(paths, stop)?;
        let shutdown = inner.shutdown_handle();
        Ok((BoundListener::Unix(inner), shutdown))
    }
    #[cfg(all(not(windows), not(unix)))]
    {
        let _ = (paths, stop);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "devboule-daemon M3a targets Windows only",
        ))
    }
}

/// This process's kernel uid: the owner name Unix clients present and the
/// server derives. Numeric, stable across restarts, and never shaped like
/// a Windows SID, so the two namespaces cannot collide.
#[cfg(unix)]
pub fn local_uid() -> u32 {
    // SAFETY: getuid takes no arguments and cannot fail.
    unsafe { libc::getuid() }
}

pub fn connect(paths: &RuntimePaths) -> io::Result<File> {
    #[cfg(windows)]
    {
        connect_pipe(&paths.pipe_name)
    }
    #[cfg(unix)]
    {
        unix_socket::connect(paths)
    }
    #[cfg(all(not(windows), not(unix)))]
    {
        let _ = paths;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "devboule-daemon M3a targets Windows only",
        ))
    }
}

/// [`connect`] that stops waiting on a busy pipe after about `budget`.
pub fn connect_within(paths: &RuntimePaths, budget: Duration) -> io::Result<File> {
    #[cfg(windows)]
    {
        connect_pipe_within(&paths.pipe_name, budget)
    }
    #[cfg(unix)]
    {
        unix_socket::connect_within(paths, budget)
    }
    #[cfg(all(not(windows), not(unix)))]
    {
        let _ = (paths, budget);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "devboule-daemon M3a targets Windows only",
        ))
    }
}

#[cfg(feature = "server")]
pub enum BoundListener {
    #[cfg(windows)]
    Windows(NamedPipeListener),
    #[cfg(unix)]
    Unix(UnixListener),
    #[cfg(all(not(windows), not(unix)))]
    Unsupported,
}

#[cfg(feature = "server")]
impl Listener for BoundListener {
    type Stream = File;

    fn accept(&mut self) -> io::Result<Self::Stream> {
        match self {
            #[cfg(windows)]
            Self::Windows(inner) => inner.accept(),
            #[cfg(unix)]
            Self::Unix(inner) => inner.accept(),
            #[cfg(all(not(windows), not(unix)))]
            Self::Unsupported => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "devboule-daemon M3a targets Windows only",
            )),
        }
    }

    fn shutdown(&mut self) -> io::Result<()> {
        match self {
            #[cfg(windows)]
            Self::Windows(inner) => inner.shutdown(),
            #[cfg(unix)]
            Self::Unix(inner) => inner.shutdown(),
            #[cfg(all(not(windows), not(unix)))]
            Self::Unsupported => Ok(()),
        }
    }
}
