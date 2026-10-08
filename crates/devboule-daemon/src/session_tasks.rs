//! Derive a session's background-task list from facts the daemon already holds.
//!
//! No stored state: the caller hands in the registry's live children and the
//! session's journal rows, and gets the ordered list back. The fold is keyed
//! upserts throughout, so applying a row twice (a replay that already holds
//! the triggering event) answers the same list.

use std::collections::{HashMap, HashSet};

use devboule_protocol::{
    AgentTaskState, SessionEvent, SessionState, SessionTask, SessionTaskKind, SessionTaskState,
    SubagentTaskStatus,
};

/// A task row names its command; it does not print it. Cut at a char
/// boundary with an ellipsis, so a cut flag reads as cut.
pub const TASK_TITLE_MAX_CHARS: usize = 120;

/// One child the registry still holds, with the display facts a task row needs.
pub struct HeldChild {
    pub id: String,
    pub title: String,
    pub state: SessionState,
    pub started_at_ms: u64,
    pub model: Option<String>,
    pub tool_call_count: Option<u64>,
}

/// The ordered task list: agents first, then commands, each block by
/// start time with the id breaking ties.
pub fn derive_tasks(
    session_id: &str,
    held: &[HeldChild],
    journal: &[(SessionEvent, Option<u64>)],
) -> Vec<SessionTask> {
    let mut fold = Fold::new(session_id);
    for child in held {
        fold.adopt_held(child);
    }
    for (event, ts) in journal {
        fold.apply(event, ts.unwrap_or(0));
    }
    fold.finish()
}

pub fn truncate_title(title: &str) -> String {
    let kept: String = title.chars().take(TASK_TITLE_MAX_CHARS).collect();
    if kept.len() < title.len() {
        format!("{kept}…")
    } else {
        kept
    }
}

fn held_state(state: &SessionState) -> SessionTaskState {
    match state {
        SessionState::Live { .. } | SessionState::Silent { .. } => SessionTaskState::Running,
        SessionState::Ended { code: Some(0), .. } => SessionTaskState::Finished,
        SessionState::Ended { .. } => SessionTaskState::Failed,
        // The roster reads a lost transcript the same way: it never
        // completed, and nothing says it failed.
        SessionState::Recovered { .. } => SessionTaskState::Cancelled,
    }
}

fn finish_state(state: AgentTaskState) -> Option<SessionTaskState> {
    match state {
        AgentTaskState::Completed => Some(SessionTaskState::Finished),
        AgentTaskState::Failed => Some(SessionTaskState::Failed),
        AgentTaskState::Canceled => Some(SessionTaskState::Cancelled),
        _ => None,
    }
}

fn notification_state(status: SubagentTaskStatus) -> SessionTaskState {
    match status {
        SubagentTaskStatus::Completed => SessionTaskState::Finished,
        SubagentTaskStatus::Failed => SessionTaskState::Failed,
        SubagentTaskStatus::Stopped => SessionTaskState::Cancelled,
    }
}

fn update_state(status: Option<&str>) -> Option<SessionTaskState> {
    match status {
        Some("completed") => Some(SessionTaskState::Finished),
        Some("failed") => Some(SessionTaskState::Failed),
        Some("cancelled") | Some("canceled") | Some("interrupted") | Some("stopped") => {
            Some(SessionTaskState::Cancelled)
        }
        _ => None,
    }
}

struct Fold<'a> {
    session_id: &'a str,
    agents: Vec<SessionTask>,
    agent_at: HashMap<String, usize>,
    commands: Vec<SessionTask>,
    command_at: HashMap<String, usize>,
    /// Ids the background set vouched for and no other frame has claimed
    /// since: only these may leave when the next set no longer names them.
    /// A task with its own lifecycle (a start, a notification, a tool call)
    /// outlives the set that happened to name it.
    set_vouched: HashSet<String>,
}

impl<'a> Fold<'a> {
    fn new(session_id: &'a str) -> Self {
        Self {
            session_id,
            agents: Vec::new(),
            agent_at: HashMap::new(),
            commands: Vec::new(),
            command_at: HashMap::new(),
            set_vouched: std::collections::HashSet::new(),
        }
    }

    fn adopt_held(&mut self, child: &HeldChild) {
        let entry = SessionTask {
            id: child.id.clone(),
            kind: SessionTaskKind::Agent,
            title: truncate_title(&child.title),
            state: held_state(&child.state),
            session_id: self.session_id.to_string(),
            child_session_id: Some(child.id.clone()),
            started_at_ms: child.started_at_ms,
            ended_at_ms: None,
            model: child.model.clone(),
            tool_call_count: child.tool_call_count,
        };
        self.upsert_agent(entry);
    }

    fn upsert_agent(&mut self, task: SessionTask) {
        if let Some(&index) = self.agent_at.get(&task.id) {
            self.agents[index] = task;
        } else {
            self.agent_at.insert(task.id.clone(), self.agents.len());
            self.agents.push(task);
        }
    }

    fn upsert_command(&mut self, task: SessionTask) {
        if let Some(&index) = self.command_at.get(&task.id) {
            self.commands[index] = task;
        } else {
            self.command_at.insert(task.id.clone(), self.commands.len());
            self.commands.push(task);
        }
    }

    fn agent_mut(&mut self, id: &str) -> Option<&mut SessionTask> {
        self.agent_at.get(id).copied().map(|i| &mut self.agents[i])
    }

    fn ended(&self, id: &str) -> bool {
        let agent = self.agent_at.get(id).copied().map(|i| &self.agents[i]);
        let command = self.command_at.get(id).copied().map(|i| &self.commands[i]);
        agent.or(command).is_some_and(|e| e.ended_at_ms.is_some())
    }

    fn command_mut(&mut self, id: &str) -> Option<&mut SessionTask> {
        self.command_at
            .get(id)
            .copied()
            .map(|i| &mut self.commands[i])
    }

    fn apply(&mut self, event: &SessionEvent, ts: u64) {
        match event {
            SessionEvent::AgentCreated {
                child_session_id,
                display_name,
                ..
            } => {
                // The journal's birth record: only a child the registry no
                // longer holds (closed, or a row from before the run) is
                // born here — a held child keeps the record's `created_at`.
                if self.agent_mut(child_session_id).is_none() {
                    self.upsert_agent(SessionTask {
                        id: child_session_id.clone(),
                        kind: SessionTaskKind::Agent,
                        title: truncate_title(display_name),
                        state: SessionTaskState::Running,
                        session_id: self.session_id.to_string(),
                        child_session_id: Some(child_session_id.clone()),
                        started_at_ms: ts,
                        ended_at_ms: None,
                        model: None,
                        tool_call_count: None,
                    });
                }
            }
            SessionEvent::ChildFinished {
                child_session_id,
                display_name,
                state,
                ..
            } => {
                // The finish verdict outranks the registry's exit-code read:
                // a stop the human asked for is cancelled, not failed.
                let Some(end) = finish_state(*state) else {
                    return;
                };
                if let Some(entry) = self.agent_mut(child_session_id) {
                    entry.state = end;
                    entry.ended_at_ms = Some(ts);
                    self.set_vouched.remove(child_session_id);
                } else {
                    self.upsert_agent(SessionTask {
                        id: child_session_id.clone(),
                        kind: SessionTaskKind::Agent,
                        title: truncate_title(display_name),
                        state: end,
                        session_id: self.session_id.to_string(),
                        child_session_id: Some(child_session_id.clone()),
                        started_at_ms: ts,
                        ended_at_ms: Some(ts),
                        model: None,
                        tool_call_count: None,
                    });
                }
            }
            SessionEvent::AgentTaskStarted {
                task_id,
                title,
                subagent_type,
                ..
            } => {
                let title = title
                    .as_deref()
                    .or(subagent_type.as_deref())
                    .unwrap_or(task_id);
                self.set_vouched.remove(task_id);
                if self.agent_mut(task_id).is_none() {
                    self.upsert_agent(SessionTask {
                        id: task_id.clone(),
                        kind: SessionTaskKind::Agent,
                        title: truncate_title(title),
                        state: SessionTaskState::Running,
                        session_id: self.session_id.to_string(),
                        child_session_id: None,
                        started_at_ms: ts,
                        ended_at_ms: None,
                        model: None,
                        tool_call_count: None,
                    });
                }
            }
            SessionEvent::AgentTaskNotification {
                task_id, status, ..
            } => {
                let end = notification_state(*status);
                self.set_vouched.remove(task_id);
                if let Some(entry) = self.agent_mut(task_id) {
                    entry.state = end;
                    entry.ended_at_ms = Some(ts);
                }
            }
            SessionEvent::AgentBackgroundTasksChanged { tasks } => {
                for task in tasks {
                    let kind = if task.task_type == "local_agent" {
                        SessionTaskKind::Agent
                    } else {
                        SessionTaskKind::Command
                    };
                    let entry = SessionTask {
                        id: task.task_id.clone(),
                        kind,
                        title: truncate_title(&task.title),
                        state: SessionTaskState::Running,
                        session_id: self.session_id.to_string(),
                        child_session_id: None,
                        started_at_ms: ts,
                        ended_at_ms: None,
                        model: None,
                        tool_call_count: None,
                    };
                    match kind {
                        SessionTaskKind::Agent => {
                            if self.agent_mut(&task.task_id).is_none() {
                                self.set_vouched.insert(task.task_id.clone());
                                self.upsert_agent(entry);
                            }
                        }
                        SessionTaskKind::Command => {
                            if self.command_mut(&task.task_id).is_none() {
                                self.set_vouched.insert(task.task_id.clone());
                                self.upsert_command(entry);
                            }
                        }
                    }
                }
                // Replacement state, and only for what the set vouched for:
                // a vouched entry the set no longer names has left the
                // background without a verdict, so it leaves the list —
                // unless it already ended, which no set rewrites.
                let present: HashSet<&str> = tasks.iter().map(|t| t.task_id.as_str()).collect();
                let gone: Vec<String> = self
                    .set_vouched
                    .iter()
                    .filter(|id| !present.contains(id.as_str()))
                    .cloned()
                    .collect();
                for id in gone {
                    if self.ended(&id) {
                        continue;
                    }
                    if let Some(index) = self.agent_at.remove(&id) {
                        self.agents.remove(index);
                        self.agent_at = reindex(&self.agents);
                    }
                    if let Some(index) = self.command_at.remove(&id) {
                        self.commands.remove(index);
                        self.command_at = reindex(&self.commands);
                    }
                    self.set_vouched.remove(&id);
                }
            }
            SessionEvent::AgentToolCall {
                tool_call_id,
                title,
                command,
                background,
                ..
            } => {
                if *background != Some(true) {
                    return;
                }
                if self.command_mut(tool_call_id).is_some() {
                    return;
                }
                self.set_vouched.remove(tool_call_id);
                let line = command.as_deref().unwrap_or(title);
                self.upsert_command(SessionTask {
                    id: tool_call_id.clone(),
                    kind: SessionTaskKind::Command,
                    title: truncate_title(line),
                    state: SessionTaskState::Running,
                    session_id: self.session_id.to_string(),
                    child_session_id: None,
                    started_at_ms: ts,
                    ended_at_ms: None,
                    model: None,
                    tool_call_count: None,
                });
            }
            SessionEvent::AgentToolUpdate {
                tool_call_id,
                status,
                ..
            } => {
                let Some(end) = update_state(status.as_deref()) else {
                    return;
                };
                self.set_vouched.remove(tool_call_id);
                if let Some(entry) = self.command_mut(tool_call_id) {
                    entry.state = end;
                    entry.ended_at_ms = Some(ts);
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<SessionTask> {
        sort(&mut self.agents);
        sort(&mut self.commands);
        self.agents.into_iter().chain(self.commands).collect()
    }
}

fn reindex(tasks: &[SessionTask]) -> HashMap<String, usize> {
    tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id.clone(), i))
        .collect()
}

fn sort(tasks: &mut [SessionTask]) {
    tasks.sort_by(|a, b| (a.started_at_ms, &a.id).cmp(&(b.started_at_ms, &b.id)));
}

#[cfg(test)]
#[path = "session_tasks_tests.rs"]
mod tests;
