//! The `devboule_list_workspaces` and `devboule_create_workspace` tool bodies.
//!
//! One responsibility: workspaces of the caller's own project. The project
//! always comes from the caller's session row, never from an argument; the
//! project folder or a sibling worktree of it are the only checkouts the
//! daemon mints, so no argument names a path.

use std::sync::Arc;

use serde_json::{json, Value};

use devboule_protocol::{OwnerId, WorkspaceIsolation};

use crate::journal::WorkspaceRecord;
use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::server::ServerState;

use super::first_use::{ensure_write_allowed, WORKSPACES_GROUP};

/// Why a workspaces call did not answer.
///
/// The two arms are the two wire shapes the broker answers with: a malformed
/// request is `rpc_error(-32602)`, a call that cannot be scoped or performed
/// is a tool result with `isError: true`.
#[derive(Debug)]
pub(crate) enum WorkspaceError {
    Invalid(String),
    Refused(String),
}

/// The arguments of a workspaces call, absent when the request carries none.
fn workspace_arguments(message: &Value) -> Value {
    message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null)
}

/// One reply shape for both tools: their own document on success, `-32602`
/// for a malformed request, and a tool error (`isError: true`) carrying the
/// sentence that says why the answer cannot be produced.
fn workspace_reply(
    id: &Value,
    result: Result<Value, WorkspaceError>,
) -> Result<Option<Value>, Value> {
    match result {
        Ok(document) => {
            let text = serde_json::to_string(&document).map_err(|error| {
                json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode workspaces: {error}")}})
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
        Err(WorkspaceError::Invalid(message)) => Ok(Some(rpc_error(id.clone(), -32602, &message))),
        Err(WorkspaceError::Refused(message)) => Ok(Some(tool_error(id, &message))),
    }
}

pub(in crate::mcp_broker) fn list(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: Value,
) -> Result<Option<Value>, Value> {
    // The caller's own project decides the listing; the bearer is the
    // identity. Deliberately no arguments are read, like the roster tool:
    // `arguments` is optional in tools/call, and a parameterless tool's
    // schema promises there is nothing to send.
    workspace_reply(
        &id,
        list_workspaces(state, &registration.session_id, &registration.owner),
    )
}

pub(in crate::mcp_broker) fn create(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    caller: McpCaller,
    registration: &RegisteredSession,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    // Identity is the bearer, never the arguments: the project the workspace
    // lands in is the caller's own, read from its session row.
    let arguments = workspace_arguments(message);
    let request = match CreateRequest::parse(&arguments) {
        Ok(request) => request,
        Err(WorkspaceError::Invalid(message)) => {
            return Ok(Some(rpc_error(id, -32602, &message)));
        }
        Err(refused) => return workspace_reply(&id, Err(refused)),
    };
    let result = create_workspace(
        state,
        broker,
        &registration.session_id,
        &registration.owner,
        &request,
    );
    audit_mcp_tool(
        state,
        &caller,
        crate::provider_catalog::MCP_CREATE_WORKSPACE_TOOL,
        &registration.session_id,
        if result.is_ok() { "ok" } else { "denied" },
    );
    workspace_reply(&id, result)
}

fn list_workspaces(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
) -> Result<Value, WorkspaceError> {
    let (_, project_id) = caller_scope(state, session_id, owner)?;
    let records = state
        .sessions
        .workspace_records(&project_id)
        .map_err(|error| refused(error.message))?;
    Ok(json!({
        "workspaces": records.iter().map(workspace_document).collect::<Vec<_>>(),
    }))
}

/// The caller's workspace and its project, from the session row. A session
/// with no workspace names no project, so the call is refused instead of
/// reading whatever project happens to exist.
fn caller_scope(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
) -> Result<(String, String), WorkspaceError> {
    state
        .sessions
        .caller_workspace_scope(session_id, owner)
        .map_err(|error| refused(error.message))
}

/// One workspace as the tools answer it. `kind` is the checkout vocabulary,
/// not the daemon's isolation words: a local workspace IS the project folder.
/// `branch` rides along because the wire `Workspace` drops it.
fn workspace_document(record: &WorkspaceRecord) -> Value {
    json!({
        "id": record.id,
        "projectId": record.project_id,
        "name": record.title,
        "path": crate::workspace::plain_path(&record.path),
        "kind": match record.isolation {
            WorkspaceIsolation::Local => "checkout",
            WorkspaceIsolation::Worktree => "worktree",
        },
        "branch": record.branch,
    })
}

/// `devboule_create_workspace` takes Paseo's shape with our stricter rule:
/// the project is the caller's own, and no path is accepted at all.
struct CreateRequest {
    isolation: WorkspaceIsolation,
    name: Option<String>,
    branch: Option<String>,
    path: Option<String>,
    project_id: Option<String>,
}

impl CreateRequest {
    fn parse(arguments: &Value) -> Result<Self, WorkspaceError> {
        let object = arguments
            .as_object()
            .ok_or_else(|| invalid("arguments must be an object"))?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "isolation" | "name" | "branch" | "path" | "projectId"
            ) {
                return Err(invalid(format!("unknown parameter '{key}'")));
            }
        }
        let isolation = match object.get("isolation") {
            Some(Value::String(value)) if value == "local" => WorkspaceIsolation::Local,
            Some(Value::String(value)) if value == "worktree" => WorkspaceIsolation::Worktree,
            _ => {
                return Err(invalid(
                    "isolation is required: \"local\" or \"worktree\"".to_string(),
                ));
            }
        };
        Ok(Self {
            isolation,
            name: optional_name(object.get("name"), "name")?,
            branch: optional_name(object.get("branch"), "branch")?,
            path: optional_raw(object.get("path"), "path")?,
            project_id: optional_name(object.get("projectId"), "projectId")?,
        })
    }
}

fn create_workspace(
    state: &ServerState,
    broker: &McpBroker,
    session_id: &str,
    owner: &OwnerId,
    request: &CreateRequest,
) -> Result<Value, WorkspaceError> {
    if request.path.is_some() {
        return Err(invalid(
            "path is not accepted: workspaces are created inside your project, \
             never at an agent-named directory",
        ));
    }
    let (_, project_id) = caller_scope(state, session_id, owner)?;
    if let Some(requested) = request.project_id.as_deref() {
        if requested != project_id {
            return Err(refused("Project not found."));
        }
    }
    if request.isolation == WorkspaceIsolation::Local && request.branch.is_some() {
        return Err(invalid("Local workspaces do not take a branch."));
    }
    if request.isolation == WorkspaceIsolation::Local {
        let records = state
            .sessions
            .workspace_records(&project_id)
            .map_err(|error| refused(error.message))?;
        if records
            .iter()
            .any(|record| record.isolation == WorkspaceIsolation::Local)
        {
            return Err(refused("This project already has a local workspace."));
        }
    }
    let (subject, facts) = create_card_facts(state, &project_id, request);
    let fact_refs = facts
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    ensure_write_allowed(
        state,
        broker,
        session_id,
        owner,
        WORKSPACES_GROUP,
        &subject,
        &fact_refs,
    )
    .map_err(refused)?;
    let workspace = state
        .sessions
        .workspace_create_titled(
            &project_id,
            request.isolation,
            request.branch.clone(),
            request.name.as_deref(),
        )
        .map_err(|error| refused(error.message))?;
    let records = state
        .sessions
        .workspace_records(&project_id)
        .map_err(|error| refused(error.message))?;
    records
        .iter()
        .find(|record| record.id == workspace.id)
        .map(workspace_document)
        .ok_or_else(|| refused("The workspace was created but could not be read back."))
}

/// What the gate card shows for one create: the project, the isolation, the
/// branch, the name and the checkout the call is about to make. Paths are
/// previews, never promises — a worktree without a branch gets its branch
/// and its sibling checkout at creation — and every lookup failure reads as
/// a plain fallback, never as a second refusal: the create itself re-checks
/// everything it needs.
fn create_card_facts(
    state: &ServerState,
    project_id: &str,
    request: &CreateRequest,
) -> (String, Vec<(&'static str, String)>) {
    let isolation = match request.isolation {
        WorkspaceIsolation::Local => "local",
        WorkspaceIsolation::Worktree => "worktree",
    };
    let subject = match (&request.name, &request.branch) {
        (Some(name), Some(branch)) => {
            format!("creating {isolation} workspace '{name}' on branch '{branch}'")
        }
        (Some(name), None) => format!("creating {isolation} workspace '{name}'"),
        (None, Some(branch)) => format!("creating {isolation} workspace on branch '{branch}'"),
        (None, None) => format!("creating {isolation} workspace"),
    };
    let path = preview_checkout_path(state, project_id, request)
        .unwrap_or_else(|| "(decided at creation)".to_string());
    let facts = vec![
        ("project", project_id.to_string()),
        ("isolation", isolation.to_string()),
        (
            "branch",
            request.branch.as_deref().unwrap_or("default").to_string(),
        ),
        (
            "name",
            request.name.as_deref().unwrap_or("default").to_string(),
        ),
        ("path", path),
    ];
    (subject, facts)
}

/// The checkout a create will make, when it is already determined: the
/// project folder for local, the sibling checkout for a worktree with an
/// explicit branch.
fn preview_checkout_path(
    state: &ServerState,
    project_id: &str,
    request: &CreateRequest,
) -> Option<String> {
    let project_path = state.sessions.project_path(project_id).ok()?;
    match request.isolation {
        WorkspaceIsolation::Local => Some(crate::workspace::plain_path(
            &project_path.to_string_lossy(),
        )),
        WorkspaceIsolation::Worktree => request.branch.as_deref().and_then(|branch| {
            crate::worktree::checkout_path_for_branch(&project_path, branch)
                .map(|path| crate::workspace::plain_path(&path.to_string_lossy()))
        }),
    }
}

/// One optional trimmed string: absent and null are missing, anything else
/// must be a non-empty string.
fn optional_name(value: Option<&Value>, field: &str) -> Result<Option<String>, WorkspaceError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() || trimmed.len() > MAX_NAME_BYTES {
                return Err(invalid(format!("{field} is required")));
            }
            Ok(Some(trimmed.to_string()))
        }
        Some(_) => Err(invalid(format!("{field} must be a string"))),
    }
}

/// One optional untrimmed string: the path is refused whatever it says, but
/// the parser still checks the shape before the body refuses the fact.
fn optional_raw(value: Option<&Value>, field: &str) -> Result<Option<String>, WorkspaceError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid(format!("{field} must be a string"))),
    }
}

/// The longest name a tool accepts, in bytes.
const MAX_NAME_BYTES: usize = 1024;

fn refused(message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::Refused(message.into())
}

fn invalid(message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::Invalid(message.into())
}

#[cfg(test)]
#[path = "mcp_workspaces_tests.rs"]
mod tests;
