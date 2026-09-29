//! The interrupt/result race on the Claude stream: the CLI's aborted result
//! for an interrupted turn can arrive after a replacement prompt was already
//! delivered, and publishing its `AgentFinished` would finish the replacement's
//! run out from under it.
//!
//! The gate is a counter, not a flag: an epoch survives the new turn's
//! activity, which is exactly the activity that made a bool-based guard
//! racy. It is shared between the interrupt writer, the prompt writers and
//! the session reader, so it lives behind one `Arc`. A poisoned state lock is
//! recovered, never propagated: both recoveries fail toward finishing the
//! run, because a suppressed finish that never comes is a hung session.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct ClaudeAbortGate {
    /// Prompts delivered to the child since spawn, prompt writers' side.
    /// Monotonic; counted only after a successful stdin write.
    prompts: AtomicU64,
    state: Mutex<GateState>,
}

#[derive(Default)]
struct GateState {
    /// The delivered-prompt count at the MOST RECENT interrupt, while its
    /// aborted result is still owed. One expectation, re-baselined by each
    /// interrupt: the result that answers belongs to the turn as of the
    /// latest stop, so an earlier stop's snapshot must not outlive it — a
    /// steer does not start a new turn, and two stops of one turn share one
    /// result. `None` means no interrupt is owed anything.
    prompts_at_interrupt: Option<u64>,
}

impl ClaudeAbortGate {
    /// One prompt reached the child's stdin. Counted only after a successful
    /// write: a frame that was refused, failed, or still sits in the mode
    /// gate's queue did not reach the child and cannot be the replacement an
    /// aborted result would be stale against.
    pub(crate) fn note_prompt_delivered(&self) {
        self.prompts.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an interrupt at the moment it is requested, before its frame is
    /// written. The snapshot is of the interrupt REQUEST: a prompt delivered
    /// after it is a replacement even if the interrupt's own bytes reach the
    /// child later — out-of-order delivery does not change the decision.
    pub(crate) fn note_interrupt(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.prompts_at_interrupt = Some(self.prompts.load(Ordering::Relaxed));
    }

    /// Settle one result frame: whether its `AgentFinished` must be withheld.
    ///
    /// Withholds only an abort-identified result that a replacement delivered
    /// after the most recent interrupt outranks; any other result just
    /// consumes the expectation, so a CLI that never sends the aborted frame
    /// cannot leave the gate armed against the next genuine one. Expectations
    /// are consumed by results and by nothing else — streaming from the
    /// replacement turn must not clear them.
    pub(crate) fn settle_result(&self, abort_marked: bool) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(prompts_at_interrupt) = state.prompts_at_interrupt.take() else {
            return false;
        };
        abort_marked && self.prompts.load(Ordering::Relaxed) > prompts_at_interrupt
    }

    /// Prompts actually counted as delivered — the refusal/failure tests'
    /// window into the counter.
    #[cfg(test)]
    pub(crate) fn delivered_prompts(&self) -> u64 {
        self.prompts.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_aborted_result_after_a_replacement_is_withheld() {
        let gate = ClaudeAbortGate::default();
        gate.note_prompt_delivered();
        gate.note_interrupt();
        gate.note_prompt_delivered();
        assert!(gate.settle_result(true));
    }

    #[test]
    fn an_aborted_result_with_no_replacement_settles_the_run() {
        let gate = ClaudeAbortGate::default();
        gate.note_prompt_delivered();
        gate.note_interrupt();
        assert!(!gate.settle_result(true));
    }

    #[test]
    fn a_genuine_result_consumes_an_unanswered_expectation() {
        let gate = ClaudeAbortGate::default();
        gate.note_prompt_delivered();
        gate.note_interrupt();
        gate.note_prompt_delivered();
        // the CLI never answered the interrupt; the replacement's own
        // genuine result must not be withheld...
        assert!(!gate.settle_result(false));
        // ...nor may the expectation outlive it against a later result.
        gate.note_prompt_delivered();
        assert!(!gate.settle_result(true));
    }

    #[test]
    fn activity_between_interrupt_and_result_does_not_clear_the_expectation() {
        let gate = ClaudeAbortGate::default();
        gate.note_prompt_delivered();
        gate.note_interrupt();
        // prompts are the only activity that counts, and only after the
        // interrupt; nothing here resembles the replacement.
        gate.note_interrupt();
        gate.note_prompt_delivered();
        assert!(gate.settle_result(true));
    }

    #[test]
    fn a_later_interrupt_rebaselines_the_expectation() {
        let gate = ClaudeAbortGate::default();
        gate.note_prompt_delivered(); // the interrupted turn's prompt
        gate.note_interrupt(); // snapshot 1
        gate.note_prompt_delivered(); // the steered replacement
        gate.note_interrupt(); // snapshot 2 replaces snapshot 1
                               // The single result answers the LATEST stop, after which nothing was
                               // delivered, so it must finish the run rather than be measured
                               // against the first stop's older snapshot.
        assert!(!gate.settle_result(true));
    }
}
