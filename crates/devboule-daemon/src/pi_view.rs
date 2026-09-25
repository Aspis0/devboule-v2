//! Translate Pi RPC events into the daemon's existing agent events.
//!
//! Pi's RPC wire has no ACP-style tool ancestry or subagent type. The adapter
//! therefore emits those fields as `None` by protocol choice.

use devboule_protocol::{AvailableCommandView, NoticeSeverity, SessionEvent, TurnUsage};
use serde_json::Value;

use crate::wire_json::{blocks_text, tool_kind_from_name, tool_status};

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
        "turn_end" => turn_end(value),
        // Paseo's own words for pi's compaction frames, shown as our
        // transcript's system line — loading, then the manual or automatic
        // sentence (`pi/agent.ts:2253-2267`; labels at
        // `packages/app/…/message-compaction-label.ts:14-16`), so a
        // `/compact` shows progress and completion (review A5-2 #2).
        "compaction_start" => vec![SessionEvent::SessionNotice {
            text: "Compacting...".to_string(),
            severity: NoticeSeverity::Info,
        }],
        "compaction_end" => vec![SessionEvent::SessionNotice {
            text: if value.get("reason").and_then(Value::as_str) == Some("manual") {
                "Context manually compacted"
            } else {
                // Any reason that is not `manual` is Paseo's automatic trigger.
                "Context automatically compacted"
            }
            .to_string(),
            severity: NoticeSeverity::Info,
        }],
        // The list reply: the reader derives it only for the reply its live
        // waiter claimed (`pi_client.rs`, the response arm), so the row this
        // publishes — and replay re-derives — is always the answered one.
        "response" => commands_from_reply(value)
            .map(|commands| SessionEvent::AvailableCommands { commands })
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// The two built-ins pi's own `get_commands` reply omits — measured: neither
/// `compact` nor `autocompact` is among the names a live reply returned
/// (RECON A5-common §A.3) — seeded exactly as Paseo seeds them
/// (`pi/agent.ts:117-130`, merged at `:139-151`), with the argument hints
/// Paseo gives them.
pub(crate) fn seeded_commands() -> Vec<AvailableCommandView> {
    vec![
        AvailableCommandView {
            name: "compact".to_string(),
            description: "Manually compact the session context".to_string(),
            hint: Some("[instructions]".to_string()),
        },
        AvailableCommandView {
            name: "autocompact".to_string(),
            description: "Toggle automatic context compaction".to_string(),
            hint: Some("[on|off|toggle]".to_string()),
        },
    ]
}

/// The list one `get_commands` reply carries, merged over the seeds by name —
/// Paseo's `mapPiSlashCommands` rule (`pi/agent.ts:139-151`): a repeated
/// name takes the reply's description (falling back to its `source`, which
/// is what Paseo shows when there is none), and pi's own `input.hint` is
/// kept, which Paseo drops (`:151`). `None` for every row that is not a
/// successful reply: the seeds for a failed one are published by the waiter
/// that asked, not by this row.
fn commands_from_reply(value: &Value) -> Option<Vec<AvailableCommandView>> {
    if value.get("type").and_then(Value::as_str) != Some("response")
        || value.get("command").and_then(Value::as_str) != Some("get_commands")
        || value.get("success").and_then(Value::as_bool) != Some(true)
    {
        return None;
    }
    // The bound: the reply is parsed synchronously on pi's reader thread, so
    // at most this many entries are inspected while the event keeps the
    // first thousand accepted names — the same split `claude_view` puts on
    // both its lists. A flood of malformed entries costs a bounded scan.
    const MAX_LISTED_COMMANDS: usize = 1000;
    const MAX_INSPECTED_COMMANDS: usize = 10_000;
    let mut merged = seeded_commands();
    // A success that carries no usable array still shows the seeds: a list
    // the daemon can offer, exactly as a failure leaves it.
    let Some(entries) = value
        .get("data")
        .and_then(|data| data.get("commands"))
        .and_then(Value::as_array)
    else {
        return Some(merged);
    };
    let mut accepted = 0usize;
    for entry in entries.iter().take(MAX_INSPECTED_COMMANDS) {
        if accepted >= MAX_LISTED_COMMANDS {
            break;
        }
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        accepted += 1;
        // Paseo's nullish fallback (`pi/agent.ts:145` `description ??
        // source`): `""` is a description and stays; only an absent or null
        // one falls back to the source (review A5-2 #6).
        let description = match entry.get("description") {
            Some(Value::String(description)) => description.clone(),
            _ => match entry.get("source") {
                Some(Value::String(source)) => source.clone(),
                _ => String::new(),
            },
        };
        let hint = entry
            .get("input")
            .and_then(|input| input.get("hint"))
            .and_then(Value::as_str)
            .filter(|hint| !hint.is_empty())
            .map(str::to_string);
        match merged.iter_mut().find(|command| command.name == name) {
            Some(existing) => {
                existing.description = description;
                if hint.is_some() {
                    existing.hint = hint;
                }
            }
            None => merged.push(AvailableCommandView {
                name: name.to_string(),
                description,
                hint,
            }),
        }
    }
    Some(merged)
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
                    kind: Some(tool_kind_from_name(tool_name).to_string()),
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
        kind: name.map(|name| tool_kind_from_name(name).to_string()),
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
            .map(|name| tool_kind_from_name(name).to_string()),
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
        status: Some(tool_status(failed).to_string()),
        text: value.get("result").and_then(tool_result_text),
        title: None,
        kind: value
            .get("toolName")
            .and_then(Value::as_str)
            .map(|name| tool_kind_from_name(name).to_string()),
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
    })
}

fn tool_result_text(value: &Value) -> Option<String> {
    let content = value.get("content")?;
    if !content.is_array() {
        return None;
    }
    let text = blocks_text(content);
    (!text.is_empty()).then_some(text)
}

/// The turn's finish, then the context reading it proves: pi's own
/// `usage.totalTokens` is the same sum Codex and grok report, so the meter
/// shows it as-is (`live: false` — the number is the end of this turn). The
/// window comes from the model list into the manifest, not from this
/// message, so `max_tokens` is absent and the app reads the manifest entry
/// of this same `model_id`.
///
/// `AgentFinished` comes first because the pi client hands its journal
/// sequence to the first event of a line and the finish is what the
/// transcript cursor belongs to.
fn turn_end(value: &Value) -> Vec<SessionEvent> {
    let Some(message) = value.get("message") else {
        return Vec::new();
    };
    let model_id = message
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    let usage_value = message.get("usage");
    let mut events = vec![SessionEvent::AgentFinished {
        stop_reason: message
            .get("stopReason")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        model_id: model_id.clone(),
        usage: usage_value.and_then(usage_from_pi),
    }];
    if let Some(used_tokens) = usage_value
        .and_then(|usage| usage.get("totalTokens"))
        .and_then(Value::as_u64)
    {
        events.push(SessionEvent::ContextUsage {
            model_id,
            used_tokens,
            max_tokens: None,
            live: false,
        });
    }
    events
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
        // The finish first — the pi client hands its journal sequence to the
        // first event of a line — then the meter's reading of the same
        // `usage.totalTokens`. The window lives in the model list, not on
        // this message, so it arrives with the manifest instead.
        assert!(matches!(
            events_from_line(&line).as_slice(),
            [
                SessionEvent::AgentFinished {
                    stop_reason,
                    model_id,
                    usage
                },
                SessionEvent::ContextUsage {
                    model_id: context_model,
                    used_tokens,
                    max_tokens,
                    live,
                },
            ] if stop_reason == "stop"
                && model_id.as_deref() == Some("z-ai/glm-5.3-flash")
                && usage.as_ref().and_then(|value| value.total_tokens) == Some(25851)
                && context_model.as_deref() == Some("z-ai/glm-5.3-flash")
                && *used_tokens == 25_851
                && max_tokens.is_none()
                && !live
        ));
    }

    #[test]
    fn a_get_commands_reply_publishes_its_commands_over_the_two_seeds() {
        // The reply pi was measured to send (RECON A5-common §A.2): one id,
        // `command`, `success`, and `data.commands` of name/description/
        // source/input.hint entries. Paseo seeds pi's two built-ins itself
        // because the reply omits them (`pi/agent.ts:117-130`); we keep
        // `input.hint`, which Paseo drops (`pi/agent.ts:151`).
        let reply = parse(
            r#"{"id":"c-7","type":"response","command":"get_commands","success":true,"data":{"commands":[{"name":"goal","description":"Set the session goal","source":"extension","input":{"hint":"<objective>"}},{"name":"skill:pdf","source":"skill"},{"name":"compact","description":"pi's own words","source":"prompt"}]}}"#,
        );
        match events_from_line(&reply).as_slice() {
            [SessionEvent::AvailableCommands { commands }] => {
                let listed = commands
                    .iter()
                    .map(|command| {
                        (
                            command.name.as_str(),
                            command.description.as_str(),
                            command.hint.as_deref(),
                        )
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    listed,
                    [
                        // seeds first, in Paseo's order, with their hints;
                        // a name the reply repeats keeps the reply's
                        // description and the seeded hint (the reply never
                        // carries the built-ins — RECON §A.3 — but if it
                        // did, its own description still wins).
                        ("compact", "pi's own words", Some("[instructions]")),
                        (
                            "autocompact",
                            "Toggle automatic context compaction",
                            Some("[on|off|toggle]")
                        ),
                        ("goal", "Set the session goal", Some("<objective>")),
                        // no description → Paseo's source fallback
                        ("skill:pdf", "skill", None),
                    ]
                );
            }
            other => panic!("expected one command list, got {other:?}"),
        }
    }

    #[test]
    fn a_get_commands_reply_that_fails_publishes_nothing_and_so_does_any_other_response() {
        // An error reply, and any other control response: the reader routes
        // every `response` row through the view, and only a successful
        // `get_commands` carries a list. The seeds for a failed reply are
        // published by the waiter that asked, not by this row.
        let refused = parse(
            r#"{"id":"c-8","type":"response","command":"get_commands","success":false,"error":"Unknown command: get_commands"}"#,
        );
        let other_command = parse(
            r#"{"id":"c-9","type":"response","command":"set_model","success":true,"data":{"provider":"p","id":"m"}}"#,
        );
        assert!(events_from_line(&refused).is_empty());
        assert!(events_from_line(&other_command).is_empty());
    }

    #[test]
    fn compaction_frames_show_progress_and_completion_as_paseo_shows_them() {
        // Paseo renders pi's own compaction frames as the transcript's
        // compaction marker: loading → "Compacting...", and on completion
        // the manual or automatic sentence (`pi/agent.ts:2253-2267`, the
        // labels at `packages/app/.../message-compaction-label.ts:14-16`).
        // Our transcript's system line is what can show them (review A5-2 #2).
        let start = parse(r#"{"type":"compaction_start","reason":"manual"}"#);
        let end_manual = parse(r#"{"type":"compaction_end","reason":"manual"}"#);
        let end_auto = parse(r#"{"type":"compaction_end","reason":"threshold"}"#);
        let notice = |text: &str| {
            vec![SessionEvent::SessionNotice {
                text: text.to_string(),
                severity: devboule_protocol::NoticeSeverity::Info,
            }]
        };
        assert_eq!(events_from_line(&start), notice("Compacting..."));
        assert_eq!(
            events_from_line(&end_manual),
            notice("Context manually compacted")
        );
        // Any reason that is not `manual` is Paseo's automatic trigger.
        assert_eq!(
            events_from_line(&end_auto),
            notice("Context automatically compacted")
        );
    }

    #[test]
    fn a_reply_copies_at_most_a_thousand_entries() {
        // The reply is parsed synchronously on pi's reader thread; a
        // correctly typed but enormous array must not become an enormous
        // event (review A5-2 #5): the first thousand entries are copied and
        // the rest dropped.
        let entries = (0..1005)
            .map(
                |index| serde_json::json!({ "name": format!("cmd{index}"), "source": "extension" }),
            )
            .collect::<Vec<_>>();
        let reply = parse(
            serde_json::json!({
                "id": "c-9",
                "type": "response",
                "command": "get_commands",
                "success": true,
                "data": { "commands": entries },
            })
            .to_string()
            .as_str(),
        );
        match events_from_line(&reply).as_slice() {
            [SessionEvent::AvailableCommands { commands }] => assert_eq!(
                commands.len(),
                1002,
                "the two seeds plus the thousand entries the bound keeps"
            ),
            other => panic!("expected one command list, got {other:?}"),
        }
    }

    #[test]
    fn malformed_entries_do_not_consume_the_thousand_entry_bound() {
        // The bound counts accepted names, like `claude_view`'s: a thousand malformed entries first must not crowd
        // out the valid ones behind them.
        let mut entries: Vec<serde_json::Value> =
            (0..1005).map(|_| serde_json::json!({})).collect();
        entries.push(serde_json::json!({ "name": "goal", "source": "extension" }));
        entries.push(serde_json::json!({ "name": "" }));
        entries.push(serde_json::json!({ "name": "skill:pdf", "source": "skill" }));
        let reply = parse(
            serde_json::json!({
                "id": "c-9",
                "type": "response",
                "command": "get_commands",
                "success": true,
                "data": { "commands": entries },
            })
            .to_string()
            .as_str(),
        );
        match events_from_line(&reply).as_slice() {
            [SessionEvent::AvailableCommands { commands }] => {
                let listed = commands
                    .iter()
                    .map(|command| command.name.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(listed, ["compact", "autocompact", "goal", "skill:pdf"]);
            }
            other => panic!("expected one command list, got {other:?}"),
        }
    }

    #[test]
    fn entries_past_the_inspection_bound_are_never_parsed() {
        // The reader-thread bound: inspection stops after ten thousand
        // entries even when nothing was accepted — the valid entry behind
        // the flood is not listed.
        let mut entries: Vec<serde_json::Value> =
            (0..10_005).map(|_| serde_json::json!({})).collect();
        entries.push(serde_json::json!({ "name": "goal", "source": "extension" }));
        let reply = parse(
            serde_json::json!({
                "id": "c-9",
                "type": "response",
                "command": "get_commands",
                "success": true,
                "data": { "commands": entries },
            })
            .to_string()
            .as_str(),
        );
        match events_from_line(&reply).as_slice() {
            [SessionEvent::AvailableCommands { commands }] => {
                let listed = commands
                    .iter()
                    .map(|command| command.name.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(listed, ["compact", "autocompact"]);
            }
            other => panic!("expected one command list, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_description_stays_empty_the_way_paseos_nullish_fallback_leaves_it() {
        // Paseo falls back to `source` only for nullish descriptions
        // (`pi/agent.ts:145` `description ?? source`): `""` is a description
        // and stays (review A5-2 #6).
        let reply = parse(
            r#"{"id":"c-6","type":"response","command":"get_commands","success":true,"data":{"commands":[{"name":"blank","description":"","source":"skill"}]}}"#,
        );
        match events_from_line(&reply).as_slice() {
            [SessionEvent::AvailableCommands { commands }] => {
                let blank = commands
                    .iter()
                    .find(|command| command.name == "blank")
                    .expect("the entry is listed");
                assert_eq!(
                    blank.description, "",
                    "an empty description is not the source"
                );
            }
            other => panic!("expected one command list, got {other:?}"),
        }
    }
}
