//! Deadline-bounded framed reads.
//!
//! The daemon's pipe framing checks its deadline only between reads, and a
//! Unix socket read that hits `SO_RCVTIMEO` surfaces as `WouldBlock`. The
//! channel sets that timeout when it is created, so retrying the tick here —
//! and only the tick — keeps the caller's deadline authoritative without
//! touching the daemon crate.

use std::time::{Duration, Instant};

use devboule_daemon::{DaemonError, Framed};
use serde::de::DeserializeOwned;

use crate::error::PluginError;

/// Read one frame, waiting a silent peer out until `timeout`. A complete
/// frame already buffered still wins after the deadline, exactly as the
/// framing itself would decide it.
pub(crate) fn recv_frame<T: DeserializeOwned>(
    framed: &Framed,
    timeout: Duration,
) -> Result<T, PluginError> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match framed.recv_timeout::<T>(remaining) {
            Ok(frame) => return Ok(frame),
            Err(error) if is_would_block(&error) => {
                if deadline.saturating_duration_since(Instant::now()).is_zero() {
                    return Err(PluginError::timed_out("reading a protocol frame"));
                }
            }
            Err(error) => return Err(PluginError::from(error)),
        }
    }
}

fn is_would_block(error: &DaemonError) -> bool {
    matches!(error, DaemonError::Io(error) if error.kind() == std::io::ErrorKind::WouldBlock)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::pipe::set_receive_timeout;
    use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;
    use std::time::Duration;

    fn reader_channel() -> (devboule_daemon::Framed, std::fs::File) {
        let (host, writer) = UnixStream::pair().expect("socketpair");
        set_receive_timeout(host.as_raw_fd(), Duration::from_millis(50)).expect("read tick");
        let host = unsafe { std::fs::File::from_raw_fd(host.into_raw_fd()) };
        let framed = Framed::new(host);
        let writer = unsafe { std::fs::File::from_raw_fd(writer.into_raw_fd()) };
        (framed, writer)
    }

    /// Run the read on a thread so a deadline that never fires fails the test
    /// instead of hanging the harness.
    fn recv_on_thread(
        framed: Framed,
        timeout: Duration,
    ) -> Result<devboule_protocol::ClientMessage, PluginError> {
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(recv_frame::<devboule_protocol::ClientMessage>(
                &framed, timeout,
            ));
        });
        match receive.recv_timeout(Duration::from_secs(3)) {
            Ok(result) => result,
            Err(_) => panic!("recv_frame blocked past its deadline"),
        }
    }

    #[test]
    fn a_silent_peer_expires_the_deadline_instead_of_blocking_forever() {
        let (framed, _writer) = reader_channel();
        let error = recv_on_thread(framed, Duration::from_millis(300))
            .expect_err("no frame was ever written");
        assert!(matches!(error, PluginError::TimedOut(_)), "{error}");
    }

    #[test]
    fn read_ticks_are_retried_until_the_frame_arrives() {
        let (framed, writer) = reader_channel();
        let late_frame = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            Framed::new(writer).send(&devboule_protocol::ClientMessage::Ping { id: 41 })
        });
        let frame: devboule_protocol::ClientMessage =
            recv_on_thread(framed, Duration::from_secs(3)).expect("late frame");
        assert_eq!(frame.request_id(), Some(41));
        late_frame.join().expect("writer thread").expect("write");
    }
}
