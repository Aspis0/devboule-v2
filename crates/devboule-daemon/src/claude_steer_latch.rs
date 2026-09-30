//! The unread-steer latch for the Claude family: person-borne steers the CLI
//! has queued but not read yet. Claude parks its turn inside the blocking
//! `can_use_tool` callback and does not read its input queue there, so a
//! multi-tool turn opens its next permission request with the steer still
//! unread. While the latch holds, every Claude permission request is denied
//! outright instead of carded — the steer is the answer — and the person
//! never sees a card for an intent they already overrode.
//!
//! Measured (CLI 2.1.284, our stream-json flags): the CLI acknowledges an
//! unread steer immediately with
//! `{"type":"command_lifecycle","command_uuid":<steer uuid>,"state":"queued"}`
//! and emits `state:"started"` for the same uuid at the moment the model
//! reads it. `queued` never releases; `started` and `completed` do.

use std::collections::HashSet;
use std::sync::Mutex;

use serde_json::Value;

/// The deny text the CLI relays to the model: the person answered with a
/// message instead of approving, and the message follows. Translated from
/// Paseo (packages/server/src/server/agent/providers/claude/agent.ts,
/// `STEER_SUPERSEDED_PERMISSION_MESSAGE`).
pub(crate) const STEER_SUPERSEDED_MESSAGE: &str =
    "The user answered with a message instead of approving. Their message follows.";

/// The unread-steer uuids of one Claude session.
#[derive(Default)]
pub(crate) struct ClaudeSteerLatch {
    unread: Mutex<HashSet<String>>,
}

impl ClaudeSteerLatch {
    /// A person-borne steer was written to the CLI: its requests are
    /// superseded until the CLI reads the steer.
    pub(crate) fn arm(&self, command_uuid: &str) {
        if let Ok(mut unread) = self.unread.lock() {
            unread.insert(command_uuid.to_string());
        }
    }

    /// Whether some steered prompt is still unread.
    pub(crate) fn is_armed(&self) -> bool {
        self.unread
            .lock()
            .map(|unread| !unread.is_empty())
            .unwrap_or(false)
    }

    /// The CLI read (or retired) this steer: its supersession ends, so a
    /// permission the agent asks for after acting on the message reaches the
    /// person.
    pub(crate) fn release(&self, command_uuid: &str) {
        if let Ok(mut unread) = self.unread.lock() {
            unread.remove(command_uuid);
        }
    }

    /// Turn end, interrupt or close: nothing is unread any more.
    pub(crate) fn clear(&self) {
        if let Ok(mut unread) = self.unread.lock() {
            unread.clear();
        }
    }

    /// Consume one inbound frame's lifecycle claim: every measured state
    /// except `queued` releases the uuid. `started`/`completed` mean the
    /// model read the steer; `cancelled` means the command is gone and will
    /// never be read — under either answer nothing is unread, so nothing is
    /// superseded. `queued` (the buffer ack sent while the CLI is still
    /// parked in its permission callback) releases nothing, and a frame that
    /// is not a lifecycle frame at all releases nothing.
    pub(crate) fn release_read(&self, frame: &Value) {
        let Some((command_uuid, state)) = read_command_lifecycle(frame) else {
            return;
        };
        if state != "queued" {
            self.release(command_uuid);
        }
    }
}

/// The `command_lifecycle` triple of one frame, when the frame is one.
fn read_command_lifecycle(frame: &Value) -> Option<(&str, &str)> {
    if frame.get("type").and_then(Value::as_str) != Some("command_lifecycle") {
        return None;
    }
    let command_uuid = frame.get("command_uuid").and_then(Value::as_str)?;
    let state = frame.get("state").and_then(Value::as_str)?;
    Some((command_uuid, state))
}
