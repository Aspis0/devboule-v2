//! Tests for one topic: the frame dispatch and the system frames — the
//! withheld-finish latch in `ingest`, init manifests and session rebinds,
//! and status-to-mode updates.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use super::ClaudeView;
use super::WITHHELD_FINISH_MARKER_TYPE;
use crate::claude_view::test_support::{init_frame, view};

#[test]
fn rebind_to_another_session_resets_the_checklist() {
    let task_update = |id: &str, task: &str| {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "id": format!("m_{id}"),
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": id,
                    "name": "TaskUpdate",
                    "input": {"taskId": task, "status": "completed"},
                }],
            }
        })
    };
    let task_result = |id: &str| {
        serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": "ok",
                }],
            }
        })
    };
    let has_checklist = |events: &[SessionEvent]| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentTasks { .. }))
    };
    let mut view = ClaudeView::new(None);
    view.ingest(&json!({"type": "system", "subtype": "init", "session_id": "s-A"}));
    let events = view.ingest(&json!({
        "type": "assistant",
        "message": {
            "id": "m1",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "t1",
                "name": "TodoWrite",
                "input": {"todos": [{"content": "A-task"}]},
            }],
        }
    }));
    assert!(has_checklist(&events), "the session lists its task");
    // A repeated init for the same session keeps the list: the update
    // still applies.
    view.ingest(&json!({"type": "system", "subtype": "init", "session_id": "s-A"}));
    view.ingest(&task_update("u1", "legacy:0"));
    assert!(has_checklist(&view.ingest(&task_result("u1"))));
    // Another session id on the same view is a rebind: the list starts
    // empty, so the old task id applies nothing.
    view.ingest(&json!({"type": "system", "subtype": "init", "session_id": "s-B"}));
    view.ingest(&task_update("u2", "legacy:0"));
    assert!(!has_checklist(&view.ingest(&task_result("u2"))));
}

#[test]
fn system_init_becomes_session_manifest_and_stores_session_id() {
    let mut mapper = view();
    let envelope = init_frame();
    let events = mapper.ingest(&envelope);
    assert_eq!(
        mapper.peer_session_id(),
        Some("cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd")
    );
    match events.as_slice() {
        [SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes,
        }] => {
            assert_eq!(provider_id.as_deref(), Some("claude"));
            assert_eq!(current_model_id.as_deref(), Some("claude-opus-5[1m]"));
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].model_id, "claude-opus-5[1m]");
            assert_eq!(models[0].name, "claude-opus-5[1m]");
            let modes = modes.as_ref().expect("Claude modes");
            assert_eq!(modes.current_mode_id, "default");
            assert_eq!(
                modes
                    .available_modes
                    .iter()
                    .map(|mode| mode.id.as_str())
                    .collect::<Vec<_>>(),
                [
                    "plan",
                    "default",
                    "acceptEdits",
                    "auto",
                    "bypassPermissions"
                ]
            );
            assert_eq!(modes.available_modes[0].name, "Plan Mode");
            assert_eq!(
                modes.available_modes[0].description.as_deref(),
                Some("Analyze the codebase without executing tools or edits")
            );
            assert_eq!(modes.available_modes[1].name, "Always Ask");
            assert_eq!(
                modes.available_modes[1].description.as_deref(),
                Some("Prompts for permission the first time a tool is used")
            );
            assert_eq!(modes.available_modes[2].name, "Accept File Edits");
            assert_eq!(
                modes.available_modes[2].description.as_deref(),
                Some("Automatically approves edit-focused tools without prompting")
            );
            assert_eq!(modes.available_modes[3].name, "Auto mode");
            assert_eq!(
                modes.available_modes[3].description.as_deref(),
                Some("Uses a model classifier to review permission prompts automatically")
            );
            assert_eq!(modes.available_modes[4].name, "Bypass");
            assert_eq!(
                modes.available_modes[4].description.as_deref(),
                Some("Skip all permission prompts (use with caution)")
            );
        }
        other => panic!("expected SessionManifest, got {other:?}"),
    }
    assert_eq!(
        envelope["subtype"], "init",
        "derivation must not consume the envelope"
    );
}

#[test]
fn system_init_without_mode_keeps_the_mode_unreported() {
    let mut mapper = view();
    let mut envelope = init_frame();
    envelope
        .as_object_mut()
        .expect("init object")
        .remove("permissionMode");

    let events = mapper.ingest(&envelope);

    assert_eq!(mapper.current_mode_id(), None);
    assert!(matches!(
        events.as_slice(),
        [SessionEvent::SessionManifest { modes: None, .. }]
    ));
}

#[test]
fn system_status_and_thinking_tokens_have_no_view() {
    let mut mapper = view();
    let status = mapper.ingest(&json!({
        "type": "system",
        "subtype": "status",
        "status": "requesting",
        "session_id": "00000000-0000-4000-8000-0000000000c2"
    }));
    assert!(status.is_empty());
    let thinking_tokens = mapper.ingest(&json!({
        "type": "system",
        "subtype": "thinking_tokens",
        "estimated_tokens": 100
    }));
    assert!(thinking_tokens.is_empty());
}

#[test]
fn system_status_updates_mode_only_when_the_cli_reports_it() {
    let mut mapper = view();
    let _ = mapper.ingest(&init_frame());
    let status = mapper.ingest(&json!({
        "type": "system",
        "subtype": "status",
        "permissionMode": "acceptEdits"
    }));
    assert!(matches!(status.as_slice(), [SessionEvent::SessionManifest {
        modes: Some(modes), ..
    }] if modes.current_mode_id == "acceptEdits"));
    assert!(mapper
        .ingest(&json!({
            "type": "system",
            "subtype": "status",
            "permission_mode": "plan"
        }))
        .is_empty());
    assert!(mapper
        .ingest(&json!({
            "type": "system",
            "subtype": "status",
            "status": "requesting"
        }))
        .is_empty());
}

#[test]
fn captured_mode_frames_map_to_manifest_updates() {
    let mut mapper = view();
    let frames = include_str!("../fixtures/wire/claude-exit-plan-mode.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("captured frame"))
        .collect::<Vec<_>>();

    let init = mapper.ingest(&frames[0]);
    assert!(matches!(init.as_slice(), [SessionEvent::SessionManifest {
        modes: Some(modes), ..
    }] if modes.current_mode_id == "plan"));
    let status = mapper.ingest(&frames[2]);
    assert!(matches!(status.as_slice(), [SessionEvent::SessionManifest {
        modes: Some(modes), ..
    }] if modes.current_mode_id == "acceptEdits"));
}

#[test]
fn a_withheld_finish_marker_suppresses_only_the_next_result_s_finish() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/claude-aborted-result.json"))
            .expect("fixture");
    let mut mapper = view();
    assert!(mapper
        .ingest(&json!({"type": WITHHELD_FINISH_MARKER_TYPE}))
        .is_empty());
    let events = mapper.ingest(&fixture);
    assert!(!events
        .iter()
        .any(|event| matches!(event, SessionEvent::AgentFinished { .. })));
    // The marker arms one suppression; an ordinary result after it
    // finishes again.
    let after = mapper.ingest(&json!({"type": "result", "stop_reason": "end_turn"}));
    assert_eq!(
        after
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentFinished { stop_reason, .. } => Some(stop_reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec!["end_turn".to_string()]
    );
}

#[test]
fn an_orphaned_marker_expires_instead_of_silencing_a_later_finish() {
    // A journal hole that dropped the marked result leaves the marker
    // followed by some other frame; the suppression must not outlive it
    // and eat a later turn's completion.
    let mut mapper = view();
    assert!(mapper
        .ingest(&json!({"type": WITHHELD_FINISH_MARKER_TYPE}))
        .is_empty());
    let _ = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "a later frame"}],
        },
    }));
    let events = mapper.ingest(&json!({"type": "result", "stop_reason": "end_turn"}));
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentFinished { stop_reason, .. } => Some(stop_reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec!["end_turn".to_string()]
    );
}
