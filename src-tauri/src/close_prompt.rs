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

/// The three buttons our dialogs show, by their exact labels. The dialog
/// plugin answers custom buttons with `Custom(label)`, so the labels are
/// the contract between the builder in `close_flow` and this mapping.
pub(crate) const TRAY_BUTTON_LABEL: &str = "Keep running in the tray";
pub(crate) const QUIT_BUTTON_LABEL: &str = "Quit";
pub(crate) const CANCEL_BUTTON_LABEL: &str = "Cancel";

/// What the user's answer means. Cancel (and anything unrecognizable) is
/// the safe nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DialogAnswer {
    Hide,
    Quit,
    Cancel,
}

/// The dialog's answer, mapped to its action. The plugin answers custom
/// buttons with `Custom(label)` on Windows and macOS; the plain `Yes`/`No`/
/// `Ok`/`Cancel` variants are mapped too, in case a platform returns them.
pub(crate) fn dialog_answer(result: &tauri_plugin_dialog::MessageDialogResult) -> DialogAnswer {
    use tauri_plugin_dialog::MessageDialogResult;
    match result {
        MessageDialogResult::Custom(label) if label == TRAY_BUTTON_LABEL => DialogAnswer::Hide,
        MessageDialogResult::Custom(label) if label == QUIT_BUTTON_LABEL => DialogAnswer::Quit,
        MessageDialogResult::Custom(_) | MessageDialogResult::Cancel => DialogAnswer::Cancel,
        MessageDialogResult::Yes => DialogAnswer::Hide,
        MessageDialogResult::No | MessageDialogResult::Ok => DialogAnswer::Quit,
    }
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

/// What a Quit answer becomes once the daemon's facts are read again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuitAct {
    /// The facts still match the sentence the dialog showed: act on it.
    Quit,
    /// The facts moved while the dialog was open, so its sentence is stale:
    /// ask again with what is true now rather than act on an old promise.
    AskAgain,
}

pub(crate) fn act_on_quit_answer(shown: &DaemonFacts, fresh: &DaemonFacts) -> QuitAct {
    if shown == fresh {
        QuitAct::Quit
    } else {
        QuitAct::AskAgain
    }
}

/// What the confirmation says. The owner's requirement: it says only what
/// is true. When another local app is connected, nothing stops — the daemon
/// refuses this quit and keeps running for it, and it is named as a running
/// Devboule, not a window: the other side may itself be hidden in its tray.
/// Otherwise the running agents and terminals are named separately, and the
/// daemon and device-access consequences are spelled out.
pub fn quit_confirmation_message(facts: &DaemonFacts) -> String {
    match facts {
        DaemonFacts::Read {
            agents,
            terminals,
            other_local_windows,
        } if *other_local_windows > 0 => {
            let others = match *other_local_windows {
                1 => "another running Devboule".to_string(),
                n => format!("the {n} other running Devboule instances"),
            };
            format!(
                "Nothing stops: the daemon keeps running for {others} and its agents and \
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
            "{}. Quitting stops the daemon, and paired devices lose access until Devboule \
             starts again.",
            running_list(*agents, *terminals)
        ),
        DaemonFacts::Unknown => {
            "The daemon's status could not be read, so Devboule cannot say what is running. \
             Quitting asks the daemon to stop; it refuses while another running Devboule is \
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
    fn the_confirmation_says_what_stops_in_full_sentences() {
        let read = |agents: u32, terminals: u32, other: u32| {
            quit_confirmation_message(&DaemonFacts::Read {
                agents,
                terminals,
                other_local_windows: other,
            })
        };
        // Every variant pinned exactly: the sentences stand alone, with the
        // stop between them, and a terminal is never called an agent.
        let tail = ". Quitting stops the daemon, and paired devices lose access until Devboule starts again.";
        assert_eq!(read(1, 0, 0), format!("1 agent will stop{tail}"));
        assert_eq!(read(0, 1, 0), format!("1 terminal will stop{tail}"));
        assert_eq!(
            read(2, 1, 0),
            format!("2 agents and 1 terminal will stop{tail}")
        );
        assert_eq!(
            read(0, 0, 0),
            "No agents or terminals are running. Quitting stops the daemon, and paired devices lose access until Devboule starts again."
        );
    }

    #[test]
    fn the_confirmation_names_another_running_devboule_not_a_window() {
        let read = |other: u32| {
            quit_confirmation_message(&DaemonFacts::Read {
                agents: 0,
                terminals: 0,
                other_local_windows: other,
            })
        };
        // The other side may be hidden in its tray: it is not a window on
        // screen, it is a running Devboule.
        assert_eq!(
            read(1),
            "Nothing stops: the daemon keeps running for another running Devboule and its agents and terminals."
        );
        assert_eq!(
            read(2),
            "Nothing stops: the daemon keeps running for the 2 other running Devboule instances and its agents and terminals."
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
    fn every_button_of_both_flavours_maps_to_its_action() {
        use tauri_plugin_dialog::MessageDialogResult;
        let custom = |label: &str| MessageDialogResult::Custom(label.to_string());
        // WindowClose: the tray suggestion, the quit, the cancel.
        assert_eq!(
            dialog_answer(&custom(TRAY_BUTTON_LABEL)),
            DialogAnswer::Hide
        );
        assert_eq!(
            dialog_answer(&custom(QUIT_BUTTON_LABEL)),
            DialogAnswer::Quit
        );
        assert_eq!(
            dialog_answer(&custom(CANCEL_BUTTON_LABEL)),
            DialogAnswer::Cancel
        );
        // QuitOnly shares the Quit and Cancel labels.
        assert_eq!(
            dialog_answer(&custom(QUIT_BUTTON_LABEL)),
            DialogAnswer::Quit
        );
        // Plain variants, in case a platform returns them instead.
        assert_eq!(dialog_answer(&MessageDialogResult::Yes), DialogAnswer::Hide);
        assert_eq!(dialog_answer(&MessageDialogResult::No), DialogAnswer::Quit);
        assert_eq!(dialog_answer(&MessageDialogResult::Ok), DialogAnswer::Quit);
        assert_eq!(
            dialog_answer(&MessageDialogResult::Cancel),
            DialogAnswer::Cancel
        );
        // The dialog's own close box and any unrecognizable label: nothing.
        assert_eq!(dialog_answer(&custom("")), DialogAnswer::Cancel);
        assert_eq!(
            dialog_answer(&custom("some other label")),
            DialogAnswer::Cancel
        );
    }

    #[test]
    fn a_stale_promise_is_asked_again_not_acted_on() {
        let shown = DaemonFacts::Read {
            agents: 2,
            terminals: 0,
            other_local_windows: 0,
        };
        // The facts the dialog showed still stand: the answer acts.
        assert_eq!(act_on_quit_answer(&shown, &shown), QuitAct::Quit);
        // Another local window connected while the dialog was open: the
        // sentence promised a stop that would now be refused.
        assert_eq!(
            act_on_quit_answer(
                &shown,
                &DaemonFacts::Read {
                    agents: 2,
                    terminals: 0,
                    other_local_windows: 1,
                },
            ),
            QuitAct::AskAgain
        );
        // The daemon became unreadable: the old sentence is stale too.
        assert_eq!(
            act_on_quit_answer(&shown, &DaemonFacts::Unknown),
            QuitAct::AskAgain
        );
        assert_eq!(
            act_on_quit_answer(&DaemonFacts::Unknown, &DaemonFacts::Unknown),
            QuitAct::Quit
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
