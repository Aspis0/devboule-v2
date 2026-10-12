//! The agent-edit reads on the wire: the request round-trips with its call
//! reference, and the replies spell their fields the way the frontend reads them.

use crate::messages::WorkspaceGitTotals;
use crate::{
    AgentCallRef, ClientMessage, WorkspaceAgentDiffStatus, WorkspaceAgentEditFile,
    WorkspaceAgentEdits, WorkspaceAgentFileDiff,
};

#[test]
fn an_agent_diff_request_round_trips_with_its_call_reference() {
    let request = ClientMessage::WorkspaceAgentDiff {
        id: 7,
        workspace_id: "ws.1".to_string(),
        path: "src/lib.rs".to_string(),
        call: Some(AgentCallRef {
            session_id: "s.a".to_string(),
            tool_call_id: "toolu_1".to_string(),
        }),
    };
    let encoded = serde_json::to_value(&request).expect("encode");
    let decoded: ClientMessage = serde_json::from_value(encoded.clone()).expect("decode");
    assert_eq!(serde_json::to_value(&decoded).expect("re-encode"), encoded);
    assert_eq!(decoded.name(), "WorkspaceAgentDiff");
}

#[test]
fn an_agent_edits_reply_spells_its_fields_in_camel_case() {
    let edits = WorkspaceAgentEdits {
        files: vec![WorkspaceAgentEditFile {
            path: "src/lib.rs".to_string(),
            additions: 3,
            deletions: 1,
            last_agent: Some("worker".to_string()),
            last_at_ms: 1_700_000_000_000,
            calls: vec![AgentCallRef {
                session_id: "s.a".to_string(),
                tool_call_id: "toolu_1".to_string(),
            }],
            truncated: false,
        }],
        totals: WorkspaceGitTotals {
            additions: 3,
            deletions: 1,
        },
        truncated: false,
        error: None,
    };
    let encoded = serde_json::to_value(&edits).expect("encode");
    let file = &encoded["files"][0];
    assert_eq!(file["lastAtMs"], 1_700_000_000_000u64);
    assert_eq!(file["lastAgent"], "worker");
    assert_eq!(file["calls"][0]["toolCallId"], "toolu_1");
    let decoded: WorkspaceAgentEdits = serde_json::from_value(encoded).expect("decode");
    assert_eq!(decoded, edits);
}

#[test]
fn a_diff_status_is_snake_case_on_the_wire() {
    let diff = WorkspaceAgentFileDiff {
        path: "src/lib.rs".to_string(),
        lines: Vec::new(),
        status: WorkspaceAgentDiffStatus::Truncated,
        error: None,
    };
    let encoded = serde_json::to_value(&diff).expect("encode");
    assert_eq!(encoded["status"], "truncated");
}
