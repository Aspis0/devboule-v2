//! The platform calls a browser page needs that Tauri 2.11 does not expose:
//! history, stop, and the page's favicon. WebView2 answers all three over
//! COM; every other target refuses them by name rather than pretending, so a
//! caller there sees a disabled control instead of one that silently does
//! nothing.

use serde::{Deserialize, Serialize};

/// What a finished page load reports back to the chrome.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageFacts {
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// The page's own `<link rel="icon">`, when it declares one.
    pub favicon: Option<String>,
}

/// The history buttons and stop. Reload is Tauri's own `Webview::reload`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Act {
    Back,
    Forward,
    Stop,
}

#[cfg(windows)]
mod imp {
    use super::{Act, PageFacts};
    use std::sync::mpsc;
    use std::time::Duration;

    use tauri::{AppHandle, Manager, Webview, Wry};
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{ExecuteScriptCompletedHandler, PermissionRequestedEventHandler};
    use windows::core::{BOOL, HSTRING};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };

    const CALL_TIMEOUT: Duration = Duration::from_secs(10);

    /// The page's declared icon as a JSON value, or `null` when it has none.
    /// The history answers are spliced around it, so one round trip carries the
    /// whole of `PageFacts`.
    const FAVICON_SCRIPT: &str =
    "(function(){var l=document.querySelector('link[rel~=\"icon\"]');return JSON.stringify(l?l.href:null);})()";

    /// Refuse every permission the page asks for, the sensors included: an
    /// embedded page cannot show a prompt the user can attribute to the site
    /// they meant, so there is nothing to ask about.
    ///
    /// `with_webview` posts the closure to the event loop and returns, so both
    /// failures are reported where the app reports its own rather than through
    /// a return value nobody is waiting for.
    pub fn deny_permissions(webview: &Webview<Wry>) {
        let mut token: i64 = 0;
        let denial = webview.with_webview(move |pw| {
            let installed = unsafe {
                pw.controller().CoreWebView2().and_then(|core| {
                    core.add_PermissionRequested(
                        &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                            if let Some(args) = args {
                                args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                            }
                            Ok(())
                        })),
                        &mut token,
                    )
                })
            };
            if let Err(error) = installed {
                eprintln!("devboule: browser permission denial could not be installed: {error}");
            }
        });
        if let Err(error) = denial {
            eprintln!("devboule: browser permission denial was never scheduled: {error}");
        }
    }

    /// Bring the child webview's window above the main webview's.
    ///
    /// wry parents every webview in its own `WS_CHILD` container, and a
    /// container created later already sits above an earlier one, so this is
    /// normally a no-op. It is here because "the page is loaded but nothing is
    /// painted" is indistinguishable from a broken pane otherwise, and the
    /// order is Windows' to decide rather than this app's to assume.
    ///
    /// Posted to the event loop like every other COM call here, so a failure
    /// is reported where the app reports its own rather than through a return
    /// value nobody is waiting for.
    pub fn raise(webview: &Webview<Wry>) {
        let scheduled = webview.with_webview(move |pw| {
            let outcome = (|| {
                // The controller's own parent window is the container wry
                // built for this webview, and that is what has to come up.
                unsafe {
                    let mut container = HWND::default();
                    pw.controller().ParentWindow(&mut container)?;
                    SetWindowPos(
                        container,
                        Some(HWND_TOP),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    )
                }
            })();
            if let Err(error) = outcome {
                eprintln!("devboule: browser page could not be raised: {error}");
            }
        });
        if let Err(error) = scheduled {
            eprintln!("devboule: browser page raise was never scheduled: {error}");
        }
    }

    fn com(error: windows::core::Error) -> String {
        error.to_string()
    }

    /// Run `f` against the page's `ICoreWebView2` on the UI thread and wait for
    /// its completion off it. The callers are `async`, so this blocks a tokio
    /// worker; blocking the main thread instead would deadlock, because every
    /// WebView2 completion arrives through that thread's message pump.
    async fn on_ui<F>(app: &AppHandle, label: &str, f: F) -> Result<String, String>
    where
        F: FnOnce(ICoreWebView2, mpsc::Sender<Result<String, String>>) -> Result<(), String>
            + Send
            + 'static,
    {
        let webview = app
            .get_webview(label)
            .ok_or_else(|| "This browser tab is no longer open.".to_owned())?;
        let (tx, rx) = mpsc::channel();
        let err_tx = tx.clone();
        webview
            .with_webview(move |pw| {
                let outcome = (|| {
                    let core = unsafe { pw.controller().CoreWebView2() }.map_err(com)?;
                    f(core, tx)
                })();
                if let Err(error) = outcome {
                    let _ = err_tx.send(Err(error));
                }
            })
            .map_err(|e| format!("with_webview: {e}"))?;
        match rx.recv_timeout(CALL_TIMEOUT) {
            Ok(result) => result,
            Err(_) => Err("The page did not answer.".to_owned()),
        }
    }

    pub async fn act(app: &AppHandle, label: &str, what: Act) -> Result<(), String> {
        on_ui(app, label, move |core, tx| {
            unsafe {
                match what {
                    Act::Back => core.GoBack(),
                    Act::Forward => core.GoForward(),
                    Act::Stop => core.Stop(),
                }
            }
            .map_err(com)?;
            let _ = tx.send(Ok(String::new()));
            Ok(())
        })
        .await
        .map(|_| ())
    }

    pub async fn facts(app: &AppHandle, label: &str) -> Result<PageFacts, String> {
        let json = on_ui(app, label, move |core, tx| unsafe {
            let (mut back, mut forward) = (BOOL(0), BOOL(0));
            core.CanGoBack(&mut back).map_err(com)?;
            core.CanGoForward(&mut forward).map_err(com)?;
            let script = HSTRING::from(FAVICON_SCRIPT);
            let handler = ExecuteScriptCompletedHandler::create(Box::new(move |hr, favicon| {
                if let Err(error) = hr {
                    let _ = tx.send(Err(format!("The page refused to be read: {}", com(error))));
                    return Ok(());
                }
                let _ = tx.send(Ok(format!(
                    r#"{{"canGoBack":{},"canGoForward":{},"favicon":{favicon}}}"#,
                    back.as_bool(),
                    forward.as_bool()
                )));
                Ok(())
            }));
            core.ExecuteScript(&script, &handler).map_err(com)?;
            Ok(())
        })
        .await?;
        serde_json::from_str(&json)
            .map_err(|_| "The page answered with something unreadable.".to_owned())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Act, PageFacts};
    use tauri::{AppHandle, Webview, Wry};

    pub fn deny_permissions(_webview: &Webview<Wry>) {}

    /// One window manager owns the stacking order on this platform, so there
    /// is nothing for a page's window to be raised above.
    pub fn raise(_webview: &Webview<Wry>) {}

    pub async fn act(_app: &AppHandle, _label: &str, what: Act) -> Result<(), String> {
        Err(match what {
            Act::Back => "Going back is not available on this platform.".to_owned(),
            Act::Forward => "Going forward is not available on this platform.".to_owned(),
            Act::Stop => "Stopping the load is not available on this platform.".to_owned(),
        })
    }

    pub async fn facts(_app: &AppHandle, _label: &str) -> Result<PageFacts, String> {
        Ok(PageFacts::default())
    }
}

pub use imp::{act, deny_permissions, facts, raise};
