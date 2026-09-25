//! Pi's slash-command surface: the `get_commands` list the composer menu
//! shows, and the two commands pi executes itself, out of band.
//!
//! Paseo is the reference (owner rule, 2026-09-24): each function translates
//! one of its `providers/pi` functions into Rust with the file and line
//! cited, and nothing here is redesigned — the look is not in scope.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use devboule_protocol::SessionEvent;
use serde_json::Value;

use super::PiControl;
use crate::session::{OutOfBandCommands, SessionRuntime};

/// What any pi request that carries its own deadline may wait: Paseo's pi
/// runtime is built with `DEFAULT_PI_RPC_TIMEOUT_MS` = 60 s
/// (`pi/agent.ts:107,1236-1243`) and that is what its `get_commands`,
/// `set_auto_compaction` and `get_state` calls wait. pi's own measured
/// latency fits inside it (RECON A5-common §A.7: "tens of seconds", one
/// successful reply inside a 120 s window). `compact` is the deliberate
/// exception and waits with no deadline at all, as Paseo does.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// One outstanding `get_commands`: the waiter it registered (absent when the
/// request could not be written, so there is no registration to clean up),
/// and the channel the reader delivers the reply on.
pub(super) struct PiCommandsReply {
    control: Arc<PiControl>,
    id: Option<String>,
    response: mpsc::Receiver<Result<Value, String>>,
}

/// Send `{"type":"get_commands"}` without waiting for it: the reply was
/// measured to take tens of seconds, and neither a session start nor a
/// prompt may sit on it (Paseo `cli-runtime.ts:210-215` sends; the reply is
/// matched by id, `jsonl-rpc-process.ts:263-292`).
///
/// A write that fails still answers with a receiver no reply can reach: the
/// waiter's own no-reply branch then leaves the list at the seeds and logs
/// its one line, rather than leaving nothing at all.
pub(super) fn begin_get_commands(control: &Arc<PiControl>) -> PiCommandsReply {
    match control.begin("get_commands", Value::Null) {
        Ok((id, response)) => PiCommandsReply {
            control: Arc::clone(control),
            id: Some(id),
            response,
        },
        Err(_) => {
            let (sender, response) = mpsc::channel();
            drop(sender);
            PiCommandsReply {
                control: Arc::clone(control),
                id: None,
                response,
            }
        }
    }
}

/// Start the list waiter detached, from the reader's first feed — the first
/// moment a session reader exists to deliver the reply and a runtime to
/// publish it. Nothing on any send path ever waits for this answer.
pub(super) fn spawn_commands_waiter(reply: PiCommandsReply, runtime: Arc<SessionRuntime>) {
    let _ = std::thread::Builder::new()
        .name("pi-get-commands".to_string())
        .spawn(move || await_commands_reply(reply, &runtime, REQUEST_TIMEOUT));
}

/// Wait out one list request. A reply that arrived is the row's business:
/// the reader publishes what `pi_view` derives from the row, so success is
/// silent here. Every way of *not* arriving leaves the list at the seeds,
/// with one log line and no user-facing error.
fn await_commands_reply(reply: PiCommandsReply, runtime: &Arc<SessionRuntime>, timeout: Duration) {
    let failure = match reply.response.recv_timeout(timeout) {
        Ok(Ok(value)) => {
            if value.get("success").and_then(Value::as_bool) == Some(true) {
                return;
            }
            format!(
                "pi get_commands failed: {}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            )
        }
        Ok(Err(message)) => format!("pi get_commands got no reply: {message}"),
        Err(RecvTimeoutError::Timeout) => {
            // The registration would otherwise sit in the table until the
            // child ends; Paseo deletes a timed-out request the same way
            // (`jsonl-rpc-process.ts:160-163`).
            if let Some(id) = reply.id.as_deref() {
                if let Ok(mut pending) = reply.control.pending.lock() {
                    pending.remove(id);
                }
            }
            format!("pi get_commands got no reply within {timeout:?}")
        }
        Err(RecvTimeoutError::Disconnected) => {
            "pi get_commands got no reply: the request was never registered".to_string()
        }
    };
    eprintln!("{failure}; the command list stays at the seeds");
    let _ = runtime.publish_daemon_event(SessionEvent::AvailableCommands {
        commands: crate::pi_view::seeded_commands(),
    });
}

/// A `/…` invocation, parsed the way Paseo parses one (`pi/agent.ts:1797-1811`):
/// the name up to the first whitespace, a name containing a second slash is
/// not a command at all, and the trimmed remainder is kept only when it is
/// non-empty.
pub(super) struct SlashInvocation {
    name: String,
    args: Option<String>,
}

pub(super) fn parse_slash_invocation(text: &str) -> Option<SlashInvocation> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') || trimmed.len() <= 1 {
        return None;
    }
    let without_prefix = &trimmed[1..];
    let first_whitespace = without_prefix
        .char_indices()
        .find(|(_, character)| character.is_whitespace());
    let name = match first_whitespace {
        Some((index, _)) => &without_prefix[..index],
        None => without_prefix,
    };
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let args = first_whitespace
        .map(|(index, character)| {
            without_prefix[index + character.len_utf8()..]
                .trim()
                .to_string()
        })
        .filter(|args| !args.is_empty());
    Some(SlashInvocation {
        name: name.to_string(),
        args,
    })
}

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
    let mode = args.unwrap_or("toggle").trim().to_ascii_lowercase();
    match mode.as_str() {
        "on" | "true" | "enable" | "enabled" => AutoCompactMode::Enabled,
        "off" | "false" | "disable" | "disabled" => AutoCompactMode::Disabled,
        "toggle" => AutoCompactMode::Toggle,
        _ => AutoCompactMode::Unknown,
    }
}

/// The two commands pi runs itself, dispatched where Paseo dispatches them
/// (`pi/agent.ts:1667-1691` `tryHandleOutOfBand`, called from
/// `agent-manager.ts:2353` `tryRunOutOfBand`): the text never becomes a
/// prompt and never begins a turn.
pub(super) struct PiOutOfBandCommands {
    pub(super) control: Arc<PiControl>,
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
            "compact" => run_compact(&self.control, invocation.args, runtime),
            "autocompact" => run_autocompact(&self.control, invocation.args, runtime),
            _ => {}
        }
    }
}

/// Paseo `pi/agent.ts:1819-1862` `executeCompactCommand`: the `compact`
/// RPC with the custom instructions when there are any
/// (`cli-runtime.ts:139-143`), awaited with no deadline — pi only replies
/// once the compaction is durable — and, after it, Paseo's failure line
/// verbatim. A success emits nothing from here: what Paseo shows after its
/// RPC is pi's own compaction timeline item, and our protocol has no event
/// for it (declared in the report as the one outcome we cannot mirror).
fn run_compact(control: &Arc<PiControl>, args: Option<String>, runtime: &Arc<SessionRuntime>) {
    let fields = match args {
        Some(args) => serde_json::json!({ "customInstructions": args }),
        None => serde_json::json!({}),
    };
    run_out_of_band_request(
        control,
        "compact",
        fields,
        None,
        runtime,
        |answer, runtime| {
            if let Err(message) = answer {
                publish_outcome(
                    runtime,
                    format!("[Error] Failed to compact context: {message}"),
                );
            }
        },
    );
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
                Some(REQUEST_TIMEOUT),
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
        Some(REQUEST_TIMEOUT),
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
    timeout: Option<Duration>,
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
    let _ = std::thread::Builder::new()
        .name(format!("pi-{command}"))
        .spawn(move || {
            let (id, response) = request;
            let answer = await_out_of_band(&control, command, id, response, timeout);
            on_answer(answer, &runtime);
        });
}

/// One out-of-band round trip, answered the way Paseo's client sees it: a
/// `success: false` carries pi's own error text (`jsonl-rpc-process.ts:283-289`
/// rejects with `response.error`), a closed channel is the reason the reader
/// woke it with, and a timeout exists only for the requests Paseo bounds —
/// `None` is its `JSONL_RPC_NO_TIMEOUT` for `compact`
/// (`cli-runtime.ts:134-143`), whose wait ends only when the child's output
/// does.
fn await_out_of_band(
    control: &Arc<PiControl>,
    command: &str,
    id: String,
    response: mpsc::Receiver<Result<Value, String>>,
    timeout: Option<Duration>,
) -> Result<Value, String> {
    let received = match timeout {
        Some(timeout) => response.recv_timeout(timeout),
        None => response.recv().map_err(|_| RecvTimeoutError::Disconnected),
    };
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
#[path = "pi_commands_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pi_out_of_band_tests.rs"]
mod out_of_band_tests;
