use std::fs::File;
use std::io;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::ptr;
#[cfg(feature = "server")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "server")]
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "server")]
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED};
use windows_sys::Win32::Foundation::{
    SetHandleInformation, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE,
    HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
};
#[cfg(feature = "server")]
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT,
};
#[cfg(feature = "server")]
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
#[cfg(feature = "server")]
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};

#[cfg(feature = "server")]
use crate::paths::RuntimePaths;
#[cfg(feature = "server")]
use crate::security::PipeSecurity;
use crate::security::{self, last_os_error, wide};
#[cfg(feature = "server")]
use crate::transport::Listener;

#[cfg(feature = "server")]
const PIPE_BUFFER: u32 = 64 * 1024;
#[cfg(feature = "server")]
const MAX_INSTANCES: u32 = 16;
const WAIT_MS: u32 = 1000;

#[cfg(feature = "server")]
#[derive(Clone)]
pub struct ListenerShutdown {
    stop: Arc<AtomicBool>,
    pipe_name: String,
}

#[cfg(feature = "server")]
impl ListenerShutdown {
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // A one-shot connect wakes ConnectNamedPipe without making process
        // shutdown wait through the normal client retry loop. It covers the
        // small window where accept has not yet observed the stop flag.
        let _ = connect_pipe_once(&self.pipe_name);
    }
}

#[cfg(feature = "server")]
pub struct NamedPipeListener {
    pipe_name: String,
    security: PipeSecurity,
    stop: Arc<AtomicBool>,
    first: bool,
}

#[cfg(feature = "server")]
impl NamedPipeListener {
    pub fn bind(paths: &RuntimePaths, stop: Arc<AtomicBool>) -> io::Result<Self> {
        let security = PipeSecurity::current_user_only()?;
        Ok(Self {
            pipe_name: paths.pipe_name.clone(),
            security,
            stop,
            first: true,
        })
    }

    fn create_instance(&mut self) -> io::Result<HANDLE> {
        let name = wide(&self.pipe_name);
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if self.first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.security.as_ptr(),
            bInheritHandle: 0,
        };
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                MAX_INSTANCES,
                PIPE_BUFFER,
                PIPE_BUFFER,
                WAIT_MS,
                &sa,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_os_error());
        }
        unsafe {
            SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
        }
        self.first = false;
        Ok(handle)
    }

    pub fn shutdown_handle(&self) -> ListenerShutdown {
        ListenerShutdown {
            stop: Arc::clone(&self.stop),
            pipe_name: self.pipe_name.clone(),
        }
    }
}

#[cfg(feature = "server")]
impl Listener for NamedPipeListener {
    type Stream = File;

    fn accept(&mut self) -> io::Result<Self::Stream> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "listener shutting down",
            ));
        }
        let handle = self.create_instance()?;
        let connected = unsafe { ConnectNamedPipe(handle, ptr::null_mut()) };
        if connected == 0 {
            let err = last_os_error();
            if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                unsafe {
                    CloseHandle(handle);
                }
                return Err(err);
            }
        }
        // Return the connected stream even if stop flipped while waiting so
        // the accept loop can send the stable `shutting_down` handshake error
        // to a client that raced the transition. The shutdown wake connection
        // follows the same path and is closed after its rejected frame.
        // SAFETY: CreateNamedPipeW returned a new owned handle. File takes
        // exclusive ownership and closes it on drop.
        let file = unsafe { File::from_raw_handle(handle as RawHandle) };
        Ok(file)
    }

    fn shutdown(&mut self) -> io::Result<()> {
        self.shutdown_handle().shutdown();
        Ok(())
    }
}

pub fn connect_pipe(pipe_name: &str) -> io::Result<File> {
    for _ in 0..20 {
        match connect_pipe_once(pipe_name) {
            Ok(file) => return Ok(file),
            Err(error) => match error.raw_os_error().map(|code| code as u32) {
                Some(ERROR_PIPE_BUSY) => {
                    let name = wide(pipe_name);
                    let _ = unsafe { WaitNamedPipeW(name.as_ptr(), WAIT_MS) };
                }
                Some(ERROR_FILE_NOT_FOUND) => return Err(error),
                _ => return Err(error),
            },
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "named pipe is busy",
    ))
}

fn connect_pipe_once(pipe_name: &str) -> io::Result<File> {
    let name = wide(pipe_name);
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_os_error());
    }
    unsafe {
        SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
    }
    // SAFETY: CreateFileW returned a new owned handle.
    Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
}

pub fn inspect_pipe_dacl(file: &File) -> io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    security::dacl_sddl(file.as_raw_handle() as HANDLE)
}

#[cfg(feature = "server")]
impl Drop for NamedPipeListener {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
