//! Codex `mcpServer/elicitation/request` approvals: the permission-in-disguise
//! card and its accept/decline/cancel reply.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use devboule_protocol::{NoticeSeverity, PermissionOption, SessionEvent};
use serde_json::Value;

use super::codex_input_requests::{
    send_result, CodexInputDeps, CodexPendingKind, CodexPendingResponse,
};
use super::permission_broker::PermissionResponseError;
use super::SessionRuntime;

/// The reply to an `mcpServer/elicitation/request`: an explicit grant is an
/// accept with the empty content the schema asks for, an explicit refusal a
/// decline, and an abandoned card (close, timeout, delivery-off) a cancel.
pub(super) fn codex_elicitation_result(result: &Value) -> Value {
    let outcome = result
        .pointer("/outcome/outcome")
        .and_then(Value::as_str)
        .unwrap_or("");
    let option_id = result
        .pointer("/outcome/optionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let (action, content) = match (outcome, option_id) {
        ("selected", "allow") => ("accept", serde_json::json!({})),
        ("selected", _) => ("decline", Value::Null),
        _ => ("cancel", Value::Null),
    };
    serde_json::json!({ "action": action, "content": content, "_meta": null })
}

/// An `mcpServer/elicitation/request`: a permission in disguise — one
/// message asking whether the MCP server may run a tool — so it becomes
/// an ordinary Allow/Deny card (`kind: tool`) answered with Paseo's
/// `{action, content, _meta}` shape. A `url` card and a schema with
/// required fields ask for more than the pair can give and are declined
/// at once, the way Paseo declines them.
pub(super) fn dispatch_elicitation(
    deps: &CodexInputDeps,
    value: &Value,
    runtime: &Arc<SessionRuntime>,
    seq: Option<u64>,
) {
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    let required = params
        .pointer("/requestedSchema/required")
        .and_then(Value::as_array)
        .is_some_and(|required| !required.is_empty());
    if params.get("mode").and_then(Value::as_str) == Some("url") || required {
        if let Some(id) = value.get("id") {
            let _ = send_result(
                &deps.stdin,
                id,
                serde_json::json!({ "action": "decline", "content": null, "_meta": null }),
            );
        }
        return;
    }
    let broker_id = deps.next_id.fetch_add(1, Ordering::Relaxed);
    let server = params
        .get("serverName")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let tool_call_id = params
        .get("elicitationId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("mcp-elicitation-{}-{broker_id}", deps.spawn_nonce));
    let event = SessionEvent::PermissionRequest {
        tool_call_id,
        title: format!("MCP approval: {server}"),
        description: params
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())
            .map(str::to_string),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            PermissionOption {
                option_id: "allow".to_string(),
                name: "Allow once".to_string(),
                kind: "allow_once".to_string(),
            },
            PermissionOption {
                option_id: "deny".to_string(),
                name: "Deny".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
        is_chooser: None,
        kind: None,
        questions: None,
        // A placeholder the daemon overwrites with the session's stored
        // origin before the request leaves for a subscriber.
        origin: devboule_protocol::SessionOrigin::unknown(),
        create_agent: None,
    };
    if let Ok(mut ids) = deps.response_ids.lock() {
        if let Some(id) = value.get("id") {
            ids.insert(
                broker_id,
                CodexPendingResponse {
                    id: id.clone(),
                    kind: CodexPendingKind::Elicitation,
                    params: Value::Null,
                },
            );
        }
    }
    if let Err(error) = deps
        .permission_broker
        .register(broker_id, event.clone(), runtime)
    {
        deps.response_ids
            .lock()
            .ok()
            .map(|mut ids| ids.remove(&broker_id));
        // The card never reached the person, so this is an abandonment,
        // not a refusal.
        if let Some(id) = value.get("id") {
            let _ = send_result(
                &deps.stdin,
                id,
                serde_json::json!({ "action": "cancel", "content": null, "_meta": null }),
            );
        }
        if !matches!(error, PermissionResponseError::AlreadyRecorded) {
            let _ = runtime.publish_session_notice(
                format!("Could not queue Codex MCP approval: {error}"),
                NoticeSeverity::Warning,
            );
        }
        return;
    }
    let _ = runtime.publish_agent_event_with_seq(event, None, seq);
}
