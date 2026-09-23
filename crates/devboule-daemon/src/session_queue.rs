//! The per-session message queue's state and its journal seam: the map the
//! edits and sends take items from, the load a first touch does, the
//! write-through every change persists, and the snapshot every change
//! publishes. The four frame edits live in `session_queue_ops.rs`, send-now
//! in `session_queue_send.rs`, the drain in `session_queue_drain.rs` — this
//! file is the state and where it is kept.
//!
//! Lock order: this module's map may be held while the registry's session map
//! or a runtime lock is taken, never the reverse — no path holding either of
//! those may reach `SessionQueues`, so the two orders cannot meet. The map is
//! **not** held across a send: a send claims its item under the lock and
//! sends outside it.

use super::*;
use devboule_protocol::QueuedMessage;

/// The most messages one session's queue may hold. A queue is the user's own
/// waiting list, not a backlog channel: 64 rows is past what a person queues
/// by hand, and the cap is what keeps a `send`-holding peer from growing the
/// journal without bound.
pub(super) const MAX_QUEUE_ITEMS: usize = 64;

/// One session's queue as the daemon holds it in memory. An absent entry in
/// [`SessionQueues`] means "not looked up yet", never "empty".
#[derive(Clone, Debug)]
pub(super) struct QueueState {
    pub(super) items: Vec<QueuedMessage>,
    /// Minted ids climb: `queue-1`, `queue-2`, … An id is never reused while
    /// a queue exists, so an edit aimed at a drained item cannot land on its
    /// successor.
    pub(super) next_id: u64,
    /// A queue send is in flight. One per session, by construction: whoever
    /// sets it takes an item out with it, and it clears only when that send
    /// has succeeded or failed.
    pub(super) draining: bool,
    /// An `on_turn_end` hook is armed for this queue: set only while a turn
    /// was running when the arm was taken, cleared only by the hook firing,
    /// so two arms cannot pile onto one turn end.
    pub(super) hook_armed: bool,
    /// Draining stopped at a failed send and waits for a queue frame from a
    /// client before it tries again (the Q1 brief's pause).
    pub(super) paused: bool,
}

impl QueueState {
    pub(super) fn new(items: Vec<QueuedMessage>) -> Self {
        // Minted ids resume past the largest one still in the queue, so a
        // fresh id cannot land on a row a restart brought back.
        let next_id = items
            .iter()
            .filter_map(|item| item.item_id.strip_prefix("queue-"))
            .filter_map(|suffix| suffix.parse::<u64>().ok())
            .max()
            .map_or(1, |largest| largest + 1);
        Self {
            items,
            next_id,
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
    }

    /// Read this session's queue from the journal the first time this
    /// process touches it. Called with this module's map held — the read is a
    /// journal round trip, and the map is what makes "loaded" a fact rather
    /// than a guess. An unreadable queue refuses the op: a journal this
    /// daemon cannot read must not read as an emptied queue.
    pub(super) fn ensure_queue_loaded(
        &self,
        session_id: &str,
        queues: &mut HashMap<String, QueueState>,
    ) -> Result<(), WireError> {
        if queues.contains_key(session_id) {
            return Ok(());
        }
        let items = match &self.journal {
            None => Vec::new(),
            Some(journal) => journal
                .queue_load(session_id)
                .map_err(|error| {
                    eprintln!("message queue unreadable for {session_id}: {error}");
                    WireError::new(
                        ErrorCode::Journal,
                        "This session's message queue could not be read.",
                    )
                })?
                .into_iter()
                .map(|row| QueuedMessage {
                    item_id: row.item_id,
                    text: row.text,
                    attachment_references: row.attachment_references,
                    error: row.error,
                })
                .collect(),
        };
        queues.insert(session_id.to_string(), QueueState::new(items));
        Ok(())
    }

    /// Persist one session's queue exactly as it stands — the same snapshot
    /// the event carries, written to the rows a restart reads back. Best
    /// effort and logged: the in-memory queue has already changed and the
    /// caller has been told, so failing the act now would report a refusal
    /// for something that happened. The log line names the session and the
    /// error, never the messages.
    pub(super) fn persist_queue(&self, session_id: &str, items: &[QueuedMessage]) {
        let Some(journal) = &self.journal else {
            return;
        };
        let rows: Vec<crate::journal::QueueRecord> = items
            .iter()
            .map(|item| crate::journal::QueueRecord {
                item_id: item.item_id.clone(),
                text: item.text.clone(),
                attachment_references: item.attachment_references.clone(),
                error: item.error.clone(),
            })
            .collect();
        if let Err(error) = journal.queue_store(session_id, &rows) {
            eprintln!("message queue could not be persisted for {session_id}: {error}");
        }
    }

    /// A session is closing: its queue goes with it, in memory and in the
    /// journal — which is what makes the rule the same before and after a
    /// restart: the rows a fresh daemon would restore are already gone.
    pub(super) fn queue_forget(&self, session_id: &str) {
        self.queues.lock().remove(session_id);
        self.persist_queue(session_id, &[]);
    }

    /// Run one mutation against a session's state under this module's lock,
    /// persist it, and publish the resulting snapshot before the lock lets
    /// go — the mutation, its rows and its snapshot are one step, so a reader
    /// can never see a snapshot older than the last mutation it has already
    /// seen applied.
    pub(super) fn edit_queue(
        &self,
        session_id: &str,
        runtime: &Arc<SessionRuntime>,
        change: impl FnOnce(&mut QueueState) -> Result<(), WireError>,
    ) -> Result<(), WireError> {
        let mut queues = self.queues.lock();
        self.ensure_queue_loaded(session_id, &mut queues)?;
        let state = queues
            .entry(session_id.to_string())
            .or_insert_with(|| QueueState::new(Vec::new()));
        change(state)?;
        let items = state.snapshot();
        self.persist_queue(session_id, &items);
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

    /// Park this session's drain by hand — the state a failed send leaves,
    /// without needing a writer that fails, so a test can hold a queue still.
    #[cfg(test)]
    pub(crate) fn queue_park_for_test(&self, session_id: &str) {
        let mut queues = self.queues.lock();
        queues
            .entry(session_id.to_string())
            .or_insert_with(|| QueueState::new(Vec::new()))
            .paused = true;
    }
}
