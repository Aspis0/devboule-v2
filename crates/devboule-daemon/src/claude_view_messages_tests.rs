//! Tests for one topic: assistant and user envelope mapping — the
//! checklist handoff from tool frames, model-change manifests, the
//! ExitPlanMode timeline, and the AskUserQuestion echo latch.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::claude_view::test_support::{init_frame, view};
use crate::claude_view::ClaudeView;

#[test]
fn sidechain_frames_do_not_feed_the_checklist() {
    let mut view = ClaudeView::new(None);
    let events = view.ingest(&json!({
        "type": "assistant",
        "parent_tool_use_id": "toolu_parent",
        "message": {
            "id": "msg_child",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "toolu_child_todowrite",
                "name": "TodoWrite",
                "input": {"todos": [{"content": "Child task"}]},
            }],
        }
    }));
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, SessionEvent::AgentTasks { .. })),
        "a subagent's TodoWrite must not feed the parent's checklist: {events:?}"
    );
    let events = view.ingest(&json!({
        "type": "user",
        "parent_tool_use_id": "toolu_parent",
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_child_todowrite",
                "content": "ok",
            }]
        },
        "tool_use_result": {"task": {"id": "9", "subject": "Child task"}},
    }));
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, SessionEvent::AgentTasks { .. })),
        "a subagent's tool_result must not feed the parent's checklist: {events:?}"
    );
}

#[test]
fn the_checklist_event_precedes_the_tool_rows_of_its_message() {
    let mut view = ClaudeView::new(None);
    let events = view.ingest(&json!({
        "type": "assistant",
        "message": {
            "id": "msg_todowrite",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "todowrite",
                "name": "TodoWrite",
                "input": {"todos": [{"content": "Inspect the project layout", "status": "pending"}]},
            }],
        }
    }));
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| match event {
            SessionEvent::AgentTasks { .. } => "agent_tasks",
            SessionEvent::AgentToolCall { .. } => "agent_tool_call",
            other => panic!("unexpected event {other:?}"),
        })
        .collect();
    assert_eq!(kinds, vec!["agent_tasks", "agent_tool_call"]);
}

#[test]
fn assistant_model_change_reemits_a_session_manifest() {
    let mut mapper = view();
    let _ = mapper.ingest(&init_frame());
    let events = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "model": "claude-sonnet-5",
            "id": "msg-model-change",
            "role": "assistant",
            "content": []
        }
    }));
    assert!(matches!(
        events.as_slice(),
        [SessionEvent::SessionManifest {
            current_model_id,
            ..
        }] if current_model_id.as_deref() == Some("claude-sonnet-5")
    ));
}

#[test]
fn subagent_model_changes_do_not_flap_session_manifests() {
    let mut mapper = view();
    let _ = mapper.ingest(&init_frame());
    let frames = [
        json!({
            "type": "assistant",
            "message": {
                "model": "claude-opus-5",
                "id": "msg-top-level-1",
                "role": "assistant",
                "content": []
            },
            "parent_tool_use_id": null
        }),
        json!({
            "type": "assistant",
            "message": {
                "model": "claude-haiku-4-5",
                "id": "msg-subagent",
                "role": "assistant",
                "content": []
            },
            "parent_tool_use_id": "toolu_x"
        }),
        json!({
            "type": "assistant",
            "message": {
                "model": "claude-opus-5",
                "id": "msg-top-level-2",
                "role": "assistant",
                "content": []
            },
            "parent_tool_use_id": null
        }),
    ];
    let manifest_count = frames
        .iter()
        .flat_map(|frame| mapper.ingest(frame))
        .filter(|event| matches!(event, SessionEvent::SessionManifest { .. }))
        .count();
    assert_eq!(manifest_count, 1, "subagent model flapped the manifest");
}

#[test]
fn exit_plan_mode_timeline_carries_plan_and_resolution() {
    let mut mapper = view();
    let calls = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "id": "plan-message",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "plan-tool",
                "name": "ExitPlanMode",
                "input": {"plan": "## Steps\n\n- Add the route"}
            }]
        }
    }));
    assert!(matches!(calls.as_slice(), [
        SessionEvent::AgentToolCall { title, kind: Some(kind), .. },
        SessionEvent::AgentToolUpdate { text: Some(plan), kind: Some(update_kind), .. }
    ] if title == "Plan" && kind == "plan" && plan == "## Steps\n\n- Add the route" && update_kind == "plan"));

    for is_error in [false, true] {
        let result = mapper.ingest(&json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "plan-tool",
                    "is_error": is_error,
                    "content": [{"type": "text", "text": "provider result must not replace the decision"}]
                }]
            }
        }));
        assert!(
            result.is_empty(),
            "provider status must not contradict the human decision: {result:?}"
        );
    }
}

fn ask_tool_use(id: &str) -> Value {
    json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "id": id,
                "name": "AskUserQuestion",
                "input": { "questions": [{ "question": "Which colour?" }] }
            }]
        }
    })
}

fn ask_tool_result(id: &str, failed: bool) -> Value {
    json!({
        "type": "user",
        "message": {
            "content": [{
                "type": "tool_result",
                "tool_use_id": id,
                "content": [{"type": "text", "text": "Green"}],
                "is_error": failed
            }]
        }
    })
}

/// A question's provider row passes through untouched: the broker's own
/// row reuses the card id, so grant and provider rows merge by call id
/// instead of doubling, whatever the result flag says.
#[test]
fn ask_user_question_tool_use_passes_through() {
    let mut mapper = view();
    let events = mapper.ingest(&ask_tool_use("ask-1"));
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        SessionEvent::AgentToolCall { tool_call_id, .. } if tool_call_id == "ask-1"
    ));
}

#[test]
fn ask_user_question_result_drops_only_a_granted_echo() {
    let mut mapper = view();
    for failed in [false, true] {
        let _ = mapper.ingest(&ask_tool_use("ask-9"));
        let events = mapper.ingest(&ask_tool_result("ask-9", failed));
        assert_eq!(events.len(), 1);
        match &events[0] {
            SessionEvent::AgentToolUpdate {
                tool_call_id,
                status,
                text,
                ..
            } => {
                assert_eq!(tool_call_id, "ask-9");
                assert_eq!(
                    status.as_deref(),
                    Some(if failed { "failed" } else { "completed" })
                );
                // A grant's echo is dropped; a refusal keeps Claude's
                // own words, since no answer row of ours carries them.
                assert_eq!(text.as_deref(), if failed { Some("Green") } else { None });
            }
            _ => panic!("expected the tool update"),
        }
    }
}

#[test]
fn turn_end_forgets_question_ids() {
    let mut mapper = view();
    let _ = mapper.ingest(&ask_tool_use("ask-9"));
    let _ = mapper.ingest(&serde_json::json!({
        "type": "result",
        "stop_reason": "end_turn",
    }));
    let events = mapper.ingest(&ask_tool_result("ask-9", false));
    assert_eq!(events.len(), 1);
    match &events[0] {
        SessionEvent::AgentToolUpdate { text, .. } => {
            assert_eq!(text.as_deref(), Some("Green"));
        }
        _ => panic!("expected the tool update"),
    }
}
