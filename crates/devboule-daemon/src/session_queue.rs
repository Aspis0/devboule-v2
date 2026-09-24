//! The per-session message queue's state and its journal seam: the map the
//! edits and sends take items from, the load a first touch does, the
//! write-through every change persists, and the snapshot every change
//! publishes. The four frame edits live in `session_queue_ops.rs`, send-now
//! in `session_queue_send.rs`, the drain in `session_queue_drain.rs` — this
//! file is the state and where it is kept.
//!
//! Two rules run through every write here (fix pass 1): a queue operation
//! whose journal write failed returns that failure and commits nothing, so
//! the in-memory queue never drifts from the durable one; and the snapshot's
//! frame is bounded, because the whole queue travels as one event.
//!
//! Lock order: this module's map may be held while the registry's session map
//! or a runtime lock is taken, never the reverse — no path holding either of
//! those may reach `SessionQueues`, so the two orders cannot meet. The map is
//! **not** held across a send: a send claims its item under the lock and
//! sends outside it.

use super::{
    internal, not_found, peer_entry, process_gone, Arc, ConnHandle, ErrorCode, HashMap, Mutex,
    OwnerId, SessionEvent, SessionRegistry, SessionRuntime, WireError,
};
use devboule_protocol::QueuedMessage;

/// The most messages one session's queue may hold. A queue is the user's own
/// waiting list, not a backlog channel: 64 rows is past what a person queues
/// by hand, and the cap is what keeps a `send`-holding peer from growing the
/// journal without bound. The byte budget (below) is the other wall — this
/// one counts rows, that one measures the frame they would travel in.
pub(super) const MAX_QUEUE_ITEMS: usize = 64;

/// The longest error a failed send leaves on its item. A provider's refusal
/// text is the provider's, unbounded; the item's copy is the daemon's, and
/// the frame budget has reserved exactly this much room for it.
pub(super) const MAX_QUEUE_ERROR_BYTES: usize = 512;

/// What the add/edit budget holds back from `MAX_FRAME_BYTES` for what the
/// later publish paths add: the bounded error at its JSON-escaped worst (a
/// control byte can cost six), the field's own quotes, and the claim's
/// flag flip — so a queue accepted at the boundary can still carry its
/// failure (P1-2).
pub(super) const QUEUE_ERROR_HEADROOM: usize = 4096;

/// One session's queue as the daemon holds it in memory. An absent entry in
/// [`SessionQueues`] means "not looked up yet", never "empty".
#[derive(Clone, Debug)]
pub(super) struct QueueState {
    pub(super) items: Vec<QueuedMessage>,
    /// The next id this queue may mint. It comes from the persisted counter
    /// when the store has one — never from the surviving rows alone, so an
    /// id issued before a restart cannot come back as a different message
    /// (P2-6) — and climbs monotonically while the queue lives.
    pub(super) next_id: u64,
    /// A queue send is in flight. One per session, by construction: whoever
    /// sets it takes the claim with it, and it clears only when that send has
    /// succeeded or failed.
    pub(super) draining: bool,
    /// An `on_turn_end` hook is armed for this queue: set only while a turn
    /// was running when the arm was taken, cleared only by the hook firing,
    /// so two arms cannot pile onto one turn end.
    pub(super) hook_armed: bool,
    /// Draining stopped — a failed send, an unwritable store, or a restored
    /// in-flight item — and waits for a queue frame from a client or an act
    /// on the item itself before it tries again (the Q1 brief's pause, and
    /// the fence under "never resent silently").
    pub(super) paused: bool,
}

impl QueueState {
    pub(super) fn new(items: Vec<QueuedMessage>, counter: Option<u64>) -> Self {
        // The floor from the surviving rows keeps a database the counter's
        // row has not reached yet as safe as it was; a present counter wins
        // because it never went backwards while rows were being removed.
        let by_rows = items
            .iter()
            .filter_map(|item| item.item_id.strip_prefix("queue-"))
            .filter_map(|suffix| suffix.parse::<u64>().ok())
            .max()
            .map_or(1, |largest| largest + 1);
        Self {
            items,
            next_id: counter.map_or(by_rows, |value| value.max(by_rows)),
            draining: false,
            hook_armed: false,
            paused: false,
        }
    }

    pub(super) fn mint_id(&mut self) -> String {
        let id = format!("queue-{}", self.next_id);
        self.next_id += 1;
        id
    }

    pub(super) fn position(&self, item_id: &str) -> Option<usize> {
        self.items.iter().position(|item| item.item_id == item_id)
    }

    /// The queue as the snapshot carries it.
    pub(super) fn snapshot(&self) -> Vec<QueuedMessage> {
        self.items.clone()
    }
}

/// Every live session's queue, behind one lock. A close forgets a state
/// explicitly, and a restore loads it from the journal.
#[derive(Debug)]
pub(super) struct SessionQueues {
    map: Mutex<HashMap<String, QueueState>>,
}

impl SessionQueues {
    pub(super) fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, QueueState>> {
        self.map.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Drop the states of exactly these sessions — the queue-map half of a
    /// retention sweep, reported by the journal the moment it commits one
    /// (P2-6). The rows are gone when this runs, so a state still resident
    /// is by definition claiming a state the disk does not hold.
    pub(crate) fn drop_states(&self, session_ids: &[String]) {
        let mut map = self.lock();
        for session_id in session_ids {
            map.remove(session_id);
        }
    }
}

pub(super) fn no_such_item(item_id: &str) -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        format!("No queued message '{item_id}' is in this session's queue."),
    )
}

/// The bytes this snapshot would occupy as the connection frames it — the
/// whole queue travels as one `SubscriptionEvent` — with the one field the
/// queue knows exactly (`sessionId`) passed and every field it cannot know
/// at check time (`generation`, `transcriptSeq`, `subscriptionId`) at their
/// longest. Conservative by construction: the real frame is never longer, so
/// a queue under this cap can always be published (P1-2, P2-11).
fn snapshot_frame_bytes(session_id: &str, items: &[QueuedMessage]) -> usize {
    let message = devboule_protocol::DaemonMessage::SubscriptionEvent {
        subscription_id: u64::MAX,
        envelope: devboule_protocol::SessionEventEnvelope {
            session_id: session_id.to_string(),
            generation: u64::MAX,
            transcript_seq: Some(u64::MAX),
            event: SessionEvent::QueueSnapshot {
                items: items.to_vec(),
            },
        },
    };
    // A serialization that fails is a queue that must not be published:
    // report the impossible size so the cap refuses it.
    serde_json::to_vec(&message).map_or(usize::MAX, |bytes| bytes.len())
}

/// The one door a queue snapshot leaves through. Every publish path — the
/// edits, the drain's claim and settle, send-now, the attach hand-over —
/// measures the frame here, so no path can hand the connection an
/// oversized frame, which the writer would refuse *with the connection*.
/// A snapshot over the cap is not published: the failure is logged, memory
/// and journal keep the queue, and the next snapshot that fits carries the
/// state instead — an undelivered update is recoverable; a disconnected
/// subscriber is not (P1-2).
pub(super) fn deliver_queue_snapshot(
    session_id: &str,
    runtime: &SessionRuntime,
    items: Vec<QueuedMessage>,
) {
    if snapshot_frame_bytes(session_id, &items) > devboule_protocol::MAX_FRAME_BYTES {
        eprintln!("queue snapshot for {session_id} is over the frame cap; it was not published");
        return;
    }
    runtime.publish_queue_snapshot(items);
}

/// The error a failed send leaves on its item, cut to the stored bound at
/// a character boundary — the headroom the add budget reserved is sized
/// for exactly this much.
pub(super) fn bounded_queue_error(message: &str) -> String {
    if message.len() <= MAX_QUEUE_ERROR_BYTES {
        return message.to_string();
    }
    let mut end = MAX_QUEUE_ERROR_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_string()
}

impl SessionRegistry {
    /// The session a queue op acts on: it exists, the caller may reach it, it
    /// is live, and it is an agent — a terminal has no queue, and the refusal
    /// is typed so a client can tell it apart from a missing session.
    pub(super) fn queue_target(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<Arc<SessionRuntime>, WireError> {
        let resolved = (|| {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = peer_entry(&map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible().ok_or_else(process_gone)?;
            if !session.metadata.kind.is_agent() {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "Only agent sessions have a message queue.",
                ));
            }
            Ok(Arc::clone(&session.runtime))
        })();
        // The seam fires only for a target that resolved and only after the
        // registry lock above has been released: the test lands a close in
        // the gap between this resolve and the queue mutation that follows
        // (fix pass 1, P2-5), and a close needs that lock.
        #[cfg(test)]
        if resolved.is_ok() {
            self.fire_queue_after_target_hook();
        }
        resolved
    }

    /// Still a live, peer-visible session — the re-check every commit point
    /// makes under this module's lock, because close and delete discard the
    /// queue and an op that resolved the session before either must not
    /// recreate its state or its rows afterwards (P2-5, P2-7). Lock order:
    /// this takes the registry map while this module's map is held, the one
    /// nesting the header allows.
    pub(super) fn queue_session_live(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .map(|map| {
                map.get(session_id)
                    .is_some_and(|entry| entry.as_peer_visible().is_some())
            })
            .unwrap_or(false)
    }

    /// Read this session's queue from the journal the first time this
    /// process touches it. Called with this module's map held — the read is a
    /// journal round trip, and the map is what makes "loaded" a fact rather
    /// than a guess. An unreadable queue refuses the op: a journal this
    /// daemon cannot read must not read as an emptied queue.
    ///
    /// Two fences come back with the rows: a row still marked in flight (the
    /// daemon died between claim and settle) comes back at the front with the
    /// queue paused under it — it may not have been sent, and nothing resends
    /// it automatically, the snapshot's `unconfirmed` flag is what asks the
    /// user (P1-4); and the durable pause (P1-6) comes back as it was left,
    /// because a pause that only lived in memory would send, after a
    /// restart, exactly the queue a failure had stopped.
    pub(super) fn ensure_queue_loaded(
        &self,
        session_id: &str,
        queues: &mut HashMap<String, QueueState>,
    ) -> Result<(), WireError> {
        if queues.contains_key(session_id) {
            return Ok(());
        }
        let (items, counter, durable_pause) = match &self.journal {
            None => (Vec::new(), None, false),
            Some(journal) => {
                let (rows, counter, durable_pause) =
                    journal.queue_load(session_id).map_err(|error| {
                        eprintln!("message queue unreadable for {session_id}: {error}");
                        WireError::new(
                            ErrorCode::Journal,
                            "This session's message queue could not be read.",
                        )
                    })?;
                let items = rows
                    .into_iter()
                    .map(|row| QueuedMessage {
                        item_id: row.item_id,
                        text: row.text,
                        attachment_references: row.attachment_references,
                        error: row.error,
                        unconfirmed: row.unconfirmed,
                    })
                    .collect();
                (items, counter, durable_pause)
            }
        };
        let mut state = QueueState::new(items, counter);
        if let Some(position) = state
            .items
            .iter()
            .position(|item| item.unconfirmed)
            .filter(|position| *position > 0)
        {
            let item = state.items.remove(position);
            state.items.insert(0, item);
        }
        if durable_pause || state.items.first().is_some_and(|item| item.unconfirmed) {
            state.paused = true;
        }
        queues.insert(session_id.to_string(), state);
        Ok(())
    }

    /// Persist one session's queue exactly as it stands — the same snapshot
    /// the event carries, written to the rows a restart reads back, the pause
    /// with them — and return the store's failure instead of hiding it: the
    /// caller commits nothing on `Err`, so memory and disk stay equal (P1-3).
    /// The log line names the session and the error, never the messages.
    pub(super) fn persist_queue(
        &self,
        session_id: &str,
        items: &[QueuedMessage],
        next_id: u64,
        paused: bool,
    ) -> Result<(), WireError> {
        let Some(journal) = &self.journal else {
            return Ok(());
        };
        let rows: Vec<crate::journal::QueueRecord> = items
            .iter()
            .map(|item| crate::journal::QueueRecord {
                item_id: item.item_id.clone(),
                text: item.text.clone(),
                attachment_references: item.attachment_references.clone(),
                error: item.error.clone(),
                unconfirmed: item.unconfirmed,
            })
            .collect();
        journal
            .queue_store(session_id, &rows, next_id, paused)
            .map_err(|error| {
                eprintln!("message queue could not be persisted for {session_id}: {error}");
                WireError::from(error)
            })
    }

    /// A session is closing: the pause is made durable FIRST — the state row
    /// flips to `paused` in its own transaction, carrying the state as it
    /// was read under this module's lock — and only then is the discard
    /// attempted. Fail closed (fix pass 4): a crash after the fence but
    /// before, during, or after a refused clear leaves the surviving rows
    /// paused on disk, so a restart can never send a queue the user asked to
    /// discard. If even the fence cannot be stored, the clear is not
    /// attempted at all — an unfenceable discard is not attempted — and the
    /// close reports the failure with memory left matching the unpaused disk
    /// state.
    pub(super) fn queue_forget(&self, session_id: &str) -> Result<(), WireError> {
        let Some(journal) = &self.journal else {
            self.queue_state_discard(session_id);
            return Ok(());
        };
        {
            let mut queues = self.queues.lock();
            self.ensure_queue_loaded(session_id, &mut queues)?;
            if let Some(state) = queues.get_mut(session_id) {
                if !state.items.is_empty() {
                    let next_id = state.next_id;
                    journal.queue_fence_paused(session_id, next_id)?;
                    state.paused = true;
                }
            }
        }
        if let Err(error) = journal.queue_clear(session_id) {
            eprintln!("message queue rows could not be cleared for {session_id}: {error}");
            // The fence is already durable — it went before the clear. Memory
            // keeps the state, paused to match, and the close reports the
            // discard that did not happen (P1-3).
            self.queues
                .lock()
                .get_mut(session_id)
                .map(|state| state.paused = true);
            return Err(WireError::new(
                ErrorCode::Journal,
                "This session's queued messages could not be discarded and \
                 remain in the journal; try closing it again.",
            ));
        }
        self.queue_state_discard(session_id);
        Ok(())
    }

    /// Drop one session's in-memory queue state, and nothing else — the
    /// road for a discard the journal has already made true (`delete_session`
    /// and retention remove the rows in their own writes; the map entry is
    /// the half they cannot reach).
    pub(super) fn queue_state_discard(&self, session_id: &str) {
        self.queues.lock().remove(session_id);
    }

    /// Drop the states of sessions nothing references any more: no live or
    /// configuring registry entry and no journal row. Retention deletes
    /// rows without holding this registry's locks (P2-7), so this runs
    /// where a fresh roster has just proven what the journal still has.
    pub(super) fn retain_referenced_queue_states(
        &self,
        live_or_configuring: &std::collections::HashSet<String>,
        roster: &std::collections::HashSet<String>,
    ) {
        self.queues.lock().retain(|session_id, _| {
            live_or_configuring.contains(session_id) || roster.contains(session_id)
        });
    }

    /// Run one user mutation against a session's state under this module's
    /// lock, persist it, and publish the resulting snapshot before the lock
    /// lets go — the mutation, its rows and its snapshot are one step, so a
    /// reader can never see a snapshot older than the last mutation it has
    /// already seen applied. The mutation runs on a trial copy: a refusal
    /// (the caller's own, the frame budget below, or the store) commits
    /// nothing. The budget is the add budget: headroom for a later failure
    /// is reserved here (P1-2).
    pub(super) fn edit_queue(
        &self,
        session_id: &str,
        runtime: &Arc<SessionRuntime>,
        change: impl FnOnce(&mut QueueState) -> Result<(), WireError>,
    ) -> Result<(), WireError> {
        self.mutate_queue(
            session_id,
            runtime,
            devboule_protocol::MAX_FRAME_BYTES.saturating_sub(QUEUE_ERROR_HEADROOM),
            "This session's queue would no longer fit the frame the whole \
             queue travels in; queue fewer or shorter messages.",
            change,
        )
    }

    /// The settle a queue send makes — the drain parking a failed item with
    /// its error, or send-now restoring one. Not a user growth: the error
    /// it writes on the item is the failure the add budget reserved room
    /// for, so the trial is judged against the frame cap itself. A trial
    /// even past that is refused and the caller parks the queue (P1-2).
    pub(super) fn settle_queue_send(
        &self,
        session_id: &str,
        runtime: &Arc<SessionRuntime>,
        change: impl FnOnce(&mut QueueState) -> Result<(), WireError>,
    ) -> Result<(), WireError> {
        self.mutate_queue(
            session_id,
            runtime,
            devboule_protocol::MAX_FRAME_BYTES,
            "This session's queue outcome no longer fits the frame the whole \
             queue travels in.",
            change,
        )
    }

    /// The one mutation body the two doors above share.
    fn mutate_queue(
        &self,
        session_id: &str,
        runtime: &Arc<SessionRuntime>,
        frame_budget: usize,
        refusal: &'static str,
        change: impl FnOnce(&mut QueueState) -> Result<(), WireError>,
    ) -> Result<(), WireError> {
        let mut queues = self.queues.lock();
        if !self.queue_session_live(session_id) {
            return Err(not_found());
        }
        self.ensure_queue_loaded(session_id, &mut queues)?;
        let mut trial = queues
            .get(session_id)
            .cloned()
            .expect("ensure_queue_loaded inserts the state");
        change(&mut trial)?;
        if snapshot_frame_bytes(session_id, &trial.items) > frame_budget {
            return Err(WireError::new(ErrorCode::InvalidRequest, refusal));
        }
        self.persist_queue(session_id, &trial.items, trial.next_id, trial.paused)?;
        let items = trial.snapshot();
        queues.insert(session_id.to_string(), trial);
        deliver_queue_snapshot(session_id, runtime, items);
        Ok(())
    }

    /// The queue as the test reads it: the items, in order, or nothing when
    /// this session's queue was never looked up.
    #[cfg(test)]
    pub(crate) fn queue_items_for_test(&self, session_id: &str) -> Vec<QueuedMessage> {
        self.queues
            .lock()
            .get(session_id)
            .map(QueueState::snapshot)
            .unwrap_or_default()
    }

    /// Whether draining is parked on this session after a failed send.
    #[cfg(test)]
    pub(crate) fn queue_paused_for_test(&self, session_id: &str) -> bool {
        self.queues
            .lock()
            .get(session_id)
            .is_some_and(|state| state.paused)
    }

    /// Whether a drain send is in flight for this session — the claim-to-
    /// settle window the P1-1 and P1-4 tests wait on.
    #[cfg(test)]
    pub(crate) fn queue_draining_for_test(&self, session_id: &str) -> bool {
        self.queues
            .lock()
            .get(session_id)
            .is_some_and(|state| state.draining)
    }

    /// Park this session's drain by hand — the state a failed send leaves,
    /// without needing a writer that fails, so a test can hold a queue still.
    #[cfg(test)]
    pub(crate) fn queue_park_for_test(&self, session_id: &str) {
        let mut queues = self.queues.lock();
        queues
            .entry(session_id.to_string())
            .or_insert_with(|| QueueState::new(Vec::new(), None))
            .paused = true;
    }
}
