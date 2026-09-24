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
        // The floor from the surviving rows keeps a pre-counter database
        // (its v17 migration left no key yet) as safe as it was; a present
        // counter wins because it never went backwards while rows were
        // being removed.
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
    /// A row still marked in flight (the daemon died between claim and
    /// settle) comes back at the front with the queue paused under it: it
    /// may not have been sent, and nothing resends it automatically — the
    /// snapshot's `unconfirmed` flag is what asks the user (P1-4).
    pub(super) fn ensure_queue_loaded(
        &self,
        session_id: &str,
        queues: &mut HashMap<String, QueueState>,
    ) -> Result<(), WireError> {
        if queues.contains_key(session_id) {
            return Ok(());
        }
        let (items, counter) = match &self.journal {
            None => (Vec::new(), None),
            Some(journal) => {
                let (rows, counter) = journal.queue_load(session_id).map_err(|error| {
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
                (items, counter)
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
        if state.items.first().is_some_and(|item| item.unconfirmed) {
            state.paused = true;
        }
        queues.insert(session_id.to_string(), state);
        Ok(())
    }

    /// Persist one session's queue exactly as it stands — the same snapshot
    /// the event carries, written to the rows a restart reads back — and
    /// return the store's failure instead of hiding it: the caller commits
    /// nothing on `Err`, so memory and disk stay equal (P1-3). The log line
    /// names the session and the error, never the messages.
    pub(super) fn persist_queue(
        &self,
        session_id: &str,
        items: &[QueuedMessage],
        next_id: u64,
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
            .queue_store(session_id, &rows, next_id)
            .map_err(|error| {
                eprintln!("message queue could not be persisted for {session_id}: {error}");
                WireError::from(error)
            })
    }

    /// A session is closing or being deleted: its queue goes with it, rows
    /// and counter and memory together — which is what makes the rule the
    /// same before and after a restart (P2-5, P2-7). A store that refuses
    /// the clear is logged, never raised: close must not fail on it.
    pub(super) fn queue_forget(&self, session_id: &str) {
        self.queues.lock().remove(session_id);
        if let Some(journal) = &self.journal {
            if let Err(error) = journal.queue_clear(session_id) {
                eprintln!("message queue rows could not be cleared for {session_id}: {error}");
            }
        }
    }

    /// Run one mutation against a session's state under this module's lock,
    /// persist it, and publish the resulting snapshot before the lock lets
    /// go — the mutation, its rows and its snapshot are one step, so a reader
    /// can never see a snapshot older than the last mutation it has already
    /// seen applied. The mutation runs on a trial copy: a refusal (the
    /// caller's own, the frame cap below, or the store) commits nothing.
    pub(super) fn edit_queue(
        &self,
        session_id: &str,
        runtime: &Arc<SessionRuntime>,
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
        if snapshot_frame_bytes(session_id, &trial.items) > devboule_protocol::MAX_FRAME_BYTES {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "This session's queue would no longer fit the frame the whole \
                 queue travels in; queue fewer or shorter messages.",
            ));
        }
        self.persist_queue(session_id, &trial.items, trial.next_id)?;
        let items = trial.snapshot();
        queues.insert(session_id.to_string(), trial);
        runtime.publish_queue_snapshot(items);
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
