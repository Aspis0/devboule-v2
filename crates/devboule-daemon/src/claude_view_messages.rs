//! Assistant and user envelope mapping: text and thought blocks, tool
//! calls and results, the model-change manifest, and the question echo
//! latch; plus the consolidated-envelope remainder match
//! (`block_remainder`) and the local-command stdout wrapper parsing.

use std::time::Instant;

use devboule_protocol::{NoticeSeverity, SessionEvent, SessionModel};
use serde_json::Value;

use super::tasks::{parent_tool_use_id, spawn_depth};
use super::tools::{tool_call_from_block, tool_update_from_result};
use super::ClaudeView;

impl ClaudeView {
    pub(super) fn ingest_assistant(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let message = match envelope.get("message") {
            Some(message) => message,
            None => return Vec::new(),
        };
        let is_subagent = envelope
            .get("parent_tool_use_id")
            .is_some_and(|value| !value.is_null());
        let parent_tool_use_id = parent_tool_use_id(envelope);
        let spawn_depth = spawn_depth(envelope);
        let model = if is_subagent {
            None
        } else {
            message.get("model").and_then(Value::as_str)
        };
        let model_changed = model.is_some_and(|model| {
            !model.is_empty()
                && self.last_manifest_model.is_some()
                && self.last_manifest_model.as_deref() != Some(model)
        });
        self.note_message(
            parent_tool_use_id.as_deref(),
            message.get("id").and_then(Value::as_str),
            if is_subagent { None } else { model },
        );
        let mut events = Vec::new();
        // A subagent's task Tools feed the child's checklist, not this
        // session's. The checklist reads its own rule here (a non-empty
        // string parent id): a non-string parent id feeds the checklist while
        // the view routes the frame to the child transcript; nothing produces
        // one.
        if parent_tool_use_id.is_none() {
            if let Some(event) = self.task_state.observe(envelope) {
                events.push(event);
            }
        }
        if model_changed {
            self.last_manifest_model = self.current_model.clone();
            if let Some(model) = self.current_model.clone() {
                events.push(SessionEvent::SessionManifest {
                    provider_id: Some("claude".to_string()),
                    current_model_id: Some(model.clone()),
                    models: vec![SessionModel {
                        accepts_images: true,
                        provider_id: None,
                        model_id: model.clone(),
                        name: model,
                        description: None,
                        context_tokens: None,
                        current_effort: None,
                        efforts: None,
                    }],
                    modes: self.mode_state(),
                    current_model_provider_id: None,
                });
            }
        }
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            return events;
        };
        for block in content.iter() {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    if let Some(text) =
                        self.block_remainder(parent_tool_use_id.as_deref(), "text", text)
                    {
                        events.push(SessionEvent::AgentMessage {
                            message_id: self.current_message_id(parent_tool_use_id.as_deref()),
                            text,
                            parent_tool_use_id: parent_tool_use_id.clone(),
                            spawn_depth,

                            images: Vec::new(),
                        });
                    }
                }
                Some("thinking") => {
                    let text = block.get("thinking").and_then(Value::as_str).unwrap_or("");
                    if let Some(text) =
                        self.block_remainder(parent_tool_use_id.as_deref(), "thinking", text)
                    {
                        events.push(SessionEvent::AgentThought {
                            message_id: self.current_message_id(parent_tool_use_id.as_deref()),
                            text,
                            parent_tool_use_id: parent_tool_use_id.clone(),
                            spawn_depth,
                        });
                    }
                }
                Some("tool_use") => {
                    let is_question =
                        block.get("name").and_then(Value::as_str) == Some("AskUserQuestion");
                    let is_plan = block.get("name").and_then(Value::as_str) == Some("ExitPlanMode");
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        // Every call counts, carded or sidechain inner: the
                        // stamp is the latest start, so the grace always runs
                        // from the most recent call still unanswered.
                        self.open_tools.insert(id.to_string());
                        self.last_tool_start = Some(Instant::now());
                        if is_question {
                            self.question_tool_ids.insert(id.to_string());
                        }
                        if is_plan {
                            self.plan_tool_ids.insert(id.to_string());
                        }
                    }
                    if let Some(event) = tool_call_from_block(
                        block,
                        self.cwd.as_deref(),
                        parent_tool_use_id.clone(),
                        spawn_depth,
                    ) {
                        events.push(event);
                    }
                    if is_plan {
                        if let Some(tool_call_id) = block.get("id").and_then(Value::as_str) {
                            let plan = block
                                .pointer("/input/plan")
                                .and_then(Value::as_str)
                                .filter(|plan| !plan.is_empty())
                                .map(str::to_string)
                                .unwrap_or_else(|| "No plan text was provided.".to_string());
                            events.push(SessionEvent::AgentToolUpdate {
                                tool_call_id: tool_call_id.to_string(),
                                status: None,
                                text: Some(plan),
                                title: None,
                                kind: Some("plan".to_string()),
                                locations: None,
                                parent_tool_use_id: parent_tool_use_id.clone(),
                                spawn_depth,
                                command: None,
                                exit_code: None,
                                replace: false,

                                images: Vec::new(),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        events
    }

    pub(super) fn ingest_user(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let parent_tool_use_id = parent_tool_use_id(envelope);
        let spawn_depth = spawn_depth(envelope);
        let mut events = Vec::new();
        // Sidechain frames do not feed this session's checklist.
        if parent_tool_use_id.is_none() {
            if let Some(event) = self.task_state.observe(envelope) {
                events.push(event);
            }
        }
        let Some(content) = envelope
            .get("message")
            .and_then(|message| message.get("content"))
        else {
            return events;
        };
        // The string form carries the whole content as one text: captured
        // history entries spell `<local-command-stdout>` this way, so the
        // wrapper scan accepts both shapes.
        if let Some(text) = content.as_str() {
            if let Some(inner) = command_stdout_inner(text) {
                events.push(SessionEvent::SessionNotice {
                    text: inner,
                    severity: NoticeSeverity::Info,
                });
            }
            return events;
        }
        let Some(content) = content.as_array() else {
            return events;
        };
        // A local slash command's answer rides the command's own user
        // envelope, wrapped in the CLI's display tags; the inner text is the
        // transcript's notice, never the wrapper.
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                    self.open_tools.remove(id);
                }
                continue;
            }
            if let Some(text) = local_command_stdout_text(block) {
                events.push(SessionEvent::SessionNotice {
                    text,
                    severity: NoticeSeverity::Info,
                });
            }
        }
        content
            .iter()
            .filter_map(|block| {
                let mut update =
                    tool_update_from_result(block, parent_tool_use_id.clone(), spawn_depth)?;
                let question = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| self.question_tool_ids.remove(id));
                let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
                // A granted question's merged row already reads Question /
                // Answer; Claude's echo would append the answer a second time.
                if question && !failed {
                    if let SessionEvent::AgentToolUpdate { text, .. } = &mut update {
                        *text = None;
                    }
                }
                if matches!(&update, SessionEvent::AgentToolUpdate { tool_call_id, .. } if self.plan_tool_ids.contains(tool_call_id)) {
                    return None;
                }
                Some(update)
            })
            .for_each(|update| events.push(update));
        events
    }

    /// The part of one text-like block of a final `assistant` envelope the
    /// stream has not emitted yet. The envelope block's stream identity is
    /// the same-kind unconsumed stream block whose accumulated streamed text
    /// is the longest prefix of the envelope text — `content` array position
    /// is not the block's identity, and the envelope may carry one block or
    /// all of them.
    ///
    /// A match consumes its stream block, so one stream block satisfies at
    /// most one envelope block: two envelope blocks carrying the same text
    /// cannot both collapse onto the one streamed block. The unavoidable
    /// mirror cost is that a re-delivered confirmation of an already
    /// consumed block emits again — the two shapes are the same bytes to a
    /// text matcher, and the dropped side is the one nothing notices. A
    /// non-prefix envelope no longer than the one unconsumed block left is
    /// that block after a provider rewrite; the deltas already showed at
    /// least that much, so it confirms rather than repeats. With more than
    /// one candidate left the rewritten one is not identifiable and the
    /// envelope's text is emitted rather than guessed away.
    fn block_remainder(
        &mut self,
        parent_tool_use_id: Option<&str>,
        kind: &str,
        full: &str,
    ) -> Option<String> {
        if full.is_empty() {
            return None;
        }
        let prefix_match = self
            .streamed
            .iter_mut()
            .filter(|((stream, _), block)| {
                stream.as_deref() == parent_tool_use_id
                    && !block.consumed
                    && block.kind.as_deref() == Some(kind)
                    && full.starts_with(block.text.as_str())
            })
            .max_by_key(|((_, _), block)| block.text.len());
        if let Some((_, block)) = prefix_match {
            let streamed_len = block.text.len();
            block.consumed = true;
            let emit = full[streamed_len..].to_string();
            return (!emit.is_empty()).then_some(emit);
        }
        let mut unconsumed = self.streamed.iter_mut().filter(|((stream, _), block)| {
            stream.as_deref() == parent_tool_use_id
                && !block.consumed
                && block.kind.as_deref() == Some(kind)
        });
        match (unconsumed.next(), unconsumed.next()) {
            (Some((_, only)), None) if only.text.len() >= full.len() => {
                only.consumed = true;
                None
            }
            _ => Some(full.to_string()),
        }
    }
}

/// The inner text of a `<local-command-stdout>` wrapper, trimmed. None
/// for a wrapper that carries nothing.
fn command_stdout_inner(text: &str) -> Option<String> {
    const OPEN: &str = "<local-command-stdout>";
    const CLOSE: &str = "</local-command-stdout>";
    let inner = text.trim().strip_prefix(OPEN)?.strip_suffix(CLOSE)?.trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

/// The inner text of a `<local-command-stdout>` wrapper block, trimmed. None
/// for any other block, and for a wrapper that carries nothing.
fn local_command_stdout_text(block: &Value) -> Option<String> {
    if block.get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    command_stdout_inner(block.get("text").and_then(Value::as_str)?)
}

#[cfg(test)]
#[path = "claude_view_messages_tests.rs"]
mod tests;
