//! What every browser-tool test drives: one MCP session inside a workspace, a
//! loopback broker answering it, and a fake host the tests answer for.
//!
//! The host is registered on the same broker the app registers on, so a test
//! sees the frame a real host would receive and answers through the same entry
//! the connection thread uses.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use devboule_protocol::{
    BrowserError, BrowserErrorCode, BrowserExecuteRequest, BrowserOutcome, DaemonMessage, OwnerId,
    PermissionOutcome, SessionKind,
};

use super::tests::{http_request, owner, response_json};
use super::tools::browser_commands::TOOLS;
use super::tools::browser_login::PREVIEW;
use super::{AgentLineage, McpServerHandle, McpSessionGuard, ServerState};
use crate::outbound::ConnOut;
use crate::provider_catalog::ToolOverlay;

pub(super) const SESSION: &str = "session";

/// One registered host, standing in for the desktop app.
pub(super) struct FakeHost {
    conn_id: u64,
    out: Arc<ConnOut>,
}

impl FakeHost {
    /// Register `conn_id` as a host that answers every command in the table,
    /// plus the saved login's preview: a host command the lane rides on with no
    /// tool of its own, which is why the table does not carry it.
    pub(super) fn register(state: &ServerState, conn_id: u64) -> FakeHost {
        let out = ConnOut::new();
        let commands = TOOLS
            .iter()
            .map(|(_, command)| (*command).to_string())
            .chain(std::iter::once(PREVIEW.to_string()))
            .collect();
        state.browser.register(conn_id, Arc::clone(&out), commands);
        FakeHost { conn_id, out }
    }

    /// The next command the broker pushed to this host.
    pub(super) fn next(&self) -> BrowserExecuteRequest {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            for message in self.out.pull_replies() {
                if let DaemonMessage::BrowserExecuteRequest(request) = message {
                    return request;
                }
            }
            assert!(
                Instant::now() < deadline,
                "no browser command reached the host"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The commands the broker pushed here and no test has answered: what a
    /// call must *not* leave behind. It does not wait, so a test that expects
    /// nothing costs nothing.
    pub(super) fn pending(&self) -> Vec<BrowserExecuteRequest> {
        self.out
            .pull_replies()
            .into_iter()
            .filter_map(|message| match message {
                DaemonMessage::BrowserExecuteRequest(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    pub(super) fn answer_ok(
        &self,
        state: &ServerState,
        request: &BrowserExecuteRequest,
        result: Value,
    ) {
        self.answer(state, request, BrowserOutcome::Ok { result });
    }

    pub(super) fn answer_error(
        &self,
        state: &ServerState,
        request: &BrowserExecuteRequest,
        outcome: BrowserOutcome,
    ) {
        self.answer(state, request, outcome);
    }

    fn answer(
        &self,
        state: &ServerState,
        request: &BrowserExecuteRequest,
        outcome: BrowserOutcome,
    ) {
        state
            .browser
            .accept_response(self.conn_id, &request.request_id, &request.host_id, outcome);
    }
}

/// A host's refusal of one command, in the shape a real host sends it.
pub(super) fn host_refusal(message: &str) -> BrowserOutcome {
    BrowserOutcome::Err(BrowserError {
        code: BrowserErrorCode::HostError,
        message: message.to_string(),
        retryable: false,
    })
}

/// A running broker with one or more registered sessions inside workspaces.
///
/// The guards are fields, not `_` bindings: dropping one revokes the session's
/// bearer, and a test that had already called through it would then get a 401.
pub(super) struct Panel {
    pub state: Arc<ServerState>,
    owner: OwnerId,
    token: String,
    guards: Vec<McpSessionGuard>,
    _server: McpServerHandle,
}

impl Drop for Panel {
    /// A test that fails while a call is parked on a card would hang the
    /// teardown that waits for that call: refusing what is pending lets it
    /// return, so the failure is reported instead of the run being stopped.
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let Some(runtime) = self.state.sessions.live_runtime(SESSION, &self.owner) else {
            return;
        };
        let Some(broker) = runtime.permission_broker() else {
            return;
        };
        for card in broker.test_pending_ids() {
            let _ = broker.test_answer(&card, PermissionOutcome::Deny, "deny");
        }
    }
}

impl Panel {
    pub(super) fn token(&self) -> &str {
        &self.token
    }

    /// Add a second session of the same broker inside `workspace_id`, and answer
    /// with its token. Used where the point is that two workspaces are two
    /// scopes.
    pub(super) fn join_workspace(&mut self, tag: &str, workspace_id: &str) -> String {
        let session_id = format!("{SESSION}-{tag}");
        let (guard, token) = register_session(
            &self.state,
            &session_id,
            tag,
            workspace_id,
            AgentLineage::root(),
        );
        self.guards.push(guard);
        token
    }

    /// The call the broker is left waiting on, so a test can watch what it
    /// refuses before it answers.
    pub(super) fn in_background(
        &self,
        tool: &str,
        arguments: Value,
    ) -> std::thread::JoinHandle<Value> {
        let url = self.state.mcp.url().to_string();
        let bearer = format!("Bearer {}", self.token);
        let body = tools_call(tool, arguments);
        std::thread::spawn(move || response_json(&http_request(&url, Some(&bearer), &body)))
    }

    /// One whole call: the host receives it, answers `result`, and the reply
    /// comes back on this thread.
    pub(super) fn call(
        &self,
        host: &FakeHost,
        tool: &str,
        arguments: Value,
        result: Value,
    ) -> (Value, BrowserExecuteRequest) {
        let reply = self.in_background(tool, arguments);
        let request = host.next();
        host.answer_ok(&self.state, &request, result);
        (reply.join().expect("tool call"), request)
    }
}

/// One broker, one session inside `workspace_id`.
pub(super) fn panel(tag: &str) -> Panel {
    panel_in(tag, "w-browser")
}

/// As [`panel`], for a session born with the `design` overlay: local like every
/// other session here, so the peer door passes it, and the only thing between it
/// and a tool is the overlay its registration carries.
pub(super) fn design_panel(tag: &str) -> Panel {
    panel_lineage(
        tag,
        "w-browser",
        AgentLineage {
            depth: 1,
            overlay: ToolOverlay::DESIGN,
        },
    )
}

/// As [`panel`], for a session whose row names `workspace_id`.
pub(super) fn panel_in(tag: &str, workspace_id: &str) -> Panel {
    panel_lineage(tag, workspace_id, AgentLineage::root())
}

fn panel_lineage(tag: &str, workspace_id: &str, lineage: AgentLineage) -> Panel {
    let state = ServerState::new(format!("mcp-browser-{tag}"));
    let (guard, token) = register_session(&state, SESSION, tag, workspace_id, lineage);
    let server = state.mcp.start(&state).expect("MCP server");
    Panel {
        state,
        owner: owner(&format!("browser-user-{tag}"), "browser-client"),
        token,
        guards: vec![guard],
        _server: server,
    }
}

fn register_session(
    state: &Arc<ServerState>,
    session_id: &str,
    tag: &str,
    workspace_id: &str,
    lineage: AgentLineage,
) -> (McpSessionGuard, String) {
    let owner = owner(&format!("browser-user-{tag}"), "browser-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        session_id,
        owner.clone(),
        workspace_id,
    );
    let guard = state
        .mcp
        .register_with_provider(session_id, &owner, &SessionKind::Acp, None, lineage)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(session_id).expect("token");
    (guard, token)
}

/// The `tools/call` body for one tool, as the agent writes it.
pub(super) fn tools_call(tool: &str, arguments: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    })
    .to_string()
}

/// The text an agent reads out of a tool reply: the first text block that is not
/// the head or the tail the daemon wraps untrusted page content in.
pub(super) fn tool_text(body: &Value) -> &str {
    body.pointer("/result/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .find(|text| {
            !text.starts_with("[devboule: untrusted content]") && !text.starts_with("content-end ")
        })
        .unwrap_or("<no text>")
}

/// The host's own document out of a reply's structured copy: the daemon adds
/// its `_untrusted` provenance beside it.
pub(super) fn host_document(body: &Value) -> Value {
    let mut document = body["result"]["structuredContent"].clone();
    if let Some(map) = document.as_object_mut() {
        map.remove("_untrusted");
    }
    document
}

/// The audit table's rows, as (tool, outcome): what the owner reads about what
/// the lane did.
pub(super) fn audit_rows(state: &ServerState) -> Vec<(String, String)> {
    let connection = rusqlite::Connection::open(state.sessions.runtime_dir().join("journal.db"))
        .expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, outcome FROM audit ORDER BY id")
        .expect("audit query");
    statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("audit rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("read audit rows")
}
