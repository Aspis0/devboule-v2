//! The forward leg of `devboule_oracle_search`: one POST to the Oracle query
//! route inside the desktop app, and the failure matrix that keeps every
//! answer honest.
//!
//! The workspace root comes from the caller's session row (never from an
//! argument), the endpoint from `oracle-app.lock` read side-effect-free — the
//! daemon never locks, writes or creates that file — and every way the call
//! can fail carries its own sentence: app not answering (one fixed sentence),
//! app silent past the timeout (its own), app refusal (`message`, forwarded
//! verbatim), or an answer this daemon cannot read (the skew sentence). A
//! body cut off after the head arrived, and a call this daemon could not
//! even prepare, each get their own sentence too. Only the first tells the
//! caller to open the app: an answer from an app that is open never becomes
//! that sentence.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use devboule_protocol::OwnerId;
use serde_json::{json, Value};

use crate::mcp_project_graph::GraphError;
use crate::oracle_app_record::{oracle_app_lock_path, OracleAppState};
use crate::paths::RuntimePaths;
use crate::server::ServerState;

/// Below the tightest bridge client (pi, 30 s): the agent receives this
/// daemon's own sentence before its own fetch aborts (`timeouts.md` §5).
const FORWARD_TIMEOUT: Duration = Duration::from_secs(25);
const QUERY_PATH: &str = "/oracle/v1/query";
const DEFAULT_LIMIT: i64 = 10;
const MAX_QUERY_CHARS: usize = 4096;
const MAX_FORWARD_BODY_BYTES: usize = 1024 * 1024;

/// The one sentence for an app that is not answering from its record: absent,
/// stale, live-but-unbound, unauthorized, or a port nobody listens on.
const APP_PHRASE: &str = "The Oracle semantic search lives in the Devboule desktop app: open the app and retry. The project graph tools do not need it.";
/// The app is open but mute past the deadline: "open the app" would be false.
const TIMEOUT_PHRASE: &str = "The Devboule desktop app did not answer within 25 seconds: it may still be loading its model. Wait a moment and retry.";
/// The app answered something this daemon's contract does not name (a mixed
/// version pair, a body that no longer parses): the app is open, so this is
/// never the app phrase.
const SKEW_PHRASE: &str = "The Devboule app answering this daemon does not speak this tool yet: update the app and retry.";
const NO_WORKSPACE_PHRASE: &str = "This session has no workspace, so there is no project to search. Start the session in a project folder and retry.";
const CAP_PHRASE: &str = "The Devboule app's answer exceeded the 1 MiB limit and was refused.";
/// The head arrived, so the app answered — but the body never finished:
/// "open the app" and "did not answer within 25 seconds" would both be false.
const CUT_PHRASE: &str = "The Devboule desktop app answered this search but the connection was cut before the answer could be read. Check that the app is still running and retry.";
/// This daemon could not build the call: no byte ever left the machine.
const BUILD_PHRASE: &str = "This daemon could not prepare the Oracle call: retry.";

pub(crate) fn search(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
    arguments: &Value,
) -> Result<Value, GraphError> {
    search_within(state, session_id, owner, arguments, FORWARD_TIMEOUT)
}

/// The seam the timeout tests inject their own duration through; production
/// always passes [`FORWARD_TIMEOUT`].
fn search_within(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
    arguments: &Value,
    timeout: Duration,
) -> Result<Value, GraphError> {
    let request = SearchRequest::parse(arguments)?;
    let root = workspace_root(state, session_id, owner)?;
    let path = oracle_app_lock_path(&RuntimePaths::from_dir(state.sessions.runtime_dir()));
    let app = OracleAppState::read(&path);
    // A record is an endpoint only when it is live *and* past its bind:
    // `is_live` alone believes a body written before the listener bound.
    let record = app.record().filter(|_| app.is_ready());
    let Some(record) = record else {
        return Err(refused(APP_PHRASE));
    };
    forward(record.port, &record.token, &root, &request, timeout)
}

fn forward(
    port: u16,
    token: &str,
    root: &Path,
    request: &SearchRequest,
    timeout: Duration,
) -> Result<Value, GraphError> {
    let document = json!({
        "root": root.to_string_lossy(),
        "query": request.query,
        "limit": request.limit,
    });
    let body = serde_json::to_vec(&document).expect("a JSON document serializes");
    let response = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|_| refused(BUILD_PHRASE))?
        .post(format!("http://127.0.0.1:{port}{QUERY_PATH}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(body)
        .send();
    let response = match response {
        Ok(response) => response,
        Err(error) if error.is_timeout() => return Err(refused(TIMEOUT_PHRASE)),
        Err(_) => return Err(refused(APP_PHRASE)),
    };
    let status = response.status().as_u16();
    if status == 401 {
        return Err(refused(APP_PHRASE));
    }
    let mut bytes = Vec::new();
    if let Err(error) = response
        .take((MAX_FORWARD_BODY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
    {
        let phrase = if body_timed_out(&error) {
            TIMEOUT_PHRASE
        } else {
            CUT_PHRASE
        };
        return Err(refused(phrase));
    }
    if bytes.len() > MAX_FORWARD_BODY_BYTES {
        return Err(refused(CAP_PHRASE));
    }
    answer(status, &bytes)
}

/// reqwest wraps its own error inside the `io::Error` a body read fails with,
/// so a body that hangs past the deadline still names itself a timeout.
fn body_timed_out(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
        .is_some_and(reqwest::Error::is_timeout)
}

fn answer(status: u16, bytes: &[u8]) -> Result<Value, GraphError> {
    match status {
        200 => {
            let Ok(document) = serde_json::from_slice::<Value>(bytes) else {
                return Err(refused(SKEW_PHRASE));
            };
            match document.get("ok") {
                Some(Value::Bool(true)) => Ok(document),
                Some(Value::Bool(false)) => app_refusal(&document),
                _ => Err(refused(SKEW_PHRASE)),
            }
        }
        // The app answered, so this is a version-skew pair, never "open the
        // app"; a `400` carries the app's own sentence when it has one.
        400 => serde_json::from_slice::<Value>(bytes)
            .ok()
            .map_or_else(|| Err(refused(SKEW_PHRASE)), |body| app_refusal(&body)),
        _ => Err(refused(SKEW_PHRASE)),
    }
}

/// A refusal envelope (`no_app_workspace`, `no_model`, `no_index`,
/// `no_vectors`, `warming`, `query_failed`, …) reaches the agent with the
/// app's `message` untouched — including `not_implemented`, which has no
/// `message` and is therefore skew.
fn app_refusal(document: &Value) -> Result<Value, GraphError> {
    match document.get("message").and_then(Value::as_str) {
        Some(message) => Err(refused(message)),
        None => Err(refused(SKEW_PHRASE)),
    }
}

/// The workspace root of the calling session, or the refusal that says why
/// the call cannot be scoped. Never falls back to a directory.
fn workspace_root(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
) -> Result<PathBuf, GraphError> {
    match state.sessions.session_workspace_root(session_id, owner) {
        Ok(Some(root)) => Ok(root),
        Ok(None) => Err(refused(NO_WORKSPACE_PHRASE)),
        Err(error) => Err(refused(error.message)),
    }
}

struct SearchRequest {
    query: String,
    limit: i64,
}

impl SearchRequest {
    /// The closed argument set: `query` (required) and `limit` (optional,
    /// default 10, clamped 1..=10). The query cap and unit match the app's
    /// own `validate_query` (4096 characters), so a query this parse accepts
    /// is a query the route accepts.
    fn parse(arguments: &Value) -> Result<Self, GraphError> {
        let object = arguments
            .as_object()
            .ok_or_else(|| invalid("arguments must be an object"))?;
        for key in object.keys() {
            if !matches!(key.as_str(), "query" | "limit") {
                return Err(invalid(format!("unknown parameter '{key}'")));
            }
        }
        let query = object
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .ok_or_else(|| invalid("query is required"))?;
        if query.chars().count() > MAX_QUERY_CHARS {
            return Err(invalid(format!(
                "query is longer than {MAX_QUERY_CHARS} characters"
            )));
        }
        let limit = match object.get("limit") {
            None | Some(Value::Null) => DEFAULT_LIMIT,
            Some(value) => {
                let limit = value
                    .as_i64()
                    .ok_or_else(|| invalid("limit must be an integer"))?;
                limit.clamp(1, DEFAULT_LIMIT)
            }
        };
        Ok(Self {
            query: query.to_string(),
            limit,
        })
    }
}

fn refused(message: impl Into<String>) -> GraphError {
    GraphError::Refused(message.into())
}

fn invalid(message: impl Into<String>) -> GraphError {
    GraphError::Invalid(message.into())
}

#[cfg(test)]
#[path = "oracle_forward_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "oracle_forward_broker_tests.rs"]
mod broker_tests;

#[cfg(test)]
#[path = "oracle_forward_body_tests.rs"]
mod body_tests;
