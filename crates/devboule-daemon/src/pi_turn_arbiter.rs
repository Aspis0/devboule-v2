//! The pi family's finish arbitration, one place: what a rejected prompt
//! answers, what a turn boundary means, when a run's end belongs to the
//! watchdog or the abort gate instead of the wire, and the tool-hold
//! bookkeeping the shared clock reads. The reader calls it; the decisions
//! live here, so they are testable without a child process.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use devboule_protocol::{SessionEvent, TurnUsage};
use serde_json::Value;

use crate::session::permission_broker::PermissionBroker;
use crate::session::turn_watch::TurnWatch;
use crate::session::SessionRuntime;

use super::pi_run_failure::PendingFailure;
use super::pi_turn_watch::{OwedTurnEnd, PiToolStarts};

/// The interrupt/end race: pi's aborted end for an interrupted turn can
/// arrive after a replacement prompt was already delivered, and publishing
/// its finish would end the replacement's run out from under it.
///
/// The counter is translated from `claude_abort.rs`'s gate. One pi difference:
/// pi ANSWERS a refused prompt on the wire, so a refusal decrements the
/// count — a prompt pi never took cannot be the replacement an aborted end
/// would be stale against. Everything else is the source's own: no armed
/// interrupt withholds nothing, and any end consumes the expectation.
#[derive(Default)]
struct AbortGate {
    delivered: AtomicU64,
    /// The delivered count at the latest interrupt, while its aborted end
    /// is still owed. `None` — no interrupt is owed anything.
    state: Mutex<Option<u64>>,
}

impl AbortGate {
    fn note_prompt_delivered(&self) {
        self.delivered.fetch_add(1, Ordering::Relaxed);
    }

    fn note_prompt_refused(&self) {
        let _ = self
            .delivered
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_sub(1))
            });
    }

    /// The snapshot is of the interrupt REQUEST, before its frame is
    /// written: a prompt delivered after it is a replacement regardless of
    /// wire order. Each interrupt re-baselines the one expectation.
    fn note_interrupt(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = Some(self.delivered.load(Ordering::Relaxed));
    }

    /// Any end consumes the expectation, so a turn that never answers the
    /// abort cannot leave the gate armed against a later genuine end; only
    /// an aborted end a delivered replacement outranks is withheld — and
    /// with no interrupt armed at all, nothing is ever withheld.
    fn settle(&self, aborted: bool) -> bool {
        let snapshot = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(snapshot) = snapshot else {
            return false;
        };
        aborted && self.delivered.load(Ordering::Relaxed) > snapshot
    }
}

/// What a run's withheld tool iterations leave owed: their finishing usage
/// sums here for the end that closes the run, and `open` says pi still owes
/// that end — no closing `turn_end` has come since the last iteration.
#[derive(Default)]
struct ToolIterations {
    open: bool,
    usage: Option<TurnUsage>,
}

impl ToolIterations {
    /// One iteration's end was withheld: the run's end is open again, and
    /// the iteration's usage joins the sum.
    fn note(&mut self, value: &Value) {
        self.open = true;
        self.usage = crate::pi_view::add_usage(
            self.usage.take(),
            value
                .get("message")
                .and_then(|message| message.get("usage"))
                .and_then(crate::pi_view::usage_from_pi),
        );
    }

    /// The summed usage, once, if an iteration left the run's end open.
    fn take(&mut self) -> Option<Option<TurnUsage>> {
        if !self.open {
            return None;
        }
        self.open = false;
        Some(self.usage.take())
    }
}

/// What a frame means for the run's finish, decided from the shared watch,
/// the expiry's expectation, the abort gate, pi's own turn bounds, and the
/// open tool set — the reader holds none of it.
pub(super) struct TurnArbiter {
    watch: Option<Arc<TurnWatch>>,
    expiry_owed: Arc<OwedTurnEnd>,
    tools: Mutex<PiToolStarts>,
    gate: AbortGate,
    /// The failure the current run holds, whether a run is in flight and
    /// whether a Stop marked it, under one lock the Stop thread, the writers
    /// and the reader take turns on: a mark set before the settle decides
    /// that run's outcome, unless the Stop's abort never reached pi.
    failure: Mutex<PendingFailure>,
    /// pi's own run, open between its `agent_start` and its `agent_end` —
    /// pi 0.87.1 emits both (`core/agent-session.js:711-716`, forwarded
    /// verbatim by `modes/json-event.js`); the aborted mark rides
    /// `turn_end`'s `message.stopReason`, `agent_end` carries none.
    pi_turn_open: AtomicBool,
    /// A refusal whose error already showed while pi's run was still open:
    /// the finish it owes waits for that run to close, whichever order the
    /// concurrent line reader delivers the aborted end, the refusal and
    /// the close in. At most one — two refusals deferred in one run share
    /// the one finish, both errors shown.
    pending_refusal: AtomicBool,
    /// The run's withheld tool iterations: their finishes were suppressed,
    /// so the end that closes the run owes their usage — and, when no
    /// closing `turn_end` follows, pi's own close owes the turn.
    tool_iterations: Mutex<ToolIterations>,
    /// The live-context poller, when the spawn wired one: the run's open
    /// and close are its poll window, an interrupt stops it, the session's
    /// end closes it. `None` on the bare arbiter the tests hold.
    usage: Option<Arc<super::pi_usage_poller::PiUsagePoller>>,
}

impl TurnArbiter {
    pub(super) fn new(watch: Option<Arc<TurnWatch>>, expiry_owed: Arc<OwedTurnEnd>) -> Self {
        Self {
            watch,
            expiry_owed,
            tools: Mutex::new(PiToolStarts::default()),
            gate: AbortGate::default(),
            failure: Mutex::new(PendingFailure::default()),
            pi_turn_open: AtomicBool::new(false),
            pending_refusal: AtomicBool::new(false),
            tool_iterations: Mutex::new(ToolIterations::default()),
            usage: None,
        }
    }

    /// Wire the live-context poller. Builder style, so the constructors the
    /// watch tests use stay untouched.
    pub(super) fn with_usage_poller(
        mut self,
        usage: Arc<super::pi_usage_poller::PiUsagePoller>,
    ) -> Self {
        self.usage = Some(usage);
        self
    }

    /// The bare arbiter the bare reader (tests, seeds) holds: every road
    /// answers "the caller owns the finish", and nothing is ever armed.
    pub(super) fn bare() -> Arc<Self> {
        Arc::new(Self::new(None, Arc::new(OwedTurnEnd::default())))
    }

    /// Every dispatched frame, once: activity, the tool-call bookkeeping,
    /// and the hold those give the clock. One call at dispatch entry, so
    /// the early-return frames — responses, cards, compaction — refresh
    /// the hold too, never leaving it a frame stale.
    pub(super) fn note_frame(&self, value: &Value) {
        if let Some(watch) = &self.watch {
            watch.note_activity();
        }
        if let Ok(mut tools) = self.tools.lock() {
            tools.observe(value);
            if let Some(watch) = &self.watch {
                watch.set_tool_hold(tools.latest());
            }
        }
    }

    /// A prompt reached the child: the turn it starts is watched, and the
    /// abort gate counts it as the replacement it may be.
    pub(super) fn note_prompt_delivered(&self) {
        self.failure().prompt_sent();
        if let Some(watch) = &self.watch {
            watch.start_turn();
        }
        self.gate.note_prompt_delivered();
    }

    /// The rejection road, whole. A refusal answers the prompt we wrote and
    /// have not seen answered: its error always shows. The run ends under it
    /// only when pi has no turn of its own open and the runtime turn is
    /// live — a rejection mid-turn is the steer-refusal case, and pi's own
    /// end finishes that run. A stale or foreign id answers no run and is
    /// logged, once per occurrence.
    pub(super) fn prompt_rejected(
        &self,
        runtime: &SessionRuntime,
        current: bool,
        id: &str,
        reason: &str,
    ) {
        if !current {
            eprintln!(
                "pi refused prompt {id}; the daemon no longer tracks it, so it answers no run: {reason}"
            );
            return;
        }
        let _ = runtime.publish_agent_error(format!("Pi rejected the prompt: {reason}"));
        self.failure().prompt_answered();
        self.gate.note_prompt_refused();
        if self.pi_turn_open.load(Ordering::Acquire) {
            // pi's own run is still open — and its close, the aborted end
            // and this refusal race on a concurrent line reader. The finish
            // this refusal owes is decided when the run closes.
            self.pending_refusal.store(true, Ordering::Release);
            return;
        }
        if !runtime.is_running_turn() {
            // Nothing is running to finish; the watch still disarms, so a
            // run that ended without a turn can never expire.
            self.owns_finish();
            return;
        }
        if self.owns_finish() {
            let _ = runtime.publish_journaled_finish(SessionEvent::AgentFinished {
                stop_reason: "error".to_string(),
                model_id: None,
                usage: None,
            });
        }
    }

    /// The pure tests' handle on the refusal decrement.
    #[cfg(test)]
    fn note_prompt_refused(&self) {
        self.gate.note_prompt_refused();
    }

    /// The interrupt was requested: the aborted end of the turn as of now
    /// is the one expectation, against the delivered count as of now. The
    /// poll window stops with it — an interrupted run polls no further.
    /// The run in flight is marked stopped first: a settle that runs while
    /// this Stop waits on a later lock must already see it. Answers whether
    /// it marked a run.
    pub(super) fn note_interrupt(&self) -> bool {
        let marked = self.failure().stop();
        if let Some(usage) = &self.usage {
            usage.stop_window();
        }
        self.gate.note_interrupt();
        marked
    }

    /// The Stop's abort never reached pi: the run's mark goes, and its
    /// settle announces its failure as if no Stop happened.
    pub(super) fn note_abort_unsent(&self) {
        self.failure().unstop();
    }

    /// pi handled the prompt itself and opened no run for it.
    pub(super) fn note_prompt_handled(&self) {
        self.failure().prompt_answered();
    }

    /// One attempt's ending: what it failed with is held for the run's
    /// settle.
    pub(super) fn hold_failure(&self, agent_end: &Value) {
        self.failure().hold(agent_end);
    }

    /// The run's own ending: what is still held, unless a Stop ended it.
    pub(super) fn settle_failure(&self) -> Option<String> {
        self.failure().settle()
    }

    fn failure(&self) -> MutexGuard<'_, PendingFailure> {
        self.failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn tool_iterations(&self) -> MutexGuard<'_, ToolIterations> {
        self.tool_iterations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The withheld iterations' summed usage, once, when one left the run's
    /// end open.
    fn take_tool_iterations(&self) -> Option<Option<TurnUsage>> {
        self.tool_iterations().take()
    }

    /// The withheld tool iterations' usage, folded onto the finish the
    /// closing `turn_end` carries, so the run's one finish reports all of
    /// it. A row that derives no finish — or an end another road suppresses
    /// — leaves the sum in hand for the end that does.
    pub(super) fn fold_withheld_usage(&self, events: &mut [SessionEvent]) {
        let Some(finish) = events.iter_mut().find_map(|event| match event {
            SessionEvent::AgentFinished { usage, .. } => Some(usage),
            _ => None,
        }) else {
            return;
        };
        let Some(withheld) = self.take_tool_iterations() else {
            return;
        };
        *finish = crate::pi_view::add_usage(finish.take(), withheld);
    }

    /// pi settled with no closing `turn_end` — the last end was a withheld
    /// tool iteration: the still-open turn ends here, once, reported `stop`
    /// (pi's own close carries no stop reason), with the iterations' summed
    /// usage. A closing `turn_end` that landed already ended the turn and
    /// took the sum, so this is silent then.
    pub(super) fn note_run_closed(&self, runtime: &SessionRuntime) {
        let Some(usage) = self.take_tool_iterations() else {
            return;
        };
        if !runtime.is_running_turn() || !self.owns_finish() {
            return;
        }
        let _ = runtime.publish_journaled_finish(SessionEvent::AgentFinished {
            stop_reason: "stop".to_string(),
            model_id: None,
            usage,
        });
    }

    /// A durable `turn_end` context reading just published: the live
    /// poll's dedup key forgets what it published before it, so the key
    /// tracks what the client last saw.
    pub(super) fn note_durable_context(&self) {
        if let Some(usage) = &self.usage {
            usage.note_durable_reading();
        }
    }

    /// A pi `agent_start`: its run opens holding no earlier failure, the
    /// watch arms — a turn pi starts with no prompt from us is begun and
    /// watched like any other — and the context poll's window opens with the
    /// run.
    pub(super) fn note_agent_start(&self) {
        self.failure().run_opened();
        self.pi_turn_open.store(true, Ordering::Release);
        if let Some(usage) = &self.usage {
            usage.run_opened();
        }
        if let Some(watch) = &self.watch {
            watch.start_turn();
        }
    }

    /// pi's run closed. A refusal whose finish was deferred while the run
    /// was open owes it now: with pi's own turn gone, a still-live runtime
    /// turn that nothing else has ended is the refused prompt's to finish —
    /// exactly once, on the journaled road. `is_running_turn` is the guard
    /// that gives every earlier finish (pi's own end, the watchdog, EOF)
    /// the win, because each of them ends the turn; the mark is spent
    /// either way. A run pi is not retrying closes its still-open turn here
    /// too: the last end was a withheld tool iteration, so no closing
    /// `turn_end` is coming.
    pub(super) fn note_agent_end(&self, runtime: &SessionRuntime, value: &Value) {
        if let Some(usage) = &self.usage {
            usage.run_closed();
        }
        self.pi_turn_open.store(false, Ordering::Release);
        if self.pending_refusal.swap(false, Ordering::AcqRel)
            && runtime.is_running_turn()
            && self.owns_finish()
        {
            let _ = runtime.publish_journaled_finish(SessionEvent::AgentFinished {
                stop_reason: "error".to_string(),
                model_id: None,
                usage: self.take_tool_iterations().flatten(),
            });
        }
        // A retry reopens the same run; anything else leaves it over.
        if value.get("willRetry").and_then(Value::as_bool) != Some(true) {
            self.note_run_closed(runtime);
        }
    }

    /// A `turn_end`'s finish is the run's own ending — unless the run is
    /// not over. A tool-using turn ends one loop iteration while pi streams
    /// on with the tool results, and the run carries one finish: the end
    /// that carries the model's answer. Also withheld are the ends another
    /// road already ended: the watchdog's expiry (its own aborted answer is
    /// owed), the abort gate (an aborted end a delivered replacement
    /// outranks), or the run is already over. The caller journals the
    /// withheld-finish marker beside the row when this answers true.
    pub(super) fn turn_end_suppressed(&self, runtime: &SessionRuntime, value: &Value) -> bool {
        let aborted = stop_reason(value) == Some("aborted");
        // The first end of any kind consumes both armed expectations — the
        // expiry's owed aborted answer and the abort gate's stale one — so
        // an end pi never followed with the answer cannot leave either
        // armed against a later genuine end.
        if self.expiry_owed.take() && aborted {
            return true;
        }
        if self.gate.settle(aborted) {
            return true;
        }
        // A tool iteration's end is not the run's end: pi streams on with
        // the tool results, so the turn stays open and the iteration's usage
        // is owed to the end that closes the run.
        if stop_reason(value) == Some("toolUse") {
            if runtime.is_running_turn() {
                self.tool_iterations().note(value);
            }
            return true;
        }
        let ours = self.owns_finish();
        if ours && runtime.is_running_turn() {
            // This end is the run's own: any refusal finish it had deferred
            // is published by this road, never by the run's close.
            self.pending_refusal.store(false, Ordering::Release);
        }
        !ours || !runtime.is_running_turn()
    }

    /// `true` when the caller owns this run's finish: the watch was never
    /// armed, or the turn was live and the call just ended it. `false` —
    /// the watchdog already abandoned the turn, and its expiry owns the
    /// finish.
    pub(super) fn owns_finish(&self) -> bool {
        match &self.watch {
            Some(watch) => watch.end_turn(),
            None => true,
        }
    }

    /// A reader EOF with the turn running ends it here — one finish, shared
    /// with the watchdog through the watch's own prompt state.
    pub(super) fn eof_ends_run(&self, runtime: &SessionRuntime) -> bool {
        self.owns_finish() && runtime.is_running_turn()
    }

    pub(super) fn bind_runtime(&self, runtime: &Arc<SessionRuntime>) {
        if let Some(usage) = &self.usage {
            usage.bind_runtime(runtime);
        }
        if let Some(watch) = &self.watch {
            watch.bind_runtime(runtime);
        }
    }

    pub(super) fn bind_broker(&self, broker: &Arc<PermissionBroker>) {
        if let Some(watch) = &self.watch {
            watch.bind_broker(broker);
        }
    }

    pub(super) fn shutdown(&self) {
        if let Some(usage) = &self.usage {
            usage.close();
        }
        if let Some(watch) = &self.watch {
            watch.shutdown();
        }
    }
}

/// The stop reason pi spells on one `turn_end`'s message.
fn stop_reason(value: &Value) -> Option<&str> {
    value
        .get("message")
        .and_then(|message| message.get("stopReason"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(stop_reason: &str) -> Value {
        serde_json::json!({
            "type": "turn_end",
            "message": {"stopReason": stop_reason},
        })
    }

    #[test]
    fn an_aborted_end_after_a_replacement_is_withheld() {
        let arbiter = TurnArbiter::new(None, Arc::new(OwedTurnEnd::default()));
        arbiter.note_prompt_delivered();
        arbiter.note_interrupt();
        arbiter.note_prompt_delivered();
        assert!(arbiter.turn_end_suppressed(&SessionRuntime::new(), &frame("aborted")));
    }

    #[test]
    fn a_tool_iteration_consumes_the_armed_interrupt_expectation() {
        let arbiter = TurnArbiter::new(None, Arc::new(OwedTurnEnd::default()));
        arbiter.note_prompt_delivered();
        arbiter.note_interrupt();
        arbiter.note_prompt_delivered();
        let runtime = SessionRuntime::new();
        runtime.begin_turn();
        assert!(arbiter.turn_end_suppressed(&runtime, &frame("toolUse")));
        assert!(
            !arbiter.turn_end_suppressed(&runtime, &frame("aborted")),
            "the first end of any kind spent the expectation, so the end after it is not the stale one"
        );
    }

    #[test]
    fn a_tool_iteration_consumes_an_owed_expiry() {
        let owed = Arc::new(OwedTurnEnd::default());
        let arbiter = TurnArbiter::new(None, Arc::clone(&owed));
        arbiter.note_prompt_delivered();
        owed.owe();
        let runtime = SessionRuntime::new();
        runtime.begin_turn();
        assert!(arbiter.turn_end_suppressed(&runtime, &frame("toolUse")));
        assert!(!owed.take(), "the first end of any kind spent the mark");
        assert!(
            !arbiter.turn_end_suppressed(&runtime, &frame("aborted")),
            "the next run's aborted end is no longer the expired run's stale one"
        );
    }

    #[test]
    fn an_aborted_end_with_no_replacement_finishes() {
        let arbiter = TurnArbiter::new(None, Arc::new(OwedTurnEnd::default()));
        arbiter.note_prompt_delivered();
        arbiter.note_interrupt();
        let runtime = SessionRuntime::new();
        runtime.begin_turn();
        // With no watch armed the end is the caller's to publish.
        assert!(!arbiter.turn_end_suppressed(&runtime, &frame("aborted")));
    }

    #[test]
    fn a_genuine_end_consumes_an_unanswered_expectation() {
        let arbiter = TurnArbiter::new(None, Arc::new(OwedTurnEnd::default()));
        arbiter.note_prompt_delivered();
        arbiter.note_interrupt();
        arbiter.note_prompt_delivered();
        let runtime = SessionRuntime::new();
        runtime.begin_turn();
        // pi never answered the interrupt; the replacement's own genuine
        // end is not withheld...
        assert!(!arbiter.turn_end_suppressed(&runtime, &frame("stop")));
        // ...nor may the expectation outlive it against a later aborted
        // one: a fresh interrupt with a delivered replacement withholds
        // again.
        arbiter.note_interrupt();
        arbiter.note_prompt_delivered();
        assert!(arbiter.turn_end_suppressed(&runtime, &frame("aborted")));
    }

    #[test]
    fn a_refused_prompt_is_no_replacement() {
        let arbiter = TurnArbiter::new(None, Arc::new(OwedTurnEnd::default()));
        arbiter.note_prompt_delivered();
        arbiter.note_interrupt();
        arbiter.note_prompt_delivered();
        arbiter.note_prompt_refused();
        let runtime = SessionRuntime::new();
        runtime.begin_turn();
        // The replacement was refused; the interrupted turn's end is its
        // run's to finish.
        assert!(!arbiter.turn_end_suppressed(&runtime, &frame("aborted")));
    }

    #[test]
    fn an_unarmed_gate_withholds_no_aborted_end() {
        let arbiter = TurnArbiter::new(None, Arc::new(OwedTurnEnd::default()));
        arbiter.note_prompt_delivered();
        arbiter.note_prompt_delivered();
        let runtime = SessionRuntime::new();
        runtime.begin_turn();
        assert!(
            !arbiter.turn_end_suppressed(&runtime, &frame("aborted")),
            "no armed interrupt: the aborted end is the run's own to publish"
        );
    }

    #[test]
    fn an_expired_watchdog_owes_only_pi_s_own_aborted_answer() {
        let owed = Arc::new(OwedTurnEnd::default());
        let arbiter = TurnArbiter::new(None, Arc::clone(&owed));
        owed.owe();
        // pi's answer to the expiry's abort is suppressed...
        assert!(arbiter.turn_end_suppressed(&SessionRuntime::new(), &frame("aborted")));
        // ...and a genuine end after a second expiry consumes the mark
        // without suppressing: the turn that ended is the one running.
        owed.owe();
        let runtime = SessionRuntime::new();
        runtime.begin_turn();
        assert!(!arbiter.turn_end_suppressed(&runtime, &frame("stop")));
    }

    fn failed_agent_end() -> Value {
        serde_json::json!({
            "type": "agent_end",
            "messages": [{"role": "assistant", "errorMessage": "Request timed out."}],
        })
    }

    #[test]
    fn a_stop_then_a_refusal_leaves_the_next_run_s_failure() {
        let arbiter = TurnArbiter::new(None, Arc::new(OwedTurnEnd::default()));
        let runtime = SessionRuntime::new();
        arbiter.note_prompt_delivered();
        arbiter.note_interrupt();
        arbiter.prompt_rejected(&runtime, true, "p-1", "busy");
        // The next prompt's run fails before any opening of its own.
        arbiter.note_prompt_delivered();
        arbiter.hold_failure(&failed_agent_end());
        assert_eq!(
            arbiter.settle_failure().as_deref(),
            Some("Request timed out."),
            "a refused prompt had no run for the stop to end"
        );
    }

    #[test]
    fn a_stop_parked_on_the_gate_has_already_marked_the_run() {
        let arbiter = Arc::new(TurnArbiter::new(None, Arc::new(OwedTurnEnd::default())));
        let failed = failed_agent_end();
        arbiter.note_prompt_delivered();
        arbiter.hold_failure(&failed);
        // The gate is a lock the Stop takes after the mark: held here, the
        // Stop parks on it.
        let gate = arbiter.gate.state.lock().expect("gate");
        let stopping = Arc::clone(&arbiter);
        let stop = std::thread::spawn(move || stopping.note_interrupt());
        // Each try is a settle under the failure lock. One that still finds
        // the failure puts the run back as it was, so only the Stop can
        // change the answer.
        let try_settle = || {
            let mut failure = arbiter.failure();
            let settled = failure.settle();
            if settled.is_some() {
                failure.prompt_sent();
                failure.hold(&failed);
            }
            settled
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut settled = try_settle();
        while settled.is_some() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
            settled = try_settle();
        }
        drop(gate);
        stop.join().expect("the stop ends once the gate frees");
        assert_eq!(
            settled, None,
            "a stop accepted before the settle leaves it nothing to announce"
        );
    }
}
