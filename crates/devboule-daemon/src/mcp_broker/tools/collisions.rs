//! `devboule_file_collisions`: who else is touching this path in the
//! caller's own repository, before it is edited rather than at rebase time.
//!
//! Two questions, two kinds of evidence, never mixed. `worktrees` is what
//! git itself says about the repository's other checkouts; `writers` is what
//! this daemon performed itself and can therefore vouch for. A shell command
//! is not in either list (see [`crate::write_evidence`]), so an empty
//! `writers` never means nobody touched the file.
//!
//! Identity is the bearer's, never an argument: the repository is the calling
//! session's own workspace and the path is confined inside it, so no argument
//! can aim this at another project. Read-only, so no card.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::file_collisions::{self, KnownCheckout};
use crate::mcp_broker::dispatch::rpc_error;
use crate::mcp_broker::RegisteredSession;
use crate::server::ServerState;
use crate::workspace_git_support::git_within;

/// The default window a writer list is read over, in minutes.
const DEFAULT_LOOKBACK_MINUTES: u16 = 60;

/// One validated call: the path asked about, and the window its writers are
/// read over.
#[derive(Debug, PartialEq, Eq)]
struct CollisionRequest {
    path: String,
    lookback_minutes: u16,
}

impl CollisionRequest {
    /// The strict parser every broker tool has: the known-parameter list is
    /// read out of the published schema, so the document an agent reads and
    /// the check this runs cannot disagree.
    fn parse(arguments: &Value) -> Result<Self, String> {
        let empty = json!({});
        let object = match arguments {
            Value::Null => &empty,
            Value::Object(_) => arguments,
            _ => return Err("arguments must be an object".to_string()),
        };
        let object = object
            .as_object()
            .ok_or_else(|| "arguments must be an object".to_string())?;
        let known = crate::provider_catalog::file_collisions_input_schema()["properties"]
            .as_object()
            .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for key in object.keys() {
            if !known.iter().any(|name| name == key) {
                return Err(format!("unknown parameter '{key}'"));
            }
        }
        let path = object
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "path is required".to_string())?;
        let lookback_minutes = match object.get("lookbackMinutes") {
            None | Some(Value::Null) => DEFAULT_LOOKBACK_MINUTES,
            Some(Value::Number(minutes)) => {
                let minutes = minutes.as_u64().filter(|minutes| {
                    (1..=crate::write_evidence::MAX_LOOKBACK.as_secs() / 60).contains(minutes)
                });
                minutes.ok_or_else(|| {
                    format!(
                        "lookbackMinutes must be an integer 1..{}",
                        crate::write_evidence::MAX_LOOKBACK.as_secs() / 60
                    )
                })? as u16
            }
            Some(_) => return Err("lookbackMinutes must be an integer".to_string()),
        };
        Ok(Self {
            path,
            lookback_minutes,
        })
    }
}

/// Why a collision call did not answer: the design's envelope carries the code
/// beside the sentence, so a caller can tell "you asked about a path I do not
/// serve" from "the repository was too slow to read".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CollisionError {
    NotFound,
    InvalidArgs,
    StaleState,
}

impl CollisionError {
    fn code(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::InvalidArgs => "invalid_args",
            Self::StaleState => "stale_state",
        }
    }

    /// Only a read that could not be taken now is worth repeating; a refused
    /// path and a workspace that is gone are not.
    fn retryable(self) -> bool {
        matches!(self, Self::StaleState)
    }
}

pub(in crate::mcp_broker) fn collisions(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    let request = match CollisionRequest::parse(&arguments) {
        Ok(request) => request,
        Err(reason) => return Ok(Some(rpc_error(id, -32602, &reason))),
    };
    Ok(Some(match report(state, registration, &request) {
        Ok(data) => envelope(state, id, &data),
        Err((error, message)) => refused(state, id, error, &message),
    }))
}

/// The answer, or the refusal that names which of the two facts is missing.
fn report(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    request: &CollisionRequest,
) -> Result<Value, (CollisionError, String)> {
    let (workspace_id, project_id) = state
        .sessions
        .caller_workspace_scope(&registration.session_id, &registration.owner)
        .map_err(|error| (CollisionError::NotFound, error.message))?;
    let root = state
        .sessions
        .workspace_cwd(&workspace_id)
        .map_err(|error| (CollisionError::NotFound, error.message))?;
    let subject = file_collisions::confine_subject(&root, &request.path)
        .map_err(|sentence| (CollisionError::InvalidArgs, sentence.to_string()))?;
    let known = state
        .sessions
        .workspace_records(&project_id)
        .map_err(|error| (CollisionError::StaleState, error.message))?
        .iter()
        .map(|record| KnownCheckout {
            path: PathBuf::from(&record.path),
            workspace_id: record.id.clone(),
        })
        .collect::<Vec<_>>();
    // The sweep runs on this repository's own git queue, so it is ordered
    // against the workspace git arms and holds a read permit like any other
    // read. It moves everything it borrows: the queue answers later, on the
    // drain thread.
    let sweep_root = root.clone();
    let sweep_subject = subject.clone();
    let scan = state
        .read_git_value(&root, move || {
            file_collisions::scan(&sweep_root, &sweep_subject, &known, &|root, arguments| {
                git_within(root, arguments, file_collisions::COMMAND_TIMEOUT)
            })
        })
        .map_err(|reason| (CollisionError::StaleState, reason.to_string()))?
        .map_err(|reason| (CollisionError::StaleState, reason))?;
    let labels = agent_labels(state, &registration.owner)?;
    let writers = crate::write_evidence::writers_for(
        &subject,
        Duration::from_secs(u64::from(request.lookback_minutes) * 60),
    )
    .into_iter()
    .map(|write| {
        json!({
            "sessionId": write.session_id,
            "agent": labels.get(&write.session_id).cloned(),
            "lastWriteAt": write.at_ms,
            "evidence": write.evidence.as_str(),
            "confidence": write.evidence.confidence(),
        })
    })
    .collect::<Vec<_>>();
    Ok(json!({
        "repoPath": crate::verbatim_path::plain_path(&root.to_string_lossy()),
        "worktrees": scan.worktrees.iter().map(worktree_document).collect::<Vec<_>>(),
        "writers": writers,
        "capped": scan.capped,
    }))
}

/// The display name each session an agent may see is shown under. Read from
/// the live roster of the caller's own owner, so a writer that is not live
/// any more answers `null` rather than a name this call has no business
/// inventing — and the session id beside it is the identity anyway.
fn agent_labels(
    state: &Arc<ServerState>,
    owner: &devboule_protocol::OwnerId,
) -> Result<HashMap<String, String>, (CollisionError, String)> {
    let labels = state
        .sessions
        .live_agent_entries(owner)
        .map_err(|error| (CollisionError::StaleState, error.message))?
        .into_iter()
        .map(|entry| {
            let name = entry
                .session
                .display_name
                .clone()
                .unwrap_or_else(|| entry.session.title.clone());
            (entry.session.id, name)
        })
        .collect();
    Ok(labels)
}

fn worktree_document(worktree: &file_collisions::OtherWorktree) -> Value {
    json!({
        "workspaceId": worktree.workspace_id,
        "branch": worktree.branch,
        "baseSha": worktree.base_sha,
        "committedChange": worktree.committed_change,
        "dirtyChange": worktree.dirty_change,
    })
}

/// The success envelope: the document under `data`, and the same document as
/// the text every other broker tool answers with, so a client that reads only
/// `content` sees the same facts.
fn envelope(state: &Arc<ServerState>, id: Value, data: &Value) -> Value {
    let document = json!({"hostId": state.host_id(), "ok": true, "data": data});
    let text = serde_json::to_string(&document)
        .unwrap_or_else(|error| format!("could not encode the collision report: {error}"));
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": text}],
            "structuredContent": document,
            "isError": false,
        }
    })
}

/// A refusal, in the envelope's own shape: the sentence an agent reads and
/// the code a caller may branch on. `isError` is set, like every other broker
/// refusal, so a client that only looks at that still sees the failure.
fn refused(state: &Arc<ServerState>, id: Value, error: CollisionError, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": message}],
            "structuredContent": {
                "hostId": state.host_id(),
                "ok": false,
                "error": {
                    "code": error.code(),
                    "message": message,
                    "retryable": error.retryable(),
                },
            },
            "isError": true,
        }
    })
}

#[cfg(test)]
#[path = "mcp_collisions_tests.rs"]
mod tests;
