//! The fate of one slash prompt Pi answers without starting a model turn.
//!
//! Measured on a live `pi --mode rpc`: an extension command Pi handles
//! itself (`/goal-list`) answers the prompt rpc `success:true`, emits its
//! output as `extension_ui_request {method:"notify"}` frames, and starts no
//! turn — no `agent_start`, no `turn_end` — so nothing ever ends the run.
//! This is the tracking one such prompt carries from its write to its
//! response: the window's output, and the decision Pi's own one `get_state`
//! answer settles. The reader acts on the decision; the writers note the
//! prompts.

use std::sync::Mutex;

use serde_json::Value;

use super::commands::{is_out_of_band_command, parse_slash_invocation};

/// What one window's output may become: a command's own notify is a line
/// or two, so collection stops at this count and this many bytes — a
/// flood past them is not output at all.
const MAX_WINDOW_NOTIFIES: usize = 8;
const MAX_WINDOW_BYTES: usize = 16 * 1024;

/// One slash prompt between its write and its response.
struct InFlight {
    id: String,
    agent_start_seen: bool,
    /// False once a confirm card has gone up: the window still owes its
    /// response the output it holds, but takes nothing more.
    collects: bool,
    notifies: Vec<String>,
}

/// The one `get_state` that decides a slash prompt's fate, named by the id
/// its answer will carry.
struct Probe {
    id: String,
}

#[derive(Default)]
struct FateState {
    in_flight: Option<InFlight>,
    probe: Option<Probe>,
    /// The id of the last prompt written and not yet answered — plain and
    /// slash alike. A prompt response answers it; a refusal answers for it
    /// and for nothing else on the wire.
    current_prompt: Option<String>,
}

/// Which slash prompts may end their own run, held between the prompt
/// writers and the reader: at most one prompt in flight, at most one probe
/// awaiting its answer.
pub(super) struct SlashPromptFate {
    state: Mutex<FateState>,
}

impl SlashPromptFate {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(FateState::default()),
        }
    }

    /// The writer's note, taken before the frame goes on the wire. The
    /// crate's one slash parser decides what counts as a command; the
    /// out-of-band commands stay untracked, from the list the door itself
    /// answers to — only an attachment prompt can carry them here and the
    /// compact guard claimed no slot for that run. A new prompt also moots
    /// an armed probe: its answer would decide a fate that is no longer
    /// pending.
    pub(super) fn note_prompt(&self, id: &str, text: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.probe = None;
        state.current_prompt = Some(id.to_string());
        state.in_flight = parse_slash_invocation(text)
            .filter(|invocation| !is_out_of_band_command(&invocation.name))
            .map(|_| InFlight {
                id: id.to_string(),
                agent_start_seen: false,
                collects: true,
                notifies: Vec::new(),
            });
    }

    /// A prompt response on the wire. `true` when it answers the prompt we
    /// wrote and have not seen answered — the id is consumed either way, so
    /// a duplicate or a late refusal for an earlier prompt is foreign.
    pub(super) fn take_current_prompt(&self, id: &str) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let matched = state.current_prompt.as_deref() == Some(id);
        if matched {
            state.current_prompt = None;
        }
        matched
    }

    /// An `agent_start` on the wire: a model turn began. A prompt whose
    /// response is still outstanding ends the normal way, and a probe whose
    /// answer has not arrived decides nothing any more.
    pub(super) fn note_agent_start(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if let Some(in_flight) = state.in_flight.as_mut() {
            in_flight.agent_start_seen = true;
        }
        state.probe = None;
    }

    /// A `notify` on the wire. Only the window between a slash prompt's
    /// write and its response is the command's output, only while nothing
    /// is waiting on a person (see [`Self::stop_collecting`]), and only up
    /// to [`MAX_WINDOW_NOTIFIES`] messages and [`MAX_WINDOW_BYTES`] of
    /// text: everything else belongs to no run and is dropped.
    pub(super) fn note_notify(&self, message: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(in_flight) = state.in_flight.as_mut() else {
            return;
        };
        let collected: usize = in_flight.notifies.iter().map(String::len).sum();
        if !in_flight.collects
            || in_flight.notifies.len() >= MAX_WINDOW_NOTIFIES
            || collected + message.len() > MAX_WINDOW_BYTES
        {
            return;
        }
        in_flight.notifies.push(message.to_string());
    }

    /// A confirm card is waiting on a person. The window stays open for
    /// its response — what it already holds still goes out — but it takes
    /// nothing more: a human's seconds are not the command's output, and
    /// this is where an unrelated extension's text would otherwise reach
    /// the journal.
    pub(super) fn stop_collecting(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if let Some(in_flight) = state.in_flight.as_mut() {
            in_flight.collects = false;
        }
    }

    /// The prompt's own response. `Some` carries the window's output — the
    /// caller publishes it whatever the fate turns out to be, a rejected
    /// prompt's included — and says whether the deciding `get_state` is
    /// warranted: only a successful response with no model turn since the
    /// write gets one. `None`: the id is not the tracked prompt.
    pub(super) fn prompt_responded(&self, id: &str, success: bool) -> Option<(Vec<String>, bool)> {
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        let tracked = state
            .in_flight
            .as_ref()
            .is_some_and(|in_flight| in_flight.id == id);
        if !tracked {
            return None;
        }
        let in_flight = state.in_flight.take()?;
        Some((in_flight.notifies, success && !in_flight.agent_start_seen))
    }

    /// The reader armed the probe it just wrote; `probe_id` is the id the
    /// answer will name.
    pub(super) fn arm_probe(&self, probe_id: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.probe = Some(Probe {
            id: probe_id.to_string(),
        });
    }

    /// A response on the wire. `true`: this is the probe's answer and Pi
    /// says nothing streams and nothing is queued — the command was handled
    /// locally, and the reader ends the run. `false`: not this probe's
    /// answer, a model turn started meanwhile, or Pi is still working — an
    /// absent or mistyped field never ends a run.
    pub(super) fn state_answered(&self, answer_id: Option<&str>, answer: &Value) -> bool {
        let Some(id) = answer_id else {
            return false;
        };
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.probe.as_ref().is_none_or(|probe| probe.id != id) {
            return false;
        }
        state.probe = None;
        answer.get("data").is_some_and(|data| {
            data.get("isStreaming").and_then(Value::as_bool) == Some(false)
                && data.get("pendingMessageCount").and_then(Value::as_u64) == Some(0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{SlashPromptFate, MAX_WINDOW_BYTES, MAX_WINDOW_NOTIFIES};

    #[test]
    fn a_window_collects_up_to_its_stated_cap() {
        let fate = SlashPromptFate::new();
        fate.note_prompt("p-1", "/goal-list");
        for line in 0..MAX_WINDOW_NOTIFIES {
            fate.note_notify(&format!("line {line}"));
        }
        fate.note_notify("the ninth line");
        assert_eq!(
            fate.prompt_responded("p-1", true)
                .expect("the tracked prompt answers")
                .0
                .len(),
            MAX_WINDOW_NOTIFIES,
            "the count cap stops collection outright"
        );

        let fate = SlashPromptFate::new();
        fate.note_prompt("p-1", "/goal-list");
        fate.note_notify(&"x".repeat(MAX_WINDOW_BYTES + 1));
        assert!(
            fate.prompt_responded("p-1", true)
                .expect("the tracked prompt answers")
                .0
                .is_empty(),
            "one message past the byte cap is not collected"
        );
    }

    #[test]
    fn a_confirm_card_closes_the_collection_side() {
        let fate = SlashPromptFate::new();
        fate.note_prompt("p-1", "/goal-list");
        fate.note_notify("before the card");
        fate.stop_collecting();
        fate.note_notify("while a person decides");
        let (output, probe) = fate.prompt_responded("p-1", true).expect("tracked");
        assert_eq!(output, ["before the card"]);
        assert!(probe, "the window still decides its own fate");
    }
}
