//! The close/quit flow: the Tauri half that acts on `close_prompt`'s
//! decision.
//!
//! One responsibility: reading the stored choice and the daemon's facts,
//! showing the confirmation (at most one at a time), and performing what
//! the user chose — hide, or quit with a last look at the daemon.

use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::client::DaemonBridge;
use crate::close_prompt::{
    act_on_quit_answer, decide_close, decide_quit, dialog_answer, quit_confirmation_message,
    stored_close_choice_text, AskFlavor, ClosePlan, ConfirmGate, DaemonFacts, QuitAct,
    CANCEL_BUTTON_LABEL, CLOSE_SURFACE_ID, QUIT_BUTTON_LABEL, TRAY_BUTTON_LABEL,
};
use crate::surface_settings;

/// One confirmation per app: a second close or quit request while a dialog
/// is open is ignored, never turned into a competing dialog.
static CONFIRM_GATE: ConfirmGate = ConfirmGate::new();

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
        .spawn(move || run_window_close_flow(flow_app));
    if spawned.is_err() {
        // With no thread to decide in, nothing may stop the daemon silently:
        // hiding keeps the app and its daemon alive.
        hide_main_window(&app);
    }
}

/// The tray's Quit entry point, and the macOS app-menu exit: always the
/// quit question, never the stored close choice, so a Quit command cannot
/// resolve to hiding.
pub fn confirm_quit(app: tauri::AppHandle) {
    // If no thread can be spawned the request dies here: undecidable means
    // keep running, never a silent quit.
    let _ = std::thread::Builder::new()
        .name("close-confirmation".into())
        .spawn(move || {
            if let ClosePlan::Ask(flavor) = decide_quit() {
                ask(&app, flavor);
            }
        });
}

fn run_window_close_flow(app: tauri::AppHandle) {
    let choice = stored_close_choice(&app);
    match decide_close(choice) {
        ClosePlan::Hide => hide_main_window(&app),
        ClosePlan::Quit => perform_quit(&app, &DaemonFacts::Unknown),
        ClosePlan::Ask(flavor) => ask(&app, flavor),
    }
}

fn ask(app: &tauri::AppHandle, flavor: AskFlavor) {
    if !CONFIRM_GATE.try_begin() {
        // A confirmation is already open: ignoring keeps the answers from
        // fighting; the open dialog is still the one true question.
        return;
    }
    let shown = daemon_facts(app);
    let builder = app
        .dialog()
        .message(quit_confirmation_message(&shown))
        .title("Quit Devboule?")
        .kind(MessageDialogKind::Warning);
    let builder = match flavor {
        AskFlavor::WindowClose => builder.buttons(MessageDialogButtons::YesNoCancelCustom(
            TRAY_BUTTON_LABEL.into(),
            QUIT_BUTTON_LABEL.into(),
            CANCEL_BUTTON_LABEL.into(),
        )),
        // The quit question offers quit and cancel, never the tray.
        AskFlavor::QuitOnly => builder.buttons(MessageDialogButtons::OkCancelCustom(
            QUIT_BUTTON_LABEL.into(),
            CANCEL_BUTTON_LABEL.into(),
        )),
    };
    let flow_app = app.clone();
    builder.show_with_result(move |result| {
        CONFIRM_GATE.end();
        match dialog_answer(&result) {
            crate::close_prompt::DialogAnswer::Hide => hide_main_window(&flow_app),
            crate::close_prompt::DialogAnswer::Quit => {
                // The dialog promised the world as `shown` had it. If the
                // daemon's facts moved while the dialog was open, the old
                // promise is not acted on: ask again with what is true now.
                let fresh = daemon_facts(&flow_app);
                match act_on_quit_answer(&shown, &fresh) {
                    QuitAct::Quit => perform_quit(&flow_app, &fresh),
                    QuitAct::AskAgain => ask(&flow_app, flavor),
                }
            }
            crate::close_prompt::DialogAnswer::Cancel => {}
        }
    });
}

/// The quit act. Before exiting, the daemon gets one last request: the
/// supervisor may be mid-reconnect with no client of its own, and quitting
/// silently would leave a daemon running with work no window can show.
/// A refusal is an answer (another local window keeps it alive), but a
/// daemon that cannot be reached is said out loud before the exit.
fn perform_quit(app: &tauri::AppHandle, facts: &DaemonFacts) {
    eprintln!("devboule: quitting; daemon facts at quit: {facts:?}");
    if let Some(bridge) = app.try_state::<DaemonBridge>() {
        if let Err(reason) = bridge.ask_daemon_to_stop() {
            eprintln!("devboule: the daemon could not be asked to stop: {reason}");
            app.dialog()
                .message(
                    "Devboule could not reach the daemon to ask it to stop. It may still be \
                     running with its agents and terminals.",
                )
                .title("Devboule is quitting")
                .kind(MessageDialogKind::Warning)
                .blocking_show();
        }
    }
    app.exit(0);
}

fn stored_close_choice(app: &tauri::AppHandle) -> crate::close_prompt::CloseChoice {
    let Ok(config_dir) = app.path().app_config_dir() else {
        return crate::close_prompt::CloseChoice::Ask;
    };
    match surface_settings::surface_settings_get_inner(&config_dir, CLOSE_SURFACE_ID) {
        Ok(value) => stored_close_choice_text(value.as_ref()),
        Err(_) => crate::close_prompt::CloseChoice::Ask,
    }
}

/// What the daemon reports, read once for the confirmation's words. A
/// missing bridge, a reconnecting client, or a failed read is Unknown —
/// never mistaken for an empty daemon.
fn daemon_facts(app: &tauri::AppHandle) -> DaemonFacts {
    let Some(bridge) = app.try_state::<DaemonBridge>() else {
        return DaemonFacts::Unknown;
    };
    let Ok(client) = bridge.client() else {
        return DaemonFacts::Unknown;
    };
    let Ok(body) = client.status() else {
        return DaemonFacts::Unknown;
    };
    let (Some(agents), Some(terminals)) = (body.agents, body.terminals) else {
        return DaemonFacts::Unknown;
    };
    DaemonFacts::Read {
        agents,
        terminals,
        other_local_windows: body.local_clients.saturating_sub(1),
    }
}

fn hide_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}
