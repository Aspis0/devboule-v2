//! `devboule_file_collisions`: who else is touching this path in the
//! caller's own repository, before it is edited rather than at rebase time.
//!
//! Two questions, two kinds of evidence, never mixed. `worktrees` is what
//! git itself says about the repository's other checkouts; `writers` is what
//! this daemon performed itself and can therefore vouch for. A shell command
//! is in neither list (see [`crate::write_evidence`]), so an empty `writers`
//! never means nobody touched the file.
//!
//! Identity is the bearer's, never an argument: the repository is the calling
//! session's own workspace and the path is confined inside it, so no argument
//! can aim this at another project. Read-only, so no card — and behind the
//! peer door, which judges it like the workspace inventory (`peer_policy`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::file_collisions::{self, KnownCheckout, ScanFailure};
use crate::mcp_broker::dispatch::rpc_error;
use crate::mcp_broker::RegisteredSession;
use crate::server::ServerState;
use crate::workspace_git_support::git_within;

/// The default window a writer list is read over, in minutes.
const DEFAULT_LOOKBACK_MINUTES: u16 = 60;

/// The codes the envelope carries beside the sentence, so a caller can tell a
/// path it may not ask about from a repository that would not answer.
const NOT_FOUND: &str = "not_found";
const INVALID_ARGS: &str = "invalid_args";
const STALE_STATE: &str = "stale_state";
const WORKTREE_UNREADABLE: &str = "worktree_unreadable";

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

/// Why a collision call did not answer: the code a caller may branch on, and
/// whether the same call a moment later could.
struct Refusal {
    code: &'static str,
    message: String,
    retryable: bool,
}

fn refusal(code: &'static str, message: impl Into<String>, retryable: bool) -> Refusal {
    Refusal {
        code,
        message: message.into(),
        retryable,
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
        Err(refused) => refusal_reply(state, id, &refused),
    }))
}

/// The answer, or the refusal that names which of the facts is missing.
fn report(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    request: &CollisionRequest,
) -> Result<Value, Refusal> {
    let (workspace_id, project_id) = state
        .sessions
        .caller_workspace_scope(&registration.session_id, &registration.owner)
        .map_err(|error| refusal(NOT_FOUND, error.message, false))?;
    let root = state
        .sessions
        .workspace_cwd(&workspace_id)
        .map_err(|error| refusal(NOT_FOUND, error.message, false))?;
    let subject = file_collisions::confine_subject(&root, &request.path)
        .map_err(|sentence| refusal(INVALID_ARGS, sentence, false))?;
    let known = state
        .sessions
        .workspace_records(&project_id)
        .map_err(|error| refusal(STALE_STATE, error.message, true))?
        .iter()
        .map(|record| KnownCheckout {
            path: PathBuf::from(&record.path),
            workspace_id: record.id.clone(),
        })
        .collect::<Vec<_>>();
    // One instant for both bounds: the sweep stops itself there, and the queue
    // call waits for it plus the one command that may still be running. The
    // caller therefore never walks away from a sweep that is still working.
    let deadline = Instant::now() + file_collisions::SCAN_DEADLINE;
    let sweep_root = root.clone();
    let sweep_subject = subject.clone();
    let scan = state
        .read_git_value(
            &root,
            file_collisions::SCAN_DEADLINE + file_collisions::COMMAND_TIMEOUT,
            move || {
                file_collisions::scan(
                    &sweep_root,
                    &sweep_subject,
                    &known,
                    deadline,
                    &|root, args| git_within(root, args, file_collisions::COMMAND_TIMEOUT),
                )
            },
        )
        .map_err(|reason| refusal(STALE_STATE, reason, true))?
        .map_err(unreadable)?;
    // Writers are the caller's own repository only: a session that wrote the
    // same relative path in another project is not touching this file.
    let listed = crate::write_evidence::repo_key(&root)
        .as_deref()
        .map(|repo| {
            crate::write_evidence::writers_for(
                repo,
                &subject,
                Duration::from_secs(u64::from(request.lookback_minutes) * 60),
            )
        })
        .unwrap_or_default();
    let labels = agent_labels(state, &registration.owner)?;
    let writers = listed
        .writers
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
        "writersMayBeIncomplete": listed.may_be_incomplete,
    }))
}

/// A sweep that could not read one checkout: its own code, because no retry
/// helps a repository in the middle of a rebase. The sweep says which of the
/// two it was — a command that failed, or a git that timed out.
fn unreadable(failure: ScanFailure) -> Refusal {
    refusal(WORKTREE_UNREADABLE, failure.message, failure.retryable)
}

/// The display name each session an agent may see is shown under. Read from
/// the live roster of the caller's own owner, so a writer that is not live
/// any more answers `null` rather than a name this call has no business
/// inventing — and the session id beside it is the identity anyway.
fn agent_labels(
    state: &Arc<ServerState>,
    owner: &devboule_protocol::OwnerId,
) -> Result<HashMap<String, String>, Refusal> {
    let labels = state
        .sessions
        .live_agent_entries(owner)
        .map_err(|error| refusal(STALE_STATE, error.message, true))?
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
fn refusal_reply(state: &Arc<ServerState>, id: Value, refused: &Refusal) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": refused.message}],
            "structuredContent": {
                "hostId": state.host_id(),
                "ok": false,
                "error": {
                    "code": refused.code,
                    "message": refused.message,
                    "retryable": refused.retryable,
                },
            },
            "isError": true,
        }
    })
}

/// The tool at the peer door, through the real road.
#[cfg(test)]
#[path = "mcp_collisions_peer_tests.rs"]
mod peer_tests;
#[cfg(test)]
#[path = "mcp_collisions_tests.rs"]
mod tests;
