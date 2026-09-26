//! The terminal writes: open a shell in the caller's own workspace, type
//! into one, kill one.
//!
//! One responsibility: the arguments, the consent and the reply of those
//! three writes. Each act carries its own consent group, so a session
//! allowed to open a shell is still carded to type into one or to kill one.
//! The two writes that name a terminal judge it through the registry's gate
//! (`reachable_terminal`, `session_terminals.rs`) before the card and again
//! inside the write, because a card waits on a person; the open names no
//! terminal and takes its workspace from the caller's own row instead.
//! Typed bytes never reach a log, a diagnostic, the card or the audit row:
//! the card counts them and the audit records the act.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, caller_conn, McpCaller};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::peer_policy::ConnPeer;
use crate::server::ServerState;

use super::first_use::{TERMINAL_CREATE_GROUP, TERMINAL_KEYS_GROUP, TERMINAL_KILL_GROUP};
use super::terminal_args::{parse_keys, parse_name, parse_terminal, KeysRequest};
use super::terminal_cards::{create_card_facts, opener_label, target_facts, write_after_card};
use super::terminal_common::{caller_workspace, terminal_reply, TerminalError};
use super::terminal_keys::{keys_preview, resolve_keys};

/// The one sentence a write answers with while the daemon is going down —
/// the guard the wire's create road takes, applied to all three acts: a
/// teardown is not a moment to open a shell, to type into a dying pty or to
/// race a kill against it.
const SHUTTING_DOWN: &str = "daemon is shutting down";

/// `devboule_create_terminal` (`{name?}`): a terminal in the caller's own
/// workspace, behind that act's card.
pub(in crate::mcp_broker) fn create(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    // A malformed request is answered before anything is audited or touched,
    // the way the sibling tools answer one.
    let name = match parse_name(&arguments) {
        Ok(name) => name,
        Err(sentence) => return terminal_reply(&id, Err(TerminalError::Invalid(sentence))),
    };
    let conn = caller_conn(state, &caller);
    let audit = |outcome_label: &str| {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_CREATE_TERMINAL_TOOL,
            &registration.session_id,
            outcome_label,
        );
    };
    let result = create_terminal(state, broker, registration, &conn.conn_peer, &id, &name);
    audit(if result.is_ok() { "ok" } else { "denied" });
    terminal_reply(&id, result)
}

/// `devboule_send_terminal_keys` (`{terminalId, keys, literal?}`): Paseo's
/// input shape, typed into one terminal of the caller's own workspace.
pub(in crate::mcp_broker) fn send_keys(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    let request = match parse_keys(&arguments) {
        Ok(request) => request,
        Err(sentence) => return terminal_reply(&id, Err(TerminalError::Invalid(sentence))),
    };
    let conn = caller_conn(state, &caller);
    let audit = |outcome_label: &str| {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_SEND_TERMINAL_KEYS_TOOL,
            &registration.session_id,
            outcome_label,
        );
    };
    let result = send_keys_to_terminal(state, broker, registration, &conn.conn_peer, &request);
    audit(if result.is_ok() { "ok" } else { "denied" });
    terminal_reply(&id, result)
}

/// `devboule_kill_terminal` (`{terminalId}`): the process tree dies and the
/// live session goes, behind that act's card; the journal row and the
/// transcript stay.
pub(in crate::mcp_broker) fn kill(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    let terminal = match parse_terminal(&arguments) {
        Ok(terminal) => terminal,
        Err(sentence) => return terminal_reply(&id, Err(TerminalError::Invalid(sentence))),
    };
    let conn = caller_conn(state, &caller);
    let audit = |outcome_label: &str| {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_KILL_TERMINAL_TOOL,
            &registration.session_id,
            outcome_label,
        );
    };
    let result = kill_one_terminal(state, broker, registration, &conn.conn_peer, &terminal);
    audit(if result.is_ok() { "ok" } else { "denied" });
    terminal_reply(&id, result)
}

fn create_terminal(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    conn_peer: &Option<ConnPeer>,
    id: &Value,
    name: &Option<String>,
) -> Result<Value, TerminalError> {
    let workspace = caller_workspace(state, registration, conn_peer)?;
    // The retry identity first, in the order the wire's create road takes
    // it: a retry answers the terminal the first call opened, and a retry
    // whose payload changed is refused with the wire's own sentence —
    // either way nothing below is judged, because nothing below would run.
    let retry_key = crate::server::creation_retry_key(&registration.session_id, id);
    let mut hold = match retry_key.as_deref() {
        Some(key) => match state.sessions.hold_creation_key(key) {
            Ok(hold) => Some(hold),
            Err(error) => return Err(TerminalError::Refused(error.message)),
        },
        None => None,
    };
    let fingerprint = format!(
        "create:terminal:{workspace}:{}",
        name.as_deref().unwrap_or("")
    );
    if let Some(key) = retry_key.as_deref() {
        match crate::server::stored_creation_session(state, &registration.owner, key, &fingerprint)
        {
            Ok(Some(existing)) => {
                if let Some(hold) = hold.as_mut() {
                    hold.commit();
                }
                return Ok(terminal_document(&existing));
            }
            Err(conflict) => return Err(TerminalError::Refused(conflict.message)),
            Ok(None) => {}
        }
    }
    // An archive in progress refuses here, before the cap slot and before
    // the card: the create re-checks the same mark under the archiving
    // guard's own lock, but a refusal the daemon already knows about must
    // never spend the person's consent. The read side is dropped on this
    // line — holding it across the card would block an archive for as long
    // as the person takes to answer.
    let archiving = state
        .sessions
        .workspace_creation_guard(Some(&workspace))
        .map_err(|error| TerminalError::Refused(error.message))?;
    drop(archiving);
    // The directory the shell would open in, resolved the way the create
    // resolves it: a workspace that cannot answer refuses the call here,
    // before the card is spent on a directory nobody can name.
    let cwd = state
        .sessions
        .workspace_cwd(&workspace)
        .map(|path| crate::workspace::plain_path(&path.to_string_lossy()))
        .map_err(|error| TerminalError::Refused(error.message))?;
    // The cap slot is taken before the card, so two creates racing at the
    // cap cannot both read room during the seconds a person spends looking
    // at the card; the reservation gives it back on every exit below.
    let slot = state
        .sessions
        .reserve_terminal_slot(&registration.session_id, &registration.owner, conn_peer)
        .map_err(|error| TerminalError::Refused(error.message))?;
    if !state.session_started() {
        return Err(TerminalError::Refused(SHUTTING_DOWN.to_string()));
    }
    // The slot is held by a guard, not by hand: the card, the spawn and
    // every lock between here and the answer can panic, and a panic must not
    // leak what `session_started` took. It is disarmed the moment the
    // terminal exists — from there its own close gives the slot back.
    let mut live_slot = LiveSessionSlot::taken(state);
    let created = (|| {
        let (subject, facts) = create_card_facts(&workspace, &cwd, name.as_deref());
        write_after_card(
            state,
            broker,
            registration,
            TERMINAL_CREATE_GROUP,
            &subject,
            &facts,
        )?;
        state
            .sessions
            .create_terminal_for(
                state,
                &registration.owner,
                Some(workspace.clone()),
                name.clone(),
                conn_peer,
                &registration.session_id,
            )
            .map_err(|error| TerminalError::Refused(error.message))
    })();
    // `?` releases the slot through the guard above on the way out.
    let session = created?;
    live_slot.disarm();
    if let Some(key) = retry_key.as_deref() {
        crate::server::remember_creation_session(
            state,
            &registration.owner,
            key,
            &fingerprint,
            &session,
        );
    }
    if let Some(hold) = hold.as_mut() {
        hold.commit();
    }
    drop(slot);
    Ok(terminal_document(&session))
}

fn send_keys_to_terminal(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    conn_peer: &Option<ConnPeer>,
    request: &KeysRequest,
) -> Result<Value, TerminalError> {
    if state.is_shutting_down() {
        return Err(TerminalError::Refused(SHUTTING_DOWN.to_string()));
    }
    // The payload is capped before the card, so an oversized call never
    // spends the person's consent: the wire's own sentence, on the bytes as
    // they arrived (`MAX_WRITE_BYTES`). The loopback door refuses an
    // envelope this size before the tool runs; the check is the tool's own
    // rule for whatever carries the call next.
    if request.keys.len() > devboule_protocol::MAX_WRITE_BYTES {
        return Err(TerminalError::Refused(
            "Session input is too large.".to_string(),
        ));
    }
    let workspace = caller_workspace(state, registration, conn_peer)?;
    // The target resolves through the gate *before* the card, so what the
    // person approves is the terminal the write will really reach.
    let target = state
        .sessions
        .terminal_target(
            &request.terminal,
            &registration.owner,
            conn_peer,
            &workspace,
        )
        .map_err(|error| TerminalError::Refused(error.message))?;
    let subject = format!("send keys to terminal '{}'", target.title);
    let opener = opener_label(target.created_by.as_deref(), &registration.session_id);
    let mut facts = target_facts(&target, &workspace, opener);
    facts.push(("keys", keys_preview(&request.keys, request.literal)));
    write_after_card(
        state,
        broker,
        registration,
        TERMINAL_KEYS_GROUP,
        &subject,
        &facts,
    )?;
    // The gate runs again inside the write itself: the card waited for a
    // person, and nothing that waited may vouch for what happens after.
    let bytes = resolve_keys(&request.keys, request.literal);
    state
        .sessions
        .terminal_send_bytes(
            &request.terminal,
            &registration.owner,
            conn_peer,
            &workspace,
            bytes.as_bytes(),
        )
        .map_err(|error| TerminalError::Refused(error.message))?;
    Ok(json!({"success": true}))
}

fn kill_one_terminal(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    conn_peer: &Option<ConnPeer>,
    terminal: &str,
) -> Result<Value, TerminalError> {
    if state.is_shutting_down() {
        return Err(TerminalError::Refused(SHUTTING_DOWN.to_string()));
    }
    let workspace = caller_workspace(state, registration, conn_peer)?;
    let target = state
        .sessions
        .terminal_target(terminal, &registration.owner, conn_peer, &workspace)
        .map_err(|error| TerminalError::Refused(error.message))?;
    let subject = format!("close terminal '{}'", target.title);
    let opener = opener_label(target.created_by.as_deref(), &registration.session_id);
    let facts = target_facts(&target, &workspace, opener);
    write_after_card(
        state,
        broker,
        registration,
        TERMINAL_KILL_GROUP,
        &subject,
        &facts,
    )?;
    let removed = state
        .sessions
        .kill_terminal(terminal, &registration.owner, conn_peer, &workspace)
        .map_err(|error| TerminalError::Refused(error.message))?;
    if !removed {
        // The gate held a moment ago and the close then removed nothing:
        // something else ended the terminal in between, and a kill that
        // killed nothing must not answer success.
        return Err(TerminalError::Refused(
            "No session with that id.".to_string(),
        ));
    }
    // Every terminal create took a live slot (`session_started`), so a
    // close that removed one gives it back — the wire's `SessionClose`
    // does the same.
    state.session_finished();
    Ok(json!({"success": true}))
}

fn terminal_document(session: &devboule_protocol::Session) -> Value {
    json!({
        "terminalId": session.id,
        "title": session.title,
        "cwd": session.cwd,
    })
}

/// The live-session slot one create took (`session_started`): released on
/// every way out of that create — a refusal, a denied card, a failed spawn,
/// a panic — and disarmed as soon as the terminal it was taken for exists,
/// because from then its own close is what gives the slot back.
struct LiveSessionSlot<'a> {
    state: &'a Arc<ServerState>,
    armed: bool,
}

impl<'a> LiveSessionSlot<'a> {
    /// The slot `session_started` just registered, armed for release.
    fn taken(state: &'a Arc<ServerState>) -> Self {
        Self { state, armed: true }
    }

    /// The terminal exists and owns the slot now; this guard must not give
    /// it back when it drops.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for LiveSessionSlot<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.state.session_finished();
        }
    }
}
