//! The turn-inactivity watchdog the stream families share: one tick thread
//! per session, a silence bound, and the holds that stop the clock (awaited
//! client work, a pending permission or question, an in-flight tool call
//! inside its grace). A family arms it per turn through the prompt phase
//! and owns the expiry road, so a run the watchdog ends is finished exactly
//! the way that family finishes every run.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use super::permission_broker::PermissionBroker;
use super::SessionRuntime;

/// Read a silence bound a test may shorten: `var` carries milliseconds, and a
/// malformed or zero value falls back to the family default. Zero is
/// rejected, not "unbounded": a zero bound is indistinguishable from an
/// unset variable, and a silently disabled watchdog is worse than a loud
/// default. Negative and overflowing inputs fail the `u64` parse and fall
/// back the same way. Read once per watch, never per tick.
pub(crate) fn silence_from_env(var: &str, default: Duration) -> Duration {
    std::env::var(var)
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Duration::from_millis)
        .filter(|duration| !duration.is_zero())
        .unwrap_or(default)
}

/// How long one open tool call holds the watchdog clock: thirty minutes.
/// The effective ceiling for one silent tool is grace plus silence, 2400 s:
/// past that a quiet tool is ended even if its result never arrives. The
/// number is a judgement, not a measurement — no half-hour tool run has
/// been observed through the daemon. Claude Code's Bash tool defaults to a
/// ten-minute ceiling, but a per-call `timeout` or `BASH_MAX_TIMEOUT_MS` can
/// raise it, so a user who did can have a legitimate longer tool ended here;
/// raise `DEVBOULE_CLAUDE_TOOL_GRACE_MS` for that. A session already reads
/// `Silent` at 300 s, so a watching user is warned long before the hold lapses.
const TOOL_GRACE: Duration = Duration::from_secs(1800);
const TOOL_GRACE_ENV: &str = "DEVBOULE_CLAUDE_TOOL_GRACE_MS";

/// Where a turn is in its life. `Abandoned` is the watchdog's mark: the id in
/// it lets a late response recognise a turn that was already ended for it.
#[derive(Clone, Copy)]
enum PromptPhase {
    Idle,
    Live(u64),
    Abandoned(u64),
}

/// What expiry does, in the family's own words: end the run and publish the
/// finish. It receives the runtime, the abandoned prompt id and the silence
/// bound the message reports.
type Expire = Arc<dyn Fn(&SessionRuntime, u64, Duration) + Send + Sync>;

pub(crate) struct TurnWatch {
    silence: Duration,
    expire: Expire,
    last_activity: Mutex<Instant>,
    prompt: Mutex<PromptPhase>,
    client_work: AtomicU64,
    stop: AtomicBool,
    runtime: Mutex<Option<Weak<SessionRuntime>>>,
    cancel: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    broker: Mutex<Option<Weak<PermissionBroker>>>,
    /// A tool call the provider has started but not answered yet. Set by
    /// the reader from the view's latest tool start; the tick treats it
    /// exactly like a pending card, but only inside `tool_grace` measured
    /// from that start — afterwards the clock runs again, so an orphaned
    /// `tool_use` degrades to ordinary silence instead of disabling the
    /// watchdog.
    tool_hold: AtomicBool,
    /// The open set's latest tool start, stamped by the view on every
    /// `tool_use` block it ingests. The tick measures the grace from here.
    tool_since: Mutex<Option<Instant>>,
    /// The open-tool grace in milliseconds: atomic because the tick reads
    /// it and only a test ever writes it.
    tool_grace_ms: AtomicU64,
}

impl TurnWatch {
    pub(crate) fn new(silence: Duration, expire: Expire) -> Arc<Self> {
        let watch = Arc::new(Self {
            silence,
            expire,
            last_activity: Mutex::new(Instant::now()),
            prompt: Mutex::new(PromptPhase::Idle),
            client_work: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            runtime: Mutex::new(None),
            cancel: Mutex::new(None),
            broker: Mutex::new(None),
            tool_hold: AtomicBool::new(false),
            tool_since: Mutex::new(None),
            tool_grace_ms: AtomicU64::new(
                silence_from_env(TOOL_GRACE_ENV, TOOL_GRACE)
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX),
            ),
        });
        let thread_watch = Arc::downgrade(&watch);
        let _ = std::thread::Builder::new()
            .name("turn-watch".to_string())
            .spawn(move || loop {
                let Some(watch) = thread_watch.upgrade() else {
                    return;
                };
                if watch.stop.load(Ordering::Acquire) {
                    return;
                }
                watch.tick();
                drop(watch);
                std::thread::sleep(Duration::from_millis(50));
            });
        watch
    }

    pub(crate) fn set_cancel(&self, cancel: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.cancel.lock() {
            *slot = Some(cancel);
        }
    }

    pub(crate) fn bind_runtime(&self, runtime: &Arc<SessionRuntime>) {
        if let Ok(mut slot) = self.runtime.lock() {
            *slot = Some(Arc::downgrade(runtime));
        }
    }

    pub(crate) fn bind_broker(&self, broker: &Arc<PermissionBroker>) {
        if let Ok(mut slot) = self.broker.lock() {
            *slot = Some(Arc::downgrade(broker));
        }
    }

    pub(crate) fn note_activity(&self) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = Instant::now();
        }
    }

    pub(crate) fn begin_client_work(&self) {
        self.client_work.fetch_add(1, Ordering::AcqRel);
        self.note_activity();
    }

    pub(crate) fn end_client_work(&self) {
        self.client_work
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
        self.note_activity();
    }

    pub(crate) fn start_prompt(&self, id: u64) {
        if let Ok(mut prompt) = self.prompt.lock() {
            *prompt = PromptPhase::Live(id);
        }
        self.note_activity();
    }

    /// Returns `false` for a prompt the watchdog already abandoned, so the
    /// late response knows the turn was ended for it and publishes no finish.
    pub(crate) fn finish_prompt(&self, id: u64) -> bool {
        let Ok(mut prompt) = self.prompt.lock() else {
            return false;
        };
        match *prompt {
            PromptPhase::Live(current) if current == id => {
                *prompt = PromptPhase::Idle;
                true
            }
            PromptPhase::Abandoned(current) if current == id => false,
            _ => false,
        }
    }

    pub(crate) fn prompt_is_live(&self) -> bool {
        matches!(
            self.prompt.lock().ok().as_deref(),
            Some(PromptPhase::Live(_))
        )
    }

    /// Arm the watch for a turn that has no response id to pair: the id-less
    /// family ends it with [`TurnWatch::end_turn`].
    pub(crate) fn start_turn(&self) {
        self.start_prompt(0);
    }

    /// The id-less family's turn end. `true` — the caller owns the finish
    /// publication (the turn was live, or the watch never armed). `false` —
    /// the watchdog already abandoned this turn, and its expiry owns the
    /// finish.
    pub(crate) fn end_turn(&self) -> bool {
        let Ok(mut prompt) = self.prompt.lock() else {
            return true;
        };
        match *prompt {
            PromptPhase::Live(_) => {
                *prompt = PromptPhase::Idle;
                true
            }
            PromptPhase::Abandoned(_) => false,
            PromptPhase::Idle => true,
        }
    }

    /// Hold the clock while a tool call is in flight. The reader passes the
    /// view's latest tool start after every frame (`None` closes the hold);
    /// the tick reads it.
    pub(crate) fn set_tool_hold(&self, since: Option<Instant>) {
        self.tool_hold.store(since.is_some(), Ordering::Release);
        if let Ok(mut slot) = self.tool_since.lock() {
            *slot = since;
        }
    }

    /// A shorter grace for one test, so an orphaned call lapses it without
    /// sleeping out thirty minutes.
    #[cfg(test)]
    pub(crate) fn set_tool_grace_for_test(&self, grace: Duration) {
        self.tool_grace_ms.store(
            grace.as_millis().try_into().unwrap_or(u64::MAX),
            Ordering::Release,
        );
    }

    /// Backdate the open set's latest start, so a test drives the grace out
    /// without waiting it out.
    #[cfg(test)]
    pub(crate) fn backdate_tool_start_for_test(&self, age: Duration) {
        if let Ok(mut since) = self.tool_since.lock() {
            *since = Some(Instant::now() - age);
        }
    }

    pub(crate) fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn abandon_live_prompt(&self) -> Option<u64> {
        let Ok(mut prompt) = self.prompt.lock() else {
            return None;
        };
        match *prompt {
            PromptPhase::Live(id) => {
                *prompt = PromptPhase::Abandoned(id);
                Some(id)
            }
            _ => None,
        }
    }

    /// Backdate the last activity past the silence bound, so a test drives
    /// one tick into expiry without sleeping out the bound.
    #[cfg(test)]
    pub(crate) fn backdate_activity_for_test(&self, extra: Duration) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = Instant::now() - self.silence - extra;
        }
    }

    /// Drive one watchdog tick synchronously. Tests only; production ticks
    /// on the watch thread.
    #[cfg(test)]
    pub(crate) fn tick_for_test(&self) {
        self.tick();
    }

    /// Abandon the live prompt without expiring. Tests only.
    #[cfg(test)]
    pub(crate) fn abandon_for_test(&self) {
        self.abandon_live_prompt();
    }

    fn pending_cards(&self) -> bool {
        self.broker
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
            .map(|broker| broker.pending_len() > 0)
            .unwrap_or(false)
    }

    /// Whether an open tool call currently holds the clock: set, and its
    /// latest start still inside the grace. Past the grace the hold lapses
    /// and the silence bound decides, so a call whose result never arrives
    /// only postpones the expiry, never cancels it.
    fn tool_hold_active(&self) -> bool {
        if !self.tool_hold.load(Ordering::Acquire) {
            return false;
        }
        let grace = Duration::from_millis(self.tool_grace_ms.load(Ordering::Acquire));
        match self.tool_since.lock().ok().and_then(|since| *since) {
            // No stamp (the stamping lock poisoned): hold rather than
            // fire — a missing clock must fail toward keeping the run.
            None => true,
            Some(start) => start.elapsed() < grace,
        }
    }

    fn tick(&self) {
        if self.stop.load(Ordering::Acquire) {
            return;
        }
        if self.client_work.load(Ordering::Acquire) != 0 {
            return;
        }
        // A hold stops the clock, it does not pause it: re-stamp every
        // tick, so the silence bound counts from the hold's release — the
        // card's answer, the tool's result — and never from before it
        // opened. Without the re-stamp a card answered after the bound
        // would end the run on the very next tick. The tool hold lapses
        // past its grace (`tool_hold_active`); the card hold has no bound
        // because a person can always answer it.
        if self.tool_hold_active() {
            self.note_activity();
            return;
        }
        if self.pending_cards() {
            self.note_activity();
            return;
        }
        let idle = self
            .last_activity
            .lock()
            .ok()
            .map(|last| last.elapsed() >= self.silence)
            .unwrap_or(false);
        if !idle {
            return;
        }
        if self.client_work.load(Ordering::Acquire) != 0 {
            return;
        }
        if self.tool_hold_active() {
            self.note_activity();
            return;
        }
        if self.pending_cards() {
            self.note_activity();
            return;
        }
        let Some(prompt_id) = self.abandon_live_prompt() else {
            return;
        };
        if let Ok(cancel) = self.cancel.lock() {
            if let Some(cancel) = cancel.as_ref() {
                cancel();
            }
        }
        if let Some(runtime) = self
            .runtime
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
        {
            (self.expire)(&runtime, prompt_id, self.silence);
        }
    }
}

impl Drop for TurnWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
