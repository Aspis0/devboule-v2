//! Newline-delimited compact JSON.
//!
//! Chosen over length-prefix because the first time this misbehaves a human
//! can attach a pipe client and read a line. PTY payloads travel as escaped
//! JSON strings, so a compact `serde_json` frame never contains a raw
//! newline. The daemon default is a 1 MiB cap; callers that own a separate
//! pipe may derive and install a different per-instance cap.
//!
//! Windows named-pipe handles are opened for overlapped I/O. Each operation
//! owns an event and waits for its own completion, so one blocking read does
//! not hold the write lock. This is important because duplicating a named
//! pipe handle and reading on one copy while writing on the other did not
//! deliver duplex traffic on this stack.
//!
//! There are two shapes, not one trait object. The Windows pipe path drives
//! raw overlapped `ReadFile`/`WriteFile` on a `HANDLE` and cannot be expressed
//! through `Read`/`Write`; a remote peer arrives as a Noise session over a
//! `TcpStream`. [`FramedInner`] keeps both concrete, so the pipe keeps its
//! semantics and the stream gets a deadline that is recomputed on every read
//! and write exactly like the overlapped path does.

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
#[cfg(feature = "server")]
use crate::peer_transport::{NoiseReader, NoiseWriter};

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

/// Both halves of one Noise session. Each half owns its own `TcpStream` (from
/// `try_clone`), so a blocking read never holds the write lock and the two
/// directions are independent, the same duplex property the pipe has.
///
/// Server-only: a client-only build has no peer listener and therefore no
/// Noise, and the whole stream path disappears with it.
#[cfg(feature = "server")]
pub struct StreamPair {
    pub reader: Mutex<NoiseReader>,
    pub writer: Mutex<NoiseWriter>,
}

enum FramedInner {
    Pipe(PipeInner),
    #[cfg(feature = "server")]
    Stream(Arc<StreamPair>),
}

#[cfg(windows)]
struct PipeInner {
    file: Arc<File>,
    write_lock: Arc<Mutex<()>>,
}

#[cfg(not(windows))]
struct PipeInner {
    file: Arc<Mutex<File>>,
}

impl Clone for FramedInner {
    fn clone(&self) -> Self {
        match self {
            Self::Pipe(pipe) => Self::Pipe(pipe.clone()),
            #[cfg(feature = "server")]
            Self::Stream(pair) => Self::Stream(Arc::clone(pair)),
        }
    }
}

#[cfg(windows)]
impl Clone for PipeInner {
    fn clone(&self) -> Self {
        Self {
            file: Arc::clone(&self.file),
            write_lock: Arc::clone(&self.write_lock),
        }
    }
}

#[cfg(not(windows))]
impl Clone for PipeInner {
    fn clone(&self) -> Self {
        Self {
            file: Arc::clone(&self.file),
        }
    }
}

#[derive(Clone)]
pub struct Framed {
    inner: FramedInner,
    buf: Arc<Mutex<Vec<u8>>>,
    max_frame_bytes: usize,
}

impl Framed {
    pub fn new(file: File) -> Self {
        Self::with_limit(file, MAX_FRAME_BYTES)
    }

    /// Carry a protocol frame over a Noise session instead of a pipe.
    #[cfg(feature = "server")]
    pub fn from_stream(reader: NoiseReader, writer: NoiseWriter) -> Self {
        Self {
            inner: FramedInner::Stream(Arc::new(StreamPair {
                reader: Mutex::new(reader),
                writer: Mutex::new(writer),
            })),
            buf: Arc::new(Mutex::new(Vec::new())),
            max_frame_bytes: MAX_FRAME_BYTES,
        }
    }

    /// Construct a pipe with an explicit frame limit. `new` remains the
    /// daemon-wire default; plugin pipes use this after deriving their limit
    /// from the host's effective payload budget.
    pub fn with_limit(file: File, max_frame_bytes: usize) -> Self {
        assert!(
            max_frame_bytes > 0,
            "a framed pipe must have a positive limit"
        );
        Self {
            #[cfg(windows)]
            inner: FramedInner::Pipe(PipeInner {
                file: Arc::new(file),
                write_lock: Arc::new(Mutex::new(())),
            }),
            #[cfg(not(windows))]
            inner: FramedInner::Pipe(PipeInner {
                file: Arc::new(Mutex::new(file)),
            }),
            buf: Arc::new(Mutex::new(Vec::new())),
            max_frame_bytes,
        }
    }

    /// Change the limit after the bootstrap hello has been received. The
    /// hello itself is always read under the default cap.
    pub fn set_max_frame_bytes(&mut self, max_frame_bytes: usize) {
        assert!(
            max_frame_bytes > 0,
            "a framed pipe must have a positive limit"
        );
        self.max_frame_bytes = max_frame_bytes;
    }

    pub fn send<T: Serialize>(&self, value: &T) -> Result<(), DaemonError> {
        self.send_with_deadline(value, None, true)
    }

    #[allow(dead_code)]
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

    /// Establish a pipe delivery barrier during connection teardown. This is
    /// intentionally separate from event writes: on Windows the barrier waits
    /// for the client to read all bytes buffered on the server end. A socket
    /// write is already a syscall into the kernel buffer, so the stream case
    /// has no barrier to establish.
    #[allow(dead_code)]
    pub(crate) fn flush_pipe(&self) -> Result<(), DaemonError> {
        match &self.inner {
            #[cfg(feature = "server")]
            FramedInner::Stream(_) => Ok(()),
            #[cfg(windows)]
            FramedInner::Pipe(pipe) => {
                let _write_lock = pipe
                    .write_lock
                    .lock()
                    .unwrap_or_else(|err| err.into_inner());
                let ok = unsafe { FlushFileBuffers(pipe.file.as_raw_handle() as HANDLE) };
                if ok == 0 {
                    return Err(DaemonError::Io(io::Error::last_os_error()));
                }
                Ok(())
            }
            #[cfg(not(windows))]
            FramedInner::Pipe(pipe) => pipe
                .file
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .flush()
                .map_err(DaemonError::from),
        }
    }

    fn send_with_deadline<T: Serialize>(
        &self,
        value: &T,
        deadline: Option<Instant>,
        flush: bool,
    ) -> Result<(), DaemonError> {
        match &self.inner {
            #[cfg(windows)]
            FramedInner::Pipe(pipe) => {
                let _write_lock = pipe
                    .write_lock
                    .lock()
                    .unwrap_or_else(|err| err.into_inner());
                write_frame(&pipe.file, value, self.max_frame_bytes, deadline, flush)
            }
            #[cfg(not(windows))]
            FramedInner::Pipe(pipe) => {
                let mut file = pipe.file.lock().unwrap_or_else(|err| err.into_inner());
                write_frame(&mut file, value, self.max_frame_bytes, flush)
            }
            #[cfg(feature = "server")]
            FramedInner::Stream(pair) => {
                // The frame is serialised under the caller's cap first, so an
                // oversized frame is refused before any byte reaches the
                // wire, exactly as on the pipe.
                let bytes = frame_bytes(value, self.max_frame_bytes)?;
                let mut writer = pair.writer.lock().unwrap_or_else(|err| err.into_inner());
                writer
                    .write_frame(&bytes, deadline)
                    .map_err(DaemonError::from)
            }
        }
    }

    #[allow(dead_code)]
    pub fn recv<T: DeserializeOwned>(&self) -> Result<T, DaemonError> {
        let line = self.read_line(None)?;
        Ok(serde_json::from_slice(&line)?)
    }

    pub fn recv_timeout<T: DeserializeOwned>(&self, timeout: Duration) -> Result<T, DaemonError> {
        let line = self.read_line(Some(Instant::now() + timeout))?;
        Ok(serde_json::from_slice(&line)?)
    }

    /// The pipe handle, when this connection is a pipe.
    ///
    /// Kernel peer identity (`GetNamedPipeClientProcessId`) only exists on the
    /// pipe path; a remote connection's identity is the Noise static key the
    /// peer authenticated with, which is decided before `Framed` is built. A
    /// caller that needs the handle must branch on this, never unwrap it: the
    /// stream case is routine, not an error.
    #[cfg(windows)]
    pub fn as_file(&self) -> Option<Arc<File>> {
        match &self.inner {
            FramedInner::Pipe(pipe) => Some(Arc::clone(&pipe.file)),
            #[cfg(feature = "server")]
            FramedInner::Stream(_) => None,
        }
    }

    /// Cancel a blocking server-side read during daemon shutdown.
    #[cfg(windows)]
    #[allow(dead_code)]
    pub fn cancel_read(&self) {
        match &self.inner {
            FramedInner::Pipe(pipe) => unsafe {
                let _ = windows_sys::Win32::System::IO::CancelIoEx(
                    pipe.file.as_raw_handle() as HANDLE,
                    std::ptr::null(),
                );
            },
            // A socket has no overlapped operation to cancel; shutting the
            // read side down is what unblocks the reader thread.
            #[cfg(feature = "server")]
            FramedInner::Stream(pair) => {
                let reader = pair.reader.lock().unwrap_or_else(|err| err.into_inner());
                let _ = reader.shutdown();
            }
        }
    }

    #[cfg(not(windows))]
    #[allow(dead_code)]
    pub fn cancel_read(&self) {
        #[cfg(feature = "server")]
        if let FramedInner::Stream(pair) = &self.inner {
            let reader = pair.reader.lock().unwrap_or_else(|err| err.into_inner());
            let _ = reader.shutdown();
        }
    }

    fn read_line(&self, deadline: Option<Instant>) -> Result<Vec<u8>, DaemonError> {
        loop {
            {
                let mut buf = self.buf.lock().unwrap_or_else(|err| err.into_inner());
                if let Some(line) = take_line(&mut buf, self.max_frame_bytes)? {
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
            let read = self.read_chunk(&mut chunk, deadline)?;
            if read == 0 {
                return Err(DaemonError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed",
                )));
            }
            let mut buf = self.buf.lock().unwrap_or_else(|err| err.into_inner());
            if buf.len() + read > self.max_frame_bytes {
                return Err(frame_limit_error(self.max_frame_bytes));
            }
            buf.extend_from_slice(&chunk[..read]);
        }
    }

    /// `0` means end of stream. A deadline that passes returns a timeout
    /// error, never `0`, so the caller cannot mistake one for the other.
    fn read_chunk(
        &self,
        chunk: &mut [u8],
        deadline: Option<Instant>,
    ) -> Result<usize, DaemonError> {
        match &self.inner {
            #[cfg(windows)]
            FramedInner::Pipe(pipe) => match read_chunk(&pipe.file, chunk, deadline)? {
                Some(read) => Ok(read),
                None => Err(DaemonError::timed_out("reading a protocol frame")),
            },
            #[cfg(not(windows))]
            FramedInner::Pipe(pipe) => {
                let mut file = pipe.file.lock().unwrap_or_else(|err| err.into_inner());
                Ok(file.read(chunk)?)
            }
            #[cfg(feature = "server")]
            FramedInner::Stream(pair) => {
                let mut reader = pair.reader.lock().unwrap_or_else(|err| err.into_inner());
                reader
                    .read_plaintext(chunk, deadline)
                    .map_err(DaemonError::from)
            }
        }
    }
}

fn take_line(buf: &mut Vec<u8>, max_frame_bytes: usize) -> Result<Option<Vec<u8>>, DaemonError> {
    let Some(pos) = buf.iter().position(|byte| *byte == b'\n') else {
        if buf.len() > max_frame_bytes {
            return Err(frame_limit_error(max_frame_bytes));
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

fn frame_bytes<T: Serialize>(value: &T, max_frame_bytes: usize) -> Result<Vec<u8>, DaemonError> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > max_frame_bytes {
        return Err(frame_limit_error(max_frame_bytes));
    }
    if bytes.contains(&b'\n') {
        return Err(DaemonError::Protocol(
            "compact JSON contained a raw newline".to_string(),
        ));
    }
    Ok(bytes)
}

fn frame_limit_error(max_frame_bytes: usize) -> DaemonError {
    if max_frame_bytes == MAX_FRAME_BYTES {
        DaemonError::Protocol("frame exceeds 1 MiB".to_string())
    } else {
        DaemonError::Protocol(format!("frame exceeds {max_frame_bytes} bytes"))
    }
}

#[cfg(windows)]
fn write_frame<T: Serialize>(
    file: &File,
    value: &T,
    max_frame_bytes: usize,
    deadline: Option<Instant>,
    flush: bool,
) -> Result<(), DaemonError> {
    let bytes = frame_bytes(value, max_frame_bytes)?;
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
fn write_frame<T: Serialize>(
    file: &mut File,
    value: &T,
    max_frame_bytes: usize,
    flush: bool,
) -> Result<(), DaemonError> {
    let bytes = frame_bytes(value, max_frame_bytes)?;
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
pub(crate) fn read_chunk(
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
pub(crate) fn write_all_overlapped(
    file: &File,
    bytes: &[u8],
    deadline: Option<Instant>,
) -> io::Result<()> {
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
        assert!(take_line(&mut buf, MAX_FRAME_BYTES)
            .expect("empty")
            .is_none());
        let line = take_line(&mut buf, MAX_FRAME_BYTES)
            .expect("line")
            .expect("frame");
        assert_eq!(line, br#"{"type":"ping","id":1}"#);
        assert_eq!(buf, b"rest");
    }

    #[test]
    fn take_line_none_until_newline() {
        let mut buf = b"{\"type\":\"ping\"".to_vec();
        assert!(take_line(&mut buf, MAX_FRAME_BYTES).expect("ok").is_none());
    }

    #[test]
    fn cursor_roundtrip_is_one_line() {
        let mut buf = br#"{"type":"output","data":"a\nb"}"#.to_vec();
        buf.push(b'\n');
        let line = take_line(&mut buf, MAX_FRAME_BYTES)
            .expect("ok")
            .expect("frame");
        assert!(!line.contains(&b'\n'));
        assert!(line.windows(2).any(|pair| pair == br"\n"));
        let _ = Cursor::new(line);
    }

    #[test]
    fn plugin_payload_budget_and_transport_share_the_same_boundary() {
        use devboule_protocol::{
            plugin_frame_limit_for_payload, plugin_payload_within_limit, ClientMessage,
        };

        let payload_limit = 4096;
        let payload = serde_json::Value::String("x".repeat(payload_limit - 2));
        assert_eq!(
            serde_json::to_vec(&payload)
                .expect("payload must serialize")
                .len(),
            payload_limit
        );
        assert!(plugin_payload_within_limit(Some(&payload), payload_limit));

        let message = ClientMessage::Invoke {
            id: u64::MAX,
            method: "x".repeat(128),
            payload: Some(payload),
        };
        let frame_limit = plugin_frame_limit_for_payload(payload_limit);
        assert!(
            frame_bytes(&message, frame_limit).is_ok(),
            "a payload accepted by the host budget must fit the plugin transport envelope"
        );
    }

    #[test]
    fn custom_frame_limit_is_used_for_serialization() {
        let error = frame_bytes(&"12345", 4).expect_err("custom limit must be enforced");
        assert_eq!(
            error.to_string(),
            "frame exceeds 4 bytes",
            "custom frame errors must expose the actual configured limit"
        );
    }
}
