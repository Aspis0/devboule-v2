//! The `devboule_project_*` tool bodies: the code-knowledge graph the indexer
//! writes for the calling session's own workspace, read-only.
//!
//! Three reads of one graph — the neighbourhood of a node, the files one file
//! imports, and the files that import one file. All three resolve the project
//! the same way, from the caller's own session row (`workspace_id`) and never
//! from an argument, then open `<root>/oracle-data/ckg.sqlite` read-only. A
//! session with no workspace and a workspace with no graph are refused with the
//! sentence that says which fact is missing: neither is ever answered with an
//! empty list, which would read as "this node has no neighbours", and neither
//! ever falls back to another project's graph.

use std::path::{Path, PathBuf};

use devboule_protocol::OwnerId;
use oracle_core::{CkgStore, OracleDataPaths};
use serde_json::{json, Value};

use crate::server::ServerState;

/// The longest node id or file path a tool accepts, in bytes.
const MAX_NAME_BYTES: usize = 1024;
/// The deepest walk `devboule_project_neighborhood` performs, in edges.
const MAX_DEPTH: i64 = 4;
/// The walk when `depth` is absent.
const DEFAULT_DEPTH: i64 = 1;

/// Why a project-graph call did not answer.
///
/// The two arms are the two wire shapes the broker answers with: a malformed
/// request is `rpc_error(-32602)`, an unreadable graph is a tool result with
/// `isError: true` and a sentence naming the missing fact.
#[derive(Debug)]
pub(crate) enum GraphError {
    Invalid(String),
    Refused(String),
}

pub(crate) fn neighborhood(
    state: &ServerState,
    session_id: &str,
    caller: &OwnerId,
    arguments: &Value,
) -> Result<Value, GraphError> {
    let request = NeighborhoodRequest::parse(arguments)?;
    let store = open_graph(&workspace_root(state, session_id, caller)?)?;
    let rows = store
        .neighborhood(&request.node, request.depth, request.kind.as_deref())
        .map_err(|error| refused(format!("The project graph could not be read: {error}")))?;
    let neighbors = rows
        .into_iter()
        .map(|(node, depth)| json!({"node": node, "depth": depth}))
        .collect::<Vec<_>>();
    Ok(json!({
        "node": request.node,
        "depth": request.depth,
        "kind": request.kind,
        "neighbors": neighbors,
    }))
}

pub(crate) fn imports(
    state: &ServerState,
    session_id: &str,
    caller: &OwnerId,
    arguments: &Value,
) -> Result<Value, GraphError> {
    let file = file_argument(arguments)?;
    let store = open_graph(&workspace_root(state, session_id, caller)?)?;
    let edges = store
        .imports_of(&file)
        .map_err(|error| refused(format!("The project graph could not be read: {error}")))?;
    Ok(json!({"file": file, "imports": edge_list(edges)}))
}

pub(crate) fn importers(
    state: &ServerState,
    session_id: &str,
    caller: &OwnerId,
    arguments: &Value,
) -> Result<Value, GraphError> {
    let file = file_argument(arguments)?;
    let store = open_graph(&workspace_root(state, session_id, caller)?)?;
    let edges = store
        .importers_of(&file)
        .map_err(|error| refused(format!("The project graph could not be read: {error}")))?;
    Ok(json!({"file": file, "importers": edge_list(edges)}))
}

/// The engine's edge rows, kept whole: `src` is the `from` end and `dst` the
/// `to` end of the edge as the graph stores it, so the two directions answer
/// the same shape and neither hides the endpoint the query returned.
fn edge_list(edges: Vec<oracle_core::CkgEdgeRow>) -> Vec<Value> {
    edges
        .into_iter()
        .map(|edge| json!({"from": edge.src, "to": edge.dst}))
        .collect()
}

/// The workspace root of the calling session, or the refusal that says why the
/// call cannot be scoped. Never falls back to a directory.
fn workspace_root(
    state: &ServerState,
    session_id: &str,
    caller: &OwnerId,
) -> Result<PathBuf, GraphError> {
    match state.sessions.session_workspace_root(session_id, caller) {
        Ok(Some(root)) => Ok(root),
        Ok(None) => Err(refused(
            "This session has no workspace, so there is no project graph to read. Start the \
             session in a project folder and retry.",
        )),
        Err(error) => Err(refused(error.message)),
    }
}

fn open_graph(root: &Path) -> Result<CkgStore, GraphError> {
    let path = OracleDataPaths::from_root_without_env(root).ckg;
    if !path.is_file() {
        return Err(refused(format!(
            "This workspace has no project graph yet: {} does not exist. Index the project, then \
             retry.",
            path.display()
        )));
    }
    CkgStore::open_read_only(&path)
        .map_err(|error| refused(format!("The project graph could not be opened: {error}")))
}

fn refused(message: impl Into<String>) -> GraphError {
    GraphError::Refused(message.into())
}

fn invalid(message: impl Into<String>) -> GraphError {
    GraphError::Invalid(message.into())
}

/// `devboule_project_imports` and `devboule_project_importers` take one closed
/// argument, `file`.
fn file_argument(arguments: &Value) -> Result<String, GraphError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| invalid("arguments must be an object"))?;
    for key in object.keys() {
        if key != "file" {
            return Err(invalid(format!("unknown parameter '{key}'")));
        }
    }
    name_field(object.get("file"), "file")
}

struct NeighborhoodRequest {
    node: String,
    depth: i64,
    kind: Option<String>,
}

impl NeighborhoodRequest {
    fn parse(arguments: &Value) -> Result<Self, GraphError> {
        let object = arguments
            .as_object()
            .ok_or_else(|| invalid("arguments must be an object"))?;
        for key in object.keys() {
            if !matches!(key.as_str(), "node" | "depth" | "kind") {
                return Err(invalid(format!("unknown parameter '{key}'")));
            }
        }
        let node = name_field(object.get("node"), "node")?;
        let depth = match object.get("depth") {
            None | Some(Value::Null) => DEFAULT_DEPTH,
            Some(value) => {
                let depth = value
                    .as_i64()
                    .ok_or_else(|| invalid("depth must be an integer"))?;
                if !(1..=MAX_DEPTH).contains(&depth) {
                    return Err(invalid(format!("depth must be between 1 and {MAX_DEPTH}")));
                }
                depth
            }
        };
        let kind = match object.get("kind") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) if value == "IMPORT" || value == "CONTAIN" => {
                Some(value.clone())
            }
            Some(Value::String(_)) => return Err(invalid("kind must be IMPORT or CONTAIN")),
            Some(_) => return Err(invalid("kind must be a string")),
        };
        Ok(Self { node, depth, kind })
    }
}

fn name_field(value: Option<&Value>, field: &str) -> Result<String, GraphError> {
    let text = value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(format!("{field} is required")))?;
    if text.len() > MAX_NAME_BYTES {
        return Err(invalid(format!(
            "{field} is longer than {MAX_NAME_BYTES} bytes"
        )));
    }
    Ok(text.to_string())
}

#[cfg(test)]
#[path = "mcp_project_graph_tests.rs"]
mod tests;
