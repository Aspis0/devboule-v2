//! `browser_fill_login`: the tool an agent logs in with, and the consent that
//! stands in front of it.
//!
//! One responsibility: ask the host what a person may choose from, put that
//! choice to the person, and have this machine type the login they chose.
//!
//! The daemon never holds a credential and never asks for one. It carries two
//! opaque things: the entry id the person picked, and the site it was picked
//! for. Those two are the whole of the audit row — not a username, not a
//! password, and not a page's own words, which is why a host refusal is split
//! in two on the way out: the sentence goes to the agent alone and the row
//! keeps the code.
//!
//! The consent is its own group per entry and per site, held in the write
//! gate's own mark table, so a session allowed one saved login on one site is
//! still asked about another login or another site. It is raised through
//! `first_use::ask_choice` rather than through the write gate itself, and that
//! is the one deliberate difference between the two: an automatic mode lets a
//! write pass with no card, and a saved-login fill raises its card whatever the
//! session's mode is.

use std::sync::Arc;

use serde_json::{json, Value};

use devboule_protocol::{
    BrowserCaller, BrowserError, PermissionOption, SessionEvent, SessionOrigin,
};

use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::tools::browser_args::{self, Call};
use crate::mcp_broker::tools::browser_commands;
use crate::mcp_broker::tools::browser_tools;
use crate::mcp_broker::tools::first_use::{ask_choice, card_id, GateMark};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::provider_catalog::MCP_BROWSER_FILL_LOGIN_TOOL;
use crate::server::ServerState;

/// What this host is asked first: which site the field is on, and which saved
/// logins that site allows. It is a host command with no tool of its own — an
/// agent cannot call it, and its answer is a list of names, not a credential.
/// The name is public inside the broker so the test host registers it, exactly
/// as the app's own host does.
pub(in crate::mcp_broker) const PREVIEW: &str = "fill_login_preview";
/// The typing itself, with the entry the person chose named in the arguments.
const FILL: &str = "fill_login";

/// The choice that approves only the call it was raised for.
const ONCE: &str = "once";
/// The choice that approves the rest of the session, for this entry on this
/// site and nothing else. Its kind is the journal's own session word, so the
/// ledger tells it from a one-shot.
const SESSION: &str = "session";
const SESSION_KIND: &str = "allow_session";
const DENY: &str = "deny";

/// One login a site allows, as the preview answers it.
#[derive(Clone)]
struct Entry {
    id: String,
    label: String,
}

/// A refusal on its way out, split in two: what the agent reads, and what the
/// owner's row keeps of it.
struct Refusal {
    /// A code and a sentence, or this tool's own sentence. Goes to the agent.
    said: String,
    /// A short word. Goes to the audit row, which never carries a sentence:
    /// a host message can be text the page wrote.
    outcome: String,
}

impl Refusal {
    fn of(sentence: impl Into<String>, outcome: &str) -> Self {
        Refusal {
            said: sentence.into(),
            outcome: outcome.to_owned(),
        }
    }
}

impl From<BrowserError> for Refusal {
    fn from(error: BrowserError) -> Self {
        Refusal {
            said: format!("{}: {}", error.code.as_str(), error.message),
            outcome: error.code.as_str().to_owned(),
        }
    }
}

/// Answer one `tools/call` for `browser_fill_login`.
pub(in crate::mcp_broker) fn call(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let Some(spec) = browser_commands::spec_for(MCP_BROWSER_FILL_LOGIN_TOOL) else {
        return Ok(Some(rpc_error(id, -32601, "Unknown tool")));
    };
    let audit = |outcome: &str| {
        audit_mcp_tool(
            state,
            &caller,
            MCP_BROWSER_FILL_LOGIN_TOOL,
            &registration.session_id,
            outcome,
        );
    };
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    let requested = match browser_args::parse(spec, MCP_BROWSER_FILL_LOGIN_TOOL, &arguments) {
        Ok(requested) if names_a_field(&requested.args) => requested,
        Ok(_) => {
            audit("invalid");
            return Ok(Some(rpc_error(
                id,
                -32602,
                "browser_fill_login: give at least one of usernameRef, passwordRef.",
            )));
        }
        Err(sentence) => {
            audit("invalid");
            return Ok(Some(rpc_error(id, -32602, &sentence)));
        }
    };
    let context = match browser_tools::browser_caller(state, registration, &caller) {
        Ok(context) => context,
        // The ownership door refused this caller its own row, so the daemon
        // cannot say which workspace its tabs belong to.
        Err(sentence) => {
            audit("denied");
            return Ok(Some(tool_error(&id, &sentence)));
        }
    };
    let (site, entries) = match preview(state, &context, &requested) {
        Ok(answer) => answer,
        Err(refusal) => {
            audit(&refusal.outcome);
            return Ok(Some(tool_error(&id, &refusal.said)));
        }
    };
    let chosen = match choose(state, broker, registration, &site, &entries) {
        Ok(chosen) => chosen,
        Err(refusal) => {
            audit(&refusal.outcome);
            return Ok(Some(tool_error(&id, &refusal.said)));
        }
    };
    match typed(state, &context, &requested, &chosen) {
        Ok(result) => {
            audit(&format!("filled {} on {site}", chosen.id));
            Ok(Some(browser_tools::browser_reply(&id, FILL, result)))
        }
        Err(refusal) => {
            audit(&format!(
                "failed {} on {} {}",
                chosen.id, site, refusal.outcome
            ));
            Ok(Some(tool_error(&id, &refusal.said)))
        }
    }
}

/// The call names a field to fill. Both refs are optional in the schema,
/// because a two-step login page takes one of them at a time; neither is
/// optional to the call, which has nothing to do without one.
fn names_a_field(args: &Value) -> bool {
    args.get("usernameRef").is_some() || args.get("passwordRef").is_some()
}

/// What this machine may offer on the site the field is on.
fn preview(
    state: &ServerState,
    context: &BrowserCaller,
    requested: &Call,
) -> Result<(String, Vec<Entry>), Refusal> {
    let answered = state
        .browser
        .execute(
            context,
            PREVIEW,
            requested.args.clone(),
            requested.browser_id.as_deref(),
            requested.timeout,
        )
        .map_err(Refusal::from)?;
    let no_login = || {
        Refusal::of(
            "no_saved_login: this page's own site has no saved login.",
            "no_saved_login",
        )
    };
    let site = answered
        .get("origin")
        .and_then(Value::as_str)
        .ok_or_else(no_login)?
        .to_owned();
    let entries = answered
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(Entry {
                        id: entry.get("id")?.as_str()?.to_owned(),
                        label: entry.get("label")?.as_str()?.to_owned(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if entries.is_empty() {
        return Err(Refusal::of(
            format!("no_saved_login: this machine has no saved login for {site}."),
            "no_saved_login",
        ));
    }
    Ok((site, entries))
}

/// Have this machine type the login the person chose. The arguments travel
/// with the entry id added, which the tool's closed argument set means no
/// agent could have put there.
fn typed(
    state: &ServerState,
    context: &BrowserCaller,
    requested: &Call,
    chosen: &Entry,
) -> Result<Value, Refusal> {
    let mut args = requested.args.clone();
    if let Some(fields) = args.as_object_mut() {
        fields.insert("entryId".to_owned(), json!(chosen.id));
    }
    state
        .browser
        .execute(
            context,
            FILL,
            args,
            requested.browser_id.as_deref(),
            requested.timeout,
        )
        .map_err(Refusal::from)
}

/// The group one grant of this card belongs to: that entry, on that site, for
/// that session. Nothing else opens it, which is the whole of "a session grant
/// is scoped to entry and origin".
fn group(entry: &str, site: &str) -> String {
    format!("saved_login:{entry}@{site}")
}

/// Ask the person, unless this session already holds the grant for one of the
/// entries on this site.
fn choose(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    site: &str,
    entries: &[Entry],
) -> Result<Entry, Refusal> {
    let session_id = registration.session_id.as_str();
    for entry in entries {
        match broker.gate_mark(session_id, &group(&entry.id, site)) {
            GateMark::Open => return Ok(entry.clone()),
            GateMark::Pending => return Err(Refusal::of("permission pending; retry", "denied")),
            GateMark::None => {}
        }
    }
    // Every entry is marked pending before the card goes up, so a second call
    // racing this one waits for the answer instead of spending the person's
    // attention a second time on the same question.
    for entry in entries {
        broker.remember_gate_mark(session_id, &group(&entry.id, site), GateMark::Pending);
    }
    let answered = ask_choice(
        state,
        session_id,
        &registration.owner,
        saved_login_card(registration, site, entries),
    );
    let granted = match answered {
        Ok(option) => match Granted::parse(&option) {
            Ok(granted) => granted,
            Err(()) => {
                clear_marks(broker, session_id, site, entries);
                return Err(Refusal::of("permission refused", "denied"));
            }
        },
        Err(said) => {
            clear_marks(broker, session_id, site, entries);
            return Err(Refusal::of(said, "denied"));
        }
    };
    let Some(chosen) = entries.iter().find(|entry| entry.id == granted.entry) else {
        clear_marks(broker, session_id, site, entries);
        return Err(Refusal::of("permission refused", "denied"));
    };
    // The grant belongs to what was chosen, not to the card: an answer of
    // `once` opens nothing, and `session` opens this entry on this site for
    // the rest of this session and nothing else.
    clear_marks(broker, session_id, site, entries);
    if granted.for_session {
        broker.remember_gate_mark(session_id, &group(&chosen.id, site), GateMark::Open);
    }
    Ok(chosen.clone())
}

/// Every entry of this card back to no mark: a card that was refused, that no
/// one answered, or whose answer named nothing is not a grant, and the next
/// call asks again.
fn clear_marks(broker: &McpBroker, session_id: &str, site: &str, entries: &[Entry]) {
    for entry in entries {
        broker.remember_gate_mark(session_id, &group(&entry.id, site), GateMark::None);
    }
}

/// One card answer: which entry it named, and whether it was the session kind.
struct Granted {
    entry: String,
    for_session: bool,
}

impl Granted {
    /// An option id is `kind:entryId`. Anything else is a card this door did
    /// not build, and is refused rather than acted on: a grant nobody asked
    /// for is not a grant.
    fn parse(option: &str) -> Result<Self, ()> {
        match option.split_once(':') {
            Some((ONCE, entry)) => Ok(Self {
                entry: entry.to_owned(),
                for_session: false,
            }),
            Some((SESSION, entry)) => Ok(Self {
                entry: entry.to_owned(),
                for_session: true,
            }),
            _ => Err(()),
        }
    }
}

/// The card: which saved login, on which site, asked by an agent. No password
/// and no username is on it — this machine holds those and the card names
/// neither.
fn saved_login_card(
    registration: &RegisteredSession,
    site: &str,
    entries: &[Entry],
) -> SessionEvent {
    let alone = entries.len() == 1;
    let title = match entries.first() {
        Some(entry) if alone => {
            format!("Use saved login '{label}' on {site}?", label = entry.label)
        }
        _ => format!("Use a saved login on {site}?"),
    };
    let listed = entries
        .iter()
        .map(|entry| format!("| {}", entry.label))
        .collect::<Vec<_>>()
        .join("\n");
    let what = match entries.first() {
        Some(entry) if alone => format!("The saved login is '{label}'.", label = entry.label),
        _ => format!("{} saved logins are saved for this site.", entries.len()),
    };
    SessionEvent::PermissionRequest {
        tool_call_id: card_id(
            registration.session_id.as_str(),
            &format!("saved_login@{site}"),
        ),
        title,
        description: Some(format!(
            "An agent asked to sign in to {site} with a login saved on this machine.\n{listed}\n\n\
             {what} The password is typed into the page by this app and is never shown to the \
             agent. \"For this session\" means this login on this site only."
        )),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: card_options(entries, site),
        // Forced: the two allow options differ in kind, which the journal
        // tells apart, so without it the app would show its own generic pair
        // and the session choice would be unclickable.
        is_chooser: Some(true),
        kind: None,
        plan: None,
        questions: None,
        origin: SessionOrigin::unknown(),
        create_agent: None,
    }
}

/// Every entry's two ways to be allowed, and the one refusal they all share.
fn card_options(entries: &[Entry], site: &str) -> Vec<PermissionOption> {
    let mut options: Vec<PermissionOption> = entries
        .iter()
        .flat_map(|entry| {
            [
                PermissionOption {
                    option_id: format!("{ONCE}:{}", entry.id),
                    name: format!("Use '{}' for this call", entry.label),
                    kind: "allow_once".to_owned(),
                },
                PermissionOption {
                    option_id: format!("{SESSION}:{}", entry.id),
                    name: format!(
                        "Use '{}' on {site} for the rest of this session",
                        entry.label
                    ),
                    kind: SESSION_KIND.to_owned(),
                },
            ]
        })
        .collect();
    options.push(PermissionOption {
        option_id: DENY.to_owned(),
        name: "Deny".to_owned(),
        kind: "reject_once".to_owned(),
    });
    options
}

#[cfg(test)]
#[path = "mcp_browser_login_tests.rs"]
mod tests;
