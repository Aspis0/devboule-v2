//! The task that drains one target's socket: replies to the waiter holding
//! their id, notifications to the subscriber, and every waiter in flight
//! released on the way out however the socket ends.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, MutexGuard, PoisonError};

use futures_util::stream::SplitStream;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use super::{Feed, Outcome, Pending, Socket, WsEvent};
use crate::browser::console;

/// The waiters, taken past a poisoned lock. A sender is inserted whole or not
/// at all, so the entries are still the truth about which calls are waiting —
/// and refusing to look would strand them, which is the one thing this module
/// must never do. It matters most on the panic path, where a second `expect`
/// would abort the process instead of answering the callers.
pub(super) fn entries(pending: &Pending) -> MutexGuard<'_, HashMap<u64, oneshot::Sender<Outcome>>> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whatever ends the reader — a closed socket, a message past the cap, a
/// panic — takes the waiters in flight with it, and marks the page gone so the
/// next call is refused rather than made to wait. Held for the reader's whole
/// body, so the panic path is covered by the same code as every other exit.
pub(super) struct ReleasedOnExit {
    pending: Pending,
    gone: Arc<AtomicBool>,
}

impl ReleasedOnExit {
    /// The guard the reader holds for its whole body, and the one `close`
    /// drops to release the waiters before it attempts its own frame.
    pub(super) fn new(pending: &Pending, gone: &Arc<AtomicBool>) -> Self {
        ReleasedOnExit {
            pending: Arc::clone(pending),
            gone: Arc::clone(gone),
        }
    }
}

impl Drop for ReleasedOnExit {
    fn drop(&mut self) {
        self.gone.store(true, Ordering::SeqCst);
        for (_, waiter) in entries(&self.pending).drain() {
            let _ = waiter.send(Outcome::Gone);
        }
    }
}

/// Route everything the socket says: a frame carrying an id is the answer to
/// one call, and a frame carrying a method is a notification. Ends when the
/// socket does, which releases every waiter that will now never be answered.
pub(super) async fn read(
    mut socket: SplitStream<Socket>,
    pending: Pending,
    feed: Feed,
    gone: Arc<AtomicBool>,
) {
    let routed = Arc::clone(&pending);
    let _released = ReleasedOnExit { pending, gone };
    while let Some(frame) = socket.next().await {
        let Ok(frame) = frame else { break };
        match frame {
            Message::Text(text) => route(&routed, &feed, text.as_str()).await,
            Message::Binary(bytes) => route(&routed, &feed, &String::from_utf8_lossy(&bytes)).await,
            Message::Close(_) => break,
            // A ping is answered by the socket itself, and a pong is an answer
            // to nothing this page asked.
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
}

async fn route(pending: &Pending, feed: &Feed, text: &str) {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return;
    };
    match (
        message.get("id").and_then(Value::as_u64),
        message.get("method").and_then(Value::as_str),
    ) {
        (Some(id), _) => {
            if let Some(waiter) = entries(pending).remove(&id) {
                let _ = waiter.send(outcome_of(&message));
            }
        }
        (None, Some(method)) => hand(
            feed,
            WsEvent {
                method: method.to_owned(),
                params: message.get("params").cloned().unwrap_or(Value::Null),
            },
        ),
        // Neither an id nor a method: the protocol did not send this, and a
        // notification with no name is nothing a subscriber could act on.
        (None, None) => {}
    }
}

/// Hand one notification to its own stream, or count what a full queue would
/// not take. A dropped state event wakes the watcher: it is an event a ref or
/// a settle may hang on, and losing it silently is the one thing this must
/// not do. A dropped voice event loses a console line, wakes nobody and is
/// only counted.
fn hand(feed: &Feed, event: WsEvent) {
    let (stream, dropped, wakes) = if console::is_voice(&event.method) {
        (&feed.voice, &feed.dropped.voice, false)
    } else {
        (&feed.state, &feed.dropped.state, true)
    };
    match stream.try_send(event) {
        Ok(()) => {}
        // The reader never waits on a full queue: a reader that stops routing
        // answers stalls every call in flight behind it.
        Err(mpsc::error::TrySendError::Full(_)) => {
            dropped.fetch_add(1, Ordering::SeqCst);
            if wakes {
                feed.notice.notify_one();
            }
        }
        // Nothing is listening: the watcher was dropped, and a count nobody
        // will read is not a loss.
        Err(mpsc::error::TrySendError::Closed(_)) => {}
    }
}

/// The two halves of a response, which arrive under the same id.
fn outcome_of(message: &Value) -> Outcome {
    match (message.get("result"), message.get("error")) {
        (_, Some(error)) => Outcome::Refused(error.clone()),
        (Some(result), None) => Outcome::Answered(result.clone()),
        (None, None) => Outcome::Refused(message.clone()),
    }
}
