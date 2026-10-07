//! The OS toast an attention raise becomes, and what a click on it means.
//!
//! One responsibility: show the toast that stands for a raise, turn a click on
//! it into the window coming forward plus one app event that names the session
//! the toast was about, and leave no toast of ours in the OS history behind.

#[cfg(windows)]
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::sync::OnceLock;

use devboule_protocol::ErrorCode;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::backend::blocking::off_main_thread;
use crate::backend::error::CommandError;

/// What a click routes to: the session the toast announced, and the workspace
/// to fall back to when the roster no longer holds that session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionTarget {
    pub session_id: String,
    pub workspace_id: Option<String>,
}

/// One toast to show: the words on it and what a click on it means.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionToast {
    pub title: String,
    pub body: String,
    pub target: AttentionTarget,
}

/// The command behind every attention toast. The words are the frontend's
/// (`attentionNotice.ts` owns the content, the preferences and the dedupe); the
/// display happens here because the click has to arrive in this process, and
/// the plugin's sender exposes no activation.
#[tauri::command]
pub async fn attention_toast_show(
    app: AppHandle,
    toast: AttentionToast,
) -> Result<(), CommandError> {
    let shown = app.clone();
    off_main_thread(move || {
        show(&shown, &toast).map_err(|message| CommandError::new(ErrorCode::Internal, message))
    })
    .await
}

/// Which sender a raise gets on this machine's install state.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToastSender {
    /// Windows' own WinRT toast: a click on it comes back to this process.
    Clickable,
    /// The notification plugin's toast: visible, with no click to route.
    VisibleOnly,
}

/// The clickable sender needs the AUMID the installer's Start-menu shortcut
/// registers: a toast raised under one Windows was never told about can be
/// dropped for good with no error to catch. `programs_dir` is that folder,
/// `None` when this process cannot name it.
#[cfg(windows)]
fn attention_sender(programs_dir: Option<&Path>, product_name: &str) -> ToastSender {
    match programs_dir {
        // The installer writes `${PRODUCTNAME}.lnk` straight into the Programs
        // folder (`nsis/installer.nsi`).
        Some(dir) if dir.join(format!("{product_name}.lnk")).is_file() => ToastSender::Clickable,
        _ => ToastSender::VisibleOnly,
    }
}

/// The Start Menu Programs folder, which is where the installer's `currentUser`
/// shortcut lands: NSIS resolves `$SMPROGRAMS` to
/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs` for that install mode.
#[cfg(windows)]
fn programs_folder() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|app_data| app_data.join(r"Microsoft\Windows\Start Menu\Programs"))
}

/// Whether this process's toasts can carry a click, asked once: the installer
/// would have to run again for the answer to move under a live app.
#[cfg(windows)]
fn clickable(app: &AppHandle) -> bool {
    *CLICKABLE.get_or_init(|| {
        attention_sender(programs_folder().as_deref(), &app.package_info().name)
            == ToastSender::Clickable
    })
}

#[cfg(windows)]
static CLICKABLE: OnceLock<bool> = OnceLock::new();

/// Whether this process ever showed a toast Windows could keep and route.
#[cfg(windows)]
static SHOWED: AtomicBool = AtomicBool::new(false);

/// The toast, through the one sender this install state can honestly use: an
/// uninstalled run keeps the visible toast it has always had instead of one
/// Windows may never show.
fn show(app: &AppHandle, toast: &AttentionToast) -> Result<(), String> {
    #[cfg(windows)]
    if clickable(app) {
        winrt::show(app, toast)?;
        SHOWED.store(true, Ordering::Relaxed);
        return Ok(());
    }
    send_with_plugin(app, toast)
}

/// Drops this app's toasts from Action Center on an orderly exit: one left
/// there outlives the process, and the target its click would open dies with
/// it. A crash is not covered.
#[cfg(windows)]
pub(crate) fn clear_pending(app: &AppHandle) {
    if !SHOWED.load(Ordering::Relaxed) {
        return;
    }
    // ClearWithId takes the AUMID's whole history, a second live instance's toasts included; one instance is the normal case.
    winrt::clear_history(app);
}

/// The notification plugin's own sender: every non-Windows platform's toast
/// (macOS keeps the toast it has always shown, with no click to route), and the
/// Windows run that owns no AUMID.
fn send_with_plugin(app: &AppHandle, toast: &AttentionToast) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(toast.title.clone())
        .body(toast.body.clone())
        .show()
        .map_err(|error| error.to_string())
}

/// Windows' own WinRT toast, the one platform whose click this app can hear.
#[cfg(windows)]
mod winrt {
    use tauri::{AppHandle, Emitter};
    use windows::core::HSTRING;
    use windows::UI::Notifications::ToastNotificationManager;

    use super::{AttentionTarget, AttentionToast};
    use crate::tray;

    /// The app event a click publishes. The frontend's listener
    /// (`attentionActivation.ts`) is the one reader; both sides declare the name.
    const ACTIVATED_EVENT: &str = "attention:activated";

    /// Drops every toast this app raised from Action Center, by the one AUMID
    /// they all carry.
    pub(super) fn clear_history(app: &AppHandle) {
        let Ok(history) = ToastNotificationManager::History() else {
            return;
        };
        let _ = history.ClearWithId(&HSTRING::from(app.config().identifier.as_str()));
    }

    /// The toast, with the click wired. The app id is the bundle identifier —
    /// the AUMID the installer writes onto the Start-menu shortcut
    /// (tauri-bundler's `SetLnkAppUserModelId`, called from
    /// `src-tauri/nsis/installer.nsi`), so an installed app's toast belongs to
    /// the shortcut the OS already knows.
    pub(super) fn show(app: &AppHandle, toast: &AttentionToast) -> Result<(), String> {
        let app_id = app.config().identifier.clone();
        let handle = app.clone();
        let target = toast.target.clone();
        tauri_winrt_notification::Toast::new(&app_id)
            .title(&toast.title)
            .text1(&toast.body)
            .on_activated(move |_| {
                activate(&handle, &target);
                Ok(())
            })
            .show()
            .map_err(|error| error.to_string())
    }

    /// A click arrives on the WinRT callback thread; the window and the event
    /// both belong to the app's main thread, so the whole act is posted there
    /// together.
    fn activate(app: &AppHandle, target: &AttentionTarget) {
        let handle = app.clone();
        let target = target.clone();
        // A click during shutdown has nothing to bring forward and no reader left.
        let _ = app.run_on_main_thread(move || {
            tray::show_main_window(&handle);
            let _ = handle.emit(ACTIVATED_EVENT, target);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toast_request_parses_the_frontend_shape() {
        let toast: AttentionToast = serde_json::from_value(serde_json::json!({
            "title": "agent one — needs approval",
            "body": "Run the migration",
            "target": { "sessionId": "s1", "workspaceId": "workspace-1" },
        }))
        .expect("the frontend's toast shape");
        assert_eq!(toast.title, "agent one — needs approval");
        assert_eq!(toast.body, "Run the migration");
        assert_eq!(
            toast.target,
            AttentionTarget {
                session_id: "s1".to_string(),
                workspace_id: Some("workspace-1".to_string()),
            }
        );
    }

    #[test]
    fn the_activation_payload_is_the_target_alone() {
        // The event the frontend reads carries ids and nothing else: never a
        // toast word, and a null workspace when the raise had none.
        let target = AttentionTarget {
            session_id: "s1".to_string(),
            workspace_id: None,
        };
        assert_eq!(
            serde_json::to_value(&target).expect("the payload serializes"),
            serde_json::json!({ "sessionId": "s1", "workspaceId": null })
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_installers_shortcut_decides_whether_the_toast_can_carry_a_click() {
        // A temp folder stands in for the Start Menu: the decision is the
        // installer's own file, never the real folder the machine has.
        let programs = tempfile::tempdir().expect("a temp Programs folder");
        assert_eq!(
            attention_sender(Some(programs.path()), "Devboule"),
            ToastSender::VisibleOnly
        );
        std::fs::write(programs.path().join("Devboule.lnk"), b"").expect("the shortcut");
        assert_eq!(
            attention_sender(Some(programs.path()), "Devboule"),
            ToastSender::Clickable
        );
        // Another product's shortcut registers another AUMID, not ours.
        std::fs::remove_file(programs.path().join("Devboule.lnk")).expect("remove it");
        std::fs::write(programs.path().join("Other.lnk"), b"").expect("another shortcut");
        assert_eq!(
            attention_sender(Some(programs.path()), "Devboule"),
            ToastSender::VisibleOnly
        );
        assert_eq!(attention_sender(None, "Devboule"), ToastSender::VisibleOnly);
    }
}
