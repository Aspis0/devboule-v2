//! The collision tool's wire: the envelope it answers in, the closed
//! argument set it accepts, and the refusals it hands back. Bodies are called
//! directly; the dispatch table's two lines for this name are covered by the
//! tool-list test below.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use devboule_protocol::{OwnerId, WorkspaceIsolation};
use serde_json::{json, Value};

use crate::mcp_broker::RegisteredSession;
use crate::provider_catalog::{ToolOverlay, MCP_FILE_COLLISIONS_TOOL};
use crate::server::ServerState;
use crate::workspace_files::NOT_PART_OF_THE_TREE;
use crate::workspace_git_support::OUTSIDE_THE_WORKSPACE;

const SESSION: &str = "collision-caller";

fn owner() -> OwnerId {
    OwnerId::new("S-1-5-21-collisions", "collisions-client").expect("owner")
}

fn registration() -> RegisteredSession {
    RegisteredSession {
        session_id: SESSION.to_string(),
        owner: owner(),
        provider_id: Some("claude".to_string()),
        depth: 0,
        overlay: ToolOverlay::NONE,
        bearer: String::new(),
        claude_config_path: None,
        runtime: None,
        broker_ready: Arc::new(AtomicBool::new(true)),
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A git project with a local workspace, one sibling worktree workspace on
/// another branch, and a live agent session inside the local one. `seed`
/// names the tracked file this case asks about, so cases running beside each
/// other never read one another's writer rows.
fn project(tag: &str, seed: &str) -> (Arc<ServerState>, PathBuf, String) {
    let state = ServerState::new(format!("mcp-collisions-{tag}"));
    let dir = crate::test_dirs::test_temp_dir(&format!("devboule-mcp-collisions-{tag}"));
    let root = dir.join("project");
    std::fs::create_dir_all(&root).expect("project folder");
    git(&root, &["init", "--quiet"]);
    git(&root, &["config", "user.email", "test@devboule.local"]);
    git(&root, &["config", "user.name", "devboule test"]);
    std::fs::write(root.join(seed), "seed\n").expect("seed file");
    git(&root, &["add", seed]);
    git(&root, &["commit", "--quiet", "--message", "seed"]);
    let project = state
        .sessions
        .project_add(root.to_str().expect("project path"))
        .expect("project row");
    let local = state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("workspace row");
    let other = state
        .sessions
        .workspace_create(
            &project.id,
            WorkspaceIsolation::Worktree,
            Some("other-branch".to_string()),
        )
        .expect("worktree workspace row");
    let other_path = PathBuf::from(&other.path);
    // Left uncommitted in that checkout: the dirty half of the answer.
    std::fs::write(other_path.join(seed), "theirs\n").expect("their copy");
    let _runtime = crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        SESSION,
        owner(),
        &local.id,
    );
    (state, dir, other.id)
}

fn call(state: &Arc<ServerState>, arguments: Value) -> Value {
    let message = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {"name": MCP_FILE_COLLISIONS_TOOL, "arguments": arguments},
    });
    super::collisions(state, &registration(), json!(7), &message)
        .expect("the tool answers a call")
        .expect("a tools/call always has a reply")
}

fn structured(reply: &Value) -> &Value {
    &reply["result"]["structuredContent"]
}

#[test]
fn collision_answers_its_own_envelope_for_the_callers_repository() {
    let (state, dir, other_workspace) = project("envelope", "envelope-seed.txt");
    let reply = call(
        &state,
        json!({"path": "envelope-seed.txt", "lookbackMinutes": 30}),
    );
    assert_eq!(reply["result"]["isError"], json!(false));
    assert_eq!(structured(&reply)["hostId"], json!(state.host_id()));
    assert_eq!(structured(&reply)["ok"], json!(true));
    let data = &structured(&reply)["data"];
    assert_eq!(data["capped"], json!(false));
    let worktrees = data["worktrees"].as_array().expect("worktrees");
    assert_eq!(worktrees.len(), 1, "{worktrees:?}");
    assert_eq!(worktrees[0]["branch"], json!("other-branch"));
    assert_eq!(
        worktrees[0]["workspaceId"],
        json!(other_workspace),
        "a checkout Devboule knows is named by its workspace id"
    );
    assert_eq!(worktrees[0]["dirtyChange"], json!(true));
    assert_eq!(worktrees[0]["committedChange"], json!(false));
    assert!(
        data["writers"].as_array().is_some_and(Vec::is_empty),
        "a fresh daemon has no writer to vouch for: {data}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn collision_reports_a_write_it_can_vouch_for_and_names_its_own_host() {
    let (state, dir, _other) = project("writers", "writers-seed.txt");
    crate::write_evidence::record_path_write("a-session-that-ended", "writers-seed.txt");
    let reply = call(&state, json!({"path": "writers-seed.txt"}));
    let writers = structured(&reply)["data"]["writers"]
        .as_array()
        .expect("writers")
        .clone();
    assert_eq!(writers.len(), 1, "{writers:?}");
    assert_eq!(writers[0]["sessionId"], json!("a-session-that-ended"));
    assert_eq!(writers[0]["evidence"], json!("agent_file_write"));
    assert_eq!(writers[0]["confidence"], json!("high"));
    assert!(
        writers[0]["lastWriteAt"].as_u64().is_some_and(|at| at > 0),
        "a writer without an instant cannot be read against a window"
    );
    assert_eq!(
        writers[0]["agent"],
        Value::Null,
        "a session that is not live has no name this call may invent"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn collision_refuses_a_path_the_caller_may_not_ask_about() {
    let (state, dir, _other) = project("refuse", "refuse-seed.txt");
    for (path, code) in [
        ("../outside/seed.txt", "invalid_args"),
        (".git/config", "invalid_args"),
    ] {
        let reply = call(&state, json!({"path": path}));
        assert_eq!(
            reply["result"]["isError"],
            json!(true),
            "{path} must be refused"
        );
        assert_eq!(structured(&reply)["ok"], json!(false), "{path}");
        assert_eq!(structured(&reply)["error"]["code"], json!(code), "{path}");
        assert_eq!(
            structured(&reply)["error"]["retryable"],
            json!(false),
            "a refused path answers the same way every time: {path}"
        );
        assert!(
            !structured(&reply)["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains(&dir.to_string_lossy().to_string()),
            "the sentence names no path"
        );
    }
    assert_eq!(
        call(&state, json!({"path": "../outside/seed.txt"}))["result"]["content"][0]["text"],
        json!(OUTSIDE_THE_WORKSPACE)
    );
    assert_eq!(
        call(&state, json!({"path": ".git/config"}))["result"]["content"][0]["text"],
        json!(NOT_PART_OF_THE_TREE)
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn collision_takes_no_argument_but_the_two_it_documents() {
    let (state, dir, _other) = project("arguments", "arguments-seed.txt");
    for (arguments, why) in [
        (json!({}), "path is required"),
        (json!({"path": 7}), "path is required"),
        (
            json!({"path": "arguments-seed.txt", "depth": 2}),
            "unknown parameter 'depth'",
        ),
        (
            json!({"path": "arguments-seed.txt", "workspaceId": "another"}),
            "unknown parameter 'workspaceId'",
        ),
        (
            json!({"path": "arguments-seed.txt", "lookbackMinutes": 0}),
            "lookbackMinutes must be an integer 1..1440",
        ),
        (
            json!({"path": "arguments-seed.txt", "lookbackMinutes": 1441}),
            "lookbackMinutes must be an integer 1..1440",
        ),
        (
            json!({"path": "arguments-seed.txt", "lookbackMinutes": "60"}),
            "lookbackMinutes must be an integer",
        ),
        (json!(["arguments-seed.txt"]), "arguments must be an object"),
    ] {
        let reply = call(&state, arguments.clone());
        assert_eq!(
            reply["error"]["code"],
            json!(-32602),
            "{arguments} must be refused: {reply}"
        );
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains(why),
            "{arguments}: {}",
            reply["error"]["message"]
        );
    }

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn collision_is_published_with_the_schema_its_parser_reads() {
    let listed = crate::mcp_broker::dispatch::enabled_tool_list(
        crate::provider_catalog::MCP_BROKER_TOOLS,
        None,
        ToolOverlay::NONE,
    );
    let tool = listed
        .iter()
        .find(|tool| tool["name"] == MCP_FILE_COLLISIONS_TOOL)
        .expect("the tool is published");
    assert_eq!(
        tool["inputSchema"],
        crate::provider_catalog::file_collisions_input_schema(),
        "the published document and the parser's known-parameter list are one document"
    );
    assert!(
        !tool["description"].as_str().unwrap_or_default().is_empty(),
        "a tool an agent must find by reading has to say what it answers"
    );
}
