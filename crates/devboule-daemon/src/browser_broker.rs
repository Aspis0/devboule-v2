//! The browser broker: the daemon side of "an agent's browser command runs in
//! the desktop app".
//!
//! The app registers a connection as a host. A caller hands the broker a
//! command; the broker picks the host, pushes `browser_execute_request` onto
//! that connection's outbound queue, and the caller waits on its own channel
//! with its own deadline. The host's answer arrives as a client frame on the
//! host's connection; the connection thread only resolves the pending entry
//! (a non-blocking send into the caller's one-slot channel) and moves on, so
//! no connection thread ever waits for another to answer.
//!
//! The hosts, the pending calls and the routing rules are `browser_registry`'s;
//! which host owns which tab is `browser_affinity`'s. Nothing here reads or
//! logs a URL, an argument, a result or page text: the log lines carry a
//! command name, an id and a code.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use devboule_protocol::{
    BrowserCaller, BrowserError, BrowserErrorCode, BrowserExecuteRequest, BrowserOutcome,
    DaemonMessage, MAX_BROWSER_PAYLOAD_BYTES,
};
use serde_json::Value;

use crate::browser_registry::{Admitted, Answer, Call, Registry};
use crate::outbound::ConnOut;

/// How long a call waits for its host when the caller names no deadline.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// What the connection thread learns from handing a host's answer over.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResponseDisposition {
    Delivered,
    /// Late, unknown, or from a connection that does not own the host. The
    /// sender learns nothing from it.
    Dropped,
    /// The answer was refused and the pending call failed with the same error;
    /// the host is told why.
    Refused(BrowserError),
}

pub(crate) struct BrowserBroker {
    registry: Mutex<Registry>,
}

impl BrowserBroker {
    pub(crate) fn new() -> Self {
        Self {
            registry: Mutex::new(Registry::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Make `conn_id` the browser host, replacing its earlier registration and
    /// failing that one's calls.
    pub(crate) fn register(
        &self,
        conn_id: u64,
        outbound: Arc<ConnOut>,
        commands: Vec<String>,
    ) -> String {
        self.lock().add_host(conn_id, outbound, commands)
    }

    /// Whether `conn_id` held `host_id` and gave it up.
    pub(crate) fn unregister(&self, conn_id: u64, host_id: &str) -> bool {
        self.lock().unregister(conn_id, host_id)
    }

    /// The connection ended: whatever host it held goes with it.
    pub(crate) fn connection_closed(&self, conn_id: u64) {
        self.lock().drop_connection(conn_id);
    }

    /// Run `command` on a browser host and wait for the answer.
    ///
    /// `caller` is the daemon's own knowledge of who is asking and is what the
    /// host receives; a `caller` key in `args` is dropped. An unscoped call
    /// goes to the most recently registered host. A call that names a tab the
    /// broker has seen for this caller's workspace routes to the host that owns
    /// it, and when that host is gone it fails as `browser_owner_unavailable`
    /// rather than moving.
    ///
    /// Waits on this thread only, up to `timeout` (15 s when `None`); the
    /// connection threads never wait for it.
    pub(crate) fn execute(
        &self,
        caller: &BrowserCaller,
        command: &str,
        mut args: Value,
        browser_id: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<Value, BrowserError> {
        if let Value::Object(map) = &mut args {
            map.remove("caller");
        }
        if compact_len(&args) > MAX_BROWSER_PAYLOAD_BYTES {
            return Err(BrowserError::daemon(
                BrowserErrorCode::ArgsTooLarge,
                "The browser command's arguments are too large to send.",
            ));
        }
        let (answer, waiting) = mpsc::sync_channel(1);
        let call = Call {
            command,
            workspace_id: caller.workspace_id.as_deref(),
            browser_id,
        };
        let admitted = self.lock().admit(&call, answer);
        let Admitted {
            request_id,
            host_id,
            outbound,
        } = admitted.inspect_err(|error| log_failure(None, error))?;
        outbound.enqueue_reply(DaemonMessage::BrowserExecuteRequest(
            BrowserExecuteRequest {
                request_id: request_id.clone(),
                host_id,
                command: command.to_string(),
                args,
                caller: caller.clone(),
            },
        ));
        let outcome = match waiting.recv_timeout(timeout.unwrap_or(DEFAULT_TIMEOUT)) {
            Ok(answer) => answer,
            Err(RecvTimeoutError::Timeout) => self.expire(&request_id, &waiting),
            Err(RecvTimeoutError::Disconnected) => Err(BrowserError::daemon(
                BrowserErrorCode::NoHost,
                "The browser host went away before it answered.",
            )),
        };
        outcome.inspect_err(|error| log_failure(Some((command, &request_id)), error))
    }

    /// The deadline passed. If the answer landed between the deadline and this
    /// lock the entry is already gone and the answer is in the channel; take it
    /// rather than reporting a timeout for a call that finished.
    fn expire(&self, request_id: &str, waiting: &Receiver<Answer>) -> Answer {
        let timed_out = || {
            Err(BrowserError::daemon(
                BrowserErrorCode::Timeout,
                "The browser host did not answer in time.",
            ))
        };
        if self.lock().take_pending(request_id).is_some() {
            return timed_out();
        }
        waiting.try_recv().unwrap_or_else(|_| timed_out())
    }

    /// Hand a host's answer to the call waiting for it. Never blocks: the
    /// call's channel has room for exactly this one answer.
    ///
    /// Accepted only from the connection that owns `host_id`, for a call that
    /// is still pending on that host. Anything else is dropped, with a log line
    /// that names the connection and no content. A host's error text is cut to
    /// `MAX_BROWSER_ERROR_MESSAGE_BYTES` before a caller sees it.
    pub(crate) fn accept_response(
        &self,
        conn_id: u64,
        request_id: &str,
        host_id: &str,
        outcome: BrowserOutcome,
    ) -> ResponseDisposition {
        let too_large = matches!(&outcome, BrowserOutcome::Ok { result }
            if compact_len(result) > MAX_BROWSER_PAYLOAD_BYTES);
        let mut registry = self.lock();
        if !registry.expects(conn_id, request_id, host_id) {
            eprintln!("browser host answer dropped on connection {conn_id}");
            return ResponseDisposition::Dropped;
        }
        let Some(pending) = registry.take_pending(request_id) else {
            return ResponseDisposition::Dropped;
        };
        if too_large {
            let error = BrowserError::daemon(
                BrowserErrorCode::ResultTooLarge,
                "The browser result is too large to return.",
            );
            let _ = pending.answer.try_send(Err(error.clone()));
            return ResponseDisposition::Refused(error);
        }
        let answer = match outcome {
            BrowserOutcome::Ok { result } => {
                registry.settle_tabs(&pending, &result, request_id);
                Ok(result)
            }
            BrowserOutcome::Err(error) => Err(error.clamped()),
        };
        let _ = pending.answer.try_send(answer);
        ResponseDisposition::Delivered
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.lock().pending_len()
    }

    #[cfg(test)]
    pub(crate) fn host_count(&self) -> usize {
        self.lock().host_count()
    }
}

fn compact_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// A command name is logged only once a host registered it; a refusal before
/// that is the caller's string and stays out.
fn log_failure(call: Option<(&str, &str)>, error: &BrowserError) {
    match call {
        Some((command, request_id)) => {
            eprintln!(
                "browser command {command} ({request_id}) failed: {:?}",
                error.code
            );
        }
        None => eprintln!("browser command refused: {:?}", error.code),
    }
}

#[cfg(test)]
#[path = "browser_broker_test_support.rs"]
mod support;

#[cfg(test)]
#[path = "browser_broker_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "browser_broker_limits_tests.rs"]
mod limits_tests;

#[cfg(test)]
#[path = "browser_broker_tab_tests.rs"]
mod tab_tests;
