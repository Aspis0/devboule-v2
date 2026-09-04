//! Derive a UI view from an inbound ACP JSON-RPC envelope.
//!
//! The envelope is the source of truth. This module never mutates it: callers
//! journal the original object and, separately, publish the derived view.

use devboule_protocol::{AvailableCommandView, SessionEvent, TurnUsage};

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
    if value.get("method").and_then(serde_json::Value::as_str) == Some("session/update") {
        return view_from_session_update(value, expected_session_id);
    }
    if classify_line(value) == Some(AcpLineKind::Response) {
        return view_from_prompt_response(value, expected_session_id);
    }
    None
}

fn view_from_session_update(
    value: &serde_json::Value,
    expected_session_id: &str,
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

#[cfg(test)]
mod tests {
    use super::{classify_line, view_from_envelope, AcpLineKind};
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

    #[test]
    fn grok_one_word_turn_journal_cost_is_measured_from_the_probe() {
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
}
