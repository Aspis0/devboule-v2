//! Newline-delimited compact JSON.
//!
//! Chosen over length-prefix because the first time this misbehaves a human
//! can attach a pipe client and read a line. PTY payloads travel as escaped
//! JSON strings, so a compact `serde_json` frame never contains a raw
//! newline. A 1 MiB cap bounds a client that omits the delimiter.

use std::fs::File;
use std::io::{self, Read, Write};
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

pub struct Framed {
    file: File,
    buf: Vec<u8>,
}

impl Framed {
    pub fn new(file: File) -> Self {
        Self {
            file,
            buf: Vec::new(),
        }
    }

    pub fn send<T: Serialize>(&mut self, value: &T) -> Result<(), DaemonError> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(DaemonError::Protocol("frame exceeds 1 MiB".to_string()));
        }
        if bytes.contains(&b'\n') {
            return Err(DaemonError::Protocol(
                "compact JSON contained a raw newline".to_string(),
            ));
        }
        self.file.write_all(&bytes)?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        Ok(())
    }

    pub fn recv<T: DeserializeOwned>(&mut self) -> Result<T, DaemonError> {
        let line = self.read_line(None)?;
        Ok(serde_json::from_slice(&line)?)
    }

    pub fn recv_timeout<T: DeserializeOwned>(
        &mut self,
        timeout: Duration,
    ) -> Result<T, DaemonError> {
        let line = self.read_line(Some(Instant::now() + timeout))?;
        Ok(serde_json::from_slice(&line)?)
    }

    pub fn as_file(&self) -> &File {
        &self.file
    }

    fn read_line(&mut self, deadline: Option<Instant>) -> Result<Vec<u8>, DaemonError> {
        loop {
            if let Some(line) = take_line(&mut self.buf)? {
                return Ok(line);
            }
            if let Some(deadline) = deadline {
                let now = Instant::now();
                if now >= deadline {
                    return Err(DaemonError::timed_out("reading a protocol frame"));
                }
                if !wait_readable(&self.file, deadline - now)? {
                    return Err(DaemonError::timed_out("reading a protocol frame"));
                }
            }
            let mut chunk = [0u8; 8192];
            let read = self.file.read(&mut chunk)?;
            if read == 0 {
                return Err(DaemonError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed",
                )));
            }
            if self.buf.len() + read > MAX_FRAME_BYTES {
                return Err(DaemonError::Protocol("frame exceeds 1 MiB".to_string()));
            }
            self.buf.extend_from_slice(&chunk[..read]);
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

#[cfg(windows)]
fn wait_readable(file: &File, timeout: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
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
        if available > 0 {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(windows))]
fn wait_readable(_file: &File, _timeout: Duration) -> io::Result<bool> {
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
        // Framing itself is File-based; the JSON contract is tested in the
        // protocol crate. This only checks the line splitter against a Cursor
        // of bytes that include an escaped newline in a string.
        let mut buf = br#"{"type":"output","data":"a\nb"}"#.to_vec();
        buf.push(b'\n');
        let line = take_line(&mut buf).expect("ok").expect("frame");
        assert!(!line.contains(&b'\n'));
        assert!(line.windows(2).any(|pair| pair == br"\n"));
        let _ = Cursor::new(line);
    }
}
