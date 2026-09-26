//! The consent step the terminal writes share, and the facts each of their
//! three cards states: what the person is being asked to allow, about which
//! terminal, in which workspace, who opened it, and where it started.

use devboule_protocol::Session;

use crate::mcp_broker::tools::first_use::ensure_write_allowed;
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::server::ServerState;

use super::terminal_common::TerminalError;

/// The consent step: the first call of one act raises that act's card on the
/// caller's own session, a later one passes on the group's own mark, and a
/// refusal is the call's answer. `group` is the act's — never a shared one,
/// so a session allowed to open a shell is still carded to type or to kill.
pub(super) fn write_after_card(
    state: &ServerState,
    broker: &McpBroker,
    registration: &RegisteredSession,
    group: &str,
    subject: &str,
    facts: &[(&'static str, String)],
) -> Result<(), TerminalError> {
    let fact_refs = facts
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    ensure_write_allowed(
        state,
        broker,
        &registration.session_id,
        &registration.owner,
        group,
        subject,
        &fact_refs,
    )
    .map_err(TerminalError::Refused)
}

/// The create card's subject and facts: the workspace the shell opens in and
/// the directory that workspace already resolved to — the same directory the
/// create will hand the process, read before the card so a workspace that
/// cannot answer it refuses the call here instead of being shown as a
/// placeholder later.
pub(super) fn create_card_facts(
    workspace: &str,
    cwd: &str,
    name: Option<&str>,
) -> (String, Vec<(&'static str, String)>) {
    let mut facts = vec![
        ("workspace", workspace.to_string()),
        ("cwd", cwd.to_string()),
    ];
    if let Some(name) = name {
        facts.push(("name", name.to_string()));
    }
    ("creating a terminal".to_string(), facts)
}

/// The facts both target cards carry: which terminal, in which workspace,
/// who opened it, and where it started — `started in` because that is what
/// the row records: a shell that has since `cd`'d elsewhere is still the
/// terminal this card named, and the card must not claim to know where it
/// is now.
pub(super) fn target_facts(
    target: &Session,
    workspace: &str,
    opener: &str,
) -> Vec<(&'static str, String)> {
    let mut facts = vec![
        ("terminal", target.title.clone()),
        ("workspace", workspace.to_string()),
        ("opened by", opener.to_string()),
    ];
    if let Some(cwd) = target.cwd.as_deref() {
        facts.push(("started in", cwd.to_string()));
    }
    facts
}

/// Whose terminal a card names: `you` when no session stamped itself as the
/// opener (the person's own, opened from the app), `this agent` when the
/// caller opened it, `another agent` when some other session did.
pub(super) fn opener_label(created_by: Option<&str>, caller: &str) -> &'static str {
    match created_by {
        None => "you",
        Some(opener) if opener == caller => "this agent",
        Some(_) => "another agent",
    }
}
