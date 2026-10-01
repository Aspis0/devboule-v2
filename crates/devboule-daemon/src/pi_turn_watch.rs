//! The pi family's turn watchdog wiring: what arms the shared clock, what
//! feeds it, and the expiry road that ends a run pi went silent on.

use std::collections::{HashMap, VecDeque};
use std::process::ChildStdin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::SessionEvent;
use serde_json::Value;

use crate::session::permission_broker::PermissionBroker;
use crate::session::turn_watch::{silence_from_env, watchdog_message, TurnWatch};

/// The turn watchdog's silence bound: quiet stretches with nothing carded
/// and nothing running past the tool grace. Twice the house silence mark —
/// a session reads `Silent` at 300 s — so past double that with nothing
/// owed, a pi that has said nothing is one that will not say anything.
/// Tunable without a rebuild; tests inject short bounds through the
/// constructor.
const TURN_SILENCE: Duration = Duration::from_secs(600);
const TURN_SILENCE_ENV: &str = "DEVBOULE_PI_TURN_SILENCE_MS";

pub(super) fn turn_silence() -> Duration {
    silence_from_env(TURN_SILENCE_ENV, TURN_SILENCE)
}

/// The abort frame the killer and the watchdog's expiry both write.
pub(super) fn write_abort_frame(stdin: &Arc<Mutex<Option<ChildStdin>>>, next_id: &Arc<AtomicU64>) {
    let frame = serde_json::json!({
        "id": format!("a-{}", next_id.fetch_add(1, Ordering::Relaxed)),
        "type": "abort",
    });
    if let Ok(mut bytes) = serde_json::to_vec(&frame) {
        bytes.push(b'\n');
        let _ = super::write_child_stdin(stdin, &bytes, "Pi");
    }
}

/// The turn end the watchdog's expiry makes owed: expiry ended a turn pi
/// never ended, so pi's aborted answer to the expiry's own abort belongs
/// to the abandoned turn. The flag is consumed by the first end of any
/// kind; only the aborted answer suppresses, so a turn that never answers
/// the abort cannot leave the mark armed against a later genuine end.
#[derive(Default)]
pub(super) struct OwedTurnEnd(AtomicBool);

impl OwedTurnEnd {
    pub(super) fn owe(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(super) fn take(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }
}

/// How many tool calls may sit open at once before the oldest is evicted.
/// A backstop, not the bound: the turn boundary's clear is what normally
/// empties the set, and this only fences a turn that leaves more than this
/// many calls open at the same moment.
const MAX_OPEN_TOOL_STARTS: usize = 64;

/// The tool calls the reader saw open, for the watch's tool grace: a call
/// opens at pi's `toolcall_start` or `tool_execution_start` and closes at
/// its `tool_execution_end`. A turn boundary closes them all — a call
/// belongs to its turn — and the backstop caps how many may sit open at
/// once, so the grace always measures the latest of a bounded set.
#[derive(Default)]
pub(super) struct PiToolStarts {
    open: HashMap<String, Instant>,
    order: VecDeque<String>,
}

impl PiToolStarts {
    pub(super) fn observe(&mut self, value: &Value) {
        match value.get("type").and_then(Value::as_str) {
            Some("turn_end") | Some("agent_end") => {
                self.open.clear();
                self.order.clear();
            }
            Some("tool_execution_start") => {
                if let Some(id) = value.get("toolCallId").and_then(Value::as_str) {
                    self.open_tool(id);
                }
            }
            Some("tool_execution_end") => {
                if let Some(id) = value.get("toolCallId").and_then(Value::as_str) {
                    self.open.remove(id);
                    self.order.retain(|open| open != id);
                }
            }
            Some("message_update") => {
                let Some(event) = value.get("assistantMessageEvent") else {
                    return;
                };
                if event.get("type").and_then(Value::as_str) == Some("toolcall_start") {
                    if let Some(id) = event.get("id").and_then(Value::as_str) {
                        self.open_tool(id);
                    }
                }
            }
            _ => {}
        }
    }

    fn open_tool(&mut self, id: &str) {
        if self.open.contains_key(id) {
            // A re-announced call restamps; its place in the order stays.
            self.open.insert(id.to_string(), Instant::now());
            return;
        }
        self.open.insert(id.to_string(), Instant::now());
        self.order.push_back(id.to_string());
        while self.open.len() > MAX_OPEN_TOOL_STARTS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.open.remove(&oldest);
                }
                None => break,
            }
        }
    }

    pub(super) fn latest(&self) -> Option<Instant> {
        self.open.values().copied().max()
    }
}

/// The pi family's watchdog expiry — the real finish path, not a bypass:
/// abort the turn through the same frame the killer writes, then the turn
/// transition `settle_turn_finish` decides under the turn-hold, then the
/// error notice and the finish row, both journaled like any daemon-authored
/// row, so activity returns to idle and the transcript says why the run
/// ended even after a restart. The next `turn_end` pi sends belongs to the
/// turn ended here, so it is marked owed and the reader suppresses it.
pub(super) fn pi_turn_watch(
    silence: Duration,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<AtomicU64>,
    broker: Arc<PermissionBroker>,
    cancelled: Arc<AtomicBool>,
    owed_late_end: Arc<OwedTurnEnd>,
    usage_poller: Option<Arc<super::pi_usage_poller::PiUsagePoller>>,
) -> Arc<TurnWatch> {
    TurnWatch::new(
        silence,
        Arc::new(move |runtime, _prompt, silence_bound| {
            // Past a kill the run is already ended — the kill wrote its
            // own abort — and settling on the gone child would publish an
            // expiry for a turn the kill owns.
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            write_abort_frame(&stdin, &next_id);
            // The expiry is the abort road: the poll window belongs to the
            // run just ended, and a silent pi will not close it itself.
            if let Some(usage) = &usage_poller {
                usage.stop_window();
            }
            broker.cancel_pending();
            let _ = runtime.settle_turn_finish(|| false);
            let _ = runtime.publish_agent_error(watchdog_message("Pi", silence_bound));
            let published = runtime.publish_daemon_event(SessionEvent::AgentFinished {
                stop_reason: "error".to_string(),
                model_id: None,
                usage: None,
            });
            // The late end is owed only for a finish that landed: a publish
            // the closed stream refused leaves nothing for a late end to
            // duplicate, and an owed mark with no finish behind it would eat
            // the next turn's own end.
            if published {
                owed_late_end.owe();
            }
        }),
    )
}

#[cfg(test)]
#[path = "pi_turn_watch_test_support.rs"]
pub(in crate::session::pi_client) mod test_support;

#[cfg(test)]
#[path = "pi_turn_watch_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pi_turn_watch_ends_tests.rs"]
mod ends_tests;

#[cfg(test)]
#[path = "pi_turn_rejection_tests.rs"]
mod rejection_tests;

#[cfg(test)]
#[path = "pi_turn_order_tests.rs"]
mod order_tests;
