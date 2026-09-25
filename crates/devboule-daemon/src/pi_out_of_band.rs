//! The two commands pi executes itself, out of band: `/compact` and
//! `/autocompact`, dispatched before a turn exists.
//!
//! Paseo is the reference (owner rule, 2026-09-24): each function translates
//! one of its `providers/pi` functions into Rust with the file and line
//! cited, and nothing here is redesigned — the look is not in scope. The
//! `get_commands` list lives in the sibling `pi_commands.rs`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use devboule_protocol::SessionEvent;
use serde_json::Value;

use super::commands::{js_trim, parse_slash_invocation, REQUEST_TIMEOUT};
use super::PiControl;
use crate::session::{OutOfBandCommands, SessionRuntime};

/// How long a `compact` round trip may wait: Paseo waits forever
/// (`JSONL_RPC_NO_TIMEOUT`, `cli-runtime.ts:139-143`) because pi only
/// replies once the compaction is durable — but a child that stops
/// answering would otherwise pin one worker and its registration for the
/// session's life, and the review required the bound (review A5-2 #4).
/// Five minutes is far longer than a durable compaction, so a real one
/// never sees it; a silent child's round trip ends in Paseo's failure line
/// instead of a thread.
const COMPACT_TIMEOUT: Duration = Duration::from_secs(300);

/// What an `/autocompact` argument means — Paseo's `parseAutoCompactMode`
/// (`pi/agent.ts:362-375`): absent means toggle, the four affirmative and
/// four negative spellings, anything else unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutoCompactMode {
    Enabled,
    Disabled,
    Toggle,
    Unknown,
}

fn parse_auto_compact_mode(args: Option<&str>) -> AutoCompactMode {
    let mode = js_trim(args.unwrap_or("toggle")).to_ascii_lowercase();
    match mode.as_str() {
        "on" | "true" | "enable" | "enabled" => AutoCompactMode::Enabled,
        "off" | "false" | "disable" | "disabled" => AutoCompactMode::Disabled,
        "toggle" => AutoCompactMode::Toggle,
        _ => AutoCompactMode::Unknown,
    }
}

/// Paseo's one-compaction-at-a-time guard (`pi/agent.ts:1821-1825`):
/// `active` is its `outOfBandCompactionEmit`, `started` its
/// `outOfBandCompactionStarted` (`:1826,1856-1860`), kept alive by the
/// reader's observation of pi's own compaction frames the way Paseo keeps
/// its alive on the `compaction_start`/`compaction_end` events
/// (`:2344-2356`). A run releases the slot when its RPC settles without the
/// compaction having begun (Paseo's `finally`), when the compaction ends,
/// or when a started compaction's RPC fails — Paseo's synthetic completed
/// item does that third one (`:1834-1845`).
#[derive(Default)]
pub(super) struct CompactGuard {
    active: AtomicBool,
    started: AtomicBool,
}

impl CompactGuard {
    /// Claim the one compact slot. `false` is Paseo's refusal.
    fn try_begin(&self) -> bool {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// One pi frame observed by the reader (Paseo `emitCompactionTimeline`,
    /// `:2344-2356`): only a compaction seen while a run is active moves the
    /// flags — an automatic compaction with no run of ours changes nothing.
    pub(super) fn observe(&self, line: &Value) {
        match line.get("type").and_then(Value::as_str) {
            Some("compaction_start") if self.active.load(Ordering::Acquire) => {
                self.started.store(true, Ordering::Release);
            }
            Some("compaction_end") if self.active.load(Ordering::Acquire) => {
                self.release();
            }
            _ => {}
        }
    }

    /// The RPC settled: a run whose compaction never started releases the
    /// slot (Paseo's `finally`, `:1856-1860`), and so does an error — for a
    /// started one that is Paseo's synthetic completed item. A compaction
    /// that succeeded keeps the slot until its `compaction_end`.
    fn settle(&self, failed: bool) {
        if failed || !self.started.load(Ordering::Acquire) {
            self.release();
        }
    }

    fn release(&self) {
        self.started.store(false, Ordering::Release);
        self.active.store(false, Ordering::Release);
    }
}

/// The two commands pi runs itself, dispatched where Paseo dispatches them
/// (`pi/agent.ts:1667-1691` `tryHandleOutOfBand`, called from
/// `agent-manager.ts:2353` `tryRunOutOfBand`): the text never becomes a
/// prompt and never begins a turn.
pub(super) struct PiOutOfBandCommands {
    control: Arc<PiControl>,
    compact: Arc<CompactGuard>,
    compact_timeout: Duration,
}

impl PiOutOfBandCommands {
    pub(super) fn new(control: Arc<PiControl>) -> Self {
        Self {
            control,
            compact: Arc::new(CompactGuard::default()),
            compact_timeout: COMPACT_TIMEOUT,
        }
    }

    /// The bound a test shortens: the refusal and bound tests would
    /// otherwise wait the production five minutes (review A5-2 #4).
    #[cfg(test)]
    pub(super) fn with_compact_timeout(mut self, timeout: Duration) -> Self {
        self.compact_timeout = timeout;
        self
    }

    /// The compact slot, shared with the reader that observes pi's own
    /// compaction frames — Paseo keeps both halves in one agent.
    pub(super) fn compact_guard(&self) -> Arc<CompactGuard> {
        Arc::clone(&self.compact)
    }

    /// Paseo `pi/agent.ts:1819-1862` `executeCompactCommand`: the guard
    /// first — a second run while one is outstanding is refused with
    /// Paseo's own sentence and writes no rpc (`:1821-1825`, surfaced as
    /// the client's `[Error] …` line, `agent-manager.ts:2381-2387`) — then
    /// the `compact` RPC with the custom instructions when there are any
    /// (`cli-runtime.ts:139-143`). Progress and completion are pi's own
    /// compaction frames, which `pi_view` shows (`review A5-2 #2`); this
    /// publishes only Paseo's failure line, verbatim.
    fn run_compact(&self, args: Option<String>, runtime: &Arc<SessionRuntime>) {
        if !self.compact.try_begin() {
            publish_outcome(
                runtime,
                "[Error] A Pi compact command is already running".to_string(),
            );
            return;
        }
        let fields = match args {
            Some(args) => serde_json::json!({ "customInstructions": args }),
            None => serde_json::json!({}),
        };
        let guard = Arc::clone(&self.compact);
        run_out_of_band_request(
            &self.control,
            "compact",
            fields,
            self.compact_timeout,
            runtime,
            move |answer, runtime| {
                guard.settle(answer.is_err());
                if let Err(message) = answer {
                    publish_outcome(
                        runtime,
                        format!("[Error] Failed to compact context: {message}"),
                    );
                }
            },
        );
    }
}

impl OutOfBandCommands for PiOutOfBandCommands {
    fn handles_out_of_band(&self, text: &str) -> bool {
        matches!(
            parse_slash_invocation(text).map(|invocation| invocation.name.to_ascii_lowercase()),
            Some(name) if name == "compact" || name == "autocompact"
        )
    }

    fn run_out_of_band(&self, text: &str, runtime: &Arc<SessionRuntime>) {
        let Some(invocation) = parse_slash_invocation(text) else {
            return;
        };
        match invocation.name.to_ascii_lowercase().as_str() {
            "compact" => self.run_compact(invocation.args, runtime),
            "autocompact" => run_autocompact(&self.control, invocation.args, runtime),
            _ => {}
        }
    }
}

/// Paseo `pi/agent.ts:1864-1912` `executeAutoCompactCommand`: the argument
/// resolves first (a usage refusal for anything else), toggle reads
/// `get_state.autoCompactionEnabled` and refuses with Paseo's sentence when
/// that state is not a boolean, and then the one `set_auto_compaction` RPC
/// (`cli-runtime.ts:145-147`) — the failure line and the success sentence
/// exactly as Paseo spells them.
fn run_autocompact(control: &Arc<PiControl>, args: Option<String>, runtime: &Arc<SessionRuntime>) {
    match parse_auto_compact_mode(args.as_deref()) {
        AutoCompactMode::Unknown => publish_outcome(
            runtime,
            "[Error] Usage: /autocompact [on|off|toggle]".to_string(),
        ),
        AutoCompactMode::Enabled => request_auto_compaction(control, true, runtime),
        AutoCompactMode::Disabled => request_auto_compaction(control, false, runtime),
        AutoCompactMode::Toggle => {
            let toggle_control = Arc::clone(control);
            run_out_of_band_request(
                control,
                "get_state",
                serde_json::json!({}),
                REQUEST_TIMEOUT,
                runtime,
                move |answer, runtime| {
                    let current = match answer {
                        Ok(value) => value
                            .get("data")
                            .and_then(|data| data.get("autoCompactionEnabled"))
                            .and_then(Value::as_bool),
                        Err(message) => {
                            publish_outcome(runtime, format!("[Error] {message}"));
                            return;
                        }
                    };
                    match current {
                        Some(current) => {
                            request_auto_compaction(&toggle_control, !current, runtime)
                        }
                        None => publish_outcome(
                            runtime,
                            "[Error] Auto-compaction state is unavailable. Use /autocompact on or /autocompact off."
                                .to_string(),
                        ),
                    }
                },
            );
        }
    }
}

/// One `set_auto_compaction` round trip and the sentence Paseo publishes
/// for it (`pi/agent.ts:1903-1911`).
fn request_auto_compaction(control: &Arc<PiControl>, enabled: bool, runtime: &Arc<SessionRuntime>) {
    run_out_of_band_request(
        control,
        "set_auto_compaction",
        serde_json::json!({ "enabled": enabled }),
        REQUEST_TIMEOUT,
        runtime,
        move |answer, runtime| match answer {
            Ok(_) => publish_outcome(
                runtime,
                format!(
                    "Auto-compaction {}.",
                    if enabled { "enabled" } else { "disabled" }
                ),
            ),
            Err(message) => publish_outcome(
                runtime,
                format!("[Error] Failed to set auto-compaction: {message}"),
            ),
        },
    );
}

/// Write one out-of-band request and wait for its answer off the send path:
/// `on_answer` runs on a thread of its own with the reply or the failure and
/// publishes whatever Paseo would have shown. The write itself happens here,
/// on the caller's thread, so a pipe that cannot take the frame is reported
/// at once rather than from a thread nobody waits for.
fn run_out_of_band_request(
    control: &Arc<PiControl>,
    command: &'static str,
    fields: Value,
    timeout: Duration,
    runtime: &Arc<SessionRuntime>,
    on_answer: impl FnOnce(Result<Value, String>, &Arc<SessionRuntime>) + Send + 'static,
) {
    let runtime = Arc::clone(runtime);
    let control = Arc::clone(control);
    let request = match control.begin(command, fields) {
        Ok(request) => request,
        Err(error) => {
            on_answer(Err(error.message), &runtime);
            return;
        }
    };
    // The answer runs on exactly one side: the worker when it starts, or
    // here when no worker could be created — never both, never neither.
    let on_answer = std::sync::Arc::new(std::sync::Mutex::new(Some(on_answer)));
    let for_worker = std::sync::Arc::clone(&on_answer);
    let runtime_if_no_worker = Arc::clone(&runtime);
    match std::thread::Builder::new()
        .name(format!("pi-{command}"))
        .spawn(move || {
            let (id, response) = request;
            let answer = await_out_of_band(&control, command, id, response, timeout);
            if let Some(run) = for_worker.lock().ok().and_then(|mut slot| slot.take()) {
                run(answer, &runtime);
            }
        }) {
        Ok(_) => {}
        Err(error) => {
            // The frame is already on the wire; without a worker nothing
            // would ever answer for it, so the outcome is reported here
            // instead of discarding the spawn result (review A5-2 #4).
            if let Some(run) = on_answer.lock().ok().and_then(|mut slot| slot.take()) {
                run(
                    Err(format!("Pi {command} worker could not start: {error}")),
                    &runtime_if_no_worker,
                );
            }
        }
    }
}

/// One out-of-band round trip, answered the way Paseo's client sees it: a
/// `success: false` carries pi's own error text (`jsonl-rpc-process.ts:283-289`
/// rejects with `response.error`), a closed channel is the reason the reader
/// woke it with, and every wait is bounded — `compact` at
/// [`COMPACT_TIMEOUT`], the rest at Paseo's own default (review A5-2 #4
/// replaced Paseo's `JSONL_RPC_NO_TIMEOUT` for it, `cli-runtime.ts:139-143`).
fn await_out_of_band(
    control: &Arc<PiControl>,
    command: &str,
    id: String,
    response: mpsc::Receiver<Result<Value, String>>,
    timeout: Duration,
) -> Result<Value, String> {
    let received = response.recv_timeout(timeout);
    let value = match received {
        Ok(Ok(value)) => value,
        Ok(Err(message)) => return Err(message),
        Err(RecvTimeoutError::Timeout) => {
            // The registration would outlive the wait that gave up on it
            // (Paseo deletes it, `jsonl-rpc-process.ts:160-163`).
            if let Ok(mut pending) = control.pending.lock() {
                pending.remove(&id);
            }
            return Err(format!("Pi {command} response timed out"));
        }
        Err(RecvTimeoutError::Disconnected) => {
            return Err("Pi control channel closed before the response arrived.".to_string());
        }
    };
    if value.get("success").and_then(Value::as_bool) == Some(true) {
        return Ok(value);
    }
    Err(value
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Pi {command} failed")))
}

/// The same line Paseo puts on the client's timeline as an `assistant_message`
/// (`agent-manager.ts:2360-2366`), published as our assistant text and
/// journaled as a daemon-authored row, so replay derives it back.
fn publish_outcome(runtime: &Arc<SessionRuntime>, text: String) {
    let _ = runtime.publish_daemon_event(SessionEvent::AgentMessage {
        message_id: None,
        text,
        parent_tool_use_id: None,
        spawn_depth: None,
    });
}

#[cfg(test)]
#[path = "pi_out_of_band_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pi_compact_tests.rs"]
mod compact_tests;
