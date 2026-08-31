//! One-shot named pipe listener. The backend binds; the host connects.
//!
//! Copied from `devboule-daemon` named-pipe accept: overlapped connect,
//! current-user DACL, `FILE_FLAG_FIRST_PIPE_INSTANCE`. Not the daemon's
//! accept loop — a plugin backend serves one host connection.

use std::fs::File;
use std::io;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::io::{FromRawHandle, RawHandle};

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
use windows_sys::Win32::Security::{
    SECURITY_ATTRIBUTES, PSECURITY_DESCRIPTOR,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
};
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
#[cfg(windows)]
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

const PIPE_BUFFER: u32 = 64 * 1024;

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
            return Err(io::Error::last_os_error());
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

/// Bind `pipe_name` and wait for one client. The host retries connect until
/// this returns; a timeout here is a spawn failure, not a hang.
pub fn bind_and_accept(pipe_name: &str, timeout: Duration) -> io::Result<File> {
    #[cfg(windows)]
    {
        bind_and_accept_windows(pipe_name, timeout)
    }
    #[cfg(not(windows))]
    {
        let _ = (pipe_name, timeout);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "plugin named pipes are Windows-only",
        ))
    }
}

#[cfg(windows)]
fn bind_and_accept_windows(pipe_name: &str, timeout: Duration) -> io::Result<File> {
    let security = PipeSecurity::current_user_only()?;
    let name = wide(pipe_name);
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
        return Err(io::Error::last_os_error());
    }
    unsafe {
        SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
    }

    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        unsafe {
            CloseHandle(handle);
        }
        return Err(io::Error::last_os_error());
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
                    "plugin backend timed out waiting for the host",
                ));
            }
            if wait != WAIT_OBJECT_0 {
                unsafe {
                    CloseHandle(event);
                    CloseHandle(handle);
                }
                return Err(io::Error::last_os_error());
            }
            let mut transferred = 0u32;
            let completed =
                unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, 1) };
            if completed == 0 {
                let error = io::Error::last_os_error();
                unsafe {
                    CloseHandle(event);
                    CloseHandle(handle);
                }
                return Err(error);
            }
        } else if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
            unsafe {
                CloseHandle(event);
                CloseHandle(handle);
            }
            return Err(err);
        }
    }
    unsafe {
        CloseHandle(event);
    }
    drop(security);
    Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
}
