//! The two acts that ask a person in every mode, however the session runs: a
//! command or message that lands on another machine, and the use of a saved
//! login. Every other card follows the session's provider mode
//! (`first_use::ensure_write_approved`); these never do, and there is no third.
//!
//! One responsibility: decide, from a call alone, whether it always asks
//! ([`always_card`]), and raise the paired-device card for the router. The
//! saved-login card names the site and the login the host's preview found, so
//! the tool body raises it after that preview, through the same mode-blind
//! `first_use::ask_card_in_any_mode`. A plan or read-only session does not act,
//! so it is refused before either card.

use std::sync::Arc;

use devboule_protocol::{PermissionOption, SessionEvent, SessionOrigin};
use serde_json::Value;

use super::first_use::{ask_card_in_any_mode, card_id, mark_fact_lines, mode_refusal};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::RegisteredSession;
use crate::provider_catalog::{
    MCP_BROWSER_FILL_LOGIN_TOOL, MCP_LIST_PEER_AGENTS_TOOL, MCP_SEND_MESSAGE_TOOL,
};
use crate::server::ServerState;
use crate::visible_text::{long_message_tail, visible_text};

/// How much of a target session an audit row keeps.
const AUDITED_TARGET: usize = 64;

/// What a tool that can name a paired device does there.
pub(in crate::mcp_broker) enum Effect {
    /// It sends something to the device: the person is asked, every call.
    Acts {
        /// The sentence the card finishes with the device's name.
        phrase: &'static str,
        /// The arguments that name the target session and carry the content.
        target_arg: &'static str,
        content_arg: &'static str,
        content_label: &'static str,
    },
    /// It only asks the device what is live there; nothing lands on it.
    Reads,
}

/// Every tool whose arguments can name a paired device, and what it does
/// there. A tool that gains a `deviceId` argument without a row here fails the
/// walk in `always_card_walk_tests.rs`, so no road to another machine is
/// classified by omission.
pub(in crate::mcp_broker) const PEER_ROADS: &[(&str, Effect)] = &[
    (
        MCP_SEND_MESSAGE_TOOL,
        Effect::Acts {
            phrase: "send a message to",
            target_arg: "to_agent",
            content_arg: "text",
            content_label: "message",
        },
    ),
    (MCP_LIST_PEER_AGENTS_TOOL, Effect::Reads),
];

/// A command bound for another machine, as the card shows it.
pub(in crate::mcp_broker) struct PeerCommand {
    pub(in crate::mcp_broker) device_id: String,
    phrase: &'static str,
    target: String,
    content_label: &'static str,
    content: String,
}

impl PeerCommand {
    /// The audit row for one decision on this command: who it went to, which
    /// session, how long it was — never the content itself.
    pub(in crate::mcp_broker) fn audit_outcome(&self, decision: &str) -> String {
        let target: String = visible_text(&self.target, false)
            .chars()
            .take(AUDITED_TARGET)
            .collect();
        format!(
            "{decision}; device {}; session {target}; {} characters",
            self.device_id,
            self.content.chars().count()
        )
    }
}

/// The acts that always ask.
pub(in crate::mcp_broker) enum AlwaysCard {
    /// The call's effect lands on a paired device.
    PairedDevice(PeerCommand),
    /// A saved login is about to be typed into a page. The card is the tool
    /// body's, raised after the host names the site and the login.
    SavedLogin,
}

/// The one place that decides a call always asks.
pub(in crate::mcp_broker) fn always_card(tool: &str, arguments: &Value) -> Option<AlwaysCard> {
    if tool == MCP_BROWSER_FILL_LOGIN_TOOL {
        return Some(AlwaysCard::SavedLogin);
    }
    let (_, effect) = PEER_ROADS.iter().find(|(name, _)| *name == tool)?;
    let Effect::Acts {
        phrase,
        target_arg,
        content_arg,
        content_label,
    } = effect
    else {
        return None;
    };
    // An absent `deviceId` is the local road, which follows the mode; a
    // malformed one is refused by the tool body before anything is sent.
    let device_id = arguments
        .get("deviceId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())?;
    let argument = |name: &str| {
        arguments
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some(AlwaysCard::PairedDevice(PeerCommand {
        device_id: device_id.to_string(),
        phrase,
        target: argument(target_arg),
        content_label,
        content: argument(content_arg),
    }))
}

/// Put one command bound for another machine to the person, whatever the
/// session's mode (a plan or read-only one is refused first), and wait for the
/// answer without a timeout. `Ok` is the person's "allow this call"; `Err` is
/// the reply the router sends instead — the refusal the tool body would give
/// for a device this session may not call, the mode's sentence, or the card's
/// own refusal.
pub(in crate::mcp_broker) fn ask_peer_command(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: &Value,
    command: &PeerCommand,
) -> Result<(), Value> {
    let device = crate::mcp_peer_agents::resolve_paired_device(
        state,
        &registration.owner,
        &command.device_id,
    )
    .map_err(|error| rpc_error(id.clone(), error.code, &error.sentence))?;
    if let Some(sentence) = mode_refusal(state, &registration.session_id, &registration.owner) {
        return Err(tool_error(id, &sentence));
    }
    let target_name = remote_session_name(state, registration, command);
    let card = peer_command_card(registration, &device.display_name, command, target_name);
    match ask_card_in_any_mode(state, &registration.session_id, &registration.owner, card) {
        Ok(option) if option == ALLOW => Ok(()),
        Ok(_) => Err(tool_error(id, "permission refused")),
        Err(sentence) => Err(tool_error(id, &sentence)),
    }
}

const ALLOW: &str = "once";
const DENY: &str = "deny";

/// The name the device's own roster gives the target session, when it answers
/// and knows it: what lets the person tell which agent would receive the
/// message. The raw id the agent typed is the fallback, never the answer.
fn remote_session_name(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    command: &PeerCommand,
) -> Option<String> {
    let roster =
        crate::mcp_peer_agents::list_peer_agents(state, &registration.owner, &command.device_id)
            .ok()?;
    roster
        .get("agents")?
        .as_array()?
        .iter()
        .find(|agent| agent.get("sessionId").and_then(Value::as_str) == Some(&command.target))?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

/// The card: which machine, which session on it, and the whole message, with
/// its length and, when it is long, its end first. Nothing is cut and nothing
/// invisible is left invisible; only this call is offered — no grant for the
/// session exists.
fn peer_command_card(
    registration: &RegisteredSession,
    device: &str,
    command: &PeerCommand,
    target_name: Option<String>,
) -> SessionEvent {
    let device = visible_text(device, false);
    let session = match target_name {
        Some(name) => format!(
            "{} ({})",
            visible_text(&command.target, false),
            visible_text(&name, false)
        ),
        None => visible_text(&command.target, false),
    };
    let rendered = visible_text(&command.content, true);
    let length = command.content.chars().count();
    // A long message says so and repeats its end first, so the last words are
    // in view without scrolling the box.
    let message = match long_message_tail(&rendered) {
        Some(tail) => format!(
            "{label}: {length} characters, longer than the box shows. It ends with:\n{tail}\n\
             The whole {label}:\n{rendered}",
            label = command.content_label
        ),
        None => format!(
            "{} ({length} characters):\n{rendered}",
            command.content_label
        ),
    };
    let facts = format!("device: {device}\nsession: {session}\n{message}");
    let marked = mark_fact_lines(&facts).join("\n");
    SessionEvent::PermissionRequest {
        tool_call_id: card_id(&registration.session_id, "peer_command"),
        title: format!("Allow this agent to {} {device}?", command.phrase),
        description: Some(format!(
            "An agent asked to {} {device}, another machine paired with this one. \
             This always asks, whatever the session's mode. What follows is everything \
             that would be sent; invisible characters and long runs of blanks are \
             spelled out.\n{marked}\n\n\"Allow this call\" approves only this call.",
            command.phrase
        )),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            PermissionOption {
                option_id: ALLOW.to_string(),
                name: "Allow this call".to_string(),
                kind: "allow_once".to_string(),
            },
            PermissionOption {
                option_id: DENY.to_string(),
                name: "Deny".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
        // Forced: the card is answered by option id, which only a chooser
        // reports back.
        is_chooser: Some(true),
        kind: None,
        plan: None,
        questions: None,
        origin: SessionOrigin::unknown(),
        create_agent: None,
    }
}
