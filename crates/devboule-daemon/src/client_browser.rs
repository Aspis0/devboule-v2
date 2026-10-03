//! The client half of the browser host: register as the place agents' browser
//! commands run, receive those commands, and answer them.
//!
//! Commands are queued, never run on the connection's reader thread. The
//! reader is the only thing that delivers the daemon's replies, so a host that
//! ran a command there and then waited on a reply (its own answer is one)
//! would wait on itself.

use std::sync::atomic::Ordering;
use std::sync::mpsc;

use devboule_protocol::{
    BrowserError, BrowserErrorCode, BrowserExecuteRequest, BrowserOutcome, ClientMessage,
    DaemonMessage,
};

use super::{unexpected, ClientInner, DaemonClient, DaemonError};

/// Commands the reader may hold for the host. A full queue means the host is
/// not draining it: the reader answers that command `browser_busy` at once
/// rather than leaving its caller to wait out the deadline.
pub(super) const REQUEST_QUEUE: usize = 64;

pub(super) fn enqueue_request(inner: &ClientInner, request: BrowserExecuteRequest) {
    let queue = inner
        .browser_requests
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let Some(queue) = queue.as_ref() else {
        return;
    };
    let Err(refused) = queue.try_send(request) else {
        return;
    };
    let request = match refused {
        mpsc::TrySendError::Full(request) | mpsc::TrySendError::Disconnected(request) => request,
    };
    // The reader must not write to the daemon itself: the daemon may be waiting
    // for this very thread to read a reply before it reads the write. The
    // refusal goes to the thread that owns the writing.
    let busy = ClientMessage::BrowserExecuteResponse {
        id: inner.next_id.fetch_add(1, Ordering::Relaxed),
        request_id: request.request_id,
        host_id: request.host_id,
        outcome: BrowserOutcome::Err(BrowserError::daemon(
            BrowserErrorCode::Busy,
            "The browser host is not keeping up; try again.",
        )),
    };
    let rejects = inner
        .browser_rejects
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if rejects
        .as_ref()
        .is_none_or(|rejects| rejects.send(busy).is_err())
    {
        eprintln!("browser command dropped: the host is not draining its queue");
    }
}

impl DaemonClient {
    /// Register this connection as the browser host for `supported_commands`.
    /// The returned host id names this registration only: register again and
    /// it is a new one, and answers carrying the old id are dropped.
    pub fn browser_host_register(
        &self,
        supported_commands: &[String],
    ) -> Result<String, DaemonError> {
        self.require_agreed(devboule_protocol::caps::BROWSER_HOST)?;
        self.start_refusal_writer()?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::BrowserHostRegister {
            id,
            supported_commands: supported_commands.to_vec(),
        })? {
            DaemonMessage::BrowserHostRegistered { host_id, .. } => Ok(host_id),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Start the thread that writes the refusals the reader queues (see
    /// `enqueue_request`). Once per connection; it ends when the connection fails.
    fn start_refusal_writer(&self) -> Result<(), DaemonError> {
        let mut rejects = self
            .inner
            .browser_rejects
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if rejects.is_some() {
            return Ok(());
        }
        let (sender, queued) = mpsc::channel::<ClientMessage>();
        let framed = self.inner.framed.clone();
        std::thread::Builder::new()
            .name("daemon-client-browser-refuse".into())
            .spawn(move || {
                for message in queued {
                    if framed.send(&message).is_err() {
                        break;
                    }
                }
            })?;
        *rejects = Some(sender);
        Ok(())
    }

    /// Stop being the browser host. Calls still waiting on it fail as retryable
    /// `browser_no_host`.
    pub fn browser_host_unregister(&self, host_id: &str) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::BROWSER_HOST)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::BrowserHostUnregister {
            id,
            host_id: host_id.to_string(),
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Answer one command. The daemon acknowledges every answer it can place,
    /// including a late one; an answer it refuses (a result over the frame
    /// budget) comes back as the daemon's own error naming the browser code.
    pub fn browser_respond(
        &self,
        request_id: &str,
        host_id: &str,
        outcome: BrowserOutcome,
    ) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::BROWSER_HOST)?;
        let outcome = match outcome {
            BrowserOutcome::Err(error) => BrowserOutcome::Err(error.clamped()),
            ok => ok,
        };
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::BrowserExecuteResponse {
            id,
            request_id: request_id.to_string(),
            host_id: host_id.to_string(),
            outcome,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// The queue of commands pushed to this connection's host, handed out once.
    /// Run each on the receiver's own thread and answer with
    /// [`Self::browser_respond`]. The receiver reports the end when the
    /// connection fails.
    pub fn take_browser_requests(&self) -> Option<mpsc::Receiver<BrowserExecuteRequest>> {
        self.inner
            .browser_request_inbox
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
    }
}
