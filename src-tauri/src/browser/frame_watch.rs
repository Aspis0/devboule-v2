//! The frame gate: WebView2's frame-navigation event fires for every nested
//! document, which the builder's own navigation hook does not see. A frame of
//! an agent-tainted tab is checked with the same policy and cancelled the same
//! way; a person's frames are never checked.

use tauri::{Webview, Wry};
use webview2_com::{take_pwstr, NavigationStartingEventHandler};
use windows::core::PWSTR;

use super::tab_watch::TabWatch;

/// Install the gate on one tab's webview. Best effort: the tab's top-level
/// navigations are gated by the builder's hook whatever happens here, so a
/// failure is reported and not fatal.
pub(super) fn install(webview: &Webview<Wry>, watch: TabWatch) {
    let outcome = webview.with_webview(move |platform| {
        let core = match unsafe { platform.controller().CoreWebView2() } {
            Ok(core) => core,
            Err(error) => {
                eprintln!("devboule: the browser frame gate could not reach the webview: {error}");
                return;
            }
        };
        let handler = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            // A frame whose address cannot be read is cancelled for an agent's
            // tab: nothing says where it goes.
            let refused = match unsafe { args.Uri(&mut uri) } {
                Ok(()) => watch.blocked_frame(&take_pwstr(uri)).is_some(),
                Err(_) => watch.blocked_frame("").is_some(),
            };
            if refused {
                let _ = unsafe { args.SetCancel(true) };
            }
            Ok(())
        }));
        // The token is not kept: the handler lives as long as the webview.
        let mut token = 0i64;
        if let Err(error) = unsafe { core.add_FrameNavigationStarting(&handler, &mut token) } {
            eprintln!("devboule: the browser frame gate did not install: {error}");
        }
    });
    if let Err(error) = outcome {
        eprintln!("devboule: the browser frame gate did not install: {error}");
    }
}
