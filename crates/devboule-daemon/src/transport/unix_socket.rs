//! Unix-domain-socket transport: bind, connect, peer identity.
//!
//! Blocking `std` sockets. The accepted stream converts into the `File`
//! the rest of the daemon already speaks, so framing, handshake and
//! dispatch stay shared with the Windows pipe path.

use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::net::{UnixListener as StdListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(feature = "server")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::paths::RuntimePaths;
#[cfg(feature = "server")]
use crate::transport::Listener;

/// `sun_path` capacity on macOS, in bytes. Linux allows 108; the lower
/// bound keeps one check for both.
const MAX_SOCKET_PATH_BYTES: usize = 104;
const CONNECT_RETRY_PAUSE: Duration = Duration::from_millis(20);

/// The socket bind refuses to create: probe, stale handling and mode live
/// here so `transport::bind` only picks the platform.
#[cfg(feature = "server")]
pub struct UnixListener {
    listener: StdListener,
    socket_path: PathBuf,
    stop: Arc<AtomicBool>,
}

#[cfg(feature = "server")]
impl UnixListener {
    pub fn bind(paths: &RuntimePaths, stop: Arc<AtomicBool>) -> io::Result<Self> {
        check_path_length(&paths.socket_path)?;
        prepare_socket_dir(&paths.socket_path)?;
        probe_before_unlink(&paths.socket_path)?;
        let listener = StdListener::bind(&paths.socket_path)?;
        restrict_socket_permissions(&paths.socket_path)?;
        Ok(Self {
            listener,
            socket_path: paths.socket_path.clone(),
            stop,
        })
    }

    pub fn shutdown_handle(&self) -> ListenerShutdown {
        ListenerShutdown {
            stop: Arc::clone(&self.stop),
            socket_path: self.socket_path.clone(),
        }
    }

    fn accept_one(&self) -> io::Result<File> {
        let (stream, _) = self.listener.accept()?;
        let peer = peer_identity_from_fd(stream.as_raw_fd())?;
        if !peer_is_current(&peer) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing daemon connection from uid {}, owned by {}",
                    peer.user,
                    current_uid()
                ),
            ));
        }
        Ok(stream_to_file(stream))
    }
}

#[cfg(feature = "server")]
impl Listener for UnixListener {
    type Stream = File;

    fn accept(&mut self) -> io::Result<Self::Stream> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "listener shutting down",
            ));
        }
        self.accept_one()
    }

    fn shutdown(&mut self) -> io::Result<()> {
        self.shutdown_handle().shutdown();
        let _ = std::fs::remove_file(&self.socket_path);
        Ok(())
    }
}

#[cfg(feature = "server")]
impl Drop for UnixListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Wakes a thread blocked in `accept`, mirroring the pipe shutdown handle:
/// a loopback connect unblocks the call without carrying a session.
#[cfg(feature = "server")]
#[derive(Clone)]
pub struct ListenerShutdown {
    stop: Arc<AtomicBool>,
    socket_path: PathBuf,
}

#[cfg(feature = "server")]
impl ListenerShutdown {
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.socket_path);
    }
}

pub fn connect(paths: &RuntimePaths) -> io::Result<File> {
    check_path_length(&paths.socket_path)?;
    Ok(stream_to_file(UnixStream::connect(&paths.socket_path)?))
}

/// [`connect`] that keeps trying until about `budget` runs out, never
/// sooner than one try: the daemon may be bound but not yet listening.
pub fn connect_within(paths: &RuntimePaths, budget: Duration) -> io::Result<File> {
    check_path_length(&paths.socket_path)?;
    let deadline = Instant::now() + budget;
    loop {
        match UnixStream::connect(&paths.socket_path) {
            Ok(stream) => return Ok(stream_to_file(stream)),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(error);
                }
                std::thread::sleep(CONNECT_RETRY_PAUSE);
            }
        }
    }
}

/// Kernel identity of the peer holding this stream, the way
/// `transport::peer_identity` reports it on Windows: numeric uid as the
/// user, kernel peer pid for diagnostics. Slice 4 plugs this into the
/// connection's owner check; `accept` already refuses another uid above.
#[cfg(feature = "server")]
pub fn peer_identity(file: &File) -> io::Result<crate::agent_report::PeerIdentity> {
    peer_identity_from_fd(file.as_raw_fd())
}

fn check_path_length(path: &Path) -> io::Result<()> {
    let bytes = path.as_os_str().as_bytes().len();
    if bytes > MAX_SOCKET_PATH_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unix socket path is {bytes} bytes, over the {MAX_SOCKET_PATH_BYTES}-byte limit: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// The socket's parent, created private: the daemon's per-user runtime dir
/// must not stay at whatever the umask left behind.
fn prepare_socket_dir(socket_path: &Path) -> io::Result<()> {
    let Some(parent) = socket_path.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("socket path has no parent: {}", socket_path.display()),
        ));
    };
    std::fs::create_dir_all(parent)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
}

fn restrict_socket_permissions(socket_path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
}

/// Stale-vs-live probe: connect first. A listener answers → someone owns
/// the path and binding must fail. Refused or missing → nobody does, so
/// the leftover file may go. Any other error is ambiguous and refuses.
fn probe_before_unlink(socket_path: &Path) -> io::Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }
    match UnixStream::connect(socket_path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "devboule daemon is already running (socket busy at {})",
                socket_path.display()
            ),
        )),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            if let Err(remove) = std::fs::remove_file(socket_path) {
                if remove.kind() != io::ErrorKind::NotFound {
                    return Err(remove);
                }
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Owned descriptor transfer into the shared stream type: the `File` closes
/// the socket on drop, and no duplicated descriptor is left inheritable.
fn stream_to_file(stream: UnixStream) -> File {
    // SAFETY: `into_raw_fd` hands over the one live owner of this socket;
    // `File` becomes that owner and closes it on drop.
    unsafe { File::from_raw_fd(stream.into_raw_fd()) }
}

fn current_uid() -> u32 {
    // SAFETY: getuid takes no arguments and cannot fail.
    unsafe { libc::getuid() }
}

/// Same-user check behind the accept refusal: the kernel uid as a string
/// is the whole comparison, so a client-supplied name never grants access.
pub fn peer_is_current(peer: &crate::agent_report::PeerIdentity) -> bool {
    peer.user == current_uid().to_string()
}

fn peer_identity_from_fd(
    fd: std::os::unix::io::RawFd,
) -> io::Result<crate::agent_report::PeerIdentity> {
    let (uid, pid) = peer_credentials(fd)?;
    Ok(crate::agent_report::PeerIdentity {
        user: uid.to_string(),
        pid,
    })
}

#[cfg(target_os = "macos")]
fn peer_credentials(fd: std::os::unix::io::RawFd) -> io::Result<(u32, u32)> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: fd is an open socket of this process; both out-pointers are
    // live locals of the right type.
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: same fd; LOCAL_PEERPID answers with a pid_t through the
    // pointer for exactly this length.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            &mut pid as *mut _ as *mut libc::c_void,
            &mut len,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((uid as u32, pid as u32))
}

#[cfg(target_os = "linux")]
fn peer_credentials(fd: std::os::unix::io::RawFd) -> io::Result<(u32, u32)> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: same fd; SO_PEERCRED answers with a ucred for this length.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((cred.uid, cred.pid as u32))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn peer_credentials(fd: std::os::unix::io::RawFd) -> io::Result<(u32, u32)> {
    let _ = fd;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "peer credentials are implemented for macOS and Linux only",
    ))
}
