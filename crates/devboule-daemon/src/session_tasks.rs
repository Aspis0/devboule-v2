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

use crate::process_argv_redact::redact_argv;

/// A task row names its command; it does not print it. Cut at a char
/// boundary with an ellipsis, so a cut flag reads as cut.
pub const TASK_TITLE_MAX_CHARS: usize = 120;
/// The list is a glance, not history: past this many rows the oldest
/// finished rows leave first. Running rows are never dropped for the cap.
pub const TASKS_MAX_ROWS: usize = 200;

/// One child the registry still holds, with the display facts a task row needs.
pub struct HeldChild {
    pub id: String,
    pub title: String,
    pub state: SessionState,
    pub started_at_ms: u64,
    /// The registry's end time for a terminal child, `None` while running.
    pub ended_at_ms: Option<u64>,
    pub model: Option<String>,
    pub tool_call_count: Option<u64>,
}

/// One journal row in fold order: replay rows carry their journal position
/// and sort by it, unpositioned rows (the just-published trigger, whose
/// sequence the publisher never learns) sort after every positioned one.
pub struct TaskRow {
    pub event: SessionEvent,
    pub ts_ms: Option<u64>,
    pub pos: Option<(u64, u64)>,
}

/// The ordered task list: agents first, then commands, each block by
/// start time with the id breaking ties.
///
/// `parent_end` is the parent session's end time when it has ended: every
/// row still running becomes cancelled at it. A background command outlives
/// a provider turn, so only the session's own end cancels it.
pub fn derive_tasks(
    session_id: &str,
    held: &[HeldChild],
    rows: &[TaskRow],
    parent_end: Option<u64>,
) -> Vec<SessionTask> {
    let mut ordered: Vec<&TaskRow> = rows.iter().collect();
    ordered.sort_by_key(|row| match row.pos {
        Some((generation, seq)) => (0, generation, seq),
        None => (1, 0, 0),
    });
    let mut fold = Fold::new(session_id);
    for child in held {
        fold.adopt_held(child);
    }
    for row in ordered {
        fold.apply(&row.event, row.ts_ms.unwrap_or(0));
    }
    if let Some(end) = parent_end {
        fold.cancel_running(end);
    }
    fold.finish()
}

/// A background row's title — a command line or a provider description —
/// with credentials masked, then cut to a row. The
/// two-word credential pass runs on the original string first — splitting
/// would destroy the `Bearer SHORT` shape — then the line is split the way
/// the process probe splits one, with shell quotes off, through the argv
/// redactor. The row is a redacted display form, not the exact line.
pub fn command_title(line: &str) -> String {
    let masked = crate::process_argv_redact::mask_scheme_credentials(line);
    let bare: String = masked.replace(['\'', '"'], "");
    let argv: Vec<String> = bare.split_whitespace().map(str::to_string).collect();
    truncate_title(&redact_argv(&argv).join(" "))
}

pub fn truncate_title(title: &str) -> String {
    let kept: String = title.chars().take(TASK_TITLE_MAX_CHARS).collect();
    if kept.len() < title.len() {
        format!("{kept}…")
    } else {
        kept
    }
}

/// A child's declared model and counted tool calls from its own journal
/// rows. The count is exact only for a complete replay: anything else
/// answers `None`, never a partial number.
pub fn summarize_child(
    events: &[SessionEvent],
    replay_complete: bool,
) -> (Option<String>, Option<u64>) {
    let mut model = None;
    let mut calls = 0u64;
    for event in events {
        match event {
            SessionEvent::SessionManifest {
                current_model_id: Some(id),
                ..
            } => model = Some(id.clone()),
            SessionEvent::AgentToolCall { .. } => calls += 1,
            _ => {}
        }
    }
    (model, replay_complete.then_some(calls))
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

/// Whether a tool result ends a backgrounded call: only a failed launch.
/// Success is the launch acknowledgement; the command runs on until the
/// provider's task notification or the session's end.
pub(crate) fn background_launch_failed(status: Option<&str>) -> bool {
    status == Some("failed")
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
            set_vouched: HashSet::new(),
        }
    }

    fn adopt_held(&mut self, child: &HeldChild) {
        let state = held_state(&child.state);
        let ended_at_ms = match state {
            SessionTaskState::Running => None,
            _ => child.ended_at_ms,
        };
        let entry = SessionTask {
            id: child.id.clone(),
            kind: SessionTaskKind::Agent,
            title: truncate_title(&child.title),
            state,
            session_id: self.session_id.to_string(),
            child_session_id: Some(child.id.clone()),
            started_at_ms: child.started_at_ms,
            ended_at_ms,
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

    /// End one command row from the provider's task notification: the
    /// notification names the originating tool call, the row's own id.
    /// Terminal rows are final, except the finish verdict below.
    fn finish_command(&mut self, id: &str, end: SessionTaskState, ts: u64) -> bool {
        let Some(entry) = self.command_mut(id) else {
            return false;
        };
        if entry.ended_at_ms.is_some() {
            return true;
        }
        entry.state = end;
        entry.ended_at_ms = Some(ts);
        true
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
                // The finish verdict outranks every other terminal read: a
                // stop the human asked for is cancelled, not failed.
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
                is_backgrounded,
                ..
            } => {
                // An explicit foreground task is not background work. An
                // absent flag is legacy: rows written before the field
                // existed read as background, the way they always did.
                if *is_backgrounded == Some(false) {
                    return;
                }
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
                task_id,
                tool_use_id,
                status,
                ..
            } => {
                let end = notification_state(*status);
                self.set_vouched.remove(task_id);
                if let Some(id) = tool_use_id {
                    self.set_vouched.remove(id);
                }
                if let Some(entry) = self.agent_mut(task_id) {
                    if entry.ended_at_ms.is_none() {
                        entry.state = end;
                        entry.ended_at_ms = Some(ts);
                    }
                } else {
                    // A background shell's completion names the tool call
                    // that launched it, by tool id or by task id.
                    let matched = tool_use_id
                        .as_deref()
                        .map(|id| self.finish_command(id, end, ts))
                        .unwrap_or(false);
                    if !matched {
                        self.finish_command(task_id, end, ts);
                    }
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
                        title: command_title(&task.title),
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
                    title: command_title(line),
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
                let Some(index) = self.command_at.get(tool_call_id).copied() else {
                    return;
                };
                if self.commands[index].ended_at_ms.is_some() {
                    return;
                }
                // Only a failed launch ends the row here: success is the
                // launch acknowledgement, and the command runs on.
                if !background_launch_failed(status.as_deref()) {
                    return;
                }
                self.set_vouched.remove(tool_call_id);
                let entry = &mut self.commands[index];
                entry.state = SessionTaskState::Failed;
                entry.ended_at_ms = Some(ts);
            }
            _ => {}
        }
    }

    fn cancel_running(&mut self, end: u64) {
        for entry in self.agents.iter_mut().chain(self.commands.iter_mut()) {
            if entry.ended_at_ms.is_none() {
                entry.state = SessionTaskState::Cancelled;
                entry.ended_at_ms = Some(end);
            }
        }
    }

    fn finish(mut self) -> Vec<SessionTask> {
        sort(&mut self.agents);
        sort(&mut self.commands);
        self.agents.into_iter().chain(self.commands).collect()
    }
}

/// Cap rows for one publish: running first, newest first, the rest counted.
/// Newest is by start time with the id breaking ties, so the choice is
/// deterministic.
pub fn cap_published(mut rows: Vec<SessionTask>) -> (Vec<SessionTask>, u32) {
    rows.sort_by(|a, b| {
        let live =
            (a.state == SessionTaskState::Running).cmp(&(b.state == SessionTaskState::Running));
        live.reverse()
            .then_with(|| (b.started_at_ms, &b.id).cmp(&(a.started_at_ms, &a.id)))
    });
    let omitted = rows.len().saturating_sub(TASKS_MAX_ROWS) as u32;
    rows.truncate(TASKS_MAX_ROWS);
    (rows, omitted)
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
