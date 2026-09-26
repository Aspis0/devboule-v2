//! The terminal writes: open a shell in the caller's own workspace, type
//! into one, kill one.
//!
//! One responsibility: the arguments, the consent and the reply of those
//! three writes. The gate a write cannot dodge lives in the registry
//! (`reachable_terminal`, `session_terminals.rs`) — this file decides which
//! terminal of the caller's own workspace the act names, asks the person the
//! first time, and audits the outcome. Typed bytes never reach a log, a
//! diagnostic, the card or the audit row: the card counts them and the audit
//! records the act.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use devboule_protocol::{validate_display_name, Session};

use crate::mcp_broker::caller::{audit_mcp_tool, caller_conn, McpCaller};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::peer_policy::ConnPeer;
use crate::server::ServerState;

use super::first_use::{ensure_write_allowed, TERMINALS_GROUP};
use super::terminal_common::{caller_workspace, terminal_reply, TerminalError};

/// Paseo's key tokens: the names one key press is spelled with and the bytes
/// each stands for (`paseo-tools.ts`, `resolveTerminalKeyToken`).
const KEY_TOKENS: [(&str, &str); 11] = [
    ("Enter", "\r"),
    ("Tab", "\t"),
    ("Escape", "\u{1b}"),
    ("Space", " "),
    ("BSpace", "\u{7f}"),
    ("C-c", "\u{3}"),
    ("C-d", "\u{4}"),
    ("C-z", "\u{1a}"),
    ("C-l", "\u{c}"),
    ("C-a", "\u{1}"),
    ("C-e", "\u{5}"),
];

/// `devboule_create_terminal` (`{name?}`): a terminal in the caller's own
/// workspace, behind the terminal-write card.
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
/// live session goes, behind the same card; the journal row and the
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
    // it: a retry answers the terminal the first call opened and creates
    // nothing, so neither the cap nor the card below has anything to judge.
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
        if let Some(existing) = crate::server::idempotent_creation_session(
            state,
            &registration.owner,
            key,
            &fingerprint,
        ) {
            if let Some(hold) = hold.as_mut() {
                hold.commit();
            }
            return Ok(terminal_document(&existing));
        }
    }
    // The cap is judged before the card: a refusal must never spend the
    // person's consent on a call the daemon had already decided to refuse.
    state
        .sessions
        .check_terminal_cap(&registration.session_id, &registration.owner, conn_peer)
        .map_err(|error| TerminalError::Refused(error.message))?;
    // The shutdown guard the wire's create road takes: false means the
    // daemon is going down, and this call has registered no live slot.
    if !state.session_started() {
        return Err(TerminalError::Refused(
            "daemon is shutting down".to_string(),
        ));
    }
    // One release point for everything the slot above was taken for: the
    // card may refuse, the spawn may fail, and either way the slot goes back
    // with the terminal that never came to be.
    let created = (|| {
        let (subject, facts) = create_card_facts(state, &workspace, name.as_deref());
        write_after_card(state, broker, registration, &subject, &facts)?;
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
    let session = match created {
        Ok(session) => session,
        Err(error) => {
            state.session_finished();
            // The hold releases itself on the way out, as every refusal
            // between it and the answer does.
            return Err(error);
        }
    };
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
    Ok(terminal_document(&session))
}

fn send_keys_to_terminal(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    conn_peer: &Option<ConnPeer>,
    request: &KeysRequest,
) -> Result<Value, TerminalError> {
    // The payload is capped before the card, so an oversized call never
    // spends the person's consent: the wire's own refusal, on the bytes as
    // they arrived (`MAX_WRITE_BYTES`, 64 KiB).
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
    let subject = format!("sending keys to terminal '{}'", target.title);
    let mut facts = target_facts(&target);
    facts.push(("keys", keys_preview(&request.keys, request.literal)));
    write_after_card(state, broker, registration, &subject, &facts)?;
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
    let workspace = caller_workspace(state, registration, conn_peer)?;
    let target = state
        .sessions
        .terminal_target(terminal, &registration.owner, conn_peer, &workspace)
        .map_err(|error| TerminalError::Refused(error.message))?;
    let subject = format!("killing terminal '{}'", target.title);
    let facts = target_facts(&target);
    write_after_card(state, broker, registration, &subject, &facts)?;
    let removed = state
        .sessions
        .kill_terminal(terminal, &registration.owner, conn_peer, &workspace)
        .map_err(|error| TerminalError::Refused(error.message))?;
    if removed {
        // Every terminal create took a live slot (`session_started`), so a
        // close that removed one gives it back — the wire's `SessionClose`
        // does the same.
        state.session_finished();
    }
    Ok(json!({"success": true}))
}

/// The consent step the three writes share: the first terminal write of a
/// session raises one card on the caller's own session, a later one passes
/// on the group's mark, and a refusal is the call's answer.
fn write_after_card(
    state: &ServerState,
    broker: &McpBroker,
    registration: &RegisteredSession,
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
        TERMINALS_GROUP,
        subject,
        &fact_refs,
    )
    .map_err(TerminalError::Refused)
}

/// The create card's subject and facts: the workspace the shell opens in
/// and the directory that workspace resolves to, plus the name when one was
/// given. The directory is the create's own preview — the create re-resolves
/// it — so a lookup that fails reads as a plain fallback and never as a
/// second refusal.
fn create_card_facts(
    state: &ServerState,
    workspace: &str,
    name: Option<&str>,
) -> (String, Vec<(&'static str, String)>) {
    let cwd = state
        .sessions
        .workspace_cwd(workspace)
        .map(|path| crate::workspace::plain_path(&path.to_string_lossy()))
        .unwrap_or_else(|_| "(the workspace directory)".to_string());
    let mut facts = vec![("workspace", workspace.to_string()), ("cwd", cwd)];
    if let Some(name) = name {
        facts.push(("name", name.to_string()));
    }
    ("creating a terminal".to_string(), facts)
}

/// The facts both target cards carry: which terminal, by title and by the
/// directory it runs in.
fn target_facts(target: &Session) -> Vec<(&'static str, String)> {
    let mut facts = vec![("terminal", target.title.clone())];
    if let Some(cwd) = target.cwd.as_deref() {
        facts.push(("cwd", cwd.to_string()));
    }
    facts
}

/// What the keys card says about `keys`: a named key by name, anything else
/// by length alone. The characters themselves never leave the caller — they
/// may be a password, and the card is rendered, journaled, and (for a peer's
/// session) sent to that peer.
fn keys_preview(keys: &str, literal: bool) -> String {
    if !literal {
        if let Some(token) = KEY_TOKENS.iter().find(|token| token.0 == keys) {
            return format!("key {}", token.0);
        }
    }
    format!("{} characters, not shown", keys.chars().count())
}

/// The bytes one `keys` payload stands for: literal text as typed, else the
/// named key it names. An unknown name is the text it is — Paseo's resolver
/// falls through its switch the same way — so "echo hi" types itself either
/// way and only a real token is translated.
fn resolve_keys(keys: &str, literal: bool) -> String {
    if literal {
        return keys.to_string();
    }
    KEY_TOKENS
        .iter()
        .find(|token| token.0 == keys)
        .map(|token| token.1.to_string())
        .unwrap_or_else(|| keys.to_string())
}

fn terminal_document(session: &Session) -> Value {
    json!({
        "terminalId": session.id,
        "title": session.title,
        "cwd": session.cwd,
    })
}

/// The closed create shape: `name` optional, and nothing else — the
/// workspace is the caller's own row, so no argument can name one. An empty
/// name is Paseo's absent name (trimmed and dropped), a name the wire
/// refuses is refused here with the wire's own sentence, and the value the
/// daemon stores is the value this call judged.
fn parse_name(arguments: &Value) -> Result<Option<String>, String> {
    let object = argument_map(arguments)?;
    for key in object.keys() {
        if key != "name" {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    match object.get("name") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            validate_display_name(trimmed).map(Some)
        }
        Some(_) => Err("name must be a string".to_string()),
    }
}

/// What one `send_terminal_keys` call carries: Paseo's three fields, closed.
fn parse_keys(arguments: &Value) -> Result<KeysRequest, String> {
    let object = argument_map(arguments)?;
    for key in object.keys() {
        if !matches!(key.as_str(), "terminalId" | "keys" | "literal") {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    let terminal = object
        .get("terminalId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "terminalId is required".to_string())?
        .to_string();
    let keys = object
        .get("keys")
        .and_then(Value::as_str)
        .ok_or_else(|| "keys is required".to_string())?
        .to_string();
    let literal = match object.get("literal") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err("literal must be a boolean".to_string()),
    };
    Ok(KeysRequest {
        terminal,
        keys,
        literal,
    })
}

/// The closed kill shape: one terminal, named by the id the roster answered.
fn parse_terminal(arguments: &Value) -> Result<String, String> {
    let object = argument_map(arguments)?;
    for key in object.keys() {
        if key != "terminalId" {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    object
        .get("terminalId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "terminalId is required".to_string())
}

/// The argument object of a `tools/call`: an absent `arguments` is the empty
/// document, anything that is not an object is a malformed request.
fn argument_map(arguments: &Value) -> Result<Map<String, Value>, String> {
    match arguments {
        Value::Null => Ok(Map::new()),
        Value::Object(object) => Ok(object.clone()),
        _ => Err("arguments must be an object".to_string()),
    }
}

struct KeysRequest {
    terminal: String,
    keys: String,
    literal: bool,
}
