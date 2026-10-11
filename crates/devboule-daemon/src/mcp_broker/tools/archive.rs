//! The `devboule_archive_workspace` tool body.

use std::sync::Arc;

use devboule_protocol::{OwnerId, WorkspaceIsolation};
use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::tools::first_use::{ensure_write_allowed, WORKSPACE_ARCHIVE_GROUP};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::server::ServerState;

use super::archive_text::quote_title;
use super::workspaces::{WorkspaceError, WorkspaceError::Refused};

pub(in crate::mcp_broker) fn archive(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    caller: McpCaller,
    registration: &RegisteredSession,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    let workspace_id = match ArchiveRequest::parse(&arguments) {
        Ok(request) => request.workspace_id,
        Err(WorkspaceError::Invalid(message)) => {
            audit_mcp_tool(
                state,
                &caller,
                crate::provider_catalog::MCP_ARCHIVE_WORKSPACE_TOOL,
                &registration.session_id,
                "invalid",
            );
            return Ok(Some(rpc_error(id, -32602, &message)));
        }
        Err(WorkspaceError::Refused(message)) => {
            audit_mcp_tool(
                state,
                &caller,
                crate::provider_catalog::MCP_ARCHIVE_WORKSPACE_TOOL,
                &registration.session_id,
                "denied",
            );
            return Ok(Some(tool_error(&id, &message)));
        }
    };
    let result = archive_workspace_audited(
        state,
        broker,
        &registration.session_id,
        &registration.owner,
        &workspace_id,
    );
    match result {
        Ok(document) => {
            let text = match serde_json::to_string(&document) {
                Ok(text) => text,
                Err(error) => {
                    audit_archive(state, &caller, &registration.session_id, "failed");
                    return Ok(Some(
                        json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode workspace archive: {error}")}}),
                    ));
                }
            };
            let reply = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": document,
                    "isError": false,
                },
            });
            audit_archive(state, &caller, &registration.session_id, "ok");
            Ok(Some(reply))
        }
        Err(error) => {
            audit_archive(
                state,
                &caller,
                &registration.session_id,
                archive_audit_outcome(&error),
            );
            match error {
                WorkspaceError::Invalid(message) => Ok(Some(rpc_error(id, -32602, &message))),
                WorkspaceError::Refused(message) => Ok(Some(tool_error(&id, &message))),
            }
        }
    }
}

fn archive_audit_outcome(error: &WorkspaceError) -> &'static str {
    match error {
        WorkspaceError::Invalid(_) => "invalid",
        WorkspaceError::Refused(message) if message == "permission refused" => "human_denied",
        WorkspaceError::Refused(message) if message == "permission card could not be delivered" => {
            "delivery_failed"
        }
        // A session starting in the workspace is a retryable refusal, not
        // anybody saying no.
        WorkspaceError::Refused(message) if message == crate::session::SESSION_STARTING_MESSAGE => {
            "session_starting"
        }
        WorkspaceError::Refused(_) => "denied",
    }
}

fn audit_archive(state: &ServerState, caller: &McpCaller, session_id: &str, outcome: &str) {
    audit_mcp_tool(
        state,
        caller,
        crate::provider_catalog::MCP_ARCHIVE_WORKSPACE_TOOL,
        session_id,
        outcome,
    );
}

struct ArchiveRequest {
    workspace_id: String,
}

impl ArchiveRequest {
    fn parse(arguments: &Value) -> Result<Self, WorkspaceError> {
        let object = arguments
            .as_object()
            .ok_or_else(|| WorkspaceError::Invalid("arguments must be an object".to_string()))?;
        for key in object.keys() {
            if key != "workspaceId" {
                return Err(WorkspaceError::Invalid(format!(
                    "unknown parameter '{key}'"
                )));
            }
        }
        match object.get("workspaceId") {
            None => Err(WorkspaceError::Invalid(
                "workspaceId is required".to_string(),
            )),
            Some(Value::String(value)) if !value.trim().is_empty() => Ok(Self {
                workspace_id: value.clone(),
            }),
            Some(Value::String(_)) => Err(WorkspaceError::Invalid(
                "workspaceId is required".to_string(),
            )),
            Some(_) => Err(WorkspaceError::Invalid(
                "workspaceId must be a string".to_string(),
            )),
        }
    }
}

#[cfg(test)]
fn archive_workspace(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    session_id: &str,
    owner: &OwnerId,
    workspace_id: &str,
) -> Result<Value, WorkspaceError> {
    archive_workspace_audited(state, broker, session_id, owner, workspace_id)
}

fn archive_workspace_audited(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    session_id: &str,
    owner: &OwnerId,
    workspace_id: &str,
) -> Result<Value, WorkspaceError> {
    let target = validate_archive_scope(state, session_id, owner, workspace_id)?;
    // A workspace with a session running is refused, as the sidebar refuses it:
    // nothing is closed on the agent's say-so.
    refuse_while_sessions_run(state, workspace_id)?;
    let subject = format!("archive workspace {}", quote_title(&target.title));
    let path = crate::verbatim_path::plain_path(&target.path);
    let facts = [("workspace", target.id.as_str()), ("path", path.as_str())];
    ensure_write_allowed(
        state,
        broker,
        session_id,
        owner,
        WORKSPACE_ARCHIVE_GROUP,
        &subject,
        &facts,
    )
    .map_err(Refused)?;

    validate_archive_scope(state, session_id, owner, workspace_id)?;
    let _archiving = state
        .sessions
        .mark_workspace_archiving(workspace_id)
        .map_err(|error| Refused(error.message))?;
    // A session that started while the approval was pending is refused the same way.
    refuse_while_sessions_run(state, workspace_id)?;
    validate_archive_scope(state, session_id, owner, workspace_id)?;
    // An archive takes the row off the list and keeps the folder: nothing is
    // deleted here, whatever the isolation. Deleting a worktree stays a
    // separate act the person takes.
    state
        .sessions
        .workspace_archive(workspace_id)
        .map_err(|error| Refused(error.message))?;
    Ok(json!({
        "workspaceId": workspace_id,
        "removed": true,
    }))
}

fn refuse_while_sessions_run(
    state: &ServerState,
    workspace_id: &str,
) -> Result<(), WorkspaceError> {
    if live_sessions(state, workspace_id)?.is_empty() {
        return Ok(());
    }
    Err(Refused(crate::session::sessions_still_running().message))
}

fn validate_archive_scope(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
    workspace_id: &str,
) -> Result<crate::journal::WorkspaceRecord, WorkspaceError> {
    let (own_workspace_id, project_id) = state
        .sessions
        .caller_workspace_scope(session_id, owner)
        .map_err(|error| Refused(error.message))?;
    let records = state
        .sessions
        .workspace_records(&project_id)
        .map_err(|error| Refused(error.message))?;
    let target = records
        .into_iter()
        .find(|record| record.id == workspace_id)
        .ok_or_else(|| Refused("Workspace not found in the caller's project.".to_string()))?;
    if target.id == own_workspace_id {
        return Err(Refused(
            "The caller's own workspace cannot be archived.".to_string(),
        ));
    }
    // The project's own folder is not a worktree: the agent does not take it off
    // the list. The person does, in the app.
    if target.isolation == WorkspaceIsolation::Local {
        return Err(Refused(
            "The local workspace is the project folder and is not removed as a worktree."
                .to_string(),
        ));
    }
    Ok(target)
}

fn live_sessions(
    state: &ServerState,
    workspace_id: &str,
) -> Result<Vec<(String, OwnerId, String)>, WorkspaceError> {
    let mut sessions = state
        .sessions
        .live_sessions_in_workspace(workspace_id)
        .map_err(|error| Refused(error.message))?;
    sessions.sort_by_key(|session| session.0.clone());
    Ok(sessions)
}

#[cfg(test)]
#[path = "mcp_workspace_archive_tests.rs"]
mod tests;
