//! The `devboule_archive_workspace` tool body.

use std::sync::Arc;

use devboule_protocol::{OwnerId, WorkspaceIsolation};
use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::tools::first_use::{ensure_write_allowed, WORKSPACE_ARCHIVE_GROUP};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::server::ServerState;

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
        Some(&caller),
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
    archive_workspace_audited(state, broker, None, session_id, owner, workspace_id)
}

fn archive_workspace_audited(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    caller: Option<&McpCaller>,
    session_id: &str,
    owner: &OwnerId,
    workspace_id: &str,
) -> Result<Value, WorkspaceError> {
    let target = validate_archive_scope(state, session_id, owner, workspace_id, false)?;
    let sessions = live_sessions(state, workspace_id)?;
    let subject = format!("archive workspace {}", quote_title(&target.title));
    let path = crate::workspace::plain_path(&target.path);
    let session_facts = describe_sessions(&sessions);
    let facts = [
        ("workspace", target.id.as_str()),
        ("path", path.as_str()),
        ("sessions to close", session_facts.as_str()),
    ];
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

    validate_archive_scope(state, session_id, owner, workspace_id, true)?;
    let current_sessions = live_sessions(state, workspace_id)?;
    if session_ids(&sessions) != session_ids(&current_sessions) {
        return Err(Refused(
            "Live sessions changed while archive approval was pending; retry.".to_string(),
        ));
    }
    let closed_session_ids = close_sessions(state, caller, current_sessions)?;

    validate_archive_scope(state, session_id, owner, workspace_id, true)?;
    state
        .sessions
        .workspace_delete(workspace_id, false)
        .map_err(|error| Refused(error.message))?;
    Ok(json!({
        "workspaceId": workspace_id,
        "removed": true,
        "closedSessionIds": closed_session_ids,
    }))
}

fn validate_archive_scope(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
    workspace_id: &str,
    check_dirty: bool,
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
    if target.isolation == WorkspaceIsolation::Local {
        return Err(Refused(
            "The local workspace is the project folder and is not removed as a worktree."
                .to_string(),
        ));
    }
    if check_dirty {
        let path = std::path::Path::new(&target.path);
        match crate::worktree::checkout_has_dirty_files(path) {
            Ok(true) => {
                return Err(Refused(crate::worktree::worktree_dirty_remove_message(
                    path,
                )));
            }
            Ok(false) => {}
            Err(error) => return Err(Refused(format!("Could not check worktree status: {error}"))),
        }
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

fn close_sessions(
    state: &Arc<ServerState>,
    caller: Option<&McpCaller>,
    sessions: Vec<(String, OwnerId, String)>,
) -> Result<Vec<String>, WorkspaceError> {
    let mut closed_session_ids = Vec::new();
    for (session_id, owner, _) in sessions {
        match state.sessions.close(&session_id, &owner, &None) {
            Ok(true) => {
                state.session_finished();
                closed_session_ids.push(session_id.clone());
                if let Some(caller) = caller {
                    audit_mcp_tool(
                        state,
                        caller,
                        "SessionClose",
                        &session_id,
                        "workspace_archive",
                    );
                }
            }
            Ok(false) => {}
            Err(error) => {
                return Err(Refused(format!(
                    "Could not close session {session_id}: {}",
                    error.message
                )));
            }
        }
    }
    Ok(closed_session_ids)
}

fn session_ids(sessions: &[(String, OwnerId, String)]) -> Vec<String> {
    sessions.iter().map(|(id, _, _)| id.clone()).collect()
}

fn describe_sessions(sessions: &[(String, OwnerId, String)]) -> String {
    let names = sessions
        .iter()
        .take(3)
        .map(|(_, _, title)| quote_title(title))
        .collect::<Vec<_>>();
    let remaining = sessions.len().saturating_sub(names.len());
    let mut description = format!("{} session(s): {}", sessions.len(), names.join(", "));
    if remaining > 0 {
        description.push_str(&format!(", and {remaining} more"));
    }
    description
}

fn quote_title(title: &str) -> String {
    let title = title
        .chars()
        .take(120)
        .map(|char| {
            if char.is_control() || matches!(char, '\\' | '"' | '\'') {
                '_'
            } else {
                char
            }
        })
        .collect::<String>();
    format!("\"{title}\"")
}

#[cfg(test)]
#[path = "mcp_workspace_archive_tests.rs"]
mod tests;
