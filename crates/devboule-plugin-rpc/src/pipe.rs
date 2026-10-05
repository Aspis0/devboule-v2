//! One-shot host↔backend channel. Windows: the backend binds a named pipe,
//! the host connects. Unix: the host creates a socketpair and hands the
//! child end to the backend as an inherited fd (no pathname, so no
//! path-length or stale-socket problem), and the peer on each end is checked
//! before the handshake.
//!
//! The Windows half is copied from `devboule-daemon` named-pipe accept:
//! overlapped connect, current-user DACL, `FILE_FLAG_FIRST_PIPE_INSTANCE`.
//! Not the daemon's accept loop — a plugin backend serves one host connection.

use std::fs::File;
use std::io;
use std::time::Duration;

#[cfg(unix)]
use std::os::raw::c_int;
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};

#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, SetHandleInformation, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED,
    HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
#[cfg(windows)]
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
#[cfg(windows)]
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
};
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
#[cfg(windows)]
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

#[cfg(windows)]
const PIPE_BUFFER: u32 = 64 * 1024;

/// How long one framed read may block before it surfaces as `WouldBlock`.
/// The framing checks its deadline only between reads, so this tick is what
/// makes a deadline reachable on a Unix socket at all.
#[cfg(unix)]
const SOCKET_READ_TICK: Duration = Duration::from_millis(100);

fn startup_context(step: &str, error: io::Error) -> io::Error {
    let detail = error.to_string();
    let detail = match error.raw_os_error() {
        Some(code) if !detail.contains(&format!("os error {code}")) => {
            format!("{detail} (os error {code})")
        }
        _ => detail,
    };
    io::Error::new(error.kind(), format!("{step}: {detail}"))
}

#[cfg(windows)]
struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
}

#[cfg(windows)]
impl PipeSecurity {
    fn current_user_only() -> io::Result<Self> {
        let sid = devboule_daemon::current_user_sid()?;
        let sddl = devboule_daemon::user_only_sddl(&sid);
        let wide = wide(&sddl);
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || descriptor.is_null() {
            return Err(startup_context(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW",
                io::Error::last_os_error(),
            ));
        }
        Ok(Self { descriptor })
    }
}

#[cfg(windows)]
impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            unsafe {
                LocalFree(self.descriptor as _);
            }
            self.descriptor = std::ptr::null_mut();
        }
    }
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Open the one host connection. `endpoint` names the channel: a Windows
/// pipe name, or — on Unix — the inherited fd number in this process. The
/// timeout applies to the Windows accept; the Unix channel is connected from
/// birth, so there is nothing to wait for.
#[cfg(windows)]
pub fn open_host_channel(endpoint: &str, timeout: Duration) -> io::Result<File> {
    open_host_channel_windows(endpoint, timeout)
}

/// Compare the kernel-reported peer PID with the PID the other side expected.
/// Keeping this comparison pure makes the security rule testable without
/// pretending a fabricated PID came from Windows.
pub fn peer_pid_matches(actual: u32, expected: u32) -> io::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("peer PID {actual} is not expected PID {expected}"),
        ))
    }
}

/// Same rule for the uid half of the peer check.
#[cfg(unix)]
pub fn peer_uid_matches(actual: u32, expected: u32) -> io::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("peer uid {actual} is not uid {expected}"),
        ))
    }
}

#[cfg(windows)]
pub fn verify_pipe_client_pid(file: &File, expected: u32) -> io::Result<()> {
    let mut actual = 0u32;
    let ok = unsafe { GetNamedPipeClientProcessId(file.as_raw_handle() as _, &mut actual) };
    if ok == 0 {
        return Err(startup_context(
            "GetNamedPipeClientProcessId",
            io::Error::last_os_error(),
        ));
    }
    peer_pid_matches(actual, expected)
        .map_err(|error| startup_context("verify_pipe_client_pid", error))
}

#[cfg(windows)]
pub fn verify_pipe_server_pid(file: &File, expected: u32) -> io::Result<()> {
    let mut actual = 0u32;
    let ok = unsafe { GetNamedPipeServerProcessId(file.as_raw_handle() as _, &mut actual) };
    if ok == 0 {
        return Err(startup_context(
            "GetNamedPipeServerProcessId",
            io::Error::last_os_error(),
        ));
    }
    peer_pid_matches(actual, expected)
        .map_err(|error| startup_context("verify_pipe_server_pid", error))
}

#[cfg(unix)]
pub fn verify_pipe_client_pid(file: &File, expected: u32) -> io::Result<()> {
    verify_peer(file, expected).map_err(|error| startup_context("verify_pipe_client_pid", error))
}

#[cfg(unix)]
pub fn verify_pipe_server_pid(file: &File, expected: u32) -> io::Result<()> {
    verify_peer(file, expected).map_err(|error| startup_context("verify_pipe_server_pid", error))
}

/// Same uid and same peer PID in one rule. The uid comes from the credentials
/// frozen on the socket at creation; the PID is the peer socket's last
/// accessor, so a descriptor handed to another process fails this check.
#[cfg(unix)]
fn verify_peer(file: &File, expected_pid: u32) -> io::Result<()> {
    peer_uid_matches(peer_uid(file)?, unsafe { libc::geteuid() } as u32)?;
    peer_pid_matches(peer_pid(file)?, expected_pid)
}

/// The peer socket's last accessor. XNU re-stamps it on every send, receive,
/// accept or poll, which is how the host learns that the spawned child — not
/// this process, and not a descriptor thief — now holds the other end.
#[cfg(unix)]
pub fn peer_pid(file: &File) -> io::Result<u32> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            file.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            &mut pid as *mut libc::pid_t as *mut libc::c_void,
            &mut length,
        )
    };
    if result != 0 {
        return Err(startup_context(
            "getsockopt(LOCAL_PEERPID)",
            io::Error::last_os_error(),
        ));
    }
    if length as usize != std::mem::size_of::<libc::pid_t>() || pid < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("LOCAL_PEERPID returned {pid} (length {length})"),
        ));
    }
    Ok(pid as u32)
}

#[cfg(unix)]
pub fn peer_uid(file: &File) -> io::Result<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    if unsafe { libc::getpeereid(file.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(startup_context("getpeereid", io::Error::last_os_error()));
    }
    Ok(uid as u32)
}

/// A connected socketpair for one plugin spawn: the host end (already set up
/// for framed reads) and the child end to hand over. Both ends close on exec
/// so only the fd this spawn deliberately inherits survives into the child.
#[cfg(unix)]
pub(crate) fn socketpair_channel() -> io::Result<(File, std::os::unix::net::UnixStream)> {
    let (host, child) = std::os::unix::net::UnixStream::pair()?;
    set_close_on_exec(host.as_raw_fd())?;
    set_close_on_exec(child.as_raw_fd())?;
    set_receive_timeout(host.as_raw_fd(), SOCKET_READ_TICK)?;
    set_receive_timeout(child.as_raw_fd(), SOCKET_READ_TICK)?;
    Ok((unsafe { File::from_raw_fd(host.into_raw_fd()) }, child))
}

/// Cap one blocking read so a framed deadline can expire mid-wait.
#[cfg(unix)]
pub(crate) fn set_receive_timeout(fd: c_int, tick: Duration) -> io::Result<()> {
    let timeval = libc::timeval {
        tv_sec: tick.as_secs() as libc::time_t,
        tv_usec: tick.subsec_micros() as libc::suseconds_t,
    };
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &timeval as *const libc::timeval as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
    if result != 0 {
        return Err(startup_context(
            "setsockopt(SO_RCVTIMEO)",
            io::Error::last_os_error(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn set_close_on_exec(fd: c_int) -> io::Result<()> {
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return Err(startup_context(
            "fcntl(FD_CLOEXEC)",
            io::Error::last_os_error(),
        ));
    }
    Ok(())
}

/// Adopt the already-connected fd the host put in `endpoint`.
#[cfg(unix)]
pub fn open_host_channel(endpoint: &str, timeout: Duration) -> io::Result<File> {
    let _ = timeout;
    let fd: c_int = endpoint.parse().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("channel endpoint {endpoint:?} is not an inherited fd number: {error}"),
        )
    })?;
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(startup_context("fstat", io::Error::last_os_error()));
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("channel fd {fd} is not a socket"),
        ));
    }
    set_receive_timeout(fd, SOCKET_READ_TICK)?;
    // The host cleared close-on-exec to hand this fd over; restore it the
    // moment it is adopted, so the backend's own children never inherit
    // the live channel. Fail closed, closing the adoption with it.
    let file = unsafe { File::from_raw_fd(fd) };
    if let Err(error) = set_close_on_exec(fd) {
        drop(file);
        return Err(error);
    }
    Ok(file)
}

#[cfg(windows)]
fn open_host_channel_windows(endpoint: &str, timeout: Duration) -> io::Result<File> {
    let security = PipeSecurity::current_user_only()?;
    let name = wide(endpoint);
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.descriptor,
        bInheritHandle: 0,
    };
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            PIPE_BUFFER,
            PIPE_BUFFER,
            1000,
            &sa,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let step = format!("CreateNamedPipeW({endpoint})");
        return Err(startup_context(&step, io::Error::last_os_error()));
    }
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
        let error = startup_context("SetHandleInformation", io::Error::last_os_error());
        unsafe {
            CloseHandle(handle);
        }
        return Err(error);
    }

    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        let error = startup_context("CreateEventW", io::Error::last_os_error());
        unsafe {
            CloseHandle(handle);
        }
        return Err(error);
    }
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = event;
    let connected = unsafe { ConnectNamedPipe(handle, &mut overlapped) };
    if connected == 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
            let millis = timeout.as_millis().min(u32::MAX as u128) as u32;
            let wait = unsafe { WaitForSingleObject(event, millis.max(1)) };
            if wait == WAIT_TIMEOUT {
                unsafe {
                    let _ = CancelIoEx(handle, &overlapped);
                    let mut transferred = 0u32;
                    let _ = GetOverlappedResult(handle, &overlapped, &mut transferred, 1);
                    CloseHandle(event);
                    CloseHandle(handle);
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "ConnectNamedPipe/WaitForSingleObject: plugin backend timed out waiting for the host",
                ));
            }
            if wait != WAIT_OBJECT_0 {
                let error = startup_context("WaitForSingleObject", io::Error::last_os_error());
                unsafe {
                    CloseHandle(event);
                    CloseHandle(handle);
                }
                return Err(error);
            }
            let mut transferred = 0u32;
            let completed =
                unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, 1) };
            if completed == 0 {
                let error = startup_context("GetOverlappedResult", io::Error::last_os_error());
                unsafe {
                    CloseHandle(event);
                    CloseHandle(handle);
                }
                return Err(error);
            }
        } else if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
            let error = startup_context("ConnectNamedPipe", err);
            unsafe {
                CloseHandle(event);
                CloseHandle(handle);
            }
            return Err(error);
        }
    }
    unsafe {
        CloseHandle(event);
    }
    drop(security);
    Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_errors_name_the_failing_windows_call_and_keep_the_os_code() {
        let error = startup_context("CreateNamedPipeW", io::Error::from_raw_os_error(123));
        assert!(error.to_string().starts_with("CreateNamedPipeW: "));
        assert!(error.to_string().contains("os error 123"));
    }

    #[test]
    fn a_known_wrong_peer_pid_is_rejected() {
        let expected = std::process::id();
        let wrong = expected.wrapping_add(1).max(1);
        let error = peer_pid_matches(wrong, expected).expect_err("wrong peer");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("not expected PID"));
    }

    #[cfg(unix)]
    #[test]
    fn a_wrong_peer_uid_is_refused() {
        let mine = unsafe { libc::geteuid() } as u32;
        let foreign = mine.wrapping_add(1);
        let error = peer_uid_matches(foreign, mine).expect_err("foreign uid");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("is not uid"));
    }
}
