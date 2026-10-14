//! The one human question about a browser, asked once per session.
//!
//! One responsibility: hold the per-session grant behind `browser_tools::call`
//! and raise the card that opens it. Reading a page is never behind it, the
//! saved-login card stays a per-call question about a password
//! (`browser_login`), and a local port the session does not own is still asked
//! on its own road (`browser_ports`) — three separate questions about three
//! different powers.

use devboule_protocol::{PermissionOption, SessionEvent, SessionOrigin};

use crate::mcp_broker::tools::first_use::{
    ask_card_in_any_mode, card_id, oneline, Gate, GateMark, BROWSER_GROUP,
};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::provider_catalog::{browser_action_tool, ModeGate};
use crate::server::ServerState;

/// The answer that keeps the grant for the rest of the session.
const OPTION_SESSION: &str = "session";
/// Any other answer is a refusal, and the card mints no other id.
const REFUSED: &str = "permission refused";
/// A call that finds the card still on the table is told to come back, the
/// sentence every other gate gives a racing caller: the question is already
/// with the person, so there is nothing to answer here.
const PENDING: &str = "permission pending; retry";

/// Pass the browser gate for `tool`, raising the session's one card the first
/// time the session's mode asks.
///
/// The mode is read now, as everywhere else in the first-use family: an
/// automatic mode approves every act and is remembered as a grant, so the rest
/// of the session is one lookup; a plan or read-only mode is refused before
/// any card, naming the mode; every other mode asks once. Allowing opens the
/// gate for the rest of the session, refusing leaves it shut and answers
/// nothing — the next browser intent asks again, because a refusal to this
/// call is not an answer about the next one.
pub(in crate::mcp_broker) fn ensure_browser_allowed(
    state: &ServerState,
    broker: &McpBroker,
    registration: &RegisteredSession,
    tool: &str,
) -> Result<(), String> {
    if !browser_action_tool(tool) {
        return Ok(());
    }
    let session_id = registration.session_id.as_str();
    match state
        .sessions
        .live_runtime(session_id, &registration.owner)
        .map(|runtime| runtime.mode_gate())
    {
        Some(ModeGate::Auto) => {
            broker.remember_gate_mark(session_id, BROWSER_GROUP, GateMark::Open);
            return Ok(());
        }
        Some(ModeGate::Refuse(sentence)) => return Err(sentence),
        _ => {}
    }
    match broker.claim_first_use(session_id, BROWSER_GROUP) {
        Gate::Open => return Ok(()),
        Gate::Pending => return Err(PENDING.to_string()),
        Gate::Claimed => {}
    }
    let card = browser_card(state, registration);
    let answer = ask_card_in_any_mode(state, session_id, &registration.owner, card);
    // Whatever the answer was, nothing is left behind: a card this gate did
    // not answer, a refusal, a card nobody answered — the next call asks
    // again rather than inheriting this one's mark.
    match answer {
        Ok(option) if option == OPTION_SESSION => {
            broker.settle_first_use(session_id, BROWSER_GROUP, GateMark::Open);
            Ok(())
        }
        Ok(_) => {
            broker.settle_first_use(session_id, BROWSER_GROUP, GateMark::None);
            Err(REFUSED.to_string())
        }
        Err(sentence) => {
            broker.settle_first_use(session_id, BROWSER_GROUP, GateMark::None);
            Err(sentence)
        }
    }
}

/// The card: who is asking, and what an allow buys.
///
/// The title names the session rather than "an agent", because the person
/// deciding is looking at a workspace of several and the question is about one
/// of them. The sentence says what allowing really buys — a whole browser for
/// the rest of the session — because neither button name carries that, and a
/// licence nobody can price is a licence nobody can give.
fn browser_card(state: &ServerState, registration: &RegisteredSession) -> SessionEvent {
    let session_id = registration.session_id.as_str();
    let name = state
        .sessions
        .display_name_of(session_id, &registration.owner)
        .map(|name| oneline(&name))
        .unwrap_or_else(|| "This agent".to_string());
    SessionEvent::PermissionRequest {
        tool_call_id: card_id(session_id, BROWSER_GROUP),
        title: format!("{name} wants to use the browser"),
        description: Some(format!(
            "Allowing lets {name} use the browser — open tabs, go to pages, click, type — \
             for the rest of this session without asking again. Denying refuses this call; \
             the next thing {name} tries in a browser asks again."
        )),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            PermissionOption {
                option_id: OPTION_SESSION.to_string(),
                name: "Allow".to_string(),
                kind: "allow_session".to_string(),
            },
            PermissionOption {
                option_id: "deny".to_string(),
                name: "Deny".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
        // Forced: the card's only allow is a session grant, not an
        // `allow_once`, so without this the app reads it as offering no allow
        // at all and shows its unclaimable notice instead of the buttons.
        is_chooser: Some(true),
        kind: None,
        plan: None,
        questions: None,
        origin: SessionOrigin::unknown(),
        create_agent: None,
    }
}
