//! The close flow: what happens when the main window is asked to close, or
//! the user picks Quit in the tray.
//!
//! One responsibility: deciding — from the stored choice and one daemon
//! status read — whether a close means hide-to-tray, quit, or a question,
//! and asking that question with the native three-button dialog.

use serde_json::Value;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

use crate::surface_settings;

/// The stored "When I close the window" choice. Mirrored by
/// `src/features/settings/closeBehaviorChoice.ts`; both sides read the same
/// document and both fall back the same way.
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

/// What a close (or a tray Quit) turns into, decided once so the caller —
/// window close, tray menu, macOS Cmd+Q — all act the same way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClosePlan {
    /// Show the confirmation dialog.
    Ask,
    /// Hide the window; the tray keeps the app and its daemon alive.
    Hide,
    /// Quit for real: the window goes and the `RunEvent::Exit` cleanup runs.
    Quit,
}

/// The close decision as pure logic. The stored choice alone decides the
/// act; the daemon facts (running agents, other local clients) only shape
/// what the confirmation says when the choice is Ask.
pub fn decide_close(
    choice: CloseChoice,
    _agents_running: u32,
    _other_local_clients: u32,
) -> ClosePlan {
    match choice {
        CloseChoice::Ask => ClosePlan::Ask,
        // The suggested act: the tray keeps the app and its daemon alive.
        CloseChoice::Tray => ClosePlan::Hide,
        CloseChoice::Quit => ClosePlan::Quit,
    }
}

/// What the confirmation says. The owner's requirement: it names what
/// stops — the running agents and the daemon — and that paired devices lose
/// access. When another local window is connected, the daemon's own
/// refusal will keep it alive, and the message says so instead of
/// promising a stop that will not happen.
pub fn close_confirmation_message(agents_running: u32, other_local_clients: u32) -> String {
    let agents = match agents_running {
        0 => "No agents are running.".to_string(),
        1 => "1 agent is running and will stop.".to_string(),
        n => format!("{n} agents are running and will stop."),
    };
    if other_local_clients > 0 {
        // The daemon refuses this quit while another local window is
        // connected, so the honest promise is the survival one.
        let window_word = if other_local_clients == 1 {
            "window"
        } else {
            "windows"
        };
        return format!(
            "{agents} The daemon keeps running for the {other_local_clients} other open Devboule \
             {window_word} and its paired devices."
        );
    }
    format!(
        "{agents} Quitting stops the Devboule daemon, and paired devices lose access until \
         Devboule starts again."
    )
}

/// The stored choice, or Ask for anything unreadable — a missing file, a
/// corrupt one, or an unknown value. Asking is the only direction that
/// cannot stop a daemon silently.
fn stored_close_choice(app: &tauri::AppHandle) -> CloseChoice {
    let Ok(config_dir) = app.path().app_config_dir() else {
        return CloseChoice::Ask;
    };
    match surface_settings::surface_settings_get_inner(&config_dir, CLOSE_SURFACE_ID) {
        Ok(value) => close_choice_from_stored(value.as_ref()),
        Err(_) => CloseChoice::Ask,
    }
}

fn close_choice_from_stored(value: Option<&Value>) -> CloseChoice {
    let Some(value) = value else {
        return CloseChoice::Ask;
    };
    match value.get("choice").and_then(|choice| choice.as_str()) {
        Some("tray") => CloseChoice::Tray,
        Some("quit") => CloseChoice::Quit,
        _ => CloseChoice::Ask,
    }
}

/// `CloseRequested` entry point. The window never closes here: a close
/// either turns into a decided act or into the confirmation — never into a
/// silent quit.
pub fn on_close_requested(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    api.prevent_close();
    let app = window.app_handle().clone();
    // The decision and the dialog never run on the main thread: the daemon
    // status read can wait on the daemon, and a blocked main thread would
    // freeze the very window the question is about.
    let flow_app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("close-confirmation".into())
        .spawn(move || run_close_flow(flow_app));
    if spawned.is_err() {
        // With no thread to decide in, nothing may stop the daemon silently:
        // hiding keeps the app and its daemon alive.
        hide_main_window(&app);
    }
}

/// The tray's Quit entry point, and the macOS app-menu exit: the same
/// decision as closing the window, so the menu is no way around it.
pub fn confirm_quit(app: tauri::AppHandle) {
    // If no thread can be spawned the request dies here: undecidable means
    // keep running, never a silent quit.
    let _ = std::thread::Builder::new()
        .name("close-confirmation".into())
        .spawn(move || run_close_flow(app));
}

fn run_close_flow(app: tauri::AppHandle) {
    let choice = stored_close_choice(&app);
    let (agents_running, other_local_clients) = daemon_facts(&app);
    match decide_close(choice, agents_running, other_local_clients) {
        ClosePlan::Hide => hide_main_window(&app),
        ClosePlan::Quit => app.exit(0),
        ClosePlan::Ask => ask(app, agents_running, other_local_clients),
    }
}

/// Running agents and other local app clients, read once from the daemon.
/// An unreachable daemon has no agents to name and no other window to
/// protect — the zeros are the honest reading, and the daemon's own refusal
/// remains the backstop.
fn daemon_facts(app: &tauri::AppHandle) -> (u32, u32) {
    let Some(bridge) = app.try_state::<crate::client::DaemonBridge>() else {
        return (0, 0);
    };
    let Ok(client) = bridge.client() else {
        return (0, 0);
    };
    match client.status() {
        Ok(body) => (body.sessions, body.local_clients.saturating_sub(1)),
        Err(_) => (0, 0),
    }
}

fn ask(app: tauri::AppHandle, agents_running: u32, other_local_clients: u32) {
    app.dialog()
        .message(close_confirmation_message(
            agents_running,
            other_local_clients,
        ))
        .title("Quit Devboule?")
        .kind(tauri_plugin_dialog::MessageDialogKind::Warning)
        .buttons(
            tauri_plugin_dialog::MessageDialogButtons::YesNoCancelCustom(
                "Keep running in the tray".into(),
                "Quit".into(),
                "Cancel".into(),
            ),
        )
        .show_with_result(move |result| match result {
            tauri_plugin_dialog::MessageDialogResult::Yes => hide_main_window(&app),
            tauri_plugin_dialog::MessageDialogResult::No => app.exit(0),
            // Cancel: exactly as things were.
            _ => {}
        });
}

fn hide_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_choice_decides_the_plan() {
        assert_eq!(decide_close(CloseChoice::Ask, 0, 0), ClosePlan::Ask);
        // The tray is the suggested act: the app and its daemon stay up.
        assert_eq!(decide_close(CloseChoice::Tray, 2, 0), ClosePlan::Hide);
        assert_eq!(decide_close(CloseChoice::Quit, 2, 0), ClosePlan::Quit);
    }

    #[test]
    fn the_confirmation_names_what_stops() {
        let message = close_confirmation_message(2, 0);
        assert!(message.contains("2"), "it names the agent count: {message}");
        assert!(message.contains("daemon"), "it names the daemon: {message}");
        assert!(
            message.contains("paired devices lose access"),
            "it says device access ends: {message}"
        );
    }

    #[test]
    fn the_confirmation_says_the_daemon_survives_for_another_window() {
        let alone = close_confirmation_message(0, 0);
        let shared = close_confirmation_message(0, 1);
        assert!(
            !alone.contains("keeps running"),
            "with no other window the daemon stops: {alone}"
        );
        assert!(
            shared.contains("keeps running"),
            "with another window connected, the daemon outlives this quit: {shared}"
        );
        assert!(
            shared.contains("1 other"),
            "it says how many other windows remain: {shared}"
        );
    }

    #[test]
    fn an_unknown_stored_value_means_ask() {
        assert_eq!(close_choice_from_stored(None), CloseChoice::Ask);
        assert_eq!(
            close_choice_from_stored(Some(&serde_json::json!({ "choice": "tray" }))),
            CloseChoice::Tray
        );
        assert_eq!(
            close_choice_from_stored(Some(&serde_json::json!({ "choice": "minimize" }))),
            CloseChoice::Ask
        );
        assert_eq!(
            close_choice_from_stored(Some(&serde_json::json!(null))),
            CloseChoice::Ask
        );
    }
}
