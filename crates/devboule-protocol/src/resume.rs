//! The attach reply's resume outcome: a plain resume, or one named reset
//! carrying the bounded tail the client continues from.
//!
//! Present only when the connection negotiated
//! [`crate::caps::SESSION_RESUME_OUTCOMES`]. A connection that did not gets
//! today's reply, byte for byte, including the generation-mismatch error.

use serde::{Deserialize, Serialize};

use crate::{Cursor, SessionEvent};

/// Why a cursor could not be resumed.
///
/// Every value means the same thing to the client — replace the timeline with
/// the tail — and none of them is delivered as a partial batch: a hole is a
/// reset, never a shorter history.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionResumeReason {
    /// The cursor names a generation this session is not on: a recreated
    /// process renumbers its rows from seq 1.
    EpochChanged,
    /// The cursor is past the head, so the client saw rows this host has
    /// never held — a rolled-back or rebuilt prefix.
    CursorAhead,
    /// The cursor is below the retained floor, so retention trimmed what it
    /// names.
    CursorCompacted,
    /// The retained rows are not contiguous. A lost row is journal corruption,
    /// and a gap is never rendered as a shorter conversation.
    JournalGap,
}

/// The bounded tail a reset hands the client: the newest retained events of the
/// current generation, contiguous, and ending at the head the reply names
/// whenever the tail carries anything.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SessionResumeTail {
    /// Where the client continues from: the seq of the last event here. An
    /// empty tail names the newest seq the journal holds for this generation,
    /// which is the head only when the newest event is the one thing over the
    /// tail budget — the case where dropping it is the budget's purpose.
    pub cursor: Cursor,
    /// Oldest first, in the same order every other event batch on this wire
    /// arrives, so a client appends them as one run and then takes live events
    /// straight after the last one.
    pub events: Vec<SessionEvent>,
    /// The tail reaches [`SessionResumeInfo::oldest_seq`], so nothing before
    /// the tail is missing. False leaves an older prefix the client never
    /// receives and cannot reconstruct.
    pub tail_complete: bool,
}

impl SessionResumeTail {
    /// An empty tail at `cursor`, and whether nothing before it is missing. The
    /// caller names the cursor: the head when the newest event was dropped over
    /// the budget, the newest committed seq when the journal has not caught up
    /// with the head. Deterministic either way — the client continues from the
    /// cursor and takes live events from there.
    pub fn empty_at(cursor: Cursor, tail_complete: bool) -> Self {
        Self {
            cursor,
            events: Vec::new(),
            tail_complete,
        }
    }
}

/// What a cursor resolved to.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SessionResumeOutcome {
    /// Replay from the client's own cursor: today's behaviour, unchanged.
    Resumed,
    /// The cursor cannot be resumed. The client replaces its timeline with
    /// `tail` and continues from `tail.cursor`; it never merges, and never
    /// re-reattaches in a loop.
    Reset {
        reason: SessionResumeReason,
        tail: SessionResumeTail,
    },
}

/// The attach reply's resume fields, present only under
/// [`crate::caps::SESSION_RESUME_OUTCOMES`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SessionResumeInfo {
    /// Flattened, so the reply reads `resume.outcome` and not
    /// `resume.resume.outcome`: one level of nesting for one answer.
    #[serde(flatten)]
    pub resume: SessionResumeOutcome,
    /// The lowest sequence still retained for the current generation.
    ///
    /// `0` when that generation holds no rows at all, which is also the value
    /// an empty journal reports for [`Self::head`]. A cursor at exactly
    /// `oldest_seq - 1` has seen everything retained and resumes; below it the
    /// outcome is `cursor_compacted`. With an empty journal nothing is
    /// compacted, because no cursor can be below `0 - 1`.
    pub oldest_seq: u64,
    /// The highest sequence allocated in the current generation when the
    /// attach captured it, `0` for an empty journal. A cursor above it is
    /// `cursor_ahead`.
    pub head: u64,
}
