//! The close/quit decision: pure choices and pure words, no Tauri.
//!
//! One responsibility: from the stored choice and the daemon's facts,
//! decide what a window close or an explicit quit becomes, and exactly
//! what the confirmation says. The Tauri half that acts on the decision
//! lives in `close_flow`.

use std::sync::Mutex;

use serde_json::Value;

/// The stored "When I close the window" choice. Mirrored by
/// `src/features/settings/closeBehaviorChoice.ts`; both sides read the same
/// document and both fall back the same way. The choice applies to closing
/// the WINDOW only — the tray's Quit and the app-level exit always ask.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CloseChoice {
    #[default]
    Ask,
    Tray,
    Quit,
}

/// The surface id the Settings row stores the choice under. Valid against
/// the backend's `^[a-z0-9-]{1,32}$` filename rule.
pub(crate) const CLOSE_SURFACE_ID: &str = "close-behavior";

/// What the daemon reported the last time it was asked. Unknown is not
/// "nothing": a failed read must never be phrased as an empty daemon.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DaemonFacts {
    Read {
        agents: u32,
        terminals: u32,
        other_local_windows: u32,
    },
    #[default]
    Unknown,
}

/// What the confirmation offers. A window close suggests the tray; an
/// explicit quit never does — it is a quit question, not a close question.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AskFlavor {
    WindowClose,
    QuitOnly,
}

/// What a close (or a tray Quit) turns into, decided once so the window,
/// the tray menu, and the macOS app-menu exit all act the same way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClosePlan {
    /// Show the confirmation dialog.
    Ask(AskFlavor),
    /// Hide the window; the tray keeps the app and its daemon alive.
    Hide,
    /// Quit for real: the window goes and the `RunEvent::Exit` cleanup runs.
    Quit,
}

/// The window-close decision as pure logic: the stored choice alone
/// decides; the daemon facts only shape what the confirmation says.
pub fn decide_close(choice: CloseChoice) -> ClosePlan {
    match choice {
        CloseChoice::Ask => ClosePlan::Ask(AskFlavor::WindowClose),
        // The suggested act: the tray keeps the app and its daemon alive.
        CloseChoice::Tray => ClosePlan::Hide,
        CloseChoice::Quit => ClosePlan::Quit,
    }
}

/// The explicit-quit decision (tray "Quit", Cmd+Q): always the quit
/// question, whatever the stored close choice says — a Quit command never
/// resolves to hiding.
pub fn decide_quit() -> ClosePlan {
    ClosePlan::Ask(AskFlavor::QuitOnly)
}

/// What the confirmation says. The owner's requirement: it says only what
/// is true. When another local window is connected, nothing stops — the
/// daemon refuses this quit and keeps running for that window. Otherwise
/// the running agents and terminals are named separately, and the daemon
/// and device-access consequences are spelled out.
pub fn quit_confirmation_message(facts: &DaemonFacts) -> String {
    match facts {
        DaemonFacts::Read {
            agents,
            terminals,
            other_local_windows,
        } if *other_local_windows > 0 => {
            let windows = match *other_local_windows {
                1 => "1 other open Devboule window".to_string(),
                n => format!("{n} other open Devboule windows"),
            };
            format!(
                "Nothing stops: the daemon keeps running for the {windows} and its agents and \
                 terminals."
            )
        }
        DaemonFacts::Read {
            agents: 0,
            terminals: 0,
            ..
        } => "No agents or terminals are running. Quitting stops the daemon, and paired devices \
              lose access until Devboule starts again."
            .to_string(),
        DaemonFacts::Read {
            agents, terminals, ..
        } => format!(
            "{} Quitting stops the daemon, and paired devices lose access until Devboule \
             starts again.",
            running_list(*agents, *terminals)
        ),
        DaemonFacts::Unknown => {
            "The daemon's status could not be read, so Devboule cannot say what is running. \
             Quitting asks the daemon to stop; it refuses while another Devboule window is \
             still connected."
                .to_string()
        }
    }
}

/// The running sessions, each named for what it is: a terminal is never
/// called an agent, and an empty daemon is never dressed up as a count.
fn running_list(agents: u32, terminals: u32) -> String {
    let mut parts: Vec<String> = Vec::new();
    if agents > 0 {
        parts.push(plural(agents, "agent"));
    }
    if terminals > 0 {
        parts.push(plural(terminals, "terminal"));
    }
    match parts.as_slice() {
        [] => "Nothing is running".to_string(),
        [only] => format!("{only} will stop"),
        [first, second] => format!("{first} and {second} will stop"),
        _ => unreachable!("two families, never more"),
    }
}

fn plural(count: u32, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

/// The stored choice, or Ask for anything unreadable — a missing file, a
/// corrupt one, or an unknown value. Asking is the only direction that
/// cannot stop a daemon silently.
pub(crate) fn stored_close_choice_text(value: Option<&Value>) -> CloseChoice {
    let Some(value) = value else {
        return CloseChoice::Ask;
    };
    match value.get("choice").and_then(|choice| choice.as_str()) {
        Some("tray") => CloseChoice::Tray,
        Some("quit") => CloseChoice::Quit,
        _ => CloseChoice::Ask,
    }
}

/// One close/quit confirmation at a time: a second request while one is
/// open is ignored, never turned into a competing dialog whose answers
/// could fight.
#[derive(Default)]
pub(crate) struct ConfirmGate(Mutex<bool>);

impl ConfirmGate {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(false))
    }

    /// Whether the caller may open the confirmation now.
    pub(crate) fn try_begin(&self) -> bool {
        let mut open = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if *open {
            return false;
        }
        *open = true;
        true
    }

    /// The dialog answered (any answer): the next request may ask again.
    pub(crate) fn end(&self) {
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_stored_choice_decides_the_window_close_plan() {
        assert_eq!(
            decide_close(CloseChoice::Ask),
            ClosePlan::Ask(AskFlavor::WindowClose)
        );
        // The tray is the suggested act: the app and its daemon stay up.
        assert_eq!(decide_close(CloseChoice::Tray), ClosePlan::Hide);
        assert_eq!(decide_close(CloseChoice::Quit), ClosePlan::Quit);
    }

    #[test]
    fn an_explicit_quit_is_always_the_quit_question_and_never_hides() {
        // The stored close choice must not reach the quit path: a Quit
        // command asks, it never resolves to hiding.
        assert_eq!(decide_quit(), ClosePlan::Ask(AskFlavor::QuitOnly));
    }

    #[test]
    fn the_confirmation_names_what_stops() {
        let message = quit_confirmation_message(&DaemonFacts::Read {
            agents: 2,
            terminals: 1,
            other_local_windows: 0,
        });
        assert!(
            message.contains("2 agents"),
            "it names the agents: {message}"
        );
        assert!(
            message.contains("1 terminal"),
            "it names the terminals: {message}"
        );
        assert!(
            message.contains("will stop"),
            "it says they stop: {message}"
        );
        assert!(
            message.contains("paired devices lose access"),
            "it says device access ends: {message}"
        );
    }

    #[test]
    fn the_confirmation_says_nothing_stops_when_another_window_is_connected() {
        let message = quit_confirmation_message(&DaemonFacts::Read {
            agents: 2,
            terminals: 1,
            other_local_windows: 1,
        });
        assert!(
            message.contains("Nothing stops"),
            "with another window the daemon refuses this quit: {message}"
        );
        assert!(
            message.contains("keeps running for the 1 other open Devboule window"),
            "it says who the daemon keeps running for: {message}"
        );
        assert!(
            !message.contains("will stop"),
            "it must not promise a stop that will not happen: {message}"
        );
    }

    #[test]
    fn the_confirmation_counts_empty_and_single_kinds_correctly() {
        let only_terminals = quit_confirmation_message(&DaemonFacts::Read {
            agents: 0,
            terminals: 1,
            other_local_windows: 0,
        });
        assert!(
            only_terminals.contains("1 terminal will stop"),
            "a terminal is never called an agent: {only_terminals}"
        );
        let empty = quit_confirmation_message(&DaemonFacts::Read {
            agents: 0,
            terminals: 0,
            other_local_windows: 0,
        });
        assert!(
            empty.contains("No agents or terminals are running"),
            "an empty daemon says so plainly: {empty}"
        );
    }

    #[test]
    fn an_unreadable_daemon_is_not_reported_as_empty() {
        let message = quit_confirmation_message(&DaemonFacts::Unknown);
        assert!(
            message.contains("could not be read"),
            "unknown is unknown: {message}"
        );
        assert!(
            !message.contains("No agents"),
            "it must not claim an empty daemon: {message}"
        );
    }

    #[test]
    fn an_unknown_stored_value_means_ask() {
        assert_eq!(stored_close_choice_text(None), CloseChoice::Ask);
        assert_eq!(
            stored_close_choice_text(Some(&json!({ "choice": "tray" }))),
            CloseChoice::Tray
        );
        assert_eq!(
            stored_close_choice_text(Some(&json!({ "choice": "minimize" }))),
            CloseChoice::Ask
        );
        assert_eq!(
            stored_close_choice_text(Some(&json!(null))),
            CloseChoice::Ask
        );
    }

    #[test]
    fn only_one_confirmation_may_open_at_a_time() {
        let gate = ConfirmGate::new();
        assert!(gate.try_begin(), "the first request opens the dialog");
        assert!(
            !gate.try_begin(),
            "a second request while one is open is refused"
        );
        gate.end();
        assert!(gate.try_begin(), "an answered dialog opens the gate again");
    }
}
