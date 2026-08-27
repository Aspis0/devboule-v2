//! Byte transport. The protocol crate never sees this. A Unix socket can
//! implement [`Listener`] later without touching wire types.

use std::fs::File;
use std::io::{self, Read, Write};
#[cfg(feature = "server")]
use std::sync::atomic::AtomicBool;
#[cfg(feature = "server")]
use std::sync::Arc;

use crate::paths::RuntimePaths;

#[cfg(windows)]
mod windows_pipe;
#[cfg(windows)]
pub use windows_pipe::{connect_pipe, inspect_pipe_dacl};
#[cfg(all(windows, feature = "server"))]
pub use windows_pipe::{peer_owner, ListenerShutdown, NamedPipeListener};

#[cfg(all(not(windows), feature = "server"))]
#[derive(Clone)]
pub struct ListenerShutdown;

#[cfg(all(not(windows), feature = "server"))]
impl ListenerShutdown {
    pub fn shutdown(&self) {}
}

/// Byte stream bound used by a future Unix-socket listener. Named pipes
/// currently yield `std::fs::File`, which already implements it.
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
    #[cfg(not(windows))]
    {
        let _ = (paths, stop);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "devboule-daemon M3a targets Windows only",
        ))
    }
}

pub fn connect(paths: &RuntimePaths) -> io::Result<File> {
    #[cfg(windows)]
    {
        connect_pipe(&paths.pipe_name)
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
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
    #[cfg(not(windows))]
    Unsupported,
}

#[cfg(feature = "server")]
impl Listener for BoundListener {
    type Stream = File;

    fn accept(&mut self) -> io::Result<Self::Stream> {
        match self {
            #[cfg(windows)]
            Self::Windows(inner) => inner.accept(),
            #[cfg(not(windows))]
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
            #[cfg(not(windows))]
            Self::Unsupported => Ok(()),
        }
    }
}
