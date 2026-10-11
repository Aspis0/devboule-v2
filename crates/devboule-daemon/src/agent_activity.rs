//! Derived activity for daemon-spawned agent sessions, plus the quiet rule.
//!
//! Hook reports (`agent_report.rs`) arrive over a named pipe and keep their
//! own per-source `seq` discipline. This file never touches that map: it
//! derives a headline state from facts the daemon already holds (liveness, a
//! running turn, a pending permission card, the last publish time) and keeps
//! a bounded in-memory ring of recent event kinds. The tool reads both; the
//! hook's last state rides alongside, never merged, so the two cannot fight.

use std::collections::VecDeque;
use std::time::Duration;

use devboule_protocol::{AgentActivityState, SessionEvent, ToolLocation};

/// Recent lines returned when the caller names no limit. Ten kinds fit one
/// glance and already separate working from blocked from quiet.
pub const ACTIVITY_DEFAULT_LIMIT: usize = 10;
/// Hard cap for one activity answer. Fifty short kind strings plus timestamps
/// stay a few kilobytes; the ring holds 64, so the cap never scans past it.
pub const ACTIVITY_MAX_LIMIT: usize = 50;
/// How many recent marks one runtime keeps. Above the hard cap, at the pending
/// queue's own frame budget, so the feed costs fixed memory and no I/O.
pub const ACTIVITY_FEED_CAP: usize = 64;
/// Quiet means Working, not blocked, with no publish for this long. Twenty
/// minutes is four times the 5-minute silence mark, past a long build or a
/// hard think (10-15 min), inside one work session. A thinking model, a long
/// build and a wedged process look identical from outside, which is why the
/// quiet path notifies and never acts.
pub const CHILD_QUIET_AFTER: Duration = Duration::from_secs(20 * 60);
/// How long Codex may report its root thread idle with a turn still open
/// before the sweep ends that turn: its completion is due at once, so a short
/// grace covers the frames in between.
pub const CODEX_IDLE_GRACE: Duration = Duration::from_secs(30);
/// How often the daemon sweeps children for quiet. A minute divides the
/// threshold twenty times, so the worst notice lands at 21 minutes.
pub const QUIET_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// One recent mark: metadata only, never transcript text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivityMark {
    pub seq: Option<u64>,
    pub kind: &'static str,
    pub ts_ms: u64,
}

/// Headline state derived from daemon facts only. `Unknown` is a value: no
/// live agent to have activity (ended, recovered, or never published).
pub fn derive_activity(is_live: bool, turn_active: bool, pending: bool) -> AgentActivityState {
    if !is_live {
        return AgentActivityState::Unknown;
    }
    if pending {
        return AgentActivityState::Blocked;
    }
    if turn_active {
        return AgentActivityState::Working;
    }
    AgentActivityState::Idle
}

/// Whether a quiet notice is owed. Pure so tests drive it without sleeping:
/// `idle` is now minus the child's last publish, `already_notified` is the
/// once-per-spell latch the registry holds.
pub fn quiet_due(
    is_live: bool,
    turn_active: bool,
    pending: bool,
    idle: Duration,
    already_notified: bool,
) -> bool {
    if !is_live || !turn_active || pending || already_notified {
        return false;
    }
    idle >= CHILD_QUIET_AFTER
}

/// Clamp a caller-supplied limit: missing means the default, explicit zero
/// means state only, anything above the cap means the cap. The fallible
/// conversion keeps 32-bit targets honest: a `u64` that cannot fit is the
/// cap, never a truncation to an empty list.
pub fn clamp_limit(requested: Option<u64>) -> usize {
    match requested {
        None => ACTIVITY_DEFAULT_LIMIT,
        Some(n) => usize::try_from(n)
            .unwrap_or(ACTIVITY_MAX_LIMIT)
            .min(ACTIVITY_MAX_LIMIT),
    }
}

/// Wall-clock millis for one feed mark.
pub fn wall_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Short kind string for one event. Spellings match the transcript mapping so
/// a supervisor sees one vocabulary everywhere. No payload text leaves here.
pub fn event_kind(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::Output { .. } => "output",
        SessionEvent::SessionNotice { .. } => "session_notice",
        SessionEvent::AgentMessage { .. } => "agent_message",
        SessionEvent::AgentUserMessage { .. } => "agent_user_message",
        SessionEvent::Steered { .. } => "steered",
        SessionEvent::AgentThought { .. } => "agent_thought",
        SessionEvent::AvailableCommands { .. } => "available_commands",
        SessionEvent::AgentToolCall { .. } => "agent_tool_call",
        SessionEvent::AgentToolUpdate { .. } => "agent_tool_update",
        SessionEvent::AgentFinished { .. } => "agent_finished",
        SessionEvent::ContextUsage { .. } => "context_usage",
        SessionEvent::PlanUsage { .. } => "plan_usage",
        SessionEvent::AgentCreated { .. } => "agent_created",
        SessionEvent::AgentResumed { .. } => "agent_resumed",
        SessionEvent::ChildFinished { .. } => "child_finished",
        SessionEvent::AgentTaskStarted { .. } => "agent_task_started",
        SessionEvent::AgentTaskNotification { .. } => "agent_task_notification",
        SessionEvent::AgentBackgroundTasksChanged { .. } => "agent_background_tasks_changed",
        SessionEvent::TasksSnapshot { .. } => "tasks_snapshot",
        SessionEvent::AgentTasks { .. } => "agent_tasks",
        SessionEvent::GoalChanged { .. } => "goal_changed",
        SessionEvent::AgentError { .. } => "agent_error",
        SessionEvent::AgentStderr { .. } => "agent_stderr",
        SessionEvent::PermissionRequest { .. } => "permission_request",
        SessionEvent::PermissionResolved { .. } => "permission_resolved",
        SessionEvent::PermissionAnswered { .. } => "permission_answered",
        SessionEvent::SessionManifest { .. } => "session_manifest",
        SessionEvent::SessionFeatureState { .. } => "session_feature_state",
        SessionEvent::AgentReported { .. } => "agent_reported",
        SessionEvent::Snapshot { .. } => "snapshot",
        SessionEvent::Exit { .. } => "exit",
        SessionEvent::Recovered { .. } => "recovered",
        SessionEvent::Detached => "detached",
        SessionEvent::Silent { .. } => "silent",
        SessionEvent::JournalDegraded { .. } => "journal_degraded",
        SessionEvent::SessionsSnapshot { .. } => "sessions_snapshot",
        SessionEvent::QueueSnapshot { .. } => "queue_snapshot",
    }
}

/// Characters kept of one message's text; the rest is cut and marked with `…`.
pub const DIGEST_TEXT_CAP: usize = 2_000;
/// Distinct tool calls the digest keeps; the oldest goes first.
pub const DIGEST_TOOL_CAP: usize = 256;
/// Distinct files the digest keeps; the first seen stay.
pub const DIGEST_FILE_CAP: usize = 100;
/// Characters kept of one file path; a longer path is cut with `…`.
pub const DIGEST_PATH_CAP: usize = 200;
/// Files one answer lists, at most.
pub const ANSWER_FILE_LIMIT: usize = 20;
/// Characters of message text one answer may carry in all. The newest messages
/// fill it first; a repeated message is listed once.
pub const DIGEST_RESPONSE_CAP: usize = 6_000;

/// `text` cut to `cap` characters, with `…` when it was cut.
fn truncate_chars(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(cap).collect();
    cut.push('…');
    cut
}

/// One agent message as the digest keeps it: the text so far and whether it was cut.
#[derive(Clone, Debug, Default)]
struct DigestMessage {
    /// The message id the provider gave its chunks, when it gave one.
    id: Option<String>,
    text: String,
    cut: bool,
}

impl DigestMessage {
    fn push(&mut self, chunk: &str) {
        if self.cut {
            return;
        }
        let room = DIGEST_TEXT_CAP.saturating_sub(self.text.chars().count());
        if chunk.chars().count() <= room {
            self.text.push_str(chunk);
        } else {
            self.text.extend(chunk.chars().take(room));
            self.cut = true;
        }
    }

    fn rendered(&self) -> String {
        if self.cut {
            format!("{}…", self.text)
        } else {
            self.text.clone()
        }
    }
}

#[derive(Clone, Debug)]
struct ToolRecord {
    id: String,
    status: String,
}

/// The text side of one agent's publishes, beside the metadata feed: its last
/// messages, each tool call's latest status and the files those calls touched.
/// The central publisher feeds it the same events the feed sees, so it shows
/// what the transcript shows. Bounded at every step; nothing here is journaled.
#[derive(Clone, Debug, Default)]
pub struct ActivityDigest {
    messages: VecDeque<DigestMessage>,
    /// Whether the last published event was a message chunk, so the next chunk
    /// continues that message instead of starting one.
    message_open: bool,
    tools: VecDeque<ToolRecord>,
    files: Vec<String>,
}

impl ActivityDigest {
    /// Fold one published event in. Only message chunks, tool calls and their
    /// updates change the digest; everything else ends an open message.
    pub fn record(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::AgentMessage {
                message_id, text, ..
            } => {
                // A chunk continues the open message only when it carries the
                // same id; a different id is a different message even with no
                // event between them.
                let continues = self.message_open
                    && self
                        .messages
                        .back()
                        .is_some_and(|open| open.id.as_deref() == message_id.as_deref());
                if !continues {
                    self.messages.push_back(DigestMessage {
                        id: message_id.clone(),
                        ..DigestMessage::default()
                    });
                    if self.messages.len() > ACTIVITY_MAX_LIMIT {
                        self.messages.pop_front();
                    }
                }
                if let Some(message) = self.messages.back_mut() {
                    message.push(text);
                }
                self.message_open = true;
                return;
            }
            SessionEvent::AgentToolCall {
                tool_call_id,
                status,
                locations,
                ..
            } => {
                self.upsert_tool(tool_call_id, Some(status));
                self.note_files(locations.as_deref());
            }
            SessionEvent::AgentToolUpdate {
                tool_call_id,
                status,
                locations,
                ..
            } => {
                self.upsert_tool(tool_call_id, status.as_deref());
                self.note_files(locations.as_deref());
            }
            _ => {}
        }
        self.message_open = false;
    }

    /// The last `limit` messages, oldest first, each as its text so far. The
    /// newest come first into a budget of [`DIGEST_RESPONSE_CAP`] characters,
    /// and a message the budget cannot hold whole is cut with `…`. A text
    /// listed twice is kept once, at its latest place.
    pub fn last_messages(&self, limit: usize) -> Vec<String> {
        let mut newest_first: Vec<String> = Vec::new();
        let mut budget = DIGEST_RESPONSE_CAP;
        for message in self.messages.iter().rev() {
            if newest_first.len() >= limit {
                break;
            }
            let text = message.rendered();
            if newest_first.contains(&text) {
                continue;
            }
            let length = text.chars().count();
            if length <= budget {
                budget -= length;
                newest_first.push(text);
            } else {
                if budget > 0 {
                    let cut: String = text.chars().take(budget - 1).collect();
                    newest_first.push(format!("{cut}…"));
                }
                break;
            }
        }
        newest_first.reverse();
        newest_first
    }

    /// Tool calls by their latest status: `(done, failed, running)`.
    pub fn tool_counts(&self) -> (usize, usize, usize) {
        self.tools
            .iter()
            .fold((0, 0, 0), |(done, failed, running), tool| {
                match tool.status.as_str() {
                    "completed" => (done + 1, failed, running),
                    "failed" => (done, failed + 1, running),
                    _ => (done, failed, running + 1),
                }
            })
    }

    /// The distinct paths the tool calls touched, first seen first: the first
    /// [`ANSWER_FILE_LIMIT`] of them, which is what one answer lists.
    pub fn answer_files(&self) -> &[String] {
        &self.files[..self.files.len().min(ANSWER_FILE_LIMIT)]
    }

    fn upsert_tool(&mut self, id: &str, status: Option<&str>) {
        match self.tools.iter_mut().find(|tool| tool.id == id) {
            Some(tool) => {
                if let Some(status) = status {
                    tool.status = status.to_string();
                }
            }
            None => {
                self.tools.push_back(ToolRecord {
                    id: id.to_string(),
                    status: status.unwrap_or("pending").to_string(),
                });
                if self.tools.len() > DIGEST_TOOL_CAP {
                    self.tools.pop_front();
                }
            }
        }
    }

    fn note_files(&mut self, locations: Option<&[ToolLocation]>) {
        for location in locations.unwrap_or_default() {
            if self.files.len() >= DIGEST_FILE_CAP {
                return;
            }
            let path = truncate_chars(&location.path, DIGEST_PATH_CAP);
            if !self.files.contains(&path) {
                self.files.push(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_outranks_a_running_turn() {
        assert_eq!(
            derive_activity(true, true, true),
            AgentActivityState::Blocked
        );
    }

    #[test]
    fn working_idle_unknown_read_off_daemon_facts() {
        assert_eq!(
            derive_activity(true, true, false),
            AgentActivityState::Working
        );
        assert_eq!(
            derive_activity(true, false, false),
            AgentActivityState::Idle
        );
        assert_eq!(
            derive_activity(false, true, false),
            AgentActivityState::Unknown
        );
    }

    #[test]
    fn quiet_needs_working_unblocked_and_long_idle() {
        let long = CHILD_QUIET_AFTER + Duration::from_secs(1);
        let short = CHILD_QUIET_AFTER - Duration::from_secs(1);
        assert!(quiet_due(true, true, false, long, false));
        assert!(!quiet_due(true, true, false, short, false));
        assert!(!quiet_due(true, true, true, long, false));
        assert!(!quiet_due(true, false, false, long, false));
        assert!(!quiet_due(false, true, false, long, false));
        assert!(!quiet_due(true, true, false, long, true));
    }

    #[test]
    fn limit_defaults_and_caps() {
        assert_eq!(clamp_limit(None), ACTIVITY_DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 0);
        assert_eq!(clamp_limit(Some(7)), 7);
        assert_eq!(clamp_limit(Some(5000)), ACTIVITY_MAX_LIMIT);
    }

    #[test]
    fn kinds_stay_on_the_transcript_vocabulary() {
        let finished = SessionEvent::AgentFinished {
            at_ms: None,
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        };
        assert_eq!(event_kind(&finished), "agent_finished");
        let reported = SessionEvent::AgentReported {
            seq: 1,
            source: "devboule:stub".to_string(),
            agent: "stub".to_string(),
            state: AgentActivityState::Working,
            message: None,
            report_seq: Some(1),
            agent_session_id: None,
            agent_session_path: None,
            session_start_source: None,
        };
        assert_eq!(event_kind(&reported), "agent_reported");
    }

    fn message(text: &str) -> SessionEvent {
        SessionEvent::AgentMessage {
            message_id: None,
            text: text.to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
            images: Vec::new(),
        }
    }

    fn call(id: &str, status: &str, path: Option<&str>) -> SessionEvent {
        SessionEvent::AgentToolCall {
            tool_call_id: id.to_string(),
            title: format!("tool {id}"),
            status: status.to_string(),
            kind: None,
            locations: path.map(|path| {
                vec![ToolLocation {
                    path: path.to_string(),
                    line: None,
                }]
            }),
            edits: None,
            subagent_type: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
            background: None,
        }
    }

    fn update(id: &str, status: &str, path: Option<&str>) -> SessionEvent {
        SessionEvent::AgentToolUpdate {
            tool_call_id: id.to_string(),
            status: Some(status.to_string()),
            text: None,
            title: None,
            kind: None,
            locations: path.map(|path| {
                vec![ToolLocation {
                    path: path.to_string(),
                    line: None,
                }]
            }),
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
            replace: false,
            images: Vec::new(),
        }
    }

    #[test]
    fn streamed_chunks_of_one_message_read_as_one_message() {
        let mut digest = ActivityDigest::default();
        for chunk in ["Hel", "lo ", "there"] {
            digest.record(&message(chunk));
        }
        assert_eq!(digest.last_messages(10), vec!["Hello there".to_string()]);
    }

    #[test]
    fn a_tool_call_ends_one_message_and_the_next_starts_a_new_one() {
        let mut digest = ActivityDigest::default();
        digest.record(&message("first"));
        digest.record(&message("-part"));
        digest.record(&call("t1", "completed", None));
        digest.record(&message("second"));
        assert_eq!(
            digest.last_messages(10),
            vec!["first-part".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn thoughts_are_not_messages() {
        let mut digest = ActivityDigest::default();
        digest.record(&SessionEvent::AgentThought {
            message_id: None,
            text: "hmm".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        });
        assert!(digest.last_messages(10).is_empty());
    }

    #[test]
    fn the_limit_takes_the_most_recent_messages_and_the_ring_is_bounded() {
        let mut digest = ActivityDigest::default();
        let total = ACTIVITY_MAX_LIMIT + 5;
        for index in 0..total {
            digest.record(&message(&format!("m{index}")));
            digest.record(&call(&format!("t{index}"), "completed", None));
        }
        let kept = digest.last_messages(ACTIVITY_MAX_LIMIT);
        assert_eq!(kept.len(), ACTIVITY_MAX_LIMIT);
        assert_eq!(kept.last().map(String::as_str), Some("m54"));
        assert_eq!(
            digest.last_messages(3),
            vec!["m52".to_string(), "m53".to_string(), "m54".to_string()]
        );
        assert!(digest.last_messages(0).is_empty());
    }

    #[test]
    fn a_long_message_is_cut_and_marked() {
        let mut digest = ActivityDigest::default();
        digest.record(&message(&"x".repeat(DIGEST_TEXT_CAP * 3)));
        let text = digest.last_messages(1).pop().expect("the message");
        assert!(
            text.chars().count() <= DIGEST_TEXT_CAP + 1,
            "{}",
            text.len()
        );
        assert!(text.ends_with('…'));
    }

    #[test]
    fn tool_calls_count_by_their_latest_status() {
        let mut digest = ActivityDigest::default();
        digest.record(&call("t1", "in_progress", None));
        digest.record(&update("t1", "completed", None));
        digest.record(&call("t2", "failed", None));
        digest.record(&call("t3", "in_progress", None));
        digest.record(&update("t3", "failed", None));
        digest.record(&call("t4", "pending", None));
        assert_eq!(digest.tool_counts(), (1, 2, 1));
    }

    #[test]
    fn files_touched_are_unique_and_in_first_seen_order() {
        let mut digest = ActivityDigest::default();
        digest.record(&call("t1", "in_progress", Some("a.rs")));
        digest.record(&update("t1", "completed", Some("a.rs")));
        digest.record(&call("t2", "completed", Some("b.rs")));
        digest.record(&update("t3", "completed", Some("a.rs")));
        assert_eq!(
            digest.answer_files(),
            ["a.rs".to_string(), "b.rs".to_string()]
        );
    }

    fn message_with_id(id: &str, text: &str) -> SessionEvent {
        SessionEvent::AgentMessage {
            message_id: Some(id.to_string()),
            text: text.to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
            images: Vec::new(),
        }
    }

    #[test]
    fn distinct_message_ids_are_distinct_messages_even_back_to_back() {
        let mut digest = ActivityDigest::default();
        digest.record(&message_with_id("m1", "first"));
        digest.record(&message_with_id("m2", "second"));
        assert_eq!(
            digest.last_messages(10),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn chunks_that_share_an_id_still_join_into_one_message() {
        let mut digest = ActivityDigest::default();
        digest.record(&message_with_id("m1", "Hel"));
        digest.record(&message_with_id("m1", "lo"));
        assert_eq!(digest.last_messages(10), vec!["Hello".to_string()]);
    }

    #[test]
    fn a_repeated_message_is_listed_once_at_its_latest_place() {
        let mut digest = ActivityDigest::default();
        digest.record(&message("Done."));
        digest.record(&call("t1", "completed", None));
        digest.record(&message("Working."));
        digest.record(&call("t2", "completed", None));
        digest.record(&message("Done."));
        assert_eq!(
            digest.last_messages(10),
            vec!["Working.".to_string(), "Done.".to_string()]
        );
    }

    #[test]
    fn the_response_is_bounded_in_characters_and_keeps_the_newest() {
        let mut digest = ActivityDigest::default();
        for index in 0..20 {
            digest.record(&message_with_id(
                &format!("m{index}"),
                &format!("{index:02}").repeat(1_000),
            ));
            digest.record(&call(&format!("t{index}"), "completed", None));
        }
        let kept = digest.last_messages(ACTIVITY_MAX_LIMIT);
        let total: usize = kept.iter().map(|text| text.chars().count()).sum();
        assert!(total <= DIGEST_RESPONSE_CAP, "{total} characters");
        assert!(
            kept.last().is_some_and(|text| text.starts_with("19")),
            "the newest message is the one kept whole or cut"
        );
    }

    #[test]
    fn a_file_path_is_cut_to_a_short_length() {
        let mut digest = ActivityDigest::default();
        let long = format!("{}.rs", "d/".repeat(400));
        digest.record(&call("t1", "completed", Some(&long)));
        let kept = digest.answer_files().first().expect("the path is kept");
        assert!(
            kept.chars().count() <= DIGEST_PATH_CAP + 1,
            "{}",
            kept.len()
        );
    }

    #[test]
    fn an_answer_lists_at_most_the_file_limit() {
        let mut digest = ActivityDigest::default();
        for index in 0..(ANSWER_FILE_LIMIT + 10) {
            digest.record(&call(
                &format!("t{index}"),
                "completed",
                Some(&format!("file{index}.rs")),
            ));
        }
        assert_eq!(digest.answer_files().len(), ANSWER_FILE_LIMIT);
        assert_eq!(digest.answer_files()[0], "file0.rs");
    }
}
