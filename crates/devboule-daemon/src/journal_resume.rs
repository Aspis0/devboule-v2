//! What a reattaching cursor may resume from, and the bounded tail a reset
//! hands it instead of a partial timeline.
//!
//! The decision order is the one a journal reader has always needed and never
//! had: the cursor's generation, then its position against the retained range,
//! then whether the retained range is itself whole. A hole is checked before
//! any position is honoured, because a position inside a domain with a missing
//! row does not mean what the caller thinks it means.

use std::collections::HashSet;
use std::path::PathBuf;

use devboule_protocol::{
    Cursor, SessionEvent, SessionKind, SessionResumeInfo, SessionResumeOutcome,
    SessionResumeReason, SessionResumeTail,
};

use crate::journal::EventRecord;
use crate::journal_lookback::{LookbackAnswer, LookbackRequest};

use crate::session::SessionRuntime;

/// The wire-byte ceiling for a reset tail. It is a share of
/// [`devboule_protocol::MAX_FRAME_BYTES`], so the attach reply that carries it
/// — and its envelope, and the escaping the events pay on the way in — still
/// fits the frame it is sent through.
pub(crate) const RESET_TAIL_BYTES: usize = 256 * 1024;

/// How far back the row scan looks for tail candidates, in stored payload
/// bytes. Stored bytes bound the walk; the budget above is paid on the wire
/// bytes of the derived events, and JSON escaping only ever grows them.
pub(crate) const TAIL_SCAN_BYTES: u64 = 1024 * 1024;

/// A row-count bound on the same walk, so a generation of tiny rows cannot turn
/// one attach into an unbounded read.
pub(crate) const TAIL_SCAN_ROWS: usize = 4096;

/// What the attach reply says, and where the replay that follows it starts.
pub(crate) struct ResumeDecision {
    pub(crate) info: SessionResumeInfo,
    /// The seq the post-reply replay starts after: the tail's own cursor, or
    /// the client's cursor on a plain resume. The replay/live seam prunes live
    /// items at or below it, so the tail, the watermark and the first live
    /// event stay one ordered run with nothing twice and nothing missing.
    pub(crate) resume_from: u64,
    /// The generation that replay starts in. A reset reads the current
    /// generation, because the client's cursor is not in its numbering; a plain
    /// resume keeps the client's own, and a cursor at seq 0 keeps the whole
    /// transcript below the attach generation, which is what a Reopen asks for
    /// when it renumbers its cursor.
    pub(crate) from_generation: u64,
    /// The permission cards this reset's tail carries, by `tool_call_id`, or
    /// `None` when the client keeps its own timeline.
    ///
    /// `Some` is what makes a pending card eligible at the seam whatever its
    /// seq: the client is told to replace its timeline with the tail, so a card
    /// the tail does not carry is gone from its screen, and a card the tail
    /// does carry has already been delivered here. `None` leaves the client's
    /// own cursor in charge — it holds the cards at or below it.
    pub(crate) reset_tail_cards: Option<HashSet<String>>,
}

/// Where a resumed cursor starts its replay: the generations below the attach
/// generation when the cursor claims nothing yet, its own otherwise.
fn resume_start_generation(cursor: Cursor) -> u64 {
    if cursor.seq > 0 {
        cursor.generation
    } else {
        0
    }
}

/// Decide whether `cursor` may resume against the generation `generation` whose
/// head was captured at `head`.
///
/// `None` when the journal could not be read: the caller then keeps today's
/// behaviour and says so through the degraded signal, which is the honest
/// answer for a floor it cannot vouch for.
pub(crate) fn resolve(
    runtime: &SessionRuntime,
    generation: u64,
    head: u64,
    cursor: Cursor,
) -> Option<ResumeDecision> {
    if !runtime.has_journal() {
        return None;
    }
    let journal = runtime.resume_domain(generation)?;
    // The cursor's own generation comes first: every later comparison is in the
    // current generation's numbering, and a cursor from another one is in none
    // of it.
    let reason = if cursor.generation != generation {
        Some(SessionResumeReason::EpochChanged)
    } else if journal
        .first_gap
        .is_some_and(|hole| hole > cursor.seq && hole <= head)
    {
        // The hole sits in the window this client is about to be sent, so every
        // row after it would render as if the missing one never existed. That
        // is corruption, not compaction, and it never ships as a short history.
        Some(SessionResumeReason::JournalGap)
    } else if cursor.seq > head {
        Some(SessionResumeReason::CursorAhead)
    } else if cursor.seq < journal.oldest_seq.saturating_sub(1) {
        // Exactly `oldest_seq - 1` has seen everything retained and resumes.
        Some(SessionResumeReason::CursorCompacted)
    } else {
        None
    };
    let Some(reason) = reason else {
        return Some(ResumeDecision {
            info: SessionResumeInfo {
                resume: SessionResumeOutcome::Resumed,
                oldest_seq: journal.oldest_seq,
                head,
            },
            resume_from: cursor.seq,
            from_generation: resume_start_generation(cursor),
            reset_tail_cards: None,
        });
    };
    let tail = build_tail(runtime, generation, head, &journal);
    let resume_from = tail.cursor.seq;
    let reset_tail_cards = permission_ids(&tail.events);
    Some(ResumeDecision {
        info: SessionResumeInfo {
            resume: SessionResumeOutcome::Reset { reason, tail },
            oldest_seq: journal.oldest_seq,
            head,
        },
        resume_from,
        from_generation: generation,
        reset_tail_cards: Some(reset_tail_cards),
    })
}

/// The cards a tail carries, by `tool_call_id`: the ones this attach's reply
/// has already delivered, so the seam never offers one of them again.
fn permission_ids(events: &[SessionEvent]) -> HashSet<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect()
}

/// An empty tail at a cursor the caller names: the head when the newest event
/// was dropped over the budget, the newest committed seq when the journal is
/// still behind the head.
fn empty_tail(
    generation: u64,
    cursor: u64,
    domain: &crate::journal::ResumeRange,
) -> SessionResumeTail {
    SessionResumeTail::empty_at(
        Cursor {
            generation,
            seq: cursor,
        },
        domain.oldest_seq == 0,
    )
}

/// The newest events of the current generation that fit the budget, in journal
/// order and ending at the head whenever the tail carries anything.
///
/// The rows are chosen newest-first, then read oldest-first, because a provider
/// view is a forward state machine: a row cannot be derived on its own. What
/// the budget cuts is therefore the front of the derived run, never its middle.
fn build_tail(
    runtime: &SessionRuntime,
    generation: u64,
    head: u64,
    domain: &crate::journal::ResumeRange,
) -> SessionResumeTail {
    // A tail that carries nothing may not name the head it was built for.
    // `try_append` is asynchronous, so an attach that lands right after a turn
    // starts can see a head the writer has not reached, and the replay after the
    // reply would then start past events that are still pending. The newest
    // committed seq is the only cursor an empty tail may claim — which for a
    // generation that holds no rows is `0`, the value before its first.
    let Some(page) = runtime.tail_candidate_rows(generation, head) else {
        return empty_tail(generation, domain.durable_head, domain);
    };
    let mut derived = Vec::new();
    TailDeriver::new(
        runtime,
        generation,
        page.cwd.clone(),
        page.records.first().map(|row| row.seq).unwrap_or(head),
    )
    .derive(&page.records, &mut derived);
    if derived.is_empty() {
        return empty_tail(generation, domain.durable_head, domain);
    }
    // The walk is backwards over what the rows derive: a run that does not fit
    // ends the tail there, and the newest event alone over the budget ends it
    // immediately. Deterministic in both cases.
    let mut bytes = 0usize;
    let mut oldest_kept = derived.len();
    for (index, (_, event)) in derived.iter().enumerate().rev() {
        // An event that will not serialize cannot cross the wire, so it ends
        // the walk rather than being stepped over: a tail with a hole in it is
        // the one thing this whole path exists to avoid.
        let Ok(event_bytes) = serde_json::to_vec(event) else {
            break;
        };
        if bytes + event_bytes.len() > RESET_TAIL_BYTES {
            break;
        }
        bytes += event_bytes.len();
        oldest_kept = index;
    }
    if oldest_kept == derived.len() {
        // The newest event alone is over the budget: dropping it is the point
        // of the budget, so this empty tail does name the head and the client
        // continues from there without it.
        return empty_tail(generation, head, domain);
    }
    let (newest_seq, _) = derived[derived.len() - 1];
    let (oldest_seq, _) = derived[oldest_kept];
    SessionResumeTail {
        // Where the client continues: the newest row the tail actually carries,
        // which is the head whenever the journal was not behind the stream.
        cursor: Cursor {
            generation,
            seq: newest_seq,
        },
        tail_complete: oldest_seq <= domain.oldest_seq,
        events: derived[oldest_kept..]
            .iter()
            .map(|(_, event)| event.clone())
            .collect(),
    }
}

/// Turns journal rows into the view events the client renders, over a fresh
/// provider view.
///
/// A fresh view is what a mid-generation resume already gets: the walk starts
/// where the tail starts, with none of the conversation before it, which is the
/// same limitation the replay seam has always had. The lookbacks that make a
/// walk of this shape possible — a Codex thread's approved plan turns, a Claude
/// running total — are asked once, here, for the same reason.
struct TailDeriver {
    generation: u64,
    kind: Option<SessionKind>,
    peer_session_id: Option<String>,
    cwd: Option<PathBuf>,
    plan_marks: HashSet<String>,
    claude: Option<crate::claude_view::ClaudeView>,
    codex: Option<crate::codex_view::CodexView>,
    pi_withheld_finish: bool,
}

impl TailDeriver {
    fn new(
        runtime: &SessionRuntime,
        generation: u64,
        cwd: Option<PathBuf>,
        before_seq: u64,
    ) -> Self {
        let kind = runtime.agent_kind();
        let mut deriver = Self {
            generation,
            kind: kind.clone(),
            peer_session_id: runtime.peer_session_id(),
            cwd,
            plan_marks: HashSet::new(),
            claude: None,
            codex: None,
            pi_withheld_finish: false,
        };
        if kind == Some(SessionKind::Codex) {
            if let LookbackAnswer::PlanMarks(marks) =
                runtime.journal_lookback(LookbackRequest::CodexPlanMarks)
            {
                deriver.plan_marks = marks;
            }
        }
        if kind == Some(SessionKind::Claude) {
            if let LookbackAnswer::CostBaseline(baseline) =
                runtime.journal_lookback(LookbackRequest::ClaudeCostBaseline {
                    generation,
                    before_seq,
                })
            {
                let mut view = crate::claude_view::ClaudeView::new(deriver.cwd.clone());
                view.restore_cost_baseline(baseline);
                deriver.claude = Some(view);
            }
        }
        deriver
    }

    fn derive(&mut self, rows: &[EventRecord], out: &mut Vec<(u64, SessionEvent)>) {
        for record in rows {
            if record.generation != self.generation {
                // A resume that reached below the attach generation is a
                // transcript-shaped read, not a live tail.
                continue;
            }
            for mut event in self.drive(record) {
                // A journalled manifest is historical catalog state; the stored
                // runtime manifest is what the replay seam emits instead.
                if matches!(event, SessionEvent::SessionManifest { .. }) {
                    continue;
                }
                crate::session::stamp_turn_time(&mut event, record.ts_ms);
                out.push((record.seq, event));
            }
        }
    }

    fn drive(&mut self, record: &EventRecord) -> Vec<SessionEvent> {
        match record.kind {
            crate::journal::EventKind::AgentReport => {
                match serde_json::from_slice::<SessionEvent>(&record.payload) {
                    Ok(mut event) => {
                        crate::plan_text::bound_permission_request(&mut event);
                        crate::journal::time_a_kindless_report(&mut event, record.ts_ms);
                        vec![event]
                    }
                    // A row the binary can no longer read is not a tail event.
                    // The pull already reports it as degraded; a reset that
                    // carried it would put an unrenderable event on the wire.
                    Err(_) => Vec::new(),
                }
            }
            crate::journal::EventKind::AcpEnvelope => {
                match serde_json::from_slice::<serde_json::Value>(&record.payload) {
                    Ok(mut value) => {
                        if self.kind == Some(SessionKind::Codex) {
                            let cwd = self.cwd.clone();
                            let view = self
                                .codex
                                .get_or_insert_with(|| crate::codex_view::CodexView::new(cwd));
                            crate::codex_view::drive_replay(
                                view,
                                self.peer_session_id.as_deref(),
                                &self.plan_marks,
                                &value,
                            )
                        } else if self.kind == Some(SessionKind::Pi) {
                            crate::pi_view::drive_replay(&mut self.pi_withheld_finish, &value)
                        } else {
                            let cwd = self.cwd.clone();
                            let view = self
                                .claude
                                .get_or_insert_with(|| crate::claude_view::ClaudeView::new(cwd));
                            crate::claude_view::drive_replay(view, &mut value)
                        }
                    }
                    Err(_) => Vec::new(),
                }
            }
            // `output` rows are terminal bytes and `exit` rows are the end of a
            // session: neither replays into a live tail.
            crate::journal::EventKind::Output | crate::journal::EventKind::Exit => Vec::new(),
        }
    }
}
/// The decision order, the retained floor, the gap and the tail budget.
#[cfg(test)]
#[path = "journal_resume_tests.rs"]
mod resume_tests;
