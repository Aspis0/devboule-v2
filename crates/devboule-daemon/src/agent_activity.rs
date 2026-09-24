//! Derived activity for daemon-spawned agent sessions, plus the quiet rule.
//!
//! Hook reports (`agent_report.rs`) arrive over a named pipe and keep their
//! own per-source `seq` discipline. This file never touches that map: it
//! derives a headline state from facts the daemon already holds (liveness, a
//! running turn, a pending permission card, the last publish time) and keeps
//! a bounded in-memory ring of recent event kinds. The tool reads both; the
//! hook's last state rides alongside, never merged, so the two cannot fight.

use std::time::Duration;

use devboule_protocol::{AgentActivityState, SessionEvent};

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
        SessionEvent::ChildFinished { .. } => "child_finished",
        SessionEvent::AgentTaskStarted { .. } => "agent_task_started",
        SessionEvent::AgentTaskNotification { .. } => "agent_task_notification",
        SessionEvent::AgentBackgroundTasksChanged { .. } => "agent_background_tasks_changed",
        SessionEvent::AgentError { .. } => "agent_error",
        SessionEvent::AgentStderr { .. } => "agent_stderr",
        SessionEvent::PermissionRequest { .. } => "permission_request",
        SessionEvent::PermissionResolved { .. } => "permission_resolved",
        SessionEvent::PermissionAnswered { .. } => "permission_answered",
        SessionEvent::SessionManifest { .. } => "session_manifest",
        SessionEvent::AgentReported { .. } => "agent_reported",
        SessionEvent::Snapshot { .. } => "snapshot",
        SessionEvent::Exit { .. } => "exit",
        SessionEvent::Recovered { .. } => "recovered",
        SessionEvent::Detached => "detached",
        SessionEvent::Silent { .. } => "silent",
        SessionEvent::JournalDegraded { .. } => "journal_degraded",
        SessionEvent::SessionsSnapshot { .. } => "sessions_snapshot",
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
}
