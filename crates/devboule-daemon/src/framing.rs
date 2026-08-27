//! Newline-delimited compact JSON.
//!
//! Chosen over length-prefix because the first time this misbehaves a human
//! can attach a pipe client and read a line. PTY payloads travel as escaped
//! JSON strings, so a compact `serde_json` frame never contains a raw
//! newline. A 1 MiB cap bounds a client that omits the delimiter.
//!
//! The pipe is shared via `Arc<Mutex<File>>` rather than `File::try_clone`.
//! Duplicating a Windows named-pipe handle and then reading on one copy
//! while writing on the other does not deliver duplex traffic on this
//! stack — ping timed out in 30s. One mutex, PeekNamedPipe before Read
//! so a sender is never stuck behind a blocking recv.

use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::MAX_FRAME_BYTES;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::DaemonError;

#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

#[derive(Clone)]
pub struct Framed {
    file: Arc<Mutex<File>>,
    buf: Arc<Mutex<Vec<u8>>>,
}

impl Framed {
    pub fn new(file: File) -> Self {
        Self {
            file: Arc::new(Mutex::new(file)),
            buf: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn send<T: Serialize>(&self, value: &T) -> Result<(), DaemonError> {
        let mut file = self.file.lock().unwrap_or_else(|err| err.into_inner());
        write_frame(&mut file, value)
    }

    pub fn recv<T: DeserializeOwned>(&self) -> Result<T, DaemonError> {
        let line = self.read_line(None)?;
        Ok(serde_json::from_slice(&line)?)
    }

    pub fn recv_timeout<T: DeserializeOwned>(&self, timeout: Duration) -> Result<T, DaemonError> {
        let line = self.read_line(Some(Instant::now() + timeout))?;
        Ok(serde_json::from_slice(&line)?)
    }

    pub fn as_file(&self) -> std::sync::MutexGuard<'_, File> {
        self.file.lock().unwrap_or_else(|err| err.into_inner())
    }

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
                let readable = {
                    let file = self.file.lock().unwrap_or_else(|err| err.into_inner());
                    peek_readable(&file)?
                };
                if !readable {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(DaemonError::timed_out("reading a protocol frame"));
                    }
                    std::thread::sleep(remaining.min(Duration::from_millis(2)));
                    continue;
                }
            }
            let mut chunk = [0u8; 8192];
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

fn write_frame<T: Serialize>(file: &mut File, value: &T) -> Result<(), DaemonError> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(DaemonError::Protocol("frame exceeds 1 MiB".to_string()));
    }
    if bytes.contains(&b'\n') {
        return Err(DaemonError::Protocol(
            "compact JSON contained a raw newline".to_string(),
        ));
    }
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
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

#[cfg(windows)]
fn peek_readable(file: &File) -> io::Result<bool> {
    let mut available = 0u32;
    let ok = unsafe {
        PeekNamedPipe(
            file.as_raw_handle() as HANDLE,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available > 0)
}

#[cfg(not(windows))]
fn peek_readable(_file: &File) -> io::Result<bool> {
    Ok(true)
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
