//! The OS toast an attention raise becomes, and what a click on it means.
//!
//! One responsibility: show the toast that stands for a raise and turn a click
//! on it into the window coming forward plus one app event that names the
//! session the toast was about.

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

/// The toast, with the click wired where the platform allows one.
///
/// A show the Windows notification platform refuses — an AUMID it will not
/// take — falls back to the plugin's own sender: a toast without a click is a
/// smaller loss than a raise nobody sees.
fn show(app: &AppHandle, toast: &AttentionToast) -> Result<(), String> {
    #[cfg(windows)]
    match winrt::show(app, toast) {
        Ok(()) => return Ok(()),
        Err(error) => eprintln!(
            "devboule: the attention toast did not reach the Windows notification platform: {error}"
        ),
    }
    send_with_plugin(app, toast)
}

/// The notification plugin's own sender: every non-Windows platform's toast
/// (macOS keeps the toast it has always shown, with no click to route), and
/// Windows' fallback when WinRT takes no toast at all.
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

    use super::{AttentionTarget, AttentionToast};
    use crate::tray;

    /// The app event a click publishes. The frontend's listener
    /// (`attentionActivation.ts`) is the one reader; both sides declare the name.
    const ACTIVATED_EVENT: &str = "attention:activated";

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
}
