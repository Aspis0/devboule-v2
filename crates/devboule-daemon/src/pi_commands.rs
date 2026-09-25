//! Pi's `get_commands` list: the request the composer menu is fed from, and
//! the waiter that turns its reply — or its absence — into the published
//! command list.
//!
//! Paseo is the reference (owner rule, 2026-09-24): each function translates
//! one of its `providers/pi` functions into Rust with the file and line
//! cited, and nothing here is redesigned — the look is not in scope. The two
//! commands pi executes itself live in the sibling `pi_out_of_band.rs`.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use devboule_protocol::SessionEvent;
use serde_json::Value;

use super::PiControl;
use crate::session::SessionRuntime;

/// What any pi request that carries its own deadline may wait: Paseo's pi
/// runtime is built with `DEFAULT_PI_RPC_TIMEOUT_MS` = 60 s
/// (`pi/agent.ts:107,1236-1243`) and that is what its `get_commands`,
/// `set_auto_compaction` and `get_state` calls wait. pi's own measured
/// latency fits inside it (RECON A5-common §A.7: "tens of seconds", one
/// successful reply inside a 120 s window).
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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
/// the reader publishes what `pi_view` derives from the row — but only for
/// the reply this waiter's registration claims (`pi_client.rs`, the response
/// arm), so a late or foreign answer reaches neither the transcript nor the
/// journal. Every way of *not* arriving leaves the list at the seeds, with
/// one log line and no user-facing error.
fn await_commands_reply(reply: PiCommandsReply, runtime: &Arc<SessionRuntime>, timeout: Duration) {
    let failure = match reply.response.recv_timeout(timeout) {
        Ok(Ok(value)) => {
            if value.get("success").and_then(Value::as_bool) == Some(true) {
                return;
            }
            refusal_log_line(
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error"),
            )
        }
        Ok(Err(message)) => format!("pi get_commands got no reply: {message}"),
        Err(RecvTimeoutError::Timeout) => match on_commands_timeout(&reply, timeout) {
            Some(failure) => failure,
            None => return,
        },
        Err(RecvTimeoutError::Disconnected) => {
            "pi get_commands got no reply: the request was never registered".to_string()
        }
    };
    eprintln!("{failure}; the command list stays at the seeds");
    let _ = runtime.publish_daemon_event(SessionEvent::AvailableCommands {
        commands: crate::pi_view::seeded_commands(),
    });
}

/// The timeout arm's decision, split out so the race it settles can be
/// staged: `abandon` fails only when the reader already removed the entry,
/// and `deliver` puts the answer on the channel under the same lock — so a
/// missing entry with a buffered answer means the reader owns it, while a
/// missing entry with an empty channel means nobody does and the waiter
/// publishes. `None` stays silent only for a successful reply the reader
/// claimed; `Some` is the one log line the seeds are published with.
fn on_commands_timeout(reply: &PiCommandsReply, timeout: Duration) -> Option<String> {
    // The registration would otherwise sit in the table until the child
    // ends; Paseo deletes a timed-out request the same way
    // (`jsonl-rpc-process.ts:160-163`).
    let Some(id) = reply.id.as_deref() else {
        // No registration was ever written, so no answer can be claimed:
        // the seeds are this waiter's to publish.
        return Some(format!("pi get_commands got no reply within {timeout:?}"));
    };
    if reply.control.abandon(id) {
        return Some(format!("pi get_commands got no reply within {timeout:?}"));
    }
    match reply.response.try_recv() {
        Ok(Ok(value)) if value.get("success").and_then(Value::as_bool) == Some(true) => None,
        Ok(Ok(value)) => Some(refusal_log_line(
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error"),
        )),
        Ok(Err(message)) => Some(format!("pi get_commands got no reply: {message}")),
        // Entry gone and channel empty: removal and send share one lock on
        // both sides, so no answer is still in flight — nobody owns this but
        // the waiter, which publishes rather than staying silent.
        _ => Some(format!("pi get_commands got no reply within {timeout:?}")),
    }
}

/// The one line a refused `get_commands` leaves in the daemon log. Extracted
/// so its shape can be pinned: pi's error field is not ours to log verbatim —
/// a provider or extension can put a local path or a config value in it
/// (review A5-2 #9) — so the line is a fixed sentence plus the text's length.
fn refusal_log_line(error: &str) -> String {
    format!(
        "pi get_commands was refused (pi's error text was {} characters long)",
        // `str::len` is UTF-8 bytes; the line names characters.
        error.chars().count()
    )
}

/// A `/…` invocation, parsed the way Paseo parses one (`pi/agent.ts:1797-1811`):
/// the name up to the first whitespace, a name containing a second slash is
/// not a command at all, and the trimmed remainder is kept only when it is
/// non-empty.
pub(super) struct SlashInvocation {
    pub(super) name: String,
    pub(super) args: Option<String>,
}

pub(super) fn parse_slash_invocation(text: &str) -> Option<SlashInvocation> {
    let trimmed = js_trim(text);
    if !trimmed.starts_with('/') || trimmed.len() <= 1 {
        return None;
    }
    let without_prefix = &trimmed[1..];
    let first_whitespace = without_prefix
        .char_indices()
        .find(|(_, character)| is_js_space(*character));
    let name = match first_whitespace {
        Some((index, _)) => &without_prefix[..index],
        None => without_prefix,
    };
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let args = first_whitespace
        .map(|(index, character)| {
            js_trim(&without_prefix[index + character.len_utf8()..]).to_string()
        })
        .filter(|args| !args.is_empty());
    Some(SlashInvocation {
        name: name.to_string(),
        args,
    })
}

/// JavaScript's whitespace set (ECMA-262 WhiteSpace + LineTerminator): it
/// includes U+FEFF and excludes U+0085, both of which differ from Rust's
/// `char::is_whitespace`. Paseo's parse runs on JS `trim()` and `/\s/`
/// (`pi/agent.ts:1802-1811`), so ours matches that set — not Rust's
/// (review A5-2 #7).
fn is_js_space(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{0b}' | '\x0C' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `String.prototype.trim` over [`is_js_space`]'s set.
pub(super) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

#[cfg(test)]
#[path = "pi_commands_tests.rs"]
mod tests;
