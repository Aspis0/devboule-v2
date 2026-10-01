//! Claude's mode vocabulary: the one mode table the manifest publishes
//! and the `unattended` marker answer derived from it.

use devboule_protocol::{SessionModeStateView, SessionModeView, UnattendedState};
use serde_json::Value;

/// One entry of the daemon's own Claude mode vocabulary, and the answer the
/// `unattended` marker derives from it.
///
/// The vocabulary and the marker's dictionary are **one table**: a mode added
/// to `mode_state` below is added to this table, and this table's
/// `unattended` field is required by the type — a new Claude mode cannot
/// exist without an answer, and there is no fall-through to be silent in.
/// This is route-B knowledge and it lives here, in the family that owns the
/// `--permission-mode` flag, never in a central table of mode names.
struct ClaudeMode {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    /// What the marker says for a session delivered in this mode. `Yes` only
    /// for `bypassPermissions` — the launch flag the daemon itself passes;
    /// every other mode stops at the human for at least one tool class, so
    /// the marker says `no` and the roster renders nothing.
    unattended: UnattendedState,
}

pub(crate) fn reported_permission_mode(envelope: &Value) -> Option<&str> {
    envelope
        .get("permissionMode")
        .or_else(|| envelope.get("mode"))
        .and_then(Value::as_str)
        .filter(|mode| !mode.is_empty())
}

/// The mode the daemon delivers when a create names none — the same default
/// the launch path passes to `--permission-mode`, so the marker and the
/// child cannot disagree about what an absent mode means.
pub(crate) const DEFAULT_MODE: &str = "default";

const CLAUDE_MODES: &[ClaudeMode] = &[
    ClaudeMode {
        id: "plan",
        name: "Plan Mode",
        description: "Analyze the codebase without executing tools or edits",
        unattended: UnattendedState::No,
    },
    ClaudeMode {
        id: "default",
        name: "Always Ask",
        description: "Prompts for permission the first time a tool is used",
        unattended: UnattendedState::No,
    },
    ClaudeMode {
        id: "acceptEdits",
        name: "Accept File Edits",
        description: "Automatically approves edit-focused tools without prompting",
        unattended: UnattendedState::No,
    },
    ClaudeMode {
        id: "auto",
        name: "Auto mode",
        description: "Uses a model classifier to review permission prompts automatically",
        unattended: UnattendedState::No,
    },
    ClaudeMode {
        id: "bypassPermissions",
        name: "Bypass",
        description: "Skip all permission prompts (use with caution)",
        unattended: UnattendedState::Yes,
    },
];

pub(crate) fn mode_state(current_mode_id: &str) -> SessionModeStateView {
    SessionModeStateView {
        current_mode_id: current_mode_id.to_string(),
        available_modes: CLAUDE_MODES
            .iter()
            .map(|mode| SessionModeView {
                id: mode.id.to_string(),
                name: mode.name.to_string(),
                description: Some(mode.description.to_string()),
            })
            .collect(),
    }
}

/// The marker's answer for one delivered Claude mode: the table walk above,
/// with the daemon's own default for a create that named none.
///
/// A mode id the table does not carry is a mode the daemon never authored —
/// it cannot be judged, and the answer is `unknown`, never `no`. The
/// delivery validation refuses such a mode before a child exists, so a
/// surviving child should never hit the miss; the miss arm exists so the
/// derivation itself stays honest if it ever is reached.
pub(crate) fn unattended_answer(delivered_mode: Option<&str>) -> UnattendedState {
    let mode_id = delivered_mode.unwrap_or(DEFAULT_MODE);
    CLAUDE_MODES
        .iter()
        .find(|mode| mode.id == mode_id)
        .map(|mode| mode.unattended)
        .unwrap_or(UnattendedState::Unknown)
}
