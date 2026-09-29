//! The daemon-owned `/goal` command: one intercept in the prompt path,
//! before provider dispatch, for every agent provider.
//!
//! The app shows a read-only composer where no agent runs, so this door is a
//! backstop, not the UI: terminals and transcripts refuse, and a
//! provider-advertised `goal` entry never forwards — the published command
//! lists drop the name, and this intercept consumes the text before any
//! provider sees it.

use std::sync::Arc;

use devboule_protocol::{ErrorCode, NoticeSeverity, SessionEvent, SessionKind, WireError};

use super::{OutOfBandCommands, SessionRuntime};

/// The longest goal text a `/goal` set stores. Beyond it the set is refused
/// with a usage line — never truncated silently, which would store words the
/// human did not write.
pub(crate) const MAX_GOAL_CHARS: usize = 2000;

const USAGE: &str = "Usage: /goal <text>|clear";

/// What the intercept decided about one prompt.
#[derive(Debug)]
pub(crate) enum GoalAction {
    /// Handled without touching the provider: a read-back, a clear, or a
    /// refusal. `turn_active` answers the send, as the out-of-band door does.
    Done { turn_active: bool },
    /// Goal stored: continue the ordinary send with this replacement text.
    /// The replacement names no slash command, so the door below cannot claim
    /// it and the provider receives exactly one ordinary user prompt.
    SendAs(String),
}

/// A provider-advertised command this daemon owns: drop it from the published
/// list, so no rival `/goal` reaches the menu. The Codex catalog keeps its own
/// entry — it names the same command, not a provider rival.
pub(crate) fn is_reserved_goal_command(name: &str) -> bool {
    name == "goal"
}

/// The one intercept: `None` when the text is not ours to handle. Runs before
/// the out-of-band door, on attachment-free prompts only, exactly like that
/// door — a `/goal` carrying attachments is refused before it.
pub(crate) fn intercept_goal(
    text: &str,
    kind: &SessionKind,
    runtime: &Arc<SessionRuntime>,
    out_of_band: Option<&Arc<dyn OutOfBandCommands>>,
) -> Result<Option<GoalAction>, WireError> {
    let Some((name, args)) = crate::codex_commands::parse_slash(text) else {
        return Ok(None);
    };
    if name != "goal" {
        return Ok(None);
    }
    // A transcript has no process to carry a goal: refuse like a terminal,
    // before any store, so a stopped session never records one. Only a
    // `/goal` command reaches this arm — ordinary text was declined by the
    // parse above and answers `process_gone` below, as it always did.
    if runtime.is_transcript() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "This session does not accept a goal.",
        ));
    }
    if !kind.is_agent() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "This session does not accept a goal.",
        ));
    }
    // The goal starts on the command's own line: a `/goal` alone on its
    // first line is a read-back, and whatever follows on later lines is not
    // the goal.
    let args = match text.trim().strip_prefix("/goal") {
        Some(rest)
            if rest
                .lines()
                .next()
                .is_some_and(|line| !line.trim().is_empty()) =>
        {
            args
        }
        _ => None,
    };
    let args = args.unwrap_or_default();
    if args.is_empty() {
        let line = match runtime.goal() {
            Some(goal) => format!("Goal: {goal}\n{USAGE}"),
            None => format!("No goal set.\n{USAGE}"),
        };
        publish_notice(runtime, line, false);
        return Ok(Some(GoalAction::Done {
            turn_active: runtime.is_running_turn(),
        }));
    }
    let arg_len = args.chars().count();
    if arg_len > MAX_GOAL_CHARS {
        publish_notice(
            runtime,
            format!("Goal is too long ({arg_len} characters, max {MAX_GOAL_CHARS}).\n{USAGE}"),
            true,
        );
        return Ok(Some(GoalAction::Done {
            turn_active: runtime.is_running_turn(),
        }));
    }
    // A native goal road claims the text whole: Codex set, clear, pause and
    // resume keep their RPCs, and the stored goal follows only a successful
    // answer (see `CodexCommands::answer`). Anything else is ours.
    if out_of_band.is_some_and(|commands| commands.handles_out_of_band(text)) {
        return Ok(None);
    }
    if args.eq_ignore_ascii_case("clear") {
        store_goal(runtime, None)?;
        publish_notice(runtime, "Goal cleared.".to_string(), false);
        return Ok(Some(GoalAction::Done {
            turn_active: runtime.is_running_turn(),
        }));
    }
    // `pause` and `resume` have no native meaning outside Codex: they are
    // goal text like anything else, sent once as an ordinary prompt.
    store_goal(runtime, Some(args.to_string()))?;
    publish_notice(runtime, format!("Goal set: {args}"), false);
    Ok(Some(GoalAction::SendAs(format!("Goal: {args}"))))
}

/// Record one goal change: the journal column first, so a failed write leaves
/// runtime and transcript exactly as they were; then the runtime, the event,
/// and the roster push that carries the new snapshot.
pub(crate) fn store_goal(
    runtime: &Arc<SessionRuntime>,
    goal: Option<String>,
) -> Result<(), WireError> {
    if let Some(journal) = runtime.journal.as_ref() {
        journal
            .set_session_goal(&runtime.session_id, goal.as_deref())
            .map_err(|error| {
                WireError::new(
                    ErrorCode::Internal,
                    format!("The goal could not be recorded: {error}"),
                )
            })?;
    }
    runtime.set_goal(goal.clone());
    let _ = runtime.publish_daemon_event(SessionEvent::GoalChanged { goal });
    runtime.request_transition();
    Ok(())
}

/// Recovery carries a goal the journal may no longer accept: the write is
/// attempted first, and the in-memory copy is published either way, so the
/// replacement never answers a read-back with an absence the old session did
/// not have. A failed write leaves a warning notice beside the carried goal.
pub(crate) fn carry_goal_into_recovery(runtime: &Arc<SessionRuntime>, goal: String) {
    if let Err(error) = store_goal(runtime, Some(goal.clone())) {
        runtime.publish_session_notice(error.message, NoticeSeverity::Warning);
        runtime.set_goal(Some(goal.clone()));
        let _ = runtime.publish_daemon_event(SessionEvent::GoalChanged { goal: Some(goal) });
        runtime.request_transition();
    }
}

fn publish_notice(runtime: &Arc<SessionRuntime>, text: String, failed: bool) {
    let _ = runtime.publish_daemon_event(SessionEvent::SessionNotice {
        text,
        severity: if failed {
            NoticeSeverity::Warning
        } else {
            NoticeSeverity::Info
        },
    });
}

#[cfg(test)]
#[path = "session_goal_test_support.rs"]
mod goal_test_support;

#[cfg(test)]
#[path = "session_goal_tests.rs"]
mod goal_tests;

#[cfg(test)]
#[path = "session_goal_dispatch_tests.rs"]
mod goal_dispatch_tests;

#[cfg(test)]
#[path = "session_goal_menu_tests.rs"]
mod goal_menu_tests;

#[cfg(test)]
#[path = "session_goal_journal_tests.rs"]
mod goal_journal_tests;
