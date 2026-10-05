//! Tests for one topic: tool presentation — kinds, titles, exact
//! commands, relativized locations, and the call and result events built
//! from them.

use std::path::PathBuf;

use devboule_protocol::SessionEvent;
use serde_json::json;

use crate::claude_view::test_support::view;
use crate::text_cap::{MAX_TEXT_BYTES, TRUNCATION_MARKER};

#[test]
fn agent_tool_use_carries_subagent_type_and_parentage() {
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "model": "claude-opus-5",
            "id": "msg_agent",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "toolu_agent",
                "name": "Agent",
                "input": {
                    "description": "Find the relevant files",
                    "subagent_type": "explorer",
                    "run_in_background": true
                }
            }]
        },
        "parent_tool_use_id": null,
        "spawn_depth": 0
    }));
    match events.as_slice() {
        [SessionEvent::AgentToolCall {
            tool_call_id,
            title,
            kind,
            subagent_type,
            parent_tool_use_id,
            spawn_depth,
            ..
        }] => {
            assert_eq!(tool_call_id, "toolu_agent");
            assert_eq!(title, "Find the relevant files");
            assert_eq!(kind.as_deref(), Some("think"));
            assert_eq!(subagent_type.as_deref(), Some("explorer"));
            assert!(parent_tool_use_id.is_none());
            assert_eq!(*spawn_depth, Some(0));
        }
        other => panic!("expected Agent tool call, got {other:?}"),
    }
}

#[test]
fn assistant_tool_use_maps_kind_and_relativized_locations() {
    // recon/probes/claude-perm-probe2-allow-host.txt
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "model": "claude-opus-5",
            "id": "msg_011CekBDWjVAzk4UYDqwDeNo",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "toolu_01SPEx5ftKiRM6gUm1VBwYKz",
                "name": "Bash",
                "input": {
                    "command": r"cmd /c del /q C:\Windows\Temp\devboule-nonexistent.txt",
                    "description": "Delete a nonexistent temp file"
                }
            }]
        }
    }));
    match events.as_slice() {
        [SessionEvent::AgentToolCall {
            tool_call_id,
            title,
            status,
            kind,
            locations,
            ..
        }] => {
            assert_eq!(tool_call_id, "toolu_01SPEx5ftKiRM6gUm1VBwYKz");
            assert_eq!(
                title,
                r"cmd /c del /q C:\Windows\Temp\devboule-nonexistent.txt"
            );
            assert_eq!(status, "pending");
            assert_eq!(kind.as_deref(), Some("execute"));
            assert!(locations.is_none());
        }
        other => panic!("expected AgentToolCall, got {other:?}"),
    }
}

#[test]
fn a_bash_call_carries_its_exact_command_and_its_result_carries_no_code() {
    // Journal rows seq 15 (call) and seq 20 (result) of session
    // s.process-10052.00000001-25bdfd5a2087aae2, verbatim relevant
    // envelope fields (wire evidence, 2026-09-30). The `command` field is
    // the bare line the agent sent, held separately from the title
    // `tool_title` builds — equal here only because this line is short —
    // and the result proves no numeric exit code: its `tool_use_result`
    // has only stdout/stderr/interrupted.
    let mut mapper = view();
    let call = mapper.ingest(&json!({
        "type": "assistant",
        "message": {"content": [{
            "type": "tool_use",
            "id": "toolu_012DmhbRw1NNTLzF9KNNNwXu",
            "name": "Bash",
            "input": {"command": "echo zombie-check", "description": "Echo zombie-check"}
        }]},
        "session_id": "be906cf2-d86f-4091-8640-cf8f5b3236e4",
        "wire_tool_inputs": {"toolu_012DmhbRw1NNTLzF9KNNNwXu": {
            "command": "echo zombie-check",
            "description": "Echo zombie-check"
        }}
    }));
    match call.as_slice() {
        [SessionEvent::AgentToolCall {
            title,
            command,
            exit_code,
            ..
        }] => {
            assert_eq!(command.as_deref(), Some("echo zombie-check"));
            assert_eq!(*exit_code, None);
            assert_eq!(title, "echo zombie-check");
        }
        other => panic!("expected AgentToolCall, got {other:?}"),
    }
    let update = mapper.ingest(&json!({
        "type": "user",
        "message": {"content": [{
            "tool_use_id": "toolu_012DmhbRw1NNTLzF9KNNNwXu",
            "type": "tool_result",
            "content": "zombie-check",
            "is_error": false
        }]},
        "session_id": "be906cf2-d86f-4091-8640-cf8f5b3236e4",
        "tool_use_result": {
            "stdout": "zombie-check",
            "stderr": "",
            "interrupted": false,
            "isImage": false,
            "noOutputExpected": false
        }
    }));
    match update.as_slice() {
        [SessionEvent::AgentToolUpdate {
            command, exit_code, ..
        }] => {
            // `None` skips serialization, so the app's reducer keeps the
            // call's command instead of clearing it.
            assert_eq!(*command, None);
            assert_eq!(*exit_code, None);
        }
        other => panic!("expected AgentToolUpdate, got {other:?}"),
    }
    // The gate is the shared kind table, not the spelling: a lowercase
    // `bash` (constructed — every captured call is spelled `Bash`)
    // carries its command, and the case-folded title arm titles the row
    // with the command instead of falling through to the description.
    let lowercase = mapper.ingest(&json!({
        "type": "assistant",
        "message": {"content": [{
            "type": "tool_use",
            "id": "toolu_lowercase_bash",
            "name": "bash",
            "input": {"command": "echo case-probe", "description": "A description the title must not become"}
        }]}
    }));
    match lowercase.as_slice() {
        [SessionEvent::AgentToolCall { title, command, .. }] => {
            assert_eq!(command.as_deref(), Some("echo case-probe"));
            assert_eq!(title, "echo case-probe");
        }
        other => panic!("expected AgentToolCall, got {other:?}"),
    }
}

#[test]
fn a_read_call_stays_commandless() {
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "assistant",
        "message": {"content": [{
            "type": "tool_use",
            "id": "toolu_read_cmd",
            "name": "Read",
            "input": {"file_path": r"C:\some\probe.txt"}
        }]}
    }));
    match events.as_slice() {
        [SessionEvent::AgentToolCall { command, .. }] => assert_eq!(*command, None),
        other => panic!("expected AgentToolCall, got {other:?}"),
    }
}

#[test]
fn read_tool_use_is_kind_read_with_relativized_path() {
    let mut mapper = view();
    let cwd = mapper.cwd.clone().expect("test cwd");
    let file = cwd.join("src").join("lib.rs");
    let events = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "id": "msg_read",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "toolu_read",
                "name": "Read",
                "input": {"file_path": file.to_string_lossy()}
            }]
        }
    }));
    match events.as_slice() {
        [SessionEvent::AgentToolCall {
            kind,
            locations,
            title,
            ..
        }] => {
            assert_eq!(kind.as_deref(), Some("read"));
            let locations = locations.as_ref().expect("locations");
            assert_eq!(locations.len(), 1);
            assert_eq!(
                locations[0].path,
                PathBuf::from("src").join("lib.rs").to_string_lossy()
            );
            assert_eq!(title, &locations[0].path);
        }
        other => panic!("expected Read tool call, got {other:?}"),
    }
}

#[test]
fn read_tool_use_falls_back_to_path_for_locations() {
    // The title (`tool_title`) already falls back from `file_path` to
    // `path`; the locations must read the same keys.
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "id": "msg_read_path",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "toolu_read_path",
                "name": "Read",
                "input": {"path": "src/main.rs"}
            }]
        }
    }));
    match events.as_slice() {
        [SessionEvent::AgentToolCall {
            kind,
            locations,
            title,
            ..
        }] => {
            assert_eq!(kind.as_deref(), Some("read"));
            assert_eq!(title, "src/main.rs");
            let locations = locations.as_ref().expect("locations");
            assert_eq!(locations.len(), 1);
            assert_eq!(locations[0].path, "src/main.rs");
        }
        other => panic!("expected Read tool call, got {other:?}"),
    }
}

#[test]
fn user_tool_result_success_and_error() {
    let mut mapper = view();
    let ok = mapper.ingest(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{
                "tool_use_id": "toolu_ok",
                "type": "tool_result",
                "content": "devboule-perm-probe",
                "is_error": false
            }]
        }
    }));
    assert_eq!(
        ok,
        vec![SessionEvent::AgentToolUpdate {
            tool_call_id: "toolu_ok".to_string(),
            status: Some("completed".to_string()),
            text: Some("devboule-perm-probe".to_string()),
            title: None,
            kind: None,
            locations: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
            replace: false,

            images: Vec::new(),
        }]
    );
    let err = mapper.ingest(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "content": "The user declined this command in the probe.",
                "is_error": true,
                "tool_use_id": "toolu_01SPEx5ftKiRM6gUm1VBwYKz"
            }]
        }
    }));
    assert_eq!(
        err,
        vec![SessionEvent::AgentToolUpdate {
            tool_call_id: "toolu_01SPEx5ftKiRM6gUm1VBwYKz".to_string(),
            status: Some("failed".to_string()),
            text: Some("The user declined this command in the probe.".to_string()),
            title: None,
            kind: None,
            locations: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
            replace: false,

            images: Vec::new(),
        }]
    );
}

#[test]
fn a_tool_result_is_cut_to_the_shared_budget() {
    // Three-byte characters against a budget that is not a multiple of three:
    // the cut backs off to a character boundary instead of splitting one.
    let kept = "\u{20ac}".repeat(MAX_TEXT_BYTES / 3);
    let huge = format!("{kept}\u{20ac}");
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_huge",
                "content": [{"type": "text", "text": huge}]
            }]
        }
    }));
    match events.as_slice() {
        [SessionEvent::AgentToolUpdate { text, .. }] => assert_eq!(
            text.as_deref(),
            Some(format!("{kept}{TRUNCATION_MARKER}").as_str()),
            "the row shows what it keeps of the answer, and says it was cut"
        ),
        other => panic!("expected a tool update, got {other:?}"),
    }
}

#[test]
fn tool_kind_mapping_covers_the_named_claude_tools() {
    let cases = [
        ("Read", "read"),
        ("Edit", "edit"),
        ("Write", "edit"),
        ("NotebookEdit", "edit"),
        ("Bash", "execute"),
        ("PowerShell", "execute"),
        ("Glob", "search"),
        ("Grep", "search"),
        ("WebFetch", "fetch"),
        ("WebSearch", "search"),
        ("Task", "think"),
        ("Agent", "think"),
        ("Skill", "other"),
    ];
    for (name, expected) in cases {
        let mut mapper = view();
        let events = mapper.ingest(&json!({
            "type": "assistant",
            "message": {
                "id": "m",
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "t", "name": name, "input": {}}]
            }
        }));
        match events.as_slice() {
            [SessionEvent::AgentToolCall { kind, .. }] => {
                assert_eq!(kind.as_deref(), Some(expected), "tool {name}");
            }
            other => panic!("tool {name}: {other:?}"),
        }
    }
}

#[test]
fn websearch_title_is_the_query_and_kind_is_search() {
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "id": "m",
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "t", "name": "WebSearch",
                "input": {"query": "how to test rust", "allowed_domains": []}}]
        }
    }));
    match events.as_slice() {
        [SessionEvent::AgentToolCall { kind, title, .. }] => {
            assert_eq!(kind.as_deref(), Some("search"));
            assert_eq!(title, "how to test rust");
        }
        other => panic!("expected WebSearch tool call, got {other:?}"),
    }
}

#[test]
fn webfetch_title_is_the_url() {
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "id": "m",
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "t", "name": "WebFetch",
                "input": {"url": "https://example.com", "prompt": "summarize"}}]
        }
    }));
    match events.as_slice() {
        [SessionEvent::AgentToolCall { kind, title, .. }] => {
            assert_eq!(kind.as_deref(), Some("fetch"));
            assert_eq!(title, "https://example.com");
        }
        other => panic!("expected WebFetch tool call, got {other:?}"),
    }
}

#[test]
fn grep_title_is_the_pattern() {
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "id": "m",
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "t", "name": "Grep",
                "input": {"pattern": "foo.*", "path": "/work"}}]
        }
    }));
    match events.as_slice() {
        [SessionEvent::AgentToolCall { kind, title, .. }] => {
            assert_eq!(kind.as_deref(), Some("search"));
            assert_eq!(title, "foo.*");
        }
        other => panic!("expected Grep tool call, got {other:?}"),
    }
}

#[test]
fn a_browser_call_titles_its_own_row_and_kinds_as_the_family() {
    // The transcript's browser row is the label plus this line, so the line has
    // to name the command and the one argument the call was given.
    let cases = [
        (
            "browser_click",
            json!({"browserId": "tab-1", "ref": "e33"}),
            "click e33",
        ),
        (
            // Claude qualifies a broker tool with the MCP server it came from,
            // and the row must be the same call under either spelling.
            "mcp__devboule__browser_click",
            json!({"browserId": "tab-1", "ref": "e33"}),
            "click e33",
        ),
        (
            "browser_new_tab",
            json!({"browserId": "tab-1", "url": "https://news.ycombinator.com/newest"}),
            "new tab news.ycombinator.com",
        ),
        ("browser_list_tabs", json!({}), "list tabs"),
    ];
    for (name, input, title) in cases {
        let mut mapper = view();
        let events = mapper.ingest(&json!({
            "type": "assistant",
            "message": {
                "id": "m",
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "t", "name": name, "input": input}]
            }
        }));
        match events.as_slice() {
            [SessionEvent::AgentToolCall {
                kind, title: got, ..
            }] => {
                assert_eq!(kind.as_deref(), Some("browser"), "{name}");
                assert_eq!(got, title, "{name}");
            }
            other => panic!("expected a browser tool call, got {other:?}"),
        }
    }
}

#[test]
fn unknown_tool_falls_back_to_the_bare_tool_name() {
    for name in ["mcp__probe__ping", "custom_tool"] {
        let mut mapper = view();
        let events = mapper.ingest(&json!({
            "type": "assistant",
            "message": {
                "id": "m",
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "t", "name": name, "input": {}}]
            }
        }));
        match events.as_slice() {
            [SessionEvent::AgentToolCall { kind, title, .. }] => {
                assert_eq!(kind.as_deref(), Some("other"));
                assert_eq!(title, name);
            }
            other => panic!("expected unknown tool call, got {other:?}"),
        }
    }
}
