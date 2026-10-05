//! Tests for the process tools' contracts: cleanup takes no pid, and
//! both read tools publish closed schemas.

use super::*;
use crate::mcp_broker::dispatch::enabled_tool_list;
use crate::provider_catalog::{ToolOverlay, MCP_CLEANUP_PROCESSES_TOOL};

/// Cleanup never takes a pid: not in the schema and not in the
/// arguments — the plan comes from the caller's own session's proof
/// alone, so there is no unowned pid to refuse because none can arrive.
#[test]
fn cleanup_refuses_unowned_pid() {
    let listed = enabled_tool_list(
        &[(MCP_CLEANUP_PROCESSES_TOOL, "cleanup")],
        None,
        ToolOverlay::NONE,
    );
    let tool = listed
        .iter()
        .find(|tool| tool["name"].as_str() == Some(MCP_CLEANUP_PROCESSES_TOOL))
        .expect("the cleanup tool is published");
    let properties = &tool["inputSchema"]["properties"];
    assert!(properties.get("pid").is_none(), "no pid in the schema");
    assert!(
        properties.get("sessionId").is_none(),
        "the caller's own session only"
    );
    assert!(
        properties.get("graceMs").is_some(),
        "graceMs is its only argument"
    );

    let id = json!("call-1");
    let message = json!({"params": {"arguments": {"pid": 4242}}});
    let error =
        strict_arguments(&id, &message, &["graceMs"]).expect_err("a pid argument does not exist");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("unknown argument")),
        "{error}"
    );
}

/// The schemas the tools/list publishes for the two read tools: closed,
/// and the owner tool states that exactly one of its keys is required.
#[test]
fn process_tool_schemas_are_closed() {
    let listed = enabled_tool_list(
        &[
            ("devboule_process_owner", "owner"),
            ("devboule_session_processes", "list"),
        ],
        None,
        ToolOverlay::NONE,
    );
    assert_eq!(listed.len(), 2);
    for tool in &listed {
        assert_eq!(
            tool["inputSchema"]["additionalProperties"],
            json!(false),
            "{}",
            tool
        );
    }
    assert!(listed[0]["inputSchema"]["properties"].get("port").is_some());
    assert!(listed[0]["inputSchema"]["properties"].get("pid").is_some());
}
