//! `devboule_ci_watch`: watch one pushed commit's CI and get woken with a
//! verdict, instead of polling `gh` from the agent.
//!
//! The arguments are a closed set: exactly one of `sha` and `branch`, an
//! optional `repo`, and `retryInfra` for the one infra retry. The repository
//! is the one the caller's own workspace's `origin` names unless `repo` says
//! otherwise; the daemon's own `gh` login does the asking. A malformed call is
//! a protocol error; everything the tool can refuse for a reason an owner can
//! act on (no `gh`, not logged in, an unknown commit or branch) is a tool
//! error in the `{hostId, ok, data?, error?}` envelope.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::ci_gh::{is_branch_name, is_commit_id, parse_repo_argument, CiError, RepoRef};
use crate::ci_watch::short;
use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::tools::first_use::{ensure_write_approved, Approval, CI_RETRY_GROUP};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::server::ServerState;

/// What the call watches: a commit id, or a branch the call resolves to its
/// head now.
enum Target {
    Sha(String),
    Branch(String),
}

struct Arguments {
    target: Target,
    repo: Option<(String, String)>,
    retry_infra: bool,
}

/// Why a call started no watch: a refusal the owner can act on, or the
/// permission gate's own sentence.
enum Refusal {
    Ci(CiError),
    Card(String),
}

/// What the call accepts, and nothing else: exactly one of a full 40-hex
/// commit id and a branch name, an optional `owner/repo`, and the one infra
/// retry as a boolean.
fn parse_arguments(arguments: &Value) -> Result<Arguments, String> {
    let object = match arguments {
        Value::Object(map) => map,
        Value::Null => return Err("sha or branch is required".to_string()),
        _ => return Err("arguments must be an object".to_string()),
    };
    for key in object.keys() {
        if !matches!(key.as_str(), "sha" | "branch" | "repo" | "retryInfra") {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    let sha = match object.get("sha") {
        None | Some(Value::Null) => None,
        Some(Value::String(sha)) if is_commit_id(sha) => Some(sha.to_ascii_lowercase()),
        Some(_) => return Err("sha must be a full 40-character commit id".to_string()),
    };
    let branch = match object.get("branch") {
        None | Some(Value::Null) => None,
        Some(Value::String(branch)) if is_branch_name(branch) => Some(branch.clone()),
        Some(_) => return Err("branch must be a branch name".to_string()),
    };
    let target = match (sha, branch) {
        (Some(sha), None) => Target::Sha(sha),
        (None, Some(branch)) => Target::Branch(branch),
        (Some(_), Some(_)) => return Err("pass exactly one of sha and branch".to_string()),
        (None, None) => return Err("sha or branch is required".to_string()),
    };
    let repo =
        match object.get("repo") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(parse_repo_argument(text).ok_or(
                "repo must be owner/repo (the host always comes from the workspace origin)",
            )?),
            Some(_) => return Err("repo must be a string".to_string()),
        };
    let retry_infra = match object.get("retryInfra") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err("retryInfra must be true or false".to_string()),
    };
    Ok(Arguments {
        target,
        repo,
        retry_infra,
    })
}

pub(in crate::mcp_broker) fn call(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let tool = crate::provider_catalog::MCP_CI_WATCH_TOOL;
    let audit = |outcome: &str| {
        audit_mcp_tool(state, &caller, tool, &registration.session_id, outcome);
    };
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    let arguments = match parse_arguments(&arguments) {
        Ok(arguments) => arguments,
        Err(sentence) => {
            audit("invalid");
            return Ok(Some(rpc_error(id, -32602, &sentence)));
        }
    };
    let host_id = host_id(state);
    match watch(state, broker, registration, arguments) {
        Ok(watched) => {
            // Who approved the retry card is part of the row: an automatic
            // mode's approval and a person's are never the same log line.
            audit(outcome_text(watched.approval));
            Ok(Some(reply(&id, &host_id, Ok(watched.data))))
        }
        Err(Refusal::Ci(error)) => {
            audit("failed");
            Ok(Some(reply(&id, &host_id, Err(error))))
        }
        Err(Refusal::Card(sentence)) => {
            audit("denied");
            Ok(Some(tool_error(&id, &sentence)))
        }
    }
}

/// What a started watch answers: the tool's data, and who approved the retry
/// card when one was raised.
struct Watched {
    data: Value,
    approval: Option<Approval>,
}

/// The audit row of a watch that started: nothing to name when no retry was
/// asked for, and the approver named whenever one was.
fn outcome_text(approval: Option<Approval>) -> &'static str {
    match approval {
        None => "ok",
        Some(Approval::Mode) => "ok; approved by automatic mode",
        Some(Approval::Person) => "ok; approved by person",
    }
}

fn watch(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    arguments: Arguments,
) -> Result<Watched, Refusal> {
    let watches = &state.ci_watches;
    // The host always comes from the workspace origin — never the argument —
    // so resolving needs the workspace even when the repo names owner/repo.
    let root = state
        .sessions
        .session_workspace_root(&registration.session_id, &registration.owner)
        .map_err(|error| Refusal::Ci(CiError::new("not_found", error.message, false)))?;
    let repo = watches
        .gh_tool()
        .resolve_repo(root.as_deref(), arguments.repo)
        .map_err(Refusal::Ci)?;
    // Branch mode resolves the remote head now and watches that commit: the
    // call is where the head is read, and the poll thread is what notices a
    // head that moves afterwards.
    let (sha, branch) = match arguments.target {
        Target::Sha(sha) => (sha, None),
        Target::Branch(branch) => {
            let sha = watches
                .gh_tool()
                .head_sha(&repo, &branch)
                .map_err(Refusal::Ci)?;
            (sha, Some(branch))
        }
    };
    let approval = if arguments.retry_infra {
        Some(approve_retry(
            state,
            broker,
            registration,
            &repo,
            branch.as_deref(),
            &sha,
        )?)
    } else {
        None
    };
    let record = watches
        .start(
            &registration.session_id,
            &registration.owner,
            &repo,
            &sha,
            branch.as_deref(),
            arguments.retry_infra,
        )
        .map_err(Refusal::Ci)?;
    let wake = watches.wake_status(&record, &state.sessions);
    let mut data = json!({
        "watchId": record.watch_id,
        "resolvedSha": record.sha,
        "state": record.state.as_str(),
        "repo": record.slug(),
        "retryCount": record.retry_count,
        "retryIssued": record.retry_issued,
        "wake": wake.as_str(),
    });
    if let Some(branch) = &record.branch {
        data["branch"] = json!(branch);
    }
    Ok(Watched { data, approval })
}

/// The card a retryInfra call raises before anything is watched: re-running
/// failed jobs is a state-changing GitHub action, so it follows the session's
/// mode like every other Devboule card, and a refusal starts no watch. Without
/// `retryInfra` this is never called and there is never a retry.
fn approve_retry(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    repo: &RepoRef,
    branch: Option<&str>,
    sha: &str,
) -> Result<Approval, Refusal> {
    let slug = repo.slug();
    let target = match branch {
        Some(branch) => format!("branch {branch} at {}", short(sha)),
        None => format!("commit {}", short(sha)),
    };
    let subject = format!("re-run failed CI jobs once for {slug}");
    let facts: [(&str, &str); 3] = [
        ("repository", &slug),
        ("watch", &target),
        (
            "retry",
            "re-run the failed jobs once, only if every failure is infrastructure",
        ),
    ];
    ensure_write_approved(
        state,
        broker,
        &registration.session_id,
        &registration.owner,
        CI_RETRY_GROUP,
        &subject,
        &facts,
    )
    .map_err(Refusal::Card)
}

/// The id that tells a caller which machine's daemon answered: its device id
/// when one exists, and a fixed word otherwise.
fn host_id(state: &ServerState) -> String {
    state
        .device_identity()
        .as_ref()
        .map(|identity| identity.device_id.clone())
        .unwrap_or_else(|_| "local".to_string())
}

fn reply(id: &Value, host_id: &str, outcome: Result<Value, CiError>) -> Value {
    let (envelope, text, is_error) = match outcome {
        Ok(data) => {
            let text = data.to_string();
            (
                json!({"hostId": host_id, "ok": true, "data": data}),
                text,
                false,
            )
        }
        Err(error) => {
            let envelope = json!({
                "hostId": host_id,
                "ok": false,
                "error": {
                    "code": error.code,
                    "message": error.message,
                    "retryable": error.retryable,
                },
            });
            (envelope, format!("{}: {}", error.code, error.message), true)
        }
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": text}],
            "structuredContent": envelope,
            "isError": is_error,
        },
    })
}

#[cfg(test)]
#[path = "ci_watch_tool_tests.rs"]
mod tests;
