//! Newline-delimited compact JSON.
//!
//! Chosen over length-prefix because the first time this misbehaves a human
//! can attach a pipe client and read a line. PTY payloads travel as escaped
//! JSON strings, so a compact `serde_json` frame never contains a raw
//! newline. A 1 MiB cap bounds a client that omits the delimiter.
//!
//! Windows named-pipe handles are opened for overlapped I/O. Each operation
//! owns an event and waits for its own completion, so one blocking read does
//! not hold the write lock. This is important because duplicating a named
//! pipe handle and reading on one copy while writing on the other did not
//! deliver duplex traffic on this stack.

use std::fs::File;
use std::io;
#[cfg(not(windows))]
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::MAX_FRAME_BYTES;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::DaemonError;

#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
#[cfg(windows)]
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

#[derive(Clone)]
pub struct Framed {
    #[cfg(windows)]
    file: Arc<File>,
    #[cfg(windows)]
    write_lock: Arc<Mutex<()>>,
    #[cfg(not(windows))]
    file: Arc<Mutex<File>>,
    buf: Arc<Mutex<Vec<u8>>>,
}

impl Framed {
    pub fn new(file: File) -> Self {
        Self {
            #[cfg(windows)]
            file: Arc::new(file),
            #[cfg(windows)]
            write_lock: Arc::new(Mutex::new(())),
            #[cfg(not(windows))]
            file: Arc::new(Mutex::new(file)),
            buf: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn send<T: Serialize>(&self, value: &T) -> Result<(), DaemonError> {
        self.send_with_deadline(value, None, true)
    }

    pub(crate) fn send_unflushed<T: Serialize>(&self, value: &T) -> Result<(), DaemonError> {
        // Event streaming must not call FlushFileBuffers. On Windows, when this
        // handle is the server end of a named pipe, FlushFileBuffers waits for
        // the client to read all buffered bytes. That is a teardown/control
        // delivery barrier, not a per-event operation: using it in the hot path
        // can park the daemon's event loop and starve request processing.
        self.send_with_deadline(value, None, false)
    }

    pub(crate) fn send_until<T: Serialize>(
        &self,
        value: &T,
        deadline: Instant,
    ) -> Result<(), DaemonError> {
        self.send_with_deadline(value, Some(deadline), false)
    }

    fn send_with_deadline<T: Serialize>(
        &self,
        value: &T,
        deadline: Option<Instant>,
        flush: bool,
    ) -> Result<(), DaemonError> {
        #[cfg(windows)]
        {
            let _write_lock = self
                .write_lock
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            write_frame(&self.file, value, deadline, flush)
        }
        #[cfg(not(windows))]
        {
            let mut file = self.file.lock().unwrap_or_else(|err| err.into_inner());
            write_frame(&mut file, value, flush)
        }
    }

    pub fn recv<T: DeserializeOwned>(&self) -> Result<T, DaemonError> {
        let line = self.read_line(None)?;
        Ok(serde_json::from_slice(&line)?)
    }

    pub fn recv_timeout<T: DeserializeOwned>(&self, timeout: Duration) -> Result<T, DaemonError> {
        let line = self.read_line(Some(Instant::now() + timeout))?;
        Ok(serde_json::from_slice(&line)?)
    }

    #[cfg(windows)]
    pub fn as_file(&self) -> Arc<File> {
        Arc::clone(&self.file)
    }

    /// Cancel a blocking server-side read during daemon shutdown.
    #[cfg(windows)]
    pub fn cancel_read(&self) {
        unsafe {
            let _ = windows_sys::Win32::System::IO::CancelIoEx(
                self.file.as_raw_handle() as HANDLE,
                std::ptr::null(),
            );
        }
    }

    #[cfg(not(windows))]
    pub fn cancel_read(&self) {}

    fn read_line(&self, deadline: Option<Instant>) -> Result<Vec<u8>, DaemonError> {
        loop {
            {
                let mut buf = self.buf.lock().unwrap_or_else(|err| err.into_inner());
                if let Some(line) = take_line(&mut buf)? {
                    return Ok(line);
                }
            }
            if let Some(deadline) = deadline {
                let now = Instant::now();
                if now >= deadline {
                    return Err(DaemonError::timed_out("reading a protocol frame"));
                }
            }
            let mut chunk = [0u8; 8192];
            #[cfg(windows)]
            let read = match read_chunk(&self.file, &mut chunk, deadline)? {
                Some(read) => read,
                None => return Err(DaemonError::timed_out("reading a protocol frame")),
            };
            #[cfg(not(windows))]
            let read = {
                let mut file = self.file.lock().unwrap_or_else(|err| err.into_inner());
                file.read(&mut chunk)?
            };
            if read == 0 {
                return Err(DaemonError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed",
                )));
            }
            let mut buf = self.buf.lock().unwrap_or_else(|err| err.into_inner());
            if buf.len() + read > MAX_FRAME_BYTES {
                return Err(DaemonError::Protocol("frame exceeds 1 MiB".to_string()));
            }
            buf.extend_from_slice(&chunk[..read]);
        }
    }
}

fn take_line(buf: &mut Vec<u8>) -> Result<Option<Vec<u8>>, DaemonError> {
    let Some(pos) = buf.iter().position(|byte| *byte == b'\n') else {
        if buf.len() > MAX_FRAME_BYTES {
            return Err(DaemonError::Protocol("frame exceeds 1 MiB".to_string()));
        }
        return Ok(None);
    };
    let mut line: Vec<u8> = buf.drain(..=pos).collect();
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    if line.is_empty() {
        return Ok(None);
    }
    Ok(Some(line))
}

fn frame_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, DaemonError> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(DaemonError::Protocol("frame exceeds 1 MiB".to_string()));
    }
    if bytes.contains(&b'\n') {
        return Err(DaemonError::Protocol(
            "compact JSON contained a raw newline".to_string(),
        ));
    }
    Ok(bytes)
}

#[cfg(windows)]
fn write_frame<T: Serialize>(
    file: &File,
    value: &T,
    deadline: Option<Instant>,
    flush: bool,
) -> Result<(), DaemonError> {
    let bytes = frame_bytes(value)?;
    write_all_overlapped(file, &bytes, deadline)?;
    write_all_overlapped(file, b"\n", deadline)?;
    if flush {
        let ok = unsafe { FlushFileBuffers(file.as_raw_handle() as HANDLE) };
        if ok == 0 {
            return Err(DaemonError::Io(io::Error::last_os_error()));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn write_frame<T: Serialize>(file: &mut File, value: &T, flush: bool) -> Result<(), DaemonError> {
    let bytes = frame_bytes(value)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    if flush {
        file.flush()?;
    }
    Ok(())
}

#[cfg(windows)]
struct OperationEvent(HANDLE);

#[cfg(windows)]
impl OperationEvent {
    fn new() -> io::Result<Self> {
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

#[cfg(windows)]
impl Drop for OperationEvent {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn wait_for_operation(
    handle: HANDLE,
    event: HANDLE,
    overlapped: &OVERLAPPED,
    deadline: Option<Instant>,
) -> io::Result<Option<u32>> {
    let wait = match deadline {
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let millis = remaining.as_millis().min(u32::MAX as u128) as u32;
            unsafe { WaitForSingleObject(event, millis.max(1)) }
        }
        None => unsafe { WaitForSingleObject(event, u32::MAX) },
    };
    if wait == WAIT_TIMEOUT {
        unsafe {
            let _ = windows_sys::Win32::System::IO::CancelIoEx(handle, overlapped);
        }
        let mut transferred = 0u32;
        let _ = unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, 1) };
        return Ok(None);
    }
    if wait != WAIT_OBJECT_0 {
        let error = io::Error::last_os_error();
        unsafe {
            let _ = windows_sys::Win32::System::IO::CancelIoEx(handle, overlapped);
        }
        let mut transferred = 0u32;
        let _ = unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, 1) };
        return Err(error);
    }
    let mut transferred = 0u32;
    let ok = unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, 1) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Some(transferred))
}

#[cfg(windows)]
fn read_chunk(
    file: &File,
    buffer: &mut [u8],
    deadline: Option<Instant>,
) -> io::Result<Option<usize>> {
    let event = OperationEvent::new()?;
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = event.0;
    let mut read = 0u32;
    let started = unsafe {
        ReadFile(
            file.as_raw_handle() as HANDLE,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            &mut read,
            &mut overlapped,
        )
    };
    if started != 0 {
        return Ok(Some(read as usize));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
        return Ok(Some(0));
    }
    if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
        return Err(error);
    }
    match wait_for_operation(
        file.as_raw_handle() as HANDLE,
        event.0,
        &overlapped,
        deadline,
    )? {
        Some(read) => Ok(Some(read as usize)),
        None => Ok(None),
    }
}

#[cfg(windows)]
fn write_all_overlapped(file: &File, bytes: &[u8], deadline: Option<Instant>) -> io::Result<()> {
    let mut written_total = 0usize;
    while written_total < bytes.len() {
        let event = OperationEvent::new()?;
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event.0;
        let mut written = 0u32;
        let started = unsafe {
            WriteFile(
                file.as_raw_handle() as HANDLE,
                bytes[written_total..].as_ptr(),
                (bytes.len() - written_total) as u32,
                &mut written,
                &mut overlapped,
            )
        };
        if started == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
            written = wait_for_operation(
                file.as_raw_handle() as HANDLE,
                event.0,
                &overlapped,
                deadline,
            )?
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "pipe write timed out"))?;
        }
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "named pipe write made no progress",
            ));
        }
        written_total += written as usize;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn take_line_strips_crlf_and_skips_empty() {
        let mut buf = b"\n{\"type\":\"ping\",\"id\":1}\r\nrest".to_vec();
        assert!(take_line(&mut buf).expect("empty").is_none());
        let line = take_line(&mut buf).expect("line").expect("frame");
        assert_eq!(line, br#"{"type":"ping","id":1}"#);
        assert_eq!(buf, b"rest");
    }

    #[test]
    fn take_line_none_until_newline() {
        let mut buf = b"{\"type\":\"ping\"".to_vec();
        assert!(take_line(&mut buf).expect("ok").is_none());
    }

    #[test]
    fn cursor_roundtrip_is_one_line() {
        let mut buf = br#"{"type":"output","data":"a\nb"}"#.to_vec();
        buf.push(b'\n');
        let line = take_line(&mut buf).expect("ok").expect("frame");
        assert!(!line.contains(&b'\n'));
        assert!(line.windows(2).any(|pair| pair == br"\n"));
        let _ = Cursor::new(line);
    }
}
