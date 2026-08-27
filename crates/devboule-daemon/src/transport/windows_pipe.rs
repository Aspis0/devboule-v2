use std::fs::File;
use std::io;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, SetHandleInformation, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    GENERIC_READ, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

use crate::paths::RuntimePaths;
use crate::security::{self, last_os_error, wide, PipeSecurity};
use crate::transport::Listener;

const PIPE_BUFFER: u32 = 64 * 1024;
const MAX_INSTANCES: u32 = 16;
const WAIT_MS: u32 = 1000;

#[derive(Clone, Copy)]
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

#[derive(Clone)]
pub struct ListenerShutdown {
    stop: Arc<AtomicBool>,
    pending: Arc<Mutex<Option<SendHandle>>>,
    pipe_name: String,
}

impl ListenerShutdown {
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let pending = {
            let slot = self.pending.lock().unwrap_or_else(|err| err.into_inner());
            *slot
        };
        if let Some(SendHandle(handle)) = pending {
            unsafe {
                DisconnectNamedPipe(handle);
            }
        }
        let _ = connect_pipe(&self.pipe_name);
    }
}

pub struct NamedPipeListener {
    pipe_name: String,
    security: PipeSecurity,
    stop: Arc<AtomicBool>,
    pending: Arc<Mutex<Option<SendHandle>>>,
    first: bool,
}

impl NamedPipeListener {
    pub fn bind(paths: &RuntimePaths, stop: Arc<AtomicBool>) -> io::Result<Self> {
        let security = PipeSecurity::current_user_only()?;
        Ok(Self {
            pipe_name: paths.pipe_name.clone(),
            security,
            stop,
            pending: Arc::new(Mutex::new(None)),
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
            pending: Arc::clone(&self.pending),
            pipe_name: self.pipe_name.clone(),
        }
    }
}

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
        {
            let mut pending = self.pending.lock().unwrap_or_else(|err| err.into_inner());
            *pending = Some(SendHandle(handle));
        }
        let connected = unsafe { ConnectNamedPipe(handle, ptr::null_mut()) };
        {
            let mut pending = self.pending.lock().unwrap_or_else(|err| err.into_inner());
            *pending = None;
        }
        if connected == 0 {
            let err = last_os_error();
            if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                unsafe {
                    CloseHandle(handle);
                }
                return Err(err);
            }
        }
        if self.stop.load(Ordering::SeqCst) {
            unsafe {
                DisconnectNamedPipe(handle);
                CloseHandle(handle);
            }
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "listener shutting down",
            ));
        }
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
    let name = wide(pipe_name);
    for _ in 0..20 {
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
        if handle != INVALID_HANDLE_VALUE {
            unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
            // SAFETY: CreateFileW returned a new owned handle.
            return Ok(unsafe { File::from_raw_handle(handle as RawHandle) });
        }
        let err = last_os_error();
        match err.raw_os_error().map(|code| code as u32) {
            Some(ERROR_PIPE_BUSY) => {
                let _ = unsafe { WaitNamedPipeW(name.as_ptr(), WAIT_MS) };
            }
            Some(ERROR_FILE_NOT_FOUND) => return Err(err),
            _ => return Err(err),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "named pipe is busy",
    ))
}

pub fn inspect_pipe_dacl(file: &File) -> io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    security::dacl_sddl(file.as_raw_handle() as HANDLE)
}

impl Drop for NamedPipeListener {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
