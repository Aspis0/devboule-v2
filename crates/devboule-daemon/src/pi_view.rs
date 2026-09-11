//! Translate Pi RPC events into the daemon's existing agent events.
//!
//! Pi's RPC wire has no ACP-style tool ancestry or subagent type. The adapter
//! therefore emits those fields as `None` by protocol choice.

use devboule_protocol::{SessionEvent, TurnUsage};
use serde_json::Value;

/// Raw pi tool names are not categories; map them the way Paseo's pi
/// mapper does so the frontend can label the row.
fn tool_kind(tool_name: &str) -> &'static str {
    match tool_name.to_ascii_lowercase().as_str() {
        "bash" | "powershell" => "execute",
        "read" => "read",
        "edit" | "write" => "edit",
        "grep" | "find" | "ls" => "search",
        _ => "other",
    }
}

/// The one field that matters, in the same priority as Claude's fallback:
/// command, then path, then pattern.
fn tool_summary(arguments: Option<&Value>) -> Option<String> {
    let arguments = arguments?;
    ["command", "path", "pattern"]
        .into_iter()
        .filter_map(|key| {
            arguments
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        })
        .next()
        .map(str::to_string)
}

pub(crate) fn events_from_line(value: &Value) -> Vec<SessionEvent> {
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };
    match kind {
        "message_update" => message_update_events(value),
        "toolcall_end" => toolcall_end(value).into_iter().collect(),
        "tool_execution_start" => tool_execution_start(value).into_iter().collect(),
        "tool_execution_end" => tool_execution_end(value).into_iter().collect(),
        "turn_end" => turn_end(value).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn message_update_events(value: &Value) -> Vec<SessionEvent> {
    let Some(event) = value.get("assistantMessageEvent") else {
        return Vec::new();
    };
    match event.get("type").and_then(Value::as_str) {
        Some("text_delta") => event
            .get("delta")
            .and_then(Value::as_str)
            .map(|text| {
                vec![SessionEvent::AgentMessage {
                    message_id: None,
                    text: text.to_string(),
                    parent_tool_use_id: None,
                    spawn_depth: None,
                }]
            })
            .unwrap_or_default(),
        Some("thinking_delta") => event
            .get("delta")
            .and_then(Value::as_str)
            .map(|text| {
                vec![SessionEvent::AgentThought {
                    message_id: None,
                    text: text.to_string(),
                    parent_tool_use_id: None,
                    spawn_depth: None,
                }]
            })
            .unwrap_or_default(),
        Some("toolcall_start") => event
            .get("id")
            .and_then(Value::as_str)
            .map(|id| {
                let tool_name = event
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                vec![SessionEvent::AgentToolCall {
                    tool_call_id: id.to_string(),
                    title: tool_name.to_string(),
                    status: "pending".to_string(),
                    kind: Some(tool_kind(tool_name).to_string()),
                    locations: None,
                    subagent_type: None,
                    parent_tool_use_id: None,
                    spawn_depth: None,
                }]
            })
            .unwrap_or_default(),
        Some("toolcall_end") => toolcall_end(value).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn toolcall_end(value: &Value) -> Option<SessionEvent> {
    let tool_call = value
        .get("assistantMessageEvent")
        .and_then(|event| event.get("toolCall"))?;
    let name = tool_call.get("name").and_then(Value::as_str);
    Some(SessionEvent::AgentToolUpdate {
        tool_call_id: tool_call.get("id")?.as_str()?.to_string(),
        status: Some("in_progress".to_string()),
        text: None,
        // Arguments arrive here, so retitle the row with the summary.
        title: tool_summary(tool_call.get("arguments")),
        kind: name.map(|name| tool_kind(name).to_string()),
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
    })
}

fn tool_execution_start(value: &Value) -> Option<SessionEvent> {
    Some(SessionEvent::AgentToolUpdate {
        tool_call_id: value.get("toolCallId")?.as_str()?.to_string(),
        status: Some("in_progress".to_string()),
        text: None,
        title: None,
        kind: value
            .get("toolName")
            .and_then(Value::as_str)
            .map(|name| tool_kind(name).to_string()),
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
    })
}

fn tool_execution_end(value: &Value) -> Option<SessionEvent> {
    let failed = value
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(SessionEvent::AgentToolUpdate {
        tool_call_id: value.get("toolCallId")?.as_str()?.to_string(),
        status: Some(if failed {
            "failed".to_string()
        } else {
            "completed".to_string()
        }),
        text: value.get("result").and_then(tool_result_text),
        title: None,
        kind: value
            .get("toolName")
            .and_then(Value::as_str)
            .map(|name| tool_kind(name).to_string()),
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
    })
}

fn tool_result_text(value: &Value) -> Option<String> {
    let content = value.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    (!text.is_empty()).then_some(text)
}

fn turn_end(value: &Value) -> Option<SessionEvent> {
    let message = value.get("message")?;
    let usage_value = message.get("usage");
    Some(SessionEvent::AgentFinished {
        stop_reason: message
            .get("stopReason")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        model_id: message
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        usage: usage_value.and_then(usage_from_pi),
    })
}

fn usage_from_pi(value: &Value) -> Option<TurnUsage> {
    let usage = TurnUsage {
        input_tokens: value.get("input").and_then(Value::as_u64),
        output_tokens: value.get("output").and_then(Value::as_u64),
        total_tokens: value.get("totalTokens").and_then(Value::as_u64),
        thought_tokens: value.get("reasoning").and_then(Value::as_u64),
    };
    (usage.input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.total_tokens.is_some()
        || usage.thought_tokens.is_some())
    .then_some(usage)
}

#[cfg(test)]
mod tests {
    use super::events_from_line;
    use devboule_protocol::SessionEvent;

    fn parse(line: &str) -> serde_json::Value {
        serde_json::from_str(line).expect("recording JSON")
    }

    #[test]
    fn recorded_text_delta_is_one_agent_message_and_text_end_is_empty() {
        let delta = parse(
            r#"{"type":"message_update","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"OK"}}"#,
        );
        let text_end = parse(
            r#"{"type":"message_update","usage":{"input":25848,"output":3,"cacheRead":0,"cacheWrite":0,"reasoning":0,"totalTokens":25851,"cost":{"input":0.0019386,"output":7.5e-7,"cacheRead":0,"cacheWrite":0,"total":0.00193935}},"assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"OK"}}"#,
        );
        assert_eq!(
            events_from_line(&delta),
            vec![SessionEvent::AgentMessage {
                message_id: None,
                text: "OK".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            }]
        );
        assert!(events_from_line(&text_end).is_empty());
    }

    #[test]
    fn recorded_tool_events_update_the_same_call() {
        let start = parse(
            r#"{"type":"message_update","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"id":"call_e855bd93a9d545228d528feb","toolName":"write"}}"#,
        );
        let end = parse(
            r#"{"type":"message_update","usage":{"input":2757,"output":21,"cacheRead":23104,"cacheWrite":0,"reasoning":0,"totalTokens":25882,"cost":{"input":0.000206775,"output":0.00000525,"cacheRead":0,"cacheWrite":0,"total":0.000212025}},"assistantMessageEvent":{"type":"toolcall_end","contentIndex":0,"toolCall":{"type":"toolCall","id":"call_e855bd93a9d545228d528feb","name":"write","arguments":{"content":"ciao\n","path":"probe_tool.txt"}}}}"#,
        );
        let execution = parse(
            r#"{"type":"tool_execution_end","toolCallId":"call_e855bd93a9d545228d528feb","toolName":"write","result":{"content":[{"type":"text","text":"Successfully wrote to probe_tool.txt"}]},"isError":false}"#,
        );
        assert!(matches!(
            events_from_line(&start).as_slice(),
            [SessionEvent::AgentToolCall { tool_call_id, title, status, kind, .. }]
                if tool_call_id == "call_e855bd93a9d545228d528feb"
                    && title == "write"
                    && status == "pending"
                    && kind.as_deref() == Some("edit")
        ));
        assert!(matches!(
            events_from_line(&end).as_slice(),
            [SessionEvent::AgentToolUpdate { tool_call_id, status, title, kind, .. }]
                if tool_call_id == "call_e855bd93a9d545228d528feb"
                    && status.as_deref() == Some("in_progress")
                    && title.as_deref() == Some("probe_tool.txt")
                    && kind.as_deref() == Some("edit")
        ));
        assert!(matches!(
            events_from_line(&execution).as_slice(),
            [SessionEvent::AgentToolUpdate { tool_call_id, status, text, .. }]
                if tool_call_id == "call_e855bd93a9d545228d528feb"
                    && status.as_deref() == Some("completed")
                    && text.as_deref() == Some("Successfully wrote to probe_tool.txt")
        ));
    }

    #[test]
    fn recorded_turn_end_carries_model_stop_reason_and_usage() {
        let line = parse(
            r#"{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"OK"}],"api":"openai-completions","provider":"openrouter","model":"z-ai/glm-5.3-flash","usage":{"input":25848,"output":3,"cacheRead":0,"cacheWrite":0,"reasoning":0,"totalTokens":25851,"cost":{"input":0.0019386,"output":7.5e-7,"cacheRead":0,"cacheWrite":0,"total":0.00193935}},"stopReason":"stop","timestamp":1788993862485,"responseId":"gen-1788993862-4cxcarrKksRnXEICsFHO","rawStopReason":"stop"},"toolResults":[]}"#,
        );
        assert!(matches!(
            events_from_line(&line).as_slice(),
            [SessionEvent::AgentFinished { stop_reason, model_id, usage }]
                if stop_reason == "stop"
                    && model_id.as_deref() == Some("z-ai/glm-5.3-flash")
                    && usage.as_ref().and_then(|value| value.total_tokens) == Some(25851)
        ));
    }
}
