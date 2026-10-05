//! One DevTools Protocol page reached over a websocket, for a target this app
//! did not embed.
//!
//! The WebView2 child answers the same [`Page`] trait from inside the window.
//! A Chromium this app started for the browser host answers it from a debugger
//! socket on loopback. Nothing above the trait can tell the two apart, and that
//! is the whole point of it: the command layer, the method strings and the
//! refusal mapping are the compatibility boundary, and this is the second thing
//! standing behind it.
//!
//! **The socket is the page's whole life.** One reader task, one id per call,
//! one waiter per id, and every waiter released the moment its answer can no
//! longer arrive — a socket that closes under a call is the same refusal the
//! WebView2 adapter gives for a page that stopped answering.
//!
//! **A call's budget covers the write, not only the wait.** Queueing behind
//! another writer and writing to a socket that has stopped reading both take
//! time a command's budget does not have, so one absolute deadline is taken
//! before anything is queued and everything after it is cut to what is left.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::stream::SplitSink;
use futures_util::SinkExt;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};
use tokio::time::{timeout_at, Instant};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};
use url::{Host, Url};

use self::read::{entries, read, ReleasedOnExit};
use super::cdp::{addresses_node, Call, CdpError, Page};

/// The cap the WebView2 adapter puts on a wait, for the same reason it puts it
/// there: a call that has not been answered by now is not coming, and a
/// command must not outlive its own budget.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// How long `close` waits for its own Close frame to go out. The frame is a
/// courtesy to the peer; the reader is stopped either way, and a write that
/// will not drain must not hold a closing tab open.
const CLOSE_WRITE: Duration = Duration::from_secs(2);

/// How many events a reader holds for a subscriber that is not reading yet.
/// A full channel drops the newest event rather than blocking the reader,
/// because a blocked reader stops routing ANSWERS and every command in flight
/// would then run out of its budget instead.
const EVENT_QUEUE: usize = 256;

/// The most one CDP message may be. The broker already refuses a browser answer
/// whose payload passes `devboule_protocol::MAX_BROWSER_PAYLOAD_BYTES`, so a
/// bigger message could never cross the pipe this app already has and reading
/// it would only spend the memory. The 16 KiB on top is the envelope — the
/// request id, the method name and the JSON punctuation — the room the
/// protocol crate leaves for the same reason. It caps the frame as well as the
/// message, so a peer cannot stream frames forever before the message cap
/// trips: past either of them the socket cannot be resumed, the reader ends and
/// the page must be opened again.
const MAX_MESSAGE: usize = devboule_protocol::MAX_BROWSER_PAYLOAD_BYTES + 16 * 1024;

/// The socket as its two owners see it: the reader task holds the read half,
/// every call takes the write half in turn.
pub(super) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Waiters by request id — one entry per call in flight, taken out by the
/// reader when its answer arrives and by the call when its budget runs out.
/// A plain lock, never held across an await: the reader's own exit has to be
/// able to take every waiter out without waiting for anything.
pub(super) type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Outcome>>>>;

/// What came back under one id: the result, the protocol's own error object,
/// or nothing at all, because the socket went away under the call.
pub(super) enum Outcome {
    Answered(Value),
    Refused(Value),
    Gone,
}

/// One CDP notification as the wire carries it: its method and its parameters.
/// Which notifications a caller acts on is the event layer's business.
pub struct WsEvent {
    pub method: String,
    pub params: Value,
}

/// One page target's debugger socket.
pub struct WsPage {
    writer: AsyncMutex<SplitSink<Socket, Message>>,
    pending: Pending,
    reader: AsyncMutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    next_id: AtomicU64,
    /// Set when the socket is finished, by `close` and by the reader's own
    /// exit: a page with no socket left refuses at once instead of making
    /// every later call sit out its own budget for an answer that cannot come.
    gone: Arc<AtomicBool>,
}

impl WsPage {
    /// Open one page target's debugger socket, and hand back the stream of
    /// events its reader sees. Only a `ws://` address on this machine's own
    /// loopback is opened: a debugger on another host is not a page this app
    /// owns.
    pub async fn connect(url: &str) -> Result<(Self, mpsc::Receiver<WsEvent>), CdpError> {
        loopback(url)?;
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(MAX_MESSAGE);
        config.max_frame_size = Some(MAX_MESSAGE);
        let opened = timeout_at(
            Instant::now() + CALL_TIMEOUT,
            connect_async_with_config(url, Some(config), false),
        )
        .await;
        let (socket, _) = match opened {
            Ok(Ok(pair)) => pair,
            Ok(Err(error)) => return Err(CdpError::Refused(format!("{url}: {error}"))),
            Err(_) => {
                return Err(CdpError::Refused(format!(
                    "{url}: the debugger did not answer"
                )))
            }
        };
        let (writer, reader) = socket.split();
        let pending: Pending = Arc::default();
        let (events, seen) = mpsc::channel(EVENT_QUEUE);
        let gone = Arc::new(AtomicBool::new(false));
        let task = tauri::async_runtime::spawn(read(
            reader,
            Arc::clone(&pending),
            events,
            Arc::clone(&gone),
        ));
        Ok((
            WsPage {
                writer: AsyncMutex::new(writer),
                pending,
                reader: AsyncMutex::new(Some(task)),
                next_id: AtomicU64::new(0),
                gone,
            },
            seen,
        ))
    }

    /// Close the target's debugger socket. The waiters are released before the
    /// Close frame is even attempted, because a write can sit behind a
    /// backpressured socket and a caller holding an answer that will never
    /// arrive must not wait for that write to give up. The reader is stopped
    /// with it: a websocket read carries no deadline of its own, so a reader
    /// left running after its target is gone is a task that never ends.
    pub async fn close(&self) {
        if self.gone.swap(true, Ordering::SeqCst) {
            return;
        }
        drop(ReleasedOnExit::new(&self.pending, &self.gone));
        let closing = async {
            let mut writer = self.writer.lock().await;
            let _ = writer.send(Message::Close(None)).await;
        };
        let _ = tokio::time::timeout(CLOSE_WRITE, closing).await;
        if let Some(reader) = self.reader.lock().await.take() {
            reader.inner().abort();
        }
    }

    /// How many calls are holding a waiter right now, which is how a test sees
    /// that a call that ran out of its budget took its entry with it.
    #[cfg(test)]
    async fn waiting(&self) -> usize {
        entries(&self.pending).len()
    }

    /// Stop the reader task the way a cancellation stops it, without closing
    /// the page first. What releases the waiters afterwards can only be the
    /// guard the reader itself holds, which is what makes this a proof about
    /// the production task rather than about the guard on its own.
    #[cfg(test)]
    async fn cancel_the_reader(&self) {
        if let Some(reader) = self.reader.lock().await.take() {
            reader.inner().abort();
        }
    }

    /// The write half, for a test that has to hold it. Loopback takes a frame
    /// far bigger than any real call before a write stops draining, and one
    /// that big spends longer being masked than any budget would allow, so a
    /// stalled writer is cheaper to arrange than to provoke.
    #[cfg(test)]
    async fn hold_the_writer(&self) -> tokio::sync::MutexGuard<'_, SplitSink<Socket, Message>> {
        self.writer.lock().await
    }

    async fn send(&self, method: &str, params: Value, limit: Duration) -> Result<Value, CdpError> {
        if self.gone.load(Ordering::SeqCst) {
            return Err(unanswered(method));
        }
        let budget = limit.min(CALL_TIMEOUT);
        if budget.is_zero() {
            // No time is no command. A call with no budget left must leave the
            // page exactly as it found it, so nothing is registered and
            // nothing is written: a navigation sent into a command that can no
            // longer be answered is a side effect nobody will ever see.
            return Err(unanswered(method));
        }
        let deadline = Instant::now() + budget;
        let named_node = addresses_node(&params);
        let (outcome, answer) = oneshot::channel();
        let id = self.register(outcome);
        if let Err(error) = self
            .write(
                deadline,
                method,
                json!({ "id": id, "method": method, "params": params }),
            )
            .await
        {
            self.forget(id);
            return Err(error);
        }
        match timeout_at(deadline, answer).await {
            Ok(Ok(Outcome::Answered(value))) => Ok(value),
            Ok(Ok(Outcome::Refused(error))) => Err(refusal(method, named_node, &error)),
            Ok(Ok(Outcome::Gone)) | Ok(Err(_)) | Err(_) => {
                self.forget(id);
                Err(unanswered(method))
            }
        }
    }

    /// Claim an id and the waiter that answers it. An id still in flight is
    /// never handed out twice: a late reply under it would then complete
    /// whichever call took it next.
    fn register(&self, outcome: oneshot::Sender<Outcome>) -> u64 {
        let mut pending = entries(&self.pending);
        loop {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            if let Entry::Vacant(slot) = pending.entry(id) {
                slot.insert(outcome);
                return id;
            }
        }
    }

    /// Take a waiter back out: an answer that arrives after the budget is
    /// spent, or after the frame gave up, has nobody left to hand it to.
    fn forget(&self, id: u64) {
        entries(&self.pending).remove(&id);
    }

    async fn write(&self, deadline: Instant, method: &str, frame: Value) -> Result<(), CdpError> {
        // One writer at a time: the lock is what keeps two calls' frames in
        // the order the protocol reads them back in, and it is waited for
        // under the same deadline as the write.
        let Ok(mut writer) = timeout_at(deadline, self.writer.lock()).await else {
            return Err(unanswered(method));
        };
        // Read again now that the writer is ours. A call queued before `close`
        // began must not slip a command onto a socket being torn down behind
        // it: the flag is set before `close` queues its own frame, so anything
        // that has not reached this line by then goes out no further. A frame
        // already inside `send` when the flag was set cannot be recalled, and
        // did not wait for this page to close.
        if self.gone.load(Ordering::SeqCst) {
            return Err(unanswered(method));
        }
        match timeout_at(deadline, writer.send(Message::text(frame.to_string()))).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(CdpError::Refused(format!("{method}: {error}"))),
            Err(_) => Err(unanswered(method)),
        }
    }
}

impl Page for WsPage {
    fn call<'a>(&'a self, method: &'a str, params: Value) -> Call<'a> {
        self.call_within(method, params, CALL_TIMEOUT)
    }

    fn call_within<'a>(&'a self, method: &'a str, params: Value, limit: Duration) -> Call<'a> {
        Box::pin(async move { self.send(method, params, limit).await })
    }
}

/// The waiters, taken past a poisoned lock. A sender is inserted whole or not
/// at all, so the entries are still the truth about which calls are waiting —
/// and refusing to look would strand them, which is the one thing this module
/// must never do. It matters most on the panic path, where a second `expect`
/// would abort the process instead of answering the callers.
/// A debugger websocket is the one network address this app opens, and it has
/// to stay on this machine. The address is built from the port the browser
/// wrote into its own profile, so it is a numeric literal: a name is refused
/// rather than resolved, because what a name resolves to on this machine is not
/// this app's decision to make.
fn loopback(url: &str) -> Result<(), CdpError> {
    let refused = || CdpError::Refused(format!("{url} is not a loopback ws:// debugger address."));
    let Ok(parsed) = Url::parse(url) else {
        return Err(refused());
    };
    if parsed.scheme() != "ws" {
        return Err(refused());
    }
    match parsed.host() {
        Some(Host::Ipv4(address)) if address.is_loopback() => Ok(()),
        Some(Host::Ipv6(address)) if address.is_loopback() => Ok(()),
        _ => Err(refused()),
    }
}

/// What a protocol error object means. An unknown method, a misspelled
/// argument and a dead `backendNodeId` arrive here as error objects, exactly
/// as WebView2 flattens all three into one HRESULT. The rule that tells them
/// apart is the same one: a refusal of a call whose parameters named a node is
/// a stale ref, and anything else is the method or argument problem it is.
fn refusal(method: &str, named_node: bool, error: &Value) -> CdpError {
    if named_node {
        return CdpError::StaleRef;
    }
    let text = error
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| error.to_string(), str::to_owned);
    CdpError::Refused(format!("{method}: {text}"))
}

/// The text a page that stops answering gets, the same one the WebView2
/// adapter gives a completion that never arrives.
fn unanswered(method: &str) -> CdpError {
    CdpError::Refused(format!("{method}: the page did not answer"))
}

#[path = "cdp_ws_read.rs"]
mod read;

#[cfg(test)]
#[path = "cdp_ws_fake.rs"]
mod fake;

#[cfg(test)]
#[path = "cdp_ws_cft_tests.rs"]
mod cft;

#[cfg(test)]
#[path = "cdp_ws_budget_tests.rs"]
mod budget_tests;

#[cfg(test)]
#[path = "cdp_ws_life_tests.rs"]
mod life_tests;

#[cfg(test)]
#[path = "cdp_ws_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cdp_ws_events_support.rs"]
mod events_support;

#[cfg(test)]
#[path = "cdp_ws_events_tests.rs"]
mod events_tests;

#[cfg(test)]
#[path = "cdp_ws_watch_tests.rs"]
mod watch_tests;
