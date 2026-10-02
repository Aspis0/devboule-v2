//! Operation identity for the mutating queue frames: the payload fingerprint,
//! the per-session ring of answered ids, and what a repeat of one gets.
//!
//! A client that adds a message and never sees the reply must be able to ask
//! again without queueing it twice. Every mutating frame therefore carries a
//! caller-chosen `client_operation_id`, and this is where the daemon decides
//! what that id means: new, already answered with this same payload — and the
//! outcome that answer was, which may have been a refusal — or reused with a
//! different one.
//!
//! The ring is in memory inside the queue it belongs to, so it is bounded by
//! [`OPERATION_RING_CAPACITY`] per session, dies with that queue, and is never
//! journaled — a daemon that restarts has no queue to deduplicate against
//! either, so it has nothing to forget.

use devboule_protocol::WireError;
use sha2::{Digest, Sha256};

use super::session_queue::QueueState;
use super::session_queue_lifecycle::QueueMutation;

/// How many answered operation ids one session's queue remembers.
///
/// A client retries the request it is waiting on, so the ids that have to
/// survive are the ones a single conversation can have in flight: a handful of
/// presses, each retried until it is answered. 128 is far above that and costs
/// one queue entry's worth of memory per session; a client whose oldest
/// unanswered request is older than 128 accepted operations gets a second
/// effect, which is a client bug and not a daemon one.
pub(crate) const OPERATION_RING_CAPACITY: usize = 128;

/// The longest `client_operation_id` the daemon accepts, in bytes.
pub(crate) const MAX_OPERATION_ID_BYTES: usize = 128;

/// A payload's fingerprint: the whole SHA-256, never cut down, because two
/// payloads that share one are answered as the same operation.
pub(crate) type Fingerprint = [u8; 32];

/// One answered operation: the id the client chose, the fingerprint of the
/// payload it was answered for, and the outcome it was answered with.
///
/// The outcome is here rather than assumed to be a success because a send-now
/// can fail after the daemon has committed to an answer: a refusal puts the row
/// back and an uncertain write drops it, and a client that lost either reply
/// must be told the same thing twice rather than told it went through.
#[derive(Clone, Debug)]
pub(crate) struct QueuedOperation {
    client_operation_id: String,
    fingerprint: Fingerprint,
    outcome: OperationOutcome,
}

/// What an operation ended up doing. Recorded only once that is known, so a
/// ring entry never answers a question the daemon has not finished.
#[derive(Clone, Debug)]
pub(crate) enum OperationOutcome {
    /// The frame applied, or the send-now's prompt is with the agent.
    Accepted,
    /// The daemon refused before anything reached the provider, and the row
    /// went back to the front of the queue carrying the reason.
    Refused(WireError),
    /// The write began and failed, so the prompt may be with the agent: the
    /// row is gone rather than offered again.
    Dropped(WireError),
}

impl OperationOutcome {
    /// The answer a repeat of this operation gets: the same one the client has
    /// lost, error and all. `replayed` rides the accepted answer alone, because
    /// the wire has nowhere else to put it.
    pub(crate) fn replay(self) -> Result<QueueMutation, WireError> {
        match self {
            OperationOutcome::Accepted => Ok(QueueMutation { replayed: true }),
            OperationOutcome::Refused(error) | OperationOutcome::Dropped(error) => Err(error),
        }
    }
}

/// What this session's queue has already done with a client operation id.
#[derive(Clone, Debug)]
pub(crate) enum OperationSeen {
    /// Nothing has answered this id: run the frame.
    Fresh,
    /// This exact payload was answered under this id before, with this outcome.
    Answered(OperationOutcome),
    /// The id was accepted for a different payload, which is a client that
    /// reused an id across two different messages.
    Conflict,
}

impl QueueState {
    /// What this queue remembers about one operation id.
    pub(crate) fn seen_operation(
        &self,
        client_operation_id: &str,
        fingerprint: Fingerprint,
    ) -> OperationSeen {
        match self
            .operations
            .iter()
            .find(|entry| entry.client_operation_id == client_operation_id)
        {
            None => OperationSeen::Fresh,
            Some(entry) if entry.fingerprint == fingerprint => {
                OperationSeen::Answered(entry.outcome.clone())
            }
            Some(_) => OperationSeen::Conflict,
        }
    }

    /// Record an answered operation with the outcome it ended in, dropping the
    /// oldest entry when the ring is full. Called under the queue lock by the
    /// door that applied the change, so an id is remembered exactly when its
    /// effect — or the refusal that says there was none — is known.
    pub(crate) fn remember_operation(
        &mut self,
        client_operation_id: &str,
        fingerprint: Fingerprint,
        outcome: OperationOutcome,
    ) {
        while self.operations.len() >= OPERATION_RING_CAPACITY {
            self.operations.pop_front();
        }
        self.operations.push_back(QueuedOperation {
            client_operation_id: client_operation_id.to_string(),
            fingerprint,
            outcome,
        });
    }
}

/// A caller-chosen operation id, checked before anything else reads it: it is
/// caller-controlled text that is stored per session, so the bound and the
/// alphabet are enforced here rather than trusted from the frame.
pub(crate) fn checked_operation_id(client_operation_id: &str) -> Result<(), WireError> {
    let acceptable = !client_operation_id.is_empty()
        && client_operation_id.len() <= MAX_OPERATION_ID_BYTES
        && client_operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if acceptable {
        return Ok(());
    }
    Err(WireError::new(
        devboule_protocol::ErrorCode::InvalidRequest,
        format!(
            "A client operation id must be 1 to {MAX_OPERATION_ID_BYTES} characters of letters, \
             digits, '.', '_' or '-'."
        ),
    ))
}

/// A frame's payload fingerprint: the SHA-256 of its semantic fields, so the
/// same operation id carrying the same message hashes the same and anything
/// else does not. Built through [`PayloadFingerprint`] rather than from a
/// `Vec<&str>` so a field cannot be added to a frame and quietly left out of its
/// fingerprint.
pub(crate) struct PayloadFingerprint(Sha256);

impl PayloadFingerprint {
    /// Start from the frame's own name, so an add and an edit that carry the
    /// same text are different operations.
    pub(crate) fn new(frame: &str) -> Self {
        Self(Sha256::new()).field(frame)
    }

    /// Length first, so a field boundary is never ambiguous: `("ab", "c")` and
    /// `("a", "bc")` are different payloads.
    pub(crate) fn field(mut self, value: &str) -> Self {
        self.0.update((value.len() as u64).to_le_bytes());
        self.0.update(value.as_bytes());
        self
    }

    pub(crate) fn number(mut self, value: u64) -> Self {
        self.0.update(value.to_le_bytes());
        self
    }

    pub(crate) fn finish(self) -> Fingerprint {
        self.0.finalize().into()
    }
}
