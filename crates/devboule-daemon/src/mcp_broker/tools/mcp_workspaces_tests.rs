//! Tests for the workspace tools: project scoping, the create rules, the
//! first-use card on the write road, the peer door arms and the policy
//! removal. Bodies are called directly; the HTTP road covers the two dispatch
//! lines once each.

use super::*;
use crate::mcp_broker::tools::first_use::{ensure_write_allowed, WORKSPACES_GROUP};
use crate::provider_catalog::{
    MCP_CREATE_WORKSPACE_TOOL, MCP_LIST_WORKSPACES_TOOL, MCP_ROSTER_TOOL,
};
use crate::server::ServerState;
use devboule_protocol::{OwnerId, PeerRole, PermissionOutcome, SessionKind, WorkspaceIsolation};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn owner() -> OwnerId {
    OwnerId::new("S-1-5-21-workspaces", "workspaces-client").expect("owner")
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    crate::test_dirs::test_temp_dir(&format!("devboule-mcp-workspaces-{tag}"))
}

/// One project folder with a local workspace row, as production builds it.
/// Returns the project id, the workspace id and the kept temp dir.
fn add_project(state: &Arc<ServerState>, tag: &str) -> (String, String, std::path::PathBuf) {
    let dir = temp_dir(tag);
    let root = dir.join(format!("Project{tag}"));
    std::fs::create_dir_all(&root).expect("project folder");
    let project = state
        .sessions
        .project_add(root.to_str().expect("project path"))
        .expect("project row");
    let workspace = state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("workspace row");
    (project.id, workspace.id, dir)
}

fn live_in(state: &Arc<ServerState>, id: &str, workspace: &str) {
    crate::session::insert_test_live_agent_in_workspace(&state.sessions, id, owner(), workspace);
}

fn git_project(state: &Arc<ServerState>, tag: &str) -> (String, std::path::PathBuf) {
    let dir = temp_dir(tag);
    let root = dir.join(format!("Git{tag}"));
    std::fs::create_dir_all(&root).expect("project folder");
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    run(&["init"]);
    std::fs::write(root.join("seed.txt"), "seed").expect("seed file");
    run(&["add", "seed.txt"]);
    run(&["commit", "-m", "seed"]);
    let project = state
        .sessions
        .project_add(root.to_str().expect("project path"))
        .expect("project row");
    (project.id, dir)
}

/// Open the write gate the way a human does: answer the raised card.
fn allow_gate(state: &Arc<ServerState>, session: &str) {
    let broker = state
        .sessions
        .live_runtime(session, &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker");
    let thread_state = Arc::clone(state);
    let session = session.to_string();
    let handle = std::thread::spawn(move || {
        ensure_write_allowed(
            &thread_state,
            &thread_state.mcp,
            &session,
            &owner(),
            WORKSPACES_GROUP,
            "testing the gate",
            &[("fact", "value")],
        )
    });
    let start = Instant::now();
    let card = loop {
        let mut ids = broker.test_pending_ids();
        if let Some(id) = ids.pop() {
            break id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the gate raised no card"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    broker
        .test_answer(&card, PermissionOutcome::AllowOnce, "session")
        .expect("answer the gate card");
    assert!(handle.join().expect("gate thread").is_ok());
}

#[test]
fn list_answers_only_the_callers_project() {
    let state = ServerState::new("mcp-workspaces-list".to_string());
    let (_, workspace_a, _dir_a) = add_project(&state, "list-a");
    let (_, workspace_b, _dir_b) = add_project(&state, "list-b");
    live_in(&state, "ws-list-a", &workspace_a);
    live_in(&state, "ws-list-b", &workspace_b);

    let listed = list_workspaces(&state, "ws-list-a", &owner()).expect("list");
    let workspaces = listed["workspaces"].as_array().expect("workspaces");
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0]["id"].as_str(), Some(workspace_a.as_str()));
    assert_eq!(workspaces[0]["kind"], "checkout");
    assert!(workspaces[0]["branch"].is_null());
    assert!(
        workspaces
            .iter()
            .all(|row| row["id"].as_str() != Some(workspace_b.as_str())),
        "another project's workspaces are absent"
    );

    // The reverse direction lists the other project and nothing else.
    let listed = list_workspaces(&state, "ws-list-b", &owner()).expect("list");
    let workspaces = listed["workspaces"].as_array().expect("workspaces");
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0]["id"].as_str(), Some(workspace_b.as_str()));
}

#[test]
fn list_reports_a_worktree_branch() {
    let state = ServerState::new("mcp-workspaces-branch".to_string());
    let (project, _dir) = git_project(&state, "branch");
    let first = state
        .sessions
        .workspace_create(
            &project,
            WorkspaceIsolation::Worktree,
            Some("feature-listed".to_string()),
        )
        .expect("worktree row");
    live_in(&state, "ws-branch", &first.id);

    let listed = list_workspaces(&state, "ws-branch", &owner()).expect("list");
    let workspaces = listed["workspaces"].as_array().expect("workspaces");
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0]["id"].as_str(), Some(first.id.as_str()));
    assert_eq!(workspaces[0]["kind"], "worktree");
    assert_eq!(workspaces[0]["branch"], "feature-listed");
    assert_eq!(workspaces[0]["projectId"].as_str(), Some(project.as_str()));
}

#[test]
fn list_refuses_a_session_with_no_workspace() {
    let state = ServerState::new("mcp-workspaces-nows".to_string());
    crate::session::insert_test_live_agent_with_kind(
        &state.sessions,
        "ws-bare",
        owner(),
        SessionKind::Acp,
    );
    match list_workspaces(&state, "ws-bare", &owner()) {
        Err(WorkspaceError::Refused(message)) => assert!(message.contains("no workspace")),
        other => panic!("a workspace-less session is refused, got {other:?}"),
    }
}

#[test]
fn create_mints_a_local_workspace_with_the_given_name() {
    let state = ServerState::new("mcp-workspaces-create-local".to_string());
    let (project, _dir) = git_project(&state, "cloc");
    // The caller lives in a worktree, so the project owns no local row yet
    // and the named local create has somewhere to land.
    let worktree = state
        .sessions
        .workspace_create(
            &project,
            WorkspaceIsolation::Worktree,
            Some("caller-branch".to_string()),
        )
        .expect("worktree row");
    live_in(&state, "ws-cloc", &worktree.id);
    allow_gate(&state, "ws-cloc");

    let request = CreateRequest::parse(&json!({
        "isolation": "local",
        "name": "Agent desk",
    }))
    .expect("parse");
    let document =
        create_workspace(&state, &state.mcp, "ws-cloc", &owner(), &request).expect("create");
    assert_eq!(document["name"], "Agent desk");
    assert_eq!(document["kind"], "checkout");
    assert!(document["branch"].is_null());
    assert_eq!(document["projectId"].as_str(), Some(project.as_str()));
}

#[test]
fn create_parses_its_closed_shape() {
    // Happy shapes parse.
    let request = CreateRequest::parse(&json!({"isolation": "local"})).expect("parse");
    assert!(request.name.is_none());
    let request = CreateRequest::parse(&json!({
        "isolation": "worktree",
        "branch": "feature-x",
        "name": "X",
        "projectId": "p.1",
    }))
    .expect("parse");
    assert_eq!(request.branch.as_deref(), Some("feature-x"));

    // Closed and required.
    for shape in [
        json!({}),
        json!({"isolation": "worktree", "mode": "branch-off"}),
        json!({"isolation": "cloud"}),
        json!({"isolation": "local", "name": ""}),
        json!({"isolation": "local", "branch": 3}),
        json!({"isolation": "local", "projectId": ""}),
    ] {
        assert!(
            CreateRequest::parse(&shape).is_err(),
            "the shape is refused: {shape}"
        );
    }
}

#[test]
fn create_refuses_paths_and_foreign_projects_without_touching_the_gate() {
    let state = ServerState::new("mcp-workspaces-scope".to_string());
    let (project, workspace, _dir) = add_project(&state, "scope");
    let (_other, _other_ws, _dir2) = add_project(&state, "scope-other");
    live_in(&state, "ws-scope", &workspace);

    for path in ["/tmp/elsewhere", "~/elsewhere", "relative/dir"] {
        let request = CreateRequest::parse(&json!({
            "isolation": "worktree",
            "path": path,
        }))
        .expect("parse");
        match create_workspace(&state, &state.mcp, "ws-scope", &owner(), &request) {
            Err(WorkspaceError::Invalid(message)) => assert!(message.contains("path")),
            other => panic!("an agent-named path is refused, got {other:?}"),
        }
    }

    let request = CreateRequest::parse(&json!({
        "isolation": "worktree",
        "projectId": "p.does-not-exist",
    }))
    .expect("parse");
    match create_workspace(&state, &state.mcp, "ws-scope", &owner(), &request) {
        Err(WorkspaceError::Refused(message)) => assert!(message.contains("not found")),
        other => panic!("a foreign project says not found, got {other:?}"),
    }

    // Nothing above reached the gate: no card was ever raised.
    assert_eq!(state.mcp.first_use_mark("ws-scope", WORKSPACES_GROUP), None);
    let _ = project;
}

#[test]
fn create_refuses_a_branch_on_local_and_a_second_local_row() {
    let state = ServerState::new("mcp-workspaces-local-rules".to_string());
    let (_project, workspace, _dir) = add_project(&state, "localrules");
    live_in(&state, "ws-localrules", &workspace);

    let request = CreateRequest::parse(&json!({
        "isolation": "local",
        "branch": "feature-x",
    }))
    .expect("parse");
    match create_workspace(&state, &state.mcp, "ws-localrules", &owner(), &request) {
        Err(WorkspaceError::Invalid(message)) => assert!(message.contains("branch")),
        other => panic!("local with a branch is invalid, got {other:?}"),
    }

    // The fixture project already owns its local workspace.
    let request = CreateRequest::parse(&json!({"isolation": "local"})).expect("parse");
    match create_workspace(&state, &state.mcp, "ws-localrules", &owner(), &request) {
        Err(WorkspaceError::Refused(message)) => assert!(message.contains("already has")),
        other => panic!("a second local row is refused, got {other:?}"),
    }
    assert_eq!(
        state.mcp.first_use_mark("ws-localrules", WORKSPACES_GROUP),
        None
    );
}

#[test]
fn create_mints_a_worktree_behind_the_gate() {
    let state = ServerState::new("mcp-workspaces-worktree".to_string());
    let (project, _dir) = git_project(&state, "wt");
    let local = state
        .sessions
        .workspace_create(&project, WorkspaceIsolation::Local, None)
        .expect("local row");
    live_in(&state, "ws-wt", &local.id);
    allow_gate(&state, "ws-wt");

    let request = CreateRequest::parse(&json!({
        "isolation": "worktree",
        "branch": "feature-created",
        "name": "Created checkout",
    }))
    .expect("parse");
    let document =
        create_workspace(&state, &state.mcp, "ws-wt", &owner(), &request).expect("create");
    assert_eq!(document["kind"], "worktree");
    assert_eq!(document["branch"], "feature-created");
    assert_eq!(document["name"], "Created checkout");
    assert_eq!(document["projectId"].as_str(), Some(project.as_str()));

    // The row survived the body: the list names it.
    let listed = list_workspaces(&state, "ws-wt", &owner()).expect("list");
    assert_eq!(listed["workspaces"].as_array().expect("rows").len(), 2);
}

#[test]
fn create_accepts_its_own_project_id() {
    let state = ServerState::new("mcp-workspaces-own-project".to_string());
    let (project, _dir) = git_project(&state, "ownp");
    let local = state
        .sessions
        .workspace_create(&project, WorkspaceIsolation::Local, None)
        .expect("local row");
    live_in(&state, "ws-ownp", &local.id);
    allow_gate(&state, "ws-ownp");

    let request = CreateRequest::parse(&json!({
        "isolation": "worktree",
        "projectId": project.clone(),
    }))
    .expect("parse");
    let document =
        create_workspace(&state, &state.mcp, "ws-ownp", &owner(), &request).expect("create");
    assert_eq!(document["projectId"].as_str(), Some(project.as_str()));
}

#[test]
fn the_peer_door_judges_both_tools_as_their_wire_frames() {
    for tool in [MCP_LIST_WORKSPACES_TOOL, MCP_CREATE_WORKSPACE_TOOL] {
        match crate::peer_policy::mcp_tool_wire(tool) {
            Some(crate::peer_policy::McpToolWire::Judged(requests)) => {
                assert!(!requests.is_empty(), "{tool} names its wire act")
            }
            other => panic!("{tool} is judged at the door, got {other:?}"),
        }
        for role in [PeerRole::Client, PeerRole::Daemon] {
            assert_eq!(
                crate::peer_policy::mcp_tool_denial(role, &[], tool),
                Some(crate::peer_policy::CAP_ADMIN),
                "{role:?} without admin is refused {tool}"
            );
            assert_eq!(
                crate::peer_policy::mcp_tool_denial(
                    role,
                    &[crate::peer_policy::CAP_ADMIN.to_string()],
                    tool
                ),
                None,
                "{role:?} with admin reaches {tool}"
            );
        }
    }
}

#[test]
fn the_policy_removes_both_tools_and_design_keeps_only_the_read() {
    use crate::mcp_broker::dispatch::{enabled_tool_list, tool_call_refusal};
    use crate::provider_catalog::ToolOverlay;
    use devboule_protocol::ToolPolicyEntry;

    // No policy: both served.
    let listed = enabled_tool_list(
        crate::provider_catalog::MCP_BROKER_TOOLS,
        None,
        ToolOverlay::NONE,
    );
    for tool in [MCP_LIST_WORKSPACES_TOOL, MCP_CREATE_WORKSPACE_TOOL] {
        assert!(
            listed.iter().any(|entry| entry["name"] == tool),
            "{tool} is served with no policy"
        );
        assert_eq!(tool_call_refusal(None, &ToolOverlay::NONE, tool), None);
    }

    // A stored policy removes each by name.
    for tool in [MCP_LIST_WORKSPACES_TOOL, MCP_CREATE_WORKSPACE_TOOL] {
        let policy = ToolPolicyEntry {
            provider_id: "claude".to_string(),
            enabled: Some(true),
            disabled_tools: vec![tool.to_string()],
        };
        let listed = enabled_tool_list(
            crate::provider_catalog::MCP_BROKER_TOOLS,
            Some(&policy),
            ToolOverlay::NONE,
        );
        assert!(
            listed.iter().all(|entry| entry["name"] != tool),
            "{tool} is hidden once disabled"
        );
        assert_eq!(
            tool_call_refusal(Some(&policy), &ToolOverlay::NONE, tool),
            Some("Tool disabled by policy")
        );
    }

    // The design preset denies the write and keeps the read.
    let listed = enabled_tool_list(
        crate::provider_catalog::MCP_BROKER_TOOLS,
        None,
        ToolOverlay::DESIGN,
    );
    assert!(listed
        .iter()
        .all(|entry| entry["name"] != MCP_CREATE_WORKSPACE_TOOL));
    assert!(listed
        .iter()
        .any(|entry| entry["name"] == MCP_LIST_WORKSPACES_TOOL));
    assert_eq!(
        tool_call_refusal(None, &ToolOverlay::DESIGN, MCP_CREATE_WORKSPACE_TOOL),
        Some("Tool disabled by policy")
    );
    assert_eq!(
        tool_call_refusal(None, &ToolOverlay::DESIGN, MCP_LIST_WORKSPACES_TOOL),
        None
    );
}

#[test]
fn the_listed_schemas_are_closed() {
    use crate::mcp_broker::dispatch::enabled_tool_list;
    use crate::provider_catalog::ToolOverlay;
    let listed = enabled_tool_list(
        crate::provider_catalog::MCP_BROKER_TOOLS,
        None,
        ToolOverlay::NONE,
    );
    for (name, required) in [
        (MCP_LIST_WORKSPACES_TOOL, serde_json::json!(null)),
        (MCP_CREATE_WORKSPACE_TOOL, serde_json::json!(["isolation"])),
    ] {
        let tool = listed
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} is served"));
        assert_eq!(
            tool["inputSchema"]["additionalProperties"], false,
            "{name} schema is closed"
        );
        if required.is_array() {
            assert_eq!(tool["inputSchema"]["required"], required, "{name}");
        }
    }
    // The roster still answers: the new rows joined the table, they did not
    // replace it.
    assert!(listed.iter().any(|tool| tool["name"] == MCP_ROSTER_TOOL));
}

#[test]
fn the_create_card_carries_the_call_facts() {
    let state = ServerState::new("mcp-workspaces-facts".to_string());
    let (project, _dir) = git_project(&state, "facts");
    let local = state
        .sessions
        .workspace_create(&project, WorkspaceIsolation::Local, None)
        .expect("local row");
    live_in(&state, "ws-facts", &local.id);

    let request = CreateRequest::parse(&json!({
        "isolation": "worktree",
        "branch": "fact-branch",
        "name": "Fact desk",
    }))
    .expect("parse");
    let thread_state = Arc::clone(&state);
    let handle = std::thread::spawn(move || {
        create_workspace(
            &thread_state,
            &thread_state.mcp,
            "ws-facts",
            &owner(),
            &request,
        )
    });
    let broker = state
        .sessions
        .live_runtime("ws-facts", &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker");
    let start = Instant::now();
    let card = loop {
        let mut ids = broker.test_pending_ids();
        if let Some(id) = ids.pop() {
            break id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the create raised no card"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let request = broker
        .test_pending_request(&card)
        .expect("the pending card");
    let devboule_protocol::SessionEvent::PermissionRequest {
        title, description, ..
    } = request
    else {
        panic!("the gate raises a permission request");
    };
    assert!(
        title.contains("creating worktree workspace"),
        "title: {title}"
    );
    let description = description.expect("description");
    for fact in [&project, "worktree", "fact-branch", "Fact desk"] {
        assert!(
            description.contains(fact),
            "the card carries {fact}: {description}"
        );
    }
    broker
        .test_answer(&card, PermissionOutcome::Deny, "deny")
        .expect("answer the gate card");
    assert!(handle.join().expect("create thread").is_err());
}

#[test]
fn two_racing_same_branch_creates_keep_the_winners_checkout() {
    let state = ServerState::new("mcp-workspaces-race".to_string());
    let (project, _dir) = git_project(&state, "race");
    let local = state
        .sessions
        .workspace_create(&project, WorkspaceIsolation::Local, None)
        .expect("local row");
    live_in(&state, "ws-race", &local.id);
    allow_gate(&state, "ws-race");

    let barrier = Arc::new(std::sync::Barrier::new(2));
    let run = |state: Arc<ServerState>, barrier: Arc<std::sync::Barrier>| {
        std::thread::spawn(move || {
            barrier.wait();
            let request = CreateRequest::parse(&json!({
                "isolation": "worktree",
                "branch": "race-branch",
            }))
            .expect("parse");
            create_workspace(&state, &state.mcp, "ws-race", &owner(), &request)
        })
    };
    let first = run(Arc::clone(&state), Arc::clone(&barrier));
    let second = run(Arc::clone(&state), Arc::clone(&barrier));
    let (first, second) = (
        first.join().expect("thread"),
        second.join().expect("thread"),
    );
    assert!(
        first.is_ok() != second.is_ok(),
        "exactly one same-branch create wins"
    );
    let winner = first.or(second).expect("the winner");
    assert!(
        std::path::Path::new(winner["path"].as_str().expect("path")).is_dir(),
        "the loser's cleanup keeps the winner's checkout"
    );
    let listed = list_workspaces(&state, "ws-race", &owner()).expect("list");
    assert_eq!(
        listed["workspaces"]
            .as_array()
            .expect("rows")
            .iter()
            .filter(|row| row["branch"] == "race-branch")
            .count(),
        1
    );
}

// The HTTP road: the two dispatch lines, once each, with the gate answered
// the way a human answers it.

fn endpoint(url: &str) -> String {
    url.strip_prefix("http://")
        .expect("loopback URL")
        .split('/')
        .next()
        .expect("loopback endpoint")
        .to_string()
}

fn http_post(url: &str, token: &str, body: &str) -> Value {
    let mut stream = TcpStream::connect(endpoint(url)).expect("MCP listener");
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {token}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("MCP request");
    stream.shutdown(Shutdown::Write).expect("request shutdown");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("MCP response");
    let text = String::from_utf8(response).expect("HTTP response");
    serde_json::from_str(text.split_once("\r\n\r\n").expect("HTTP body").1).expect("JSON response")
}

fn http_call(url: &str, token: &str, tool: &str, arguments: Value) -> Value {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    })
    .to_string();
    http_post(url, token, &body)
}

fn http_state(tag: &str) -> (Arc<ServerState>, String, std::path::PathBuf) {
    let state = ServerState::new(format!("mcp-workspaces-http-{tag}"));
    let (project, workspace, dir) = add_project(&state, &format!("http{tag}"));
    live_in(&state, &format!("ws-http-{tag}"), &workspace);
    let _guard = state
        .mcp
        .register(&format!("ws-http-{tag}"), &owner(), &SessionKind::Acp)
        .expect("registration")
        .expect("caller MCP guard");
    std::mem::forget(_guard);
    let _ = project;
    (state, format!("ws-http-{tag}"), dir)
}

#[test]
fn the_list_tool_answers_without_an_arguments_field() {
    let (state, session, _dir) = http_state("noargs");
    let token = state.mcp.test_token(&session).expect("token");
    let _server = state.mcp.start(&state).expect("MCP server");

    // `arguments` is optional in tools/call: a parameterless tool answers
    // when the field is absent, like the roster tool.
    let answer = http_post(
        state.mcp.url(),
        &token,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_workspaces"}}"#,
    );
    assert_eq!(answer["result"]["isError"], false);
    assert_eq!(
        answer["result"]["structuredContent"]["workspaces"]
            .as_array()
            .expect("workspaces")
            .len(),
        1
    );
}

#[test]
fn the_list_tool_answers_over_http_scoped_to_the_callers_project() {
    let (state, session, _dir) = http_state("list");
    let token = state.mcp.test_token(&session).expect("token");
    let _server = state.mcp.start(&state).expect("MCP server");

    let answer = http_call(state.mcp.url(), &token, MCP_LIST_WORKSPACES_TOOL, json!({}));
    assert_eq!(answer["result"]["isError"], false);
    let workspaces = answer["result"]["structuredContent"]["workspaces"]
        .as_array()
        .expect("workspaces")
        .clone();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0]["kind"], "checkout");
}

#[test]
fn the_create_tool_waits_for_the_card_and_mints_over_http() {
    let state = ServerState::new("mcp-workspaces-http-create".to_string());
    let (project, _dir) = git_project(&state, "httpcreate");
    // The caller lives in a worktree, so the named local create passes the
    // duplicate guard and reaches the gate.
    let caller_ws = state
        .sessions
        .workspace_create(
            &project,
            WorkspaceIsolation::Worktree,
            Some("http-caller".to_string()),
        )
        .expect("caller worktree");
    live_in(&state, "ws-http-create", &caller_ws.id);
    let _guard = state
        .mcp
        .register("ws-http-create", &owner(), &SessionKind::Acp)
        .expect("registration")
        .expect("caller MCP guard");
    std::mem::forget(_guard);
    let token = state.mcp.test_token("ws-http-create").expect("token");
    let _server = state.mcp.start(&state).expect("MCP server");

    let url = state.mcp.url().to_string();
    let handle = std::thread::spawn(move || {
        http_call(
            &url,
            &token,
            MCP_CREATE_WORKSPACE_TOOL,
            json!({"isolation": "local", "name": "HTTP desk"}),
        )
    });
    let broker = state
        .sessions
        .live_runtime("ws-http-create", &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker");
    let start = Instant::now();
    let card = loop {
        let mut ids = broker.test_pending_ids();
        if let Some(id) = ids.pop() {
            break id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the create raised no card"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    broker
        .test_answer(&card, PermissionOutcome::AllowOnce, "session")
        .expect("answer the gate card");
    let answer = handle.join().expect("http thread");
    assert_eq!(answer["result"]["isError"], false);
    assert_eq!(answer["result"]["structuredContent"]["name"], "HTTP desk");
    assert_eq!(answer["result"]["structuredContent"]["kind"], "checkout");
    assert_eq!(
        answer["result"]["structuredContent"]["projectId"].as_str(),
        Some(project.as_str())
    );
}

#[test]
fn a_denied_create_creates_nothing_over_http() {
    let state = ServerState::new("mcp-workspaces-http-deny".to_string());
    let (project, _dir) = git_project(&state, "httpdeny");
    let local = state
        .sessions
        .workspace_create(&project, WorkspaceIsolation::Local, None)
        .expect("local row");
    live_in(&state, "ws-http-deny", &local.id);
    let session = "ws-http-deny".to_string();
    let _guard = state
        .mcp
        .register(&session, &owner(), &SessionKind::Acp)
        .expect("registration")
        .expect("caller MCP guard");
    std::mem::forget(_guard);
    let token = state.mcp.test_token(&session).expect("token");
    let _server = state.mcp.start(&state).expect("MCP server");

    // A worktree create passes every pre-gate check, so the denial lands at
    // the card and creates nothing.
    let url = state.mcp.url().to_string();
    let handle = std::thread::spawn(move || {
        http_call(
            &url,
            &token,
            MCP_CREATE_WORKSPACE_TOOL,
            json!({"isolation": "worktree", "branch": "denied-branch"}),
        )
    });
    let broker = state
        .sessions
        .live_runtime(&session, &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker");
    let start = Instant::now();
    let card = loop {
        let mut ids = broker.test_pending_ids();
        if let Some(id) = ids.pop() {
            break id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the create raised no card"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    broker
        .test_answer(&card, PermissionOutcome::Deny, "deny")
        .expect("answer the gate card");
    let answer = handle.join().expect("http thread");
    assert_eq!(answer["result"]["isError"], true);

    // The fixture local workspace is still the only row.
    let listed = list_workspaces(&state, &session, &owner()).expect("list");
    assert_eq!(
        listed["workspaces"].as_array().expect("rows").len(),
        1,
        "a denied create creates nothing"
    );
}
