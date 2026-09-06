//! Direct npm update execution for provider lifecycle commands.
//!
//! The runner is a deliberately small seam: provider resolution and wire
//! policy stay in the server, while tests can replace the only operation
//! that would otherwise start npm.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::process_tree::JobObject;

pub(crate) const UPDATE_TIMEOUT: Duration = Duration::from_secs(180);
pub(crate) const MAX_UPDATE_LOG_BYTES: usize = 64 * 1024;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpmInstallResult {
    pub exit_code: Option<i32>,
    pub log: String,
}

/// Test and production seam for `npm install -g <package>@latest`.
pub trait NpmInstallRunner: Send + Sync {
    fn run(
        &self,
        program: &Path,
        prefix_args: &[String],
        args: &[String],
        job: &JobObject,
    ) -> NpmInstallResult;
}

#[derive(Debug, Default)]
pub struct ProcessNpmInstallRunner;

impl NpmInstallRunner for ProcessNpmInstallRunner {
    fn run(
        &self,
        program: &Path,
        prefix_args: &[String],
        args: &[String],
        job: &JobObject,
    ) -> NpmInstallResult {
        let mut command = Command::new(program);
        command
            .args(prefix_args)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return NpmInstallResult {
                    exit_code: None,
                    log: bounded_log(format!("could not start npm: {error}").as_bytes()),
                };
            }
        };

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            if let Err(error) = job.assign(child.as_raw_handle()) {
                let _ = child.kill();
                let exit_code = child.wait().ok().and_then(|status| status.code());
                return NpmInstallResult {
                    exit_code,
                    log: bounded_log(
                        format!("could not assign npm to the daemon Job Object: {error}")
                            .as_bytes(),
                    ),
                };
            }
        }
        #[cfg(not(windows))]
        let _ = job;

        let stdout = child
            .stdout
            .take()
            .map(|stream| std::thread::spawn(move || read_tail(stream)));
        let stderr = child
            .stderr
            .take()
            .map(|stream| std::thread::spawn(move || read_tail(stream)));

        let deadline = Instant::now() + UPDATE_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    let _ = child.kill();
                    break child.wait().ok();
                }
                Err(_) => {
                    let _ = child.kill();
                    break child.wait().ok();
                }
            }
        };

        let mut output = Vec::new();
        join_tail(stdout, &mut output);
        join_tail(stderr, &mut output);
        output = tail_bytes(output, MAX_UPDATE_LOG_BYTES);
        NpmInstallResult {
            exit_code: status.and_then(|status| status.code()),
            log: bounded_log(&output),
        }
    }
}

fn read_tail<R: Read>(mut reader: R) -> Vec<u8> {
    let mut tail = Vec::with_capacity(MAX_UPDATE_LOG_BYTES);
    let mut chunk = [0u8; 8192];
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        tail.extend_from_slice(&chunk[..read]);
        if tail.len() > MAX_UPDATE_LOG_BYTES {
            let excess = tail.len() - MAX_UPDATE_LOG_BYTES;
            tail.drain(..excess);
        }
    }
    tail
}

fn join_tail(handle: Option<JoinHandle<Vec<u8>>>, output: &mut Vec<u8>) {
    if let Some(handle) = handle {
        if let Ok(bytes) = handle.join() {
            output.extend(bytes);
        }
    }
}

fn tail_bytes(mut bytes: Vec<u8>, max: usize) -> Vec<u8> {
    if bytes.len() > max {
        let excess = bytes.len() - max;
        bytes.drain(..excess);
    }
    bytes
}

pub(crate) fn bounded_log(bytes: &[u8]) -> String {
    let log = String::from_utf8_lossy(bytes);
    if log.len() <= MAX_UPDATE_LOG_BYTES {
        return log.into_owned();
    }
    let mut end = MAX_UPDATE_LOG_BYTES;
    while !log.is_char_boundary(end) {
        end -= 1;
    }
    log[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_error_end_of_oversized_output() {
        let mut output = vec![b'a'; MAX_UPDATE_LOG_BYTES + 8];
        output.extend_from_slice(b"npm error last");
        let tail = tail_bytes(output, MAX_UPDATE_LOG_BYTES);
        assert_eq!(tail.len(), MAX_UPDATE_LOG_BYTES);
        assert!(tail.ends_with(b"npm error last"));
    }

    #[test]
    fn lossy_output_stays_bounded_at_the_wire_limit() {
        let bytes = vec![0xff; MAX_UPDATE_LOG_BYTES];
        assert!(bounded_log(&bytes).len() <= MAX_UPDATE_LOG_BYTES);
    }
}
