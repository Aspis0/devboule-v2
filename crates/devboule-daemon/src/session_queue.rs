//! One session's follow-up queue as the daemon holds it: the list, the
//! revision its snapshots carry, the flags that keep a send single, and the map
//! they live in.
//!
//! The queue is memory only. It is never written to the journal, never
//! appended to a transcript and never replayed, so a daemon restart starts
//! every session with an empty queue; the one that has to be recovered from is
//! the row in the UI, and the owner accepts that.
//!
//! Lock order: the registry's session map first, then this map — close,
//! delete and a queue edit take them in that order, and no path holding this
//! map takes the registry map. A send is never made under it: a claim takes
//! the row out and the send itself runs on its own. `session_queue_ops.rs`
//! drives the edits against it and `session_queue_drain.rs` the sends.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use devboule_protocol::{DroppedQueuedMessage, DroppedReason, QueuedMessage, WireError};

use super::session_queue_operations::{Fingerprint, OperationOutcome, QueuedOperation};

use super::ErrorCode;

/// The most messages one session's queue may hold.
///
/// A queue is the user's own waiting list, not a backlog channel: 64 rows is
/// well past what a person queues by hand, and the cap is what stops a
/// send-holding peer from growing a queue without bound. The byte budget
/// (below) is the other wall — this one counts rows, that one measures the
/// text they hold.
pub(crate) const MAX_QUEUE_ITEMS: usize = 64;

/// The most text one session's queue may hold across every row, in bytes.
///
/// The whole queue travels as one frame per snapshot, so the text budget is
/// what keeps a snapshot inside the frame cap: 64 KiB is the wire's own
/// per-prompt write cap (`MAX_WRITE_BYTES`), so a queue may hold as much text
/// as one prompt and its snapshot still fits comfortably inside the 1 MiB cap
/// with room for every row's envelope and recorded failure.
pub(crate) const MAX_QUEUE_TEXT_BYTES: usize = 64 * 1024;

/// The longest error a failed send leaves on its row.
///
/// A provider's refusal text is the provider's and unbounded; the row's copy
/// is the daemon's, and the text budget above is what keeps it bounded. Cut
/// at a character boundary so a multi-byte refusal cannot be sliced in half.
pub(crate) const MAX_QUEUE_ERROR_BYTES: usize = 512;

/// One session's queue. An absent entry in [`SessionQueues`] means "not
/// looked up yet", never "empty" — the same distinction the journal rows make.
#[derive(Clone, Debug)]
pub(crate) struct QueueState {
    pub(crate) items: Vec<QueuedMessage>,
    /// The next id this queue may mint. Daemon-assigned and never reused while
    /// the queue lives, so an edit or a move aimed at a removed row can never
    /// land on a different message.
    next_id: u64,
    /// Counts this session's queue states, one per published snapshot. A
    /// client drops a snapshot whose revision is not newer than one it has
    /// already applied.
    revision: u64,
    /// A send has claimed the front row and its outcome has not come back. One
    /// per session by construction: whoever sets it holds the claim.
    pub(crate) draining: bool,
    /// A turn-end hook is armed for this queue, so two arms cannot pile onto
    /// one turn's end.
    pub(crate) hook_armed: bool,
    /// The front row carries a recorded refusal and nothing resends it until a
    /// client acts. Without it the drain loop would take the same row again the
    /// moment it put it back.
    pub(crate) paused: bool,
    /// The session stopped being sendable and the queue is being cleared. Set
    /// under the same lock as the clear, checked by every frame, the drain and
    /// send-now, so nothing can reopen a queue whose session has gone. A
    /// successful resume clears it and reopens the queue for what is queued
    /// afterwards.
    pub(crate) fenced: bool,
    /// The client operation ids this queue has already answered, oldest first.
    /// In memory with the rest of the queue and never journaled: after a
    /// restart there is no queue to deduplicate against either.
    pub(crate) operations: VecDeque<QueuedOperation>,
    /// The client operation id of the send-now whose send is on the wire now,
    /// with the fingerprint of its payload. A repeat of this id is refused
    /// instead of being answered from the ring, because its outcome is not
    /// known yet: an id is remembered once it has one. The fingerprint tells a
    /// retry of the press from another payload under the same id.
    pub(crate) sending_operation: Option<(String, Fingerprint)>,
}

impl QueueState {
    pub(crate) fn new() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
            revision: 0,
            draining: false,
            hook_armed: false,
            paused: false,
            fenced: false,
            operations: VecDeque::new(),
            sending_operation: None,
        }
    }

    pub(crate) fn mint_id(&mut self) -> String {
        let id = format!("queue-{}", self.next_id);
        self.next_id += 1;
        id
    }

    pub(crate) fn position(&self, item_id: &str) -> Option<usize> {
        self.items.iter().position(|item| item.item_id == item_id)
    }

    /// The payload fingerprint of the send on the wire, when this client
    /// operation id is the one that started it.
    pub(crate) fn sending(&self, client_operation_id: &str) -> Option<Fingerprint> {
        self.sending_operation
            .as_ref()
            .filter(|(id, _)| id == client_operation_id)
            .map(|(_, fingerprint)| *fingerprint)
    }

    /// What this operation id ended up doing, once it is known: the ring for
    /// the ones already answered, nothing for the one still in flight.
    pub(crate) fn remember_outcome(
        &mut self,
        client_operation_id: &str,
        fingerprint: Fingerprint,
        outcome: OperationOutcome,
    ) {
        self.remember_operation(client_operation_id, fingerprint, outcome);
        if self.sending(client_operation_id).is_some() {
            self.sending_operation = None;
        }
    }

    /// The whole queue as a snapshot carries it, with the revision that
    /// snapshot is published under.
    pub(crate) fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            revision: self.revision,
            items: self.items.clone(),
            dropped: None,
        }
    }

    /// One accepted change: the state it leaves behind, and the revision its
    /// snapshot goes out under.
    pub(crate) fn publish(&mut self) -> QueueSnapshot {
        self.revision += 1;
        self.snapshot()
    }

    /// The same change, with the row this snapshot dropped named beside it. A
    /// drop is the one change a client cannot infer from the row list, so it
    /// travels with that list and with no other.
    pub(crate) fn publish_with_drop(&mut self, item_id: String) -> QueueSnapshot {
        let mut snapshot = self.publish();
        snapshot.dropped = Some(DroppedQueuedMessage {
            item_id,
            reason: DroppedReason::DeliveryUnknown,
        });
        snapshot
    }

    /// What the rows hold against the text budget, error headroom included:
    /// an error is written onto a row without an add, so the reserve has to
    /// exist before one does.
    pub(crate) fn text_bytes(&self) -> usize {
        self.items
            .iter()
            .map(|item| item.text.len() + MAX_QUEUE_ERROR_BYTES)
            .sum()
    }
}

/// The queue as one snapshot names it: the revision every client compares, the
/// rows in order, and the row this change dropped if it dropped one.
pub(crate) struct QueueSnapshot {
    pub(crate) revision: u64,
    pub(crate) items: Vec<QueuedMessage>,
    pub(crate) dropped: Option<DroppedQueuedMessage>,
}

/// Every live session's queue, behind one lock. The daemon's own memory: a
/// queue that no session names is dropped, never restored.
#[derive(Debug)]
pub(crate) struct SessionQueues {
    map: Mutex<HashMap<String, QueueState>>,
    /// This daemon process's instance id, stamped on every snapshot it
    /// publishes. A revision counts from 1 in this process's memory, so a
    /// client that kept one across a restart needs this to tell the two
    /// counters apart.
    epoch: String,
}

impl SessionQueues {
    pub(crate) fn new(epoch: String) -> Arc<Self> {
        Arc::new(Self {
            map: Mutex::new(HashMap::new()),
            epoch,
        })
    }

    pub(crate) fn epoch(&self) -> &str {
        &self.epoch
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, QueueState>> {
        self.map.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// The queue for one session, creating an empty one the first time this
    /// process is asked about it.
    pub(crate) fn entry(
        &self,
        session_id: &str,
    ) -> std::sync::MutexGuard<'_, HashMap<String, QueueState>> {
        let mut map = self.lock();
        map.entry(session_id.to_string())
            .or_insert_with(QueueState::new);
        map
    }

    /// The current snapshot of one session, or an empty one at revision 0 for a
    /// session nothing has queued. This is the attach hand-over's read: it
    /// creates no state, so attaching to a session with no queue does not make
    /// one.
    pub(crate) fn read(&self, session_id: &str) -> QueueSnapshot {
        self.lock()
            .get(session_id)
            .map(QueueState::snapshot)
            .unwrap_or(QueueSnapshot {
                revision: 0,
                items: Vec::new(),
                dropped: None,
            })
    }

    /// Drop one session's queue outright.
    pub(crate) fn forget(&self, session_id: &str) {
        self.lock().remove(session_id);
    }
}

pub(crate) fn no_such_item(item_id: &str) -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        format!("No queued message '{item_id}' is in this session's queue."),
    )
}

/// The refusal a fence answers with, whether the queue held rows or not.
pub(crate) fn queue_fenced() -> WireError {
    WireError::new(
        ErrorCode::SessionNotFound,
        "This session is closing; its queue no longer accepts messages.",
    )
}

/// The refusal a client operation id already answered with a different payload
/// gets. The id is echoed because the client chose it and can name it in a
/// bug report; the payload is not, because this daemon has no copy of it.
pub(crate) fn operation_conflict(client_operation_id: &str) -> WireError {
    WireError::new(
        ErrorCode::OperationConflict,
        format!(
            "This queue already answered client operation '{client_operation_id}' for a different message."
        ),
    )
}

/// The refusal a repeat of the same payload gets while the operation it names
/// is still being carried out. It is its own code because the client's answer
/// differs from a conflict's: it asks again with the identical id and payload
/// until the recorded answer arrives, where a conflict is final.
pub(crate) fn operation_in_flight(client_operation_id: &str) -> WireError {
    WireError::new(
        ErrorCode::OperationInFlight,
        format!(
            "Client operation '{client_operation_id}' is still being carried out for this session; \
             ask again with the same id and message until it answers."
        ),
    )
}

/// The refusal a session already sending another queued message gets.
pub(crate) fn queue_draining() -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        "A queued message is already being sent for this session.",
    )
}

/// The error a failed send leaves on its row, cut to the stored bound.
pub(crate) fn bounded_queue_error(message: &str) -> String {
    if message.len() <= MAX_QUEUE_ERROR_BYTES {
        return message.to_string();
    }
    let mut end = MAX_QUEUE_ERROR_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_string()
}

/// The queue as a test reads it: the rows, in order, and nothing when this
/// session's queue was never looked up.
#[cfg(test)]
pub(crate) fn queue_items_for_test(
    registry: &super::SessionRegistry,
    session_id: &str,
) -> Vec<QueuedMessage> {
    registry
        .queues
        .lock()
        .get(session_id)
        .map(|state| state.items.clone())
        .unwrap_or_default()
}

/// The revision this session's queue stands at, so a test can pin the ordering
/// without counting frames.
#[cfg(test)]
pub(crate) fn queue_revision_for_test(registry: &super::SessionRegistry, session_id: &str) -> u64 {
    registry
        .queues
        .lock()
        .get(session_id)
        .map_or(0, |state| state.revision)
}

/// Whether the daemon still holds any queue state for this session — the
/// entry itself, which an empty list and revision 0 cannot tell from no entry.
#[cfg(test)]
pub(crate) fn queue_is_held_for_test(registry: &super::SessionRegistry, session_id: &str) -> bool {
    registry.queues.lock().contains_key(session_id)
}
