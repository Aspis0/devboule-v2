//! The pi live-context poller: the window a pi run opens, the session
//! stats each tick reads, and the reading it publishes.
//!
//! One poller per pi session, built at spawn. A run opens the window
//! (`agent_start`) and closes it (`agent_end`); while it is open a tick
//! every [`POLL_PERIOD`] asks `get_session_stats` and publishes a
//! `ContextUsage { live: true }` only when the reading changed. The
//! readings are live state — never journaled — so the `turn_end` reading
//! stays the durable one, replay restores exactly what it always
//! restored, and live and replay agree at rest.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use devboule_protocol::SessionEvent;

use super::{ControlBudget, PiCatalog, PiControl, PollRoundFail};
use crate::pi_view::StatsReply;
use crate::session::SessionRuntime;

/// The poll period, Paseo's `usage-poller.ts` schedule (3 000 ms).
const POLL_PERIOD: Duration = Duration::from_secs(3);
/// How often the poll thread wakes to check its window. The poll itself is
/// gated by [`POLL_PERIOD`]; this is only the wake cadence.
const TICK: Duration = Duration::from_millis(100);

/// The poll window, owned by one lock: open or closed, the model it
/// opened under, and the epoch every in-flight read must still match when
/// its reply lands. A close bumps the epoch — a reply in flight across a
/// close (a run's end, a stop, a switch) is stale by definition, and a
/// close+open leaves it stale against the new window too.
#[derive(Default)]
struct WindowState {
    open: bool,
    epoch: u64,
    model: Option<String>,
}

pub(super) struct PiUsagePoller {
    control: Arc<PiControl>,
    catalog: Arc<Mutex<PiCatalog>>,
    /// The session this poller publishes through, bound when the reader
    /// first holds a runtime — a `Weak`, exactly like the watch's.
    runtime: Mutex<Option<Weak<SessionRuntime>>>,
    /// The thread's exit and the session's end: set by `close`, checked
    /// before any work.
    stop: AtomicBool,
    /// The window, under its one lock. The lock is taken short — never
    /// across the catalog read or a round trip — so a tick stalled on the
    /// catalog cannot pin the stop roads behind it.
    window: Mutex<WindowState>,
    /// When the next poll may fire; `None` while no window is scheduled.
    next_poll: Mutex<Option<Instant>>,
    /// The last published reading — what the client last saw from the
    /// poll. The durable `turn_end` reading resets it
    /// (`note_durable_reading`): the key claims to track what the client
    /// last saw, and at a turn boundary that is the durable number.
    last_reading: Mutex<Option<(u64, Option<u64>)>>,
    /// Whether this binary speaks `get_session_stats` at all. `false`
    /// only after pi answered the command with a refusal — the
    /// unknown-command marker an older binary leaves. A timeout or a
    /// reply without `contextUsage` is no reading this tick, never a
    /// give-up.
    supported: AtomicBool,
    /// One round trip's patience; the hot budget in production. Test-only
    /// writes go through the mutex so `&self` keeps ticking.
    budget: Mutex<ControlBudget>,
}

impl PiUsagePoller {
    /// The inert poller: every state piece, no thread. Tests drive
    /// [`Self::tick_for_test`]; production wraps this in [`Self::spawn`].
    pub(super) fn new(control: Arc<PiControl>, catalog: Arc<Mutex<PiCatalog>>) -> Arc<Self> {
        Arc::new(Self {
            control,
            catalog,
            runtime: Mutex::new(None),
            stop: AtomicBool::new(false),
            window: Mutex::new(WindowState::default()),
            next_poll: Mutex::new(None),
            last_reading: Mutex::new(None),
            supported: AtomicBool::new(true),
            budget: Mutex::new(ControlBudget::hot()),
        })
    }

    /// The production poller: [`Self::new`] plus its own tick thread, the
    /// way the watch is built.
    pub(super) fn spawn(control: Arc<PiControl>, catalog: Arc<Mutex<PiCatalog>>) -> Arc<Self> {
        let poller = Self::new(control, catalog);
        let thread_poller = Arc::downgrade(&poller);
        let _ = std::thread::Builder::new()
            .name("pi-usage-poll".to_string())
            .spawn(move || loop {
                let Some(poller) = thread_poller.upgrade() else {
                    return;
                };
                if poller.stop.load(Ordering::Acquire) {
                    return;
                }
                poller.tick();
                drop(poller);
                std::thread::sleep(TICK);
            });
        poller
    }

    pub(super) fn bind_runtime(&self, runtime: &Arc<SessionRuntime>) {
        if let Ok(mut slot) = self.runtime.lock() {
            *slot = Some(Arc::downgrade(runtime));
        }
    }

    /// pi's run opened (`agent_start`): the window opens under the current
    /// model, first poll one period out. An already-open window keeps its
    /// stamp — one poller, one window.
    pub(super) fn run_opened(&self) {
        if self.stop.load(Ordering::Acquire) {
            return;
        }
        let model = self.current_model_id().flatten();
        let Ok(mut window) = self.window.lock() else {
            return;
        };
        if window.open {
            return;
        }
        window.epoch += 1;
        window.open = true;
        window.model = model;
        if let Ok(mut next) = self.next_poll.lock() {
            *next = Some(Instant::now() + POLL_PERIOD);
        }
    }

    /// pi's run closed (`agent_end`): the window shuts. The durable
    /// `turn_end` reading is the turn's final value, so no completion read
    /// is owed — and the epoch bump retires any reply still in flight,
    /// which can therefore never publish, into this turn's end or the
    /// next turn's beginning.
    pub(super) fn run_closed(&self) {
        self.close_window();
    }

    /// The window stops: an interrupt, a watchdog expiry, a model switch.
    pub(super) fn stop_window(&self) {
        self.close_window();
    }

    /// The session is over: the thread winds down and nothing polls again.
    pub(super) fn close(&self) {
        self.close_window();
        self.stop.store(true, Ordering::Release);
    }

    /// The durable `turn_end` reading just published: the dedup key
    /// forgets everything before it — it claims to track what the client
    /// last saw, and at a turn boundary that is the durable number, so a
    /// new turn whose first live reading equals the old one still shows.
    pub(super) fn note_durable_reading(&self) {
        if let Ok(mut last) = self.last_reading.lock() {
            *last = None;
        }
    }

    fn close_window(&self) {
        let Ok(mut window) = self.window.lock() else {
            return;
        };
        if window.open {
            window.open = false;
            window.epoch += 1;
        }
    }

    fn runtime(&self) -> Option<Arc<SessionRuntime>> {
        self.runtime
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
    }

    /// `None` — the catalog lock is gone (hold, don't fire); the inner
    /// `Option` is the model it names.
    fn current_model_id(&self) -> Option<Option<String>> {
        self.catalog
            .lock()
            .ok()
            .map(|catalog| catalog.current_key())
    }

    /// The manifest window of the model the reading was read under —
    /// captured, never re-read at publish time.
    fn manifest_window(&self, model: &str) -> Option<u64> {
        let catalog = self.catalog.lock().ok()?;
        match catalog.lookup(model) {
            super::PiLookup::Found(model) => model.context_tokens,
            super::PiLookup::Ambiguous(_) | super::PiLookup::Missing => None,
        }
    }

    /// Drive one poll tick synchronously. Tests only; production ticks on
    /// the poll thread.
    #[cfg(test)]
    pub(super) fn tick_for_test(&self) {
        self.tick();
    }

    /// Schedule the next poll for now, so the very next tick fires it. Tests
    /// only: it moves the poll clock, it does not sleep one out.
    #[cfg(test)]
    pub(super) fn arm_next_poll_now_for_test(&self) {
        if let Ok(mut next) = self.next_poll.lock() {
            *next = Some(Instant::now());
        }
    }

    /// Shorten one round trip's patience. Tests only: it lets a held reply
    /// run a real timeout out deterministically.
    #[cfg(test)]
    pub(super) fn set_budget_for_test(&self, wait: Duration) {
        if let Ok(mut budget) = self.budget.lock() {
            *budget = ControlBudget::with_wait_for_test(wait);
        }
    }

    /// Whether a window is open. Tests only: it pins the run wiring — the
    /// arbiter's `agent_start`/`agent_end` and the stop roads — without
    /// waiting on wall-clock polls.
    #[cfg(test)]
    pub(super) fn window_open_for_test(&self) -> bool {
        self.window
            .lock()
            .map(|window| window.open)
            .unwrap_or_default()
    }

    /// Whether the reader's first feed has bound the runtime. Tests only:
    /// a tick before the bind publishes nowhere.
    #[cfg(test)]
    pub(super) fn runtime_bound_for_test(&self) -> bool {
        self.runtime().is_some()
    }

    fn tick(&self) {
        if self.stop.load(Ordering::Acquire) || !self.supported.load(Ordering::Acquire) {
            return;
        }
        let Some(runtime) = self.runtime() else {
            return;
        };
        // Under the window's own lock: open, model current, due — and the
        // epoch and model the coming read is bound to.
        let Some((epoch, model)) = self.window_due(Instant::now()) else {
            return;
        };
        let Some(reading) = self.read_stats(model.as_deref()) else {
            return;
        };
        // The window may have closed — or closed and reopened — while the
        // read was in flight: a stale reply is turn N's count arriving
        // into turn N+1, and it publishes nowhere. A switch committing
        // during the round trip touches neither `open` nor `epoch`, so the
        // model is compared too — re-read before the lock, never under it.
        let Some(current) = self.current_model_id() else {
            return;
        };
        match self.window.lock() {
            Ok(window) if window.open && window.epoch == epoch && current == model => {}
            _ => return,
        }
        self.publish(&runtime, model.as_deref(), reading);
    }

    /// The pre-read window decision: open, model current, due — answering
    /// with the epoch and model the read binds to. The model is read
    /// before the window's lock (see the field's note). A switch observed
    /// here closes the window; one committing during the round trip is
    /// the post-read model check in `tick` to discard. A gone catalog
    /// lock holds the tick rather than firing it.
    fn window_due(&self, now: Instant) -> Option<(u64, Option<String>)> {
        let model = self.current_model_id()?;
        let mut window = self.window.lock().ok()?;
        if !window.open {
            return None;
        }
        if window.model != model {
            window.open = false;
            window.epoch += 1;
            return None;
        }
        let due = self.next_poll.lock().ok()?.is_some_and(|due| now >= due);
        if !due {
            return None;
        }
        if let Ok(mut next) = self.next_poll.lock() {
            *next = Some(now + POLL_PERIOD);
        }
        Some((window.epoch, window.model.clone()))
    }

    /// One reading: ask `get_session_stats`, binding the window to the
    /// model at read time. `None` — nothing to publish this tick: no
    /// answer arrived (a timeout, the channel's end), pi answered without
    /// a `contextUsage` (no model or window), or the count was still null
    /// just after compaction. A refusal — pi saying the command does not
    /// exist — is the one failure that ends the session's polling, once,
    /// out loud.
    fn read_stats(&self, model: Option<&str>) -> Option<(u64, Option<u64>)> {
        let budget = match self.budget.lock() {
            Ok(budget) => *budget,
            Err(_) => return None,
        };
        match self.control.poll_round_trip("get_session_stats", &budget) {
            Err(PollRoundFail::Refused) => {
                self.supported.store(false, Ordering::Release);
                self.close_window();
                eprintln!(
                    "pi answered get_session_stats with a refusal; the session's live context \
                     meter stops and the turn_end reading carries it alone"
                );
                None
            }
            Err(PollRoundFail::Unreachable) => None,
            Ok(reply) => match crate::pi_view::stats_reply_from_response(&reply) {
                StatsReply::Reading {
                    used_tokens,
                    window_tokens,
                } => Some((
                    used_tokens,
                    window_tokens.or_else(|| model.and_then(|model| self.manifest_window(model))),
                )),
                StatsReply::NullTokens | StatsReply::Unusable => None,
            },
        }
    }

    /// Publish the reading when it changed. The dedup key advances only
    /// when the publish landed, so a refused publish cannot silence the
    /// next identical reading.
    fn publish(&self, runtime: &SessionRuntime, model: Option<&str>, reading: (u64, Option<u64>)) {
        let (used_tokens, max_tokens) = reading;
        let key = (used_tokens, max_tokens);
        match self.last_reading.lock() {
            Ok(last) if *last == Some(key) => return,
            Ok(_) => {}
            Err(_) => return,
        }
        // The bool is the silence-transition flag, not delivery: a publish
        // the stream refuses returns before the event exists, so the key
        // always advances — a refused publish leaves nothing to suppress.
        let _ = runtime.publish_agent_event(
            SessionEvent::ContextUsage {
                model_id: model.map(str::to_string),
                used_tokens,
                max_tokens,
                live: true,
            },
            None,
        );
        if let Ok(mut last) = self.last_reading.lock() {
            *last = Some(key);
        }
    }
}

impl Drop for PiUsagePoller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
#[path = "pi_usage_poller_test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "pi_usage_poller_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pi_usage_poller_lock_tests.rs"]
mod lock_tests;
