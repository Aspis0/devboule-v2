//! The result one creation answers with, tools state included.

use serde_json::{json, Value};

use crate::mcp_broker::{compute_tools_state, ToolsState};

/// The result of one creation, whether it just happened or is being re-answered
/// (`S5` §2; `create-from-profile`):
/// `{sessionId, taskId, contextId, displayName, state: "submitted"}`.
///
/// `taskId` is the session id: a Devboule child *is* the task, and a caller that
/// had to keep a map of task to session would be keeping a private copy of a
/// fact the daemon already has. `contextId` is the child's context, which is the
/// creator's context — so a creator and everything it commissions, at any depth,
/// name one family without any bookkeeping of their own. The fallback is the rule
/// `Session::context_id` states (a session with no creator is its own context),
/// applied for a client that reads a frame from a daemon older than this field.
///
/// `notify` is the creator's `notifyOnFinish`: when it is set, the answer says
/// that the child's finish wakes the creator, so the creator ends its turn
/// instead of polling for the child. `note` is one sentence the creator needs to
/// read, such as a mode the child did not start in.
pub(in crate::mcp_broker) fn created_result(
    id: &Value,
    session: &devboule_protocol::Session,
    registered: bool,
    notify: bool,
    note: Option<&str>,
) -> Value {
    // S2 honesty, S8 fact: the result reports verification, the card promised
    // it. `registered` is the broker row — a fresh registered child is not yet
    // verified (its first proof lands after this answer); an unregistered one
    // has no tools at all. Hosted is renderable via `created_result_for_tools`
    // (pinned by test) and arrives on live paths when verification flips the
    // runtime. Routed through the S1 single computation point.
    let tools = compute_tools_state(&session.kind, registered, false);
    let wake = match (notify, note) {
        (true, Some(note)) => Some(format!(
            "{} {note}",
            crate::provider_catalog::AGENT_WAKE_SENTENCE
        )),
        (true, None) => Some(crate::provider_catalog::AGENT_WAKE_SENTENCE.to_string()),
        (false, note) => note.map(str::to_string),
    };
    created_result_for_tools(id, session, tools, wake.as_deref())
}

/// The result body for one explicit tools state (S2 test hook): the `tools`
/// word rides `structuredContent`, and `unavailable`/`unverified` add the one
/// model-readable sentence. `hosted` adds none — the tools themselves are the
/// proof. The forbidden state is a result claiming `hosted` for a session with
/// no bearer; the test pins it by calling this with `Hosted` for a kind that
/// `created_result` would never produce.
pub(in crate::mcp_broker) fn created_result_for_tools(
    id: &Value,
    session: &devboule_protocol::Session,
    tools: ToolsState,
    wake: Option<&str>,
) -> Value {
    let tools_sentence = match tools {
        ToolsState::Hosted => "",
        ToolsState::Unavailable => {
            " This session starts without Devboule tools: it cannot create, message or list agents."
        }
        ToolsState::Unverified => {
            " This session's tools are unverified: they will be verified at start."
        }
    };
    let display_name = session
        .display_name
        .clone()
        .unwrap_or_else(|| session.title.clone());
    let context_id = session
        .context_id
        .clone()
        .unwrap_or_else(|| session.id.clone());
    let wake_sentence = wake.map_or_else(String::new, |sentence| format!(" {sentence}"));
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": format!("submitted {}{}{}", session.id, tools_sentence, wake_sentence)}],
            "structuredContent": {
                "sessionId": session.id,
                "taskId": session.id,
                "contextId": context_id,
                "displayName": display_name,
                "state": devboule_protocol::AgentTaskState::Submitted.as_str(),
                "tools": tools.as_str(),
            },
            "isError": false,
        },
    })
}
