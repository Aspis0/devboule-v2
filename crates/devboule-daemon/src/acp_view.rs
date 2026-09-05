//! Derive a UI view from an inbound ACP JSON-RPC envelope.
//!
//! The envelope is the source of truth. This module never mutates it: callers
//! journal the original object and, separately, publish the derived view.

use std::path::Path;

use crate::claude_view::relativize_tool_path;
use devboule_protocol::{
    AvailableCommandView, SessionEvent, SessionModeStateView, SessionModeView, SessionModel,
    SessionModelEffort, ToolLocation, TurnUsage,
};

/// Kind of a JSON-RPC line. Requests carry a method *and* an id; treating a
/// request as a response is how the previous dispatcher went mute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AcpLineKind {
    Request { method: String },
    Notification { method: String },
    Response,
}

pub(crate) fn classify_line(value: &serde_json::Value) -> Option<AcpLineKind> {
    let method = value.get("method").and_then(serde_json::Value::as_str);
    let has_id = value.get("id").is_some();
    match (method, has_id) {
        (Some(method), true) => Some(AcpLineKind::Request {
            method: method.to_string(),
        }),
        (Some(method), false) => Some(AcpLineKind::Notification {
            method: method.to_string(),
        }),
        (None, true) => Some(AcpLineKind::Response),
        (None, false) => None,
    }
}

/// Derive the UI view from an inbound envelope. `None` means we do not model
/// this message yet; the envelope is still the source of truth and must be
/// kept.
pub(crate) fn view_from_envelope(
    value: &serde_json::Value,
    expected_session_id: &str,
) -> Option<SessionEvent> {
    view_from_envelope_in(value, expected_session_id, None)
}

pub(crate) fn view_from_envelope_in(
    value: &serde_json::Value,
    expected_session_id: &str,
    cwd: Option<&Path>,
) -> Option<SessionEvent> {
    if value.get("method").and_then(serde_json::Value::as_str) == Some("session/update") {
        return view_from_session_update(value, expected_session_id, cwd);
    }
    if value.get("method").and_then(serde_json::Value::as_str) == Some("_x.ai/models/update") {
        let params = value.get("params")?;
        return session_manifest_from_models_update(params, None);
    }
    if classify_line(value) == Some(AcpLineKind::Response) {
        return view_from_prompt_response(value, expected_session_id);
    }
    None
}

fn view_from_session_update(
    value: &serde_json::Value,
    expected_session_id: &str,
    cwd: Option<&Path>,
) -> Option<SessionEvent> {
    let params = value.get("params")?;
    if !expected_session_id.is_empty()
        && params.get("sessionId").and_then(serde_json::Value::as_str) != Some(expected_session_id)
    {
        return None;
    }
    let update = params.get("update")?;
    let message_id = update
        .get("messageId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    match update
        .get("sessionUpdate")
        .and_then(serde_json::Value::as_str)
    {
        Some("user_message_chunk") => {
            let text = text_from_content(update.get("content"))?;
            Some(SessionEvent::AgentUserMessage {
                message_id,
                text: text.to_string(),
            })
        }
        Some("agent_thought_chunk") => {
            let text = text_from_content(update.get("content"))?;
            Some(SessionEvent::AgentThought {
                message_id,
                text: text.to_string(),
            })
        }
        Some("agent_message_chunk") => {
            let text = text_from_content(update.get("content"))?;
            Some(SessionEvent::AgentMessage {
                message_id,
                text: text.to_string(),
            })
        }
        Some("available_commands_update") => {
            let commands = commands_from_update(update)?;
            Some(SessionEvent::AvailableCommands { commands })
        }
        Some("tool_call") => Some(SessionEvent::AgentToolCall {
            tool_call_id: update
                .get("toolCallId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: update
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status: update
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("in_progress")
                .to_string(),
            kind: update
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            locations: locations_from_value(update.get("locations"), cwd, false),
        }),
        Some("tool_call_update") => {
            let text = text_from_content(update.get("content")).map(str::to_string);
            Some(SessionEvent::AgentToolUpdate {
                tool_call_id: update
                    .get("toolCallId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                status: update
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                text,
                kind: update
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                locations: locations_from_value(update.get("locations"), cwd, true),
            })
        }
        _ => None,
    }
}

fn view_from_prompt_response(
    value: &serde_json::Value,
    expected_session_id: &str,
) -> Option<SessionEvent> {
    let result = value.get("result")?;
    let stop_reason = result
        .get("stopReason")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let meta = result.get("_meta");
    if let Some(session_id) = meta
        .and_then(|meta| meta.get("sessionId"))
        .and_then(serde_json::Value::as_str)
    {
        if session_id != expected_session_id {
            return None;
        }
    }
    let model_id = meta
        .and_then(|meta| meta.get("modelId"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let usage = meta.and_then(usage_from_meta).or_else(|| {
        result.get("usage").and_then(|usage| {
            usage_from_meta(usage).or_else(|| {
                let usage = TurnUsage {
                    input_tokens: usage.get("inputTokens").and_then(serde_json::Value::as_u64),
                    output_tokens: usage
                        .get("outputTokens")
                        .and_then(serde_json::Value::as_u64),
                    total_tokens: usage.get("totalTokens").and_then(serde_json::Value::as_u64),
                    thought_tokens: usage
                        .get("thoughtTokens")
                        .and_then(serde_json::Value::as_u64),
                };
                if usage.input_tokens.is_none()
                    && usage.output_tokens.is_none()
                    && usage.total_tokens.is_none()
                    && usage.thought_tokens.is_none()
                {
                    None
                } else {
                    Some(usage)
                }
            })
        })
    });
    Some(SessionEvent::AgentFinished {
        stop_reason,
        model_id,
        usage,
    })
}

fn text_from_content(content: Option<&serde_json::Value>) -> Option<&str> {
    let content = content?;
    if content.get("type").and_then(serde_json::Value::as_str) != Some("text") {
        return None;
    }
    content.get("text").and_then(serde_json::Value::as_str)
}

fn usage_from_meta(meta: &serde_json::Value) -> Option<TurnUsage> {
    let input_tokens = meta.get("inputTokens").and_then(serde_json::Value::as_u64);
    let output_tokens = meta.get("outputTokens").and_then(serde_json::Value::as_u64);
    let total_tokens = meta.get("totalTokens").and_then(serde_json::Value::as_u64);
    let thought_tokens = meta
        .get("thoughtTokens")
        .or_else(|| meta.get("reasoningTokens"))
        .and_then(serde_json::Value::as_u64);
    if input_tokens.is_none()
        && output_tokens.is_none()
        && total_tokens.is_none()
        && thought_tokens.is_none()
    {
        return None;
    }
    Some(TurnUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        thought_tokens,
    })
}

fn commands_from_update(update: &serde_json::Value) -> Option<Vec<AvailableCommandView>> {
    let commands = update.get("availableCommands")?.as_array()?;
    Some(
        commands
            .iter()
            .filter_map(|command| {
                let name = command.get("name")?.as_str()?.to_string();
                let description = command.get("description")?.as_str()?.to_string();
                let hint = command
                    .get("input")
                    .and_then(|input| input.get("hint"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                Some(AvailableCommandView {
                    name,
                    description,
                    hint,
                })
            })
            .collect(),
    )
}

pub(crate) fn session_manifest_from_initialize(
    result: &serde_json::Value,
    provider_id: Option<String>,
) -> Option<SessionEvent> {
    let state = result
        .get("_meta")
        .and_then(|meta| meta.get("modelState"))?;
    manifest_from_vendor_models(state, provider_id, None)
}

pub(crate) fn session_manifest_from_new_session(
    result: &serde_json::Value,
    provider_id: Option<String>,
) -> Option<SessionEvent> {
    let models = result.get("models").and_then(|value| {
        manifest_from_vendor_models(value, provider_id.clone(), modes_from_standard(result))
    });
    if models.is_some() {
        return models;
    }
    modes_from_standard(result).map(|modes| SessionEvent::SessionManifest {
        provider_id,
        current_model_id: None,
        models: Vec::new(),
        modes: Some(modes),
    })
}

pub(crate) fn session_manifest_from_models_update(
    params: &serde_json::Value,
    provider_id: Option<String>,
) -> Option<SessionEvent> {
    manifest_from_vendor_models(params, provider_id, None)
}

pub(crate) fn merge_handshake_manifest(
    initialize_result: &serde_json::Value,
    new_session_result: &serde_json::Value,
    provider_id: Option<String>,
) -> Option<SessionEvent> {
    let modes = modes_from_standard(new_session_result);
    let from_new_models = new_session_result
        .get("models")
        .and_then(|value| manifest_from_vendor_models(value, None, None));
    let from_init = session_manifest_from_initialize(initialize_result, None);
    match (from_new_models, from_init, modes) {
        (
            Some(SessionEvent::SessionManifest {
                current_model_id,
                models,
                ..
            }),
            _,
            modes,
        )
        | (
            None,
            Some(SessionEvent::SessionManifest {
                current_model_id,
                models,
                ..
            }),
            modes,
        ) => Some(SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes,
        }),
        (None, None, Some(modes)) => Some(SessionEvent::SessionManifest {
            provider_id,
            current_model_id: None,
            models: Vec::new(),
            modes: Some(modes),
        }),
        _ => None,
    }
}

fn manifest_from_vendor_models(
    value: &serde_json::Value,
    provider_id: Option<String>,
    modes: Option<SessionModeStateView>,
) -> Option<SessionEvent> {
    let available = value.get("availableModels")?.as_array()?;
    let current_model_id = value
        .get("currentModelId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let models: Vec<SessionModel> = available
        .iter()
        .filter_map(session_model_from_vendor)
        .collect();
    if models.is_empty() && current_model_id.is_none() && modes.is_none() {
        return None;
    }
    Some(SessionEvent::SessionManifest {
        provider_id,
        current_model_id,
        models,
        modes,
    })
}

fn session_model_from_vendor(value: &serde_json::Value) -> Option<SessionModel> {
    let model_id = value.get("modelId")?.as_str()?.to_string();
    let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&model_id)
        .to_string();
    let description = value
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let meta = value.get("_meta");
    let context_tokens = meta
        .and_then(|meta| meta.get("totalContextTokens"))
        .and_then(serde_json::Value::as_u64);
    let supports_effort = meta
        .and_then(|meta| meta.get("supportsReasoningEffort"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let efforts = if supports_effort {
        meta.and_then(|meta| meta.get("reasoningEfforts"))
            .and_then(serde_json::Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(session_effort_from_vendor)
                    .collect::<Vec<_>>()
            })
            .filter(|entries| !entries.is_empty())
    } else {
        None
    };
    let current_effort = if supports_effort {
        meta.and_then(|meta| meta.get("reasoningEffort"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    } else {
        None
    };
    Some(SessionModel {
        model_id,
        name,
        description,
        context_tokens,
        current_effort,
        efforts,
    })
}

fn session_effort_from_vendor(value: &serde_json::Value) -> Option<SessionModelEffort> {
    Some(SessionModelEffort {
        id: value.get("id")?.as_str()?.to_string(),
        label: value
            .get("label")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| {
                value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
            })
            .to_string(),
        description: value
            .get("description")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        default: value.get("default").and_then(serde_json::Value::as_bool),
    })
}

fn locations_from_value(
    value: Option<&serde_json::Value>,
    cwd: Option<&Path>,
    empty_is_replace: bool,
) -> Option<Vec<ToolLocation>> {
    let value = value?;
    let entries = value.as_array()?;
    if entries.is_empty() {
        return if empty_is_replace {
            Some(Vec::new())
        } else {
            None
        };
    }
    Some(
        entries
            .iter()
            .filter_map(|location| {
                let path = location.get("path").and_then(serde_json::Value::as_str)?;
                let line = location
                    .get("line")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok());
                Some(ToolLocation {
                    path: relativize_tool_path(path, cwd),
                    line,
                })
            })
            .collect(),
    )
}

fn modes_from_standard(result: &serde_json::Value) -> Option<SessionModeStateView> {
    let modes = result.get("modes")?;
    let current_mode_id = modes
        .get("currentModeId")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let available = modes.get("availableModes")?.as_array()?;
    let available_modes: Vec<SessionModeView> = available
        .iter()
        .filter_map(|mode| {
            Some(SessionModeView {
                id: mode.get("id")?.as_str()?.to_string(),
                name: mode.get("name")?.as_str()?.to_string(),
                description: mode
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    if available_modes.is_empty() {
        return None;
    }
    Some(SessionModeStateView {
        current_mode_id,
        available_modes,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        classify_line, merge_handshake_manifest, session_manifest_from_models_update,
        session_manifest_from_new_session, view_from_envelope, view_from_envelope_in, AcpLineKind,
    };
    use devboule_protocol::SessionEvent;

    // Reconstructed from recon/probes/grok-acp-fullcaps.txt (2026-09-04).
    // The probe file truncates long lines; these objects keep the measured
    // field names and the token-sized chunking.
    const SESSION: &str = "01a06c70-ea2b-7882-ad27-aae8188fc243";

    fn parse(line: &str) -> serde_json::Value {
        serde_json::from_str(line).expect("probe json")
    }

    #[test]
    fn request_with_id_is_not_classified_as_a_response() {
        let line = parse(
            r#"{"jsonrpc":"2.0","id":9,"method":"terminal/create","params":{"sessionId":"s","command":"echo"}}"#,
        );
        assert_eq!(
            classify_line(&line),
            Some(AcpLineKind::Request {
                method: "terminal/create".to_string()
            })
        );
    }

    #[test]
    fn probe_user_message_chunk_becomes_user_view() {
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"Reply with exactly one word: PONG"},"_meta":{"modelId":"grok-4.6","promptIndex":0}}}}"#,
        );
        let view = view_from_envelope(&line, SESSION).expect("user chunk is modeled");
        assert_eq!(
            view,
            SessionEvent::AgentUserMessage {
                message_id: None,
                text: "Reply with exactly one word: PONG".to_string(),
            }
        );
        assert_eq!(
            line["params"]["update"]["sessionUpdate"], "user_message_chunk",
            "derivation must not consume the envelope"
        );
    }

    #[test]
    fn probe_thought_chunks_stay_one_event_per_token() {
        let first = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"The"}}}}"#,
        );
        let second = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":" user"}}}}"#,
        );
        let a = view_from_envelope(&first, SESSION).expect("thought chunk");
        let b = view_from_envelope(&second, SESSION).expect("thought chunk");
        assert_eq!(
            a,
            SessionEvent::AgentThought {
                message_id: None,
                text: "The".to_string(),
            }
        );
        assert_eq!(
            b,
            SessionEvent::AgentThought {
                message_id: None,
                text: " user".to_string(),
            }
        );
    }

    #[test]
    fn probe_message_chunks_stay_one_event_per_token() {
        let first = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"P"}}}}"#,
        );
        let second = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ONG"}}}}"#,
        );
        let a = view_from_envelope(&first, SESSION).expect("message chunk");
        let b = view_from_envelope(&second, SESSION).expect("message chunk");
        assert_eq!(
            a,
            SessionEvent::AgentMessage {
                message_id: None,
                text: "P".to_string(),
            }
        );
        assert_eq!(
            b,
            SessionEvent::AgentMessage {
                message_id: None,
                text: "ONG".to_string(),
            }
        );
    }

    #[test]
    fn probe_available_commands_update_lists_slash_commands() {
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Compress conversation history to save context window","input":{"hint":"optional context"}}]}}}"#,
        );
        match view_from_envelope(&line, SESSION) {
            Some(SessionEvent::AvailableCommands { commands }) => {
                assert_eq!(commands.len(), 1);
                assert_eq!(commands[0].name, "compact");
                assert_eq!(
                    commands[0].description,
                    "Compress conversation history to save context window"
                );
                assert_eq!(commands[0].hint.as_deref(), Some("optional context"));
            }
            other => panic!("expected available commands, got {other:?}"),
        }
    }

    #[test]
    fn probe_prompt_result_carries_stop_reason_model_and_usage() {
        let line = parse(
            r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","_meta":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","modelId":"grok-4.6","inputTokens":18883,"outputTokens":43,"totalTokens":18926}}}"#,
        );
        match view_from_envelope(&line, SESSION) {
            Some(SessionEvent::AgentFinished {
                stop_reason,
                model_id,
                usage,
            }) => {
                assert_eq!(stop_reason, "end_turn");
                assert_eq!(model_id.as_deref(), Some("grok-4.6"));
                let usage = usage.expect("usage from _meta");
                assert_eq!(usage.input_tokens, Some(18883));
                assert_eq!(usage.output_tokens, Some(43));
                assert_eq!(usage.total_tokens, Some(18926));
            }
            other => panic!("expected agent finished, got {other:?}"),
        }
    }

    #[test]
    fn xai_extension_has_no_view_and_keeps_the_envelope() {
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"_x.ai/mcp/servers_updated","params":{"mcpServers":[]}}"#,
        );
        assert!(view_from_envelope(&line, SESSION).is_none());
        assert_eq!(line["method"], "_x.ai/mcp/servers_updated");
        assert_eq!(
            classify_line(&line),
            Some(AcpLineKind::Notification {
                method: "_x.ai/mcp/servers_updated".to_string()
            })
        );
    }

    #[test]
    fn foreign_session_update_is_ignored() {
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"other","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"no"}}}}"#,
        );
        assert!(view_from_envelope(&line, SESSION).is_none());
    }

    /// A shape demonstration on SYNTHETIC input, not a measurement.
    ///
    /// The fixtures below are padded on purpose so the ratio is visible at a
    /// glance; the byte counts this produces describe the fixtures and nothing
    /// else. Its previous name claimed it measured the probe, and those numbers
    /// reached a public commit message before anyone recomputed them.
    ///
    /// The real turn, counted line by line from
    /// `recon/probes/grok-acp-fullcaps.txt`: 56 inbound lines, 92,586 bytes, of
    /// which 64,263 -- 69% -- are three dumps of the slash-command catalogue.
    /// Thought fragments are 15,692 bytes over 34 lines, about 8x their view.
    /// Written up in `recon/M6-completion-plan.md` section 12.
    #[test]
    fn an_envelope_costs_more_than_the_view_derived_from_it() {
        fn envelope_len(value: &serde_json::Value) -> usize {
            serde_json::to_vec(value).expect("envelope").len()
        }
        fn view_len(value: &serde_json::Value) -> usize {
            view_from_envelope(value, SESSION)
                .and_then(|view| serde_json::to_vec(&view).ok())
                .map(|bytes| bytes.len())
                .unwrap_or(0)
        }

        // Thought-chunk envelopes in recon/probes/grok-acp-fullcaps.txt are
        // annotated ~459 bytes because grok attaches _meta per token.
        let thought = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": SESSION,
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "The"},
                    "_meta": {
                        "totalTokens": 1687,
                        "eventId": "01a06c70-ea2b-7882-ad27-aae8188fc243-3",
                        "agentTimestampMs": 1788525739000u64,
                        "pad": "x".repeat(220)
                    }
                }
            }
        });
        let user = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"Reply with exactly one word: PONG"}}}}"#,
        );
        let message = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"PONG"}}}}"#,
        );
        let mut commands = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": SESSION,
                "update": {
                    "sessionUpdate": "available_commands_update",
                    "availableCommands": [{
                        "name": "compact",
                        "description": "Compress conversation history to save context window",
                        "input": {"hint": "optional context"}
                    }]
                }
            }
        });
        commands["params"]["update"]["availableCommands"][0]["description"] =
            serde_json::Value::String("x".repeat(20_800));
        let xai = parse(
            r#"{"jsonrpc":"2.0","method":"_x.ai/sessions/changed","params":{"upserted":[{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","modelId":"grok-4.6"}]}}"#,
        );
        let finished = parse(
            r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","_meta":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","modelId":"grok-4.6","inputTokens":19016,"outputTokens":41,"totalTokens":19057}}}"#,
        );

        let mut envelopes = Vec::new();
        envelopes.push(("user", user));
        for _ in 0..32 {
            envelopes.push(("thought", thought.clone()));
        }
        envelopes.push(("message", message));
        envelopes.push(("commands", commands.clone()));
        envelopes.push(("commands", commands));
        for _ in 0..6 {
            envelopes.push(("xai", xai.clone()));
        }
        envelopes.push(("finished", finished));

        let mut envelope_bytes = 0usize;
        let mut view_bytes = 0usize;
        let mut modeled = 0usize;
        let mut raw_only = 0usize;
        let mut thought_envelope = 0usize;
        let mut thought_view = 0usize;
        let mut command_envelope = 0usize;
        let mut command_view = 0usize;
        for (kind, value) in &envelopes {
            let e = envelope_len(value);
            let v = view_len(value);
            envelope_bytes += e;
            view_bytes += v;
            match *kind {
                "thought" => {
                    thought_envelope += e;
                    thought_view += v;
                }
                "commands" => {
                    command_envelope += e;
                    command_view += v;
                }
                _ => {}
            }
            if v == 0 {
                raw_only += 1;
                assert_eq!(*kind, "xai");
            } else {
                modeled += 1;
            }
        }
        eprintln!(
            "grok-like one-word turn: n={} envelope={envelope_bytes} view={view_bytes} ratio={:.1}x modeled={modeled} raw_only={raw_only} thoughts 32× envelope={thought_envelope} view={thought_view} ratio={:.1}x commands 2× envelope={command_envelope} view={command_view}",
            envelopes.len(),
            envelope_bytes as f64 / view_bytes.max(1) as f64,
            thought_envelope as f64 / thought_view.max(1) as f64
        );
        assert!(envelope_bytes > view_bytes);
        assert_eq!(envelopes.len(), 1 + 32 + 1 + 2 + 6 + 1);
    }

    // Reconstructed from recon/probes/acp-handshake.txt (2026-09-04) initialize
    // result._meta.modelState / session/new.result.models. Truncated probe
    // lines keep these field names and the grok-4.6 vs grok-4.5 effort split.
    const GROK_MODELS: &str = r#"{
        "currentModelId": "grok-4.6",
        "availableModels": [
            {
                "modelId": "grok-4.6",
                "name": "Grok 4.6",
                "description": "SpaceXAI's latest frontier model",
                "_meta": {
                    "totalContextTokens": 500000,
                    "agentType": "grok-build-plan",
                    "supportsReasoningEffort": true,
                    "reasoningEffort": "xhigh",
                    "reasoningEfforts": [
                        {"id": "xhigh", "value": "xhigh", "label": "Extra High Effort", "description": "Highest effort and reasoning level", "default": false},
                        {"id": "high", "value": "high", "label": "High Effort", "description": "Higher implementation quality with extensive reasoning", "default": true},
                        {"id": "medium", "value": "medium", "label": "Medium Effort", "description": "Balanced effort with standard implementation and testing", "default": false},
                        {"id": "low", "value": "low", "label": "Low Effort", "description": "Quick, fast implementations", "default": false}
                    ]
                }
            },
            {
                "modelId": "grok-4.5",
                "name": "Grok 4.5",
                "_meta": {
                    "totalContextTokens": 500000,
                    "agentType": "grok-build-plan",
                    "supportsReasoningEffort": true,
                    "reasoningEffort": "high",
                    "reasoningEfforts": [
                        {"id": "high", "value": "high", "label": "High Effort", "description": "Highest implementation quality with extensive reasoning", "default": true},
                        {"id": "medium", "value": "medium", "label": "Medium Effort", "description": "Balanced effort with standard implementation and testing", "default": false},
                        {"id": "low", "value": "low", "label": "Low Effort", "description": "Quick, fast implementations", "default": false}
                    ]
                }
            }
        ]
    }"#;

    #[test]
    fn grok_session_new_models_become_a_session_manifest() {
        let result = parse(&format!(
            r#"{{"sessionId":"{SESSION}","models":{GROK_MODELS}}}"#
        ));
        let SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes,
        } = session_manifest_from_new_session(&result, Some("grok".to_string()))
            .expect("vendor models must parse")
        else {
            panic!("expected SessionManifest");
        };
        assert_eq!(provider_id.as_deref(), Some("grok"));
        assert_eq!(current_model_id.as_deref(), Some("grok-4.6"));
        assert!(modes.is_none());
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model_id, "grok-4.6");
        assert_eq!(models[0].context_tokens, Some(500_000));
        assert_eq!(models[0].current_effort.as_deref(), Some("xhigh"));
        let grok46_ids: Vec<&str> = models[0]
            .efforts
            .as_ref()
            .expect("grok-4.6 declares efforts")
            .iter()
            .map(|effort| effort.id.as_str())
            .collect();
        assert!(grok46_ids.contains(&"xhigh"));
        let grok45_ids: Vec<&str> = models[1]
            .efforts
            .as_ref()
            .expect("grok-4.5 declares efforts")
            .iter()
            .map(|effort| effort.id.as_str())
            .collect();
        assert!(!grok45_ids.contains(&"xhigh"));
    }

    #[test]
    fn standard_acp_modes_are_parsed_from_session_new() {
        let result = parse(
            r#"{"sessionId":"s1","modes":{"currentModeId":"ask","availableModes":[{"id":"ask","name":"Always ask","description":"Ask before every tool call."},{"id":"acceptEdits","name":"Accept edits"}]}}"#,
        );
        let SessionEvent::SessionManifest { modes, models, .. } =
            session_manifest_from_new_session(&result, None).expect("standard modes must parse")
        else {
            panic!("expected SessionManifest");
        };
        assert!(models.is_empty());
        let modes = modes.expect("modes");
        assert_eq!(modes.current_mode_id, "ask");
        assert_eq!(modes.available_modes.len(), 2);
        assert_eq!(modes.available_modes[0].name, "Always ask");
        assert_eq!(modes.available_modes[1].id, "acceptEdits");
    }

    #[test]
    fn handshake_prefers_session_new_models_over_initialize_meta() {
        let initialize = parse(&format!(
            r#"{{"_meta":{{"modelState":{{"currentModelId":"stale","availableModels":[{{"modelId":"stale","name":"Stale"}}]}}}}}}"#
        ));
        let new_session = parse(&format!(
            r#"{{"sessionId":"{SESSION}","models":{GROK_MODELS}}}"#
        ));
        let SessionEvent::SessionManifest {
            current_model_id, ..
        } = merge_handshake_manifest(&initialize, &new_session, Some("grok".to_string()))
            .expect("handshake merge")
        else {
            panic!("expected SessionManifest");
        };
        assert_eq!(current_model_id.as_deref(), Some("grok-4.6"));
    }

    // Wire shape measured from a live journal:
    // recon/probes/grok-xai-models-update.txt. The two `_x.ai/models/update`
    // envelopes there match GROK_MODELS above.
    #[test]
    fn xai_models_update_replaces_the_manifest() {
        let params = parse(GROK_MODELS);
        let SessionEvent::SessionManifest {
            current_model_id,
            models,
            ..
        } = session_manifest_from_models_update(&params, Some("grok".to_string()))
            .expect("models update must parse")
        else {
            panic!("expected SessionManifest");
        };
        assert_eq!(current_model_id.as_deref(), Some("grok-4.6"));
        assert_eq!(models[0].current_effort.as_deref(), Some("xhigh"));

        let envelope = parse(&format!(
            r#"{{"jsonrpc":"2.0","method":"_x.ai/models/update","params":{GROK_MODELS}}}"#
        ));
        let view = view_from_envelope(&envelope, SESSION).expect("models update is modeled");
        let SessionEvent::SessionManifest {
            current_model_id, ..
        } = view
        else {
            panic!("models update must become SessionManifest, got {view:?}");
        };
        assert_eq!(current_model_id.as_deref(), Some("grok-4.6"));
    }

    #[test]
    fn tool_call_forwards_kind_and_relativized_locations() {
        let cwd = std::path::Path::new(r"C:\work");
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"tool_call","toolCallId":"call-1","title":"Read lib.rs","status":"pending","kind":"read","locations":[{"path":"C:\\work\\src\\lib.rs","line":12}]}}}"#,
        );
        match view_from_envelope_in(&line, SESSION, Some(cwd)) {
            Some(SessionEvent::AgentToolCall {
                tool_call_id,
                kind,
                locations,
                ..
            }) => {
                assert_eq!(tool_call_id, "call-1");
                assert_eq!(kind.as_deref(), Some("read"));
                let locations = locations.expect("locations");
                assert_eq!(locations.len(), 1);
                assert_eq!(
                    locations[0].path,
                    std::path::PathBuf::from("src")
                        .join("lib.rs")
                        .to_string_lossy()
                );
                assert_eq!(locations[0].line, Some(12));
            }
            other => panic!("expected tool call with locations, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_update_locations_replace_and_are_not_merged() {
        let cwd = std::path::Path::new(r"C:\work");
        let line = parse(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"01a06c70-ea2b-7882-ad27-aae8188fc243","update":{"sessionUpdate":"tool_call_update","toolCallId":"call-1","status":"completed","kind":"edit","locations":[{"path":"C:\\work\\src\\main.rs"}]}}}"#,
        );
        match view_from_envelope_in(&line, SESSION, Some(cwd)) {
            Some(SessionEvent::AgentToolUpdate {
                kind,
                locations,
                status,
                ..
            }) => {
                assert_eq!(status.as_deref(), Some("completed"));
                assert_eq!(kind.as_deref(), Some("edit"));
                let locations = locations.expect("replaced locations");
                assert_eq!(locations.len(), 1);
                assert_eq!(
                    locations[0].path,
                    std::path::PathBuf::from("src")
                        .join("main.rs")
                        .to_string_lossy()
                );
                assert!(locations[0].line.is_none());
            }
            other => panic!("expected tool update with replaced locations, got {other:?}"),
        }
    }
}
