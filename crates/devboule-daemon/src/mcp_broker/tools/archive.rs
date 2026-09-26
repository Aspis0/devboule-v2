//! The `devboule_archive_workspace` tool body.

use std::sync::Arc;

use devboule_protocol::{OwnerId, WorkspaceIsolation};
use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::tools::first_use::{ensure_write_allowed, ARCHIVE_WORKSPACES_GROUP};
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
        Err(WorkspaceError::Invalid(message)) => return Ok(Some(rpc_error(id, -32602, &message))),
        Err(WorkspaceError::Refused(message)) => return Ok(Some(tool_error(&id, &message))),
    };
    let result = archive_workspace(
        state,
        broker,
        &registration.session_id,
        &registration.owner,
        &workspace_id,
    );
    audit_mcp_tool(
        state,
        &caller,
        crate::provider_catalog::MCP_ARCHIVE_WORKSPACE_TOOL,
        &registration.session_id,
        if result.is_ok() { "ok" } else { "denied" },
    );
    match result {
        Ok(document) => {
            let text = serde_json::to_string(&document).map_err(|error| {
                json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode workspace archive: {error}")}})
            })?;
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": document,
                    "isError": false,
                },
            })))
        }
        Err(WorkspaceError::Invalid(message)) => Ok(Some(rpc_error(id, -32602, &message))),
        Err(WorkspaceError::Refused(message)) => Ok(Some(tool_error(&id, &message))),
    }
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
            Some(Value::String(value)) if !value.trim().is_empty() => Ok(Self {
                workspace_id: value.trim().to_string(),
            }),
            Some(Value::String(_)) => Err(WorkspaceError::Invalid(
                "workspaceId is required".to_string(),
            )),
            _ => Err(WorkspaceError::Invalid(
                "workspaceId must be a string".to_string(),
            )),
        }
    }
}

fn archive_workspace(
    state: &ServerState,
    broker: &McpBroker,
    session_id: &str,
    owner: &OwnerId,
    workspace_id: &str,
) -> Result<Value, WorkspaceError> {
    let (callers_workspace_id, project_id) = state
        .sessions
        .caller_workspace_scope(session_id, owner)
        .map_err(|error| Refused(error.message))?;
    let records = state
        .sessions
        .workspace_records(&project_id)
        .map_err(|error| Refused(error.message))?;
    let target = records
        .iter()
        .find(|record| record.id == workspace_id)
        .ok_or_else(|| Refused("Workspace not found in the caller's project.".to_string()))?;
    if target.id == callers_workspace_id {
        return Err(Refused(
            "The caller's own workspace cannot be archived.".to_string(),
        ));
    }
    if target.isolation == WorkspaceIsolation::Local {
        return match state.sessions.workspace_delete(workspace_id, false) {
            Err(error) => Err(Refused(error.message)),
            Ok(()) => Err(Refused(
                "The local workspace is the project folder and is not removed as a worktree."
                    .to_string(),
            )),
        };
    }

    let subject = format!("archive workspace '{}'", target.title);
    let path = crate::workspace::plain_path(&target.path);
    let facts = [("workspace", target.id.as_str()), ("path", path.as_str())];
    let fact_refs = facts
        .iter()
        .map(|(key, value)| (*key, *value))
        .collect::<Vec<_>>();
    ensure_write_allowed(
        state,
        broker,
        session_id,
        owner,
        ARCHIVE_WORKSPACES_GROUP,
        &subject,
        &fact_refs,
    )
    .map_err(Refused)?;

    state
        .sessions
        .workspace_delete(workspace_id, false)
        .map_err(|error| Refused(error.message))?;
    Ok(json!({"workspaceId": workspace_id, "removed": true}))
}

#[cfg(test)]
#[path = "mcp_workspace_archive_tests.rs"]
mod tests;
