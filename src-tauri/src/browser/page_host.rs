//! The platform calls a browser page needs that Tauri 2.11 does not expose:
//! history, stop, and the page's favicon. WebView2 answers all three over
//! COM; every other target refuses them by name rather than pretending, so a
//! caller there sees a disabled control instead of one that silently does
//! nothing.

use serde::{Deserialize, Serialize};

use super::tab::BrowserChord;

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
    use super::{chord_for, usable_icon, Act, BrowserChord, PageFacts};
    use std::sync::mpsc;
    use std::time::Duration;

    use tauri::{AppHandle, Manager, Webview, Wry};
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{
        AcceleratorKeyPressedEventHandler, DownloadStartingEventHandler,
        ExecuteScriptCompletedHandler, PermissionRequestedEventHandler,
    };
    use windows::core::{Interface, BOOL, HSTRING};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };

    const CALL_TIMEOUT: Duration = Duration::from_secs(10);

    /// The page's declared icon as a `data:` URL, or `null`. The bytes are
    /// read IN the page, so the app frame never contacts the icon's host: an
    /// `<img>` in the app's own webview pointed at a page's URL would be a
    /// third party beacon fired from a record on disk, on every start.
    ///
    /// Synchronous on purpose: `ExecuteScript` answers with the value of its
    /// expression, and a promise is not a value. A cross-origin icon this
    /// cannot read - no CORS, no page - is a `null`, not a fetch the app makes
    /// on the page's behalf.
    const FAVICON_SCRIPT: &str = r#"(function () {
  var link = document.querySelector('link[rel~="icon"]');
  if (!link) return "null";
  try {
    var request = new XMLHttpRequest();
    request.open("GET", link.href, false);
    request.overrideMimeType("text/plain; charset=x-user-defined");
    request.send(null);
    if (request.status !== 0 && request.status !== 200) return "null";
    var bytes = request.responseText;
    if (bytes.length > 16384) return "null";
    var binary = "";
    for (var i = 0; i < bytes.length; i += 1) binary += String.fromCharCode(bytes.charCodeAt(i) & 0xff);
    return JSON.stringify("data:image/png;base64," + btoa(binary));
  } catch (error) {
    return "null";
  }
})()"#;

    /// Every handler this app answers a page with, installed in one blocking
    /// round trip and before the page is allowed to load anything:
    ///
    /// - every permission refused, the sensors included: an embedded page
    ///   cannot show a prompt the user can attribute to the site they meant,
    ///   so there is nothing to ask about;
    /// - every download cancelled, with one note on the pane's own line;
    /// - the two chords a browser tab answers wherever the focus is, reported
    ///   down the tab's own channel.
    ///
    /// One call because one gap is the whole risk: a page that asks for
    /// something between being created and being restricted has asked a
    /// question this app had not installed a "no" for yet. So a failure here
    /// fails the open — an unrestricted page is never shown.
    pub fn restrict(
        webview: &Webview<Wry>,
        on_download: impl Fn() + Send + 'static,
        on_chord: impl Fn(BrowserChord) + Send + 'static,
    ) -> Result<(), String> {
        on_controller(webview, move |controller, tx| {
            let mut token: i64 = 0;
            unsafe {
                let core = controller.CoreWebView2().map_err(com)?;
                core.add_PermissionRequested(
                    &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                        if let Some(args) = args {
                            args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                        }
                        Ok(())
                    })),
                    &mut token,
                )
                .map_err(com)?;
                // `DownloadStarting` arrived with the fourth revision of the
                // core interface. Without it a download runs on WebView2's own
                // terms, which is no policy at all.
                let core = core.cast::<ICoreWebView2_4>().map_err(com)?;
                core.add_DownloadStarting(
                    &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                        if let Some(args) = args {
                            args.SetCancel(true)?;
                            on_download();
                        }
                        Ok(())
                    })),
                    &mut token,
                )
                .map_err(com)?;
                controller
                    .add_AcceleratorKeyPressed(
                        &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                            if let Some(args) = args {
                                if let Some(chord) = pressed_chord(&args) {
                                    // The page must not also see its own Ctrl+L:
                                    // the app answered it.
                                    args.SetHandled(true)?;
                                    on_chord(chord);
                                }
                            }
                            Ok(())
                        })),
                        &mut token,
                    )
                    .map_err(com)?;
            }
            let _ = tx.send(Ok(()));
            Ok(())
        })
    }

    /// The chord a key press is, or None. The modifiers are read here because
    /// only the OS knows them; which key means what is [`chord_for`].
    fn pressed_chord(args: &ICoreWebView2AcceleratorKeyPressedEventArgs) -> Option<BrowserChord> {
        let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
        let mut key = 0u32;
        unsafe {
            args.KeyEventKind(&mut kind).ok()?;
            args.VirtualKey(&mut key).ok()?;
        }
        if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
            return None;
        }
        chord_for(key as u16, held(VK_CONTROL), held(VK_SHIFT), held(VK_MENU))
    }

    /// Whether a modifier key is down right now. WebView2 reports the key and
    /// the event kind on a chord; the modifiers are only on the OS.
    fn held(key: VIRTUAL_KEY) -> bool {
        unsafe { GetKeyState(key.0 as i32) < 0 }
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

    /// Run `f` against this webview's `ICoreWebView2Controller` on the UI
    /// thread and wait for its answer off it. `with_webview` posts the closure
    /// to the event loop and returns, so without the wait the caller would be
    /// told "installed" about something that has not happened yet.
    ///
    /// The callers are `async`, so this blocks a tokio worker; blocking the
    /// main thread instead would deadlock, because every WebView2 completion
    /// arrives through that thread's message pump.
    fn on_controller<F>(webview: &Webview<Wry>, f: F) -> Result<(), String>
    where
        F: FnOnce(ICoreWebView2Controller, mpsc::Sender<Result<(), String>>) -> Result<(), String>
            + Send
            + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let err_tx = tx.clone();
        webview
            .with_webview(move |pw| {
                if let Err(error) = f(pw.controller(), tx) {
                    let _ = err_tx.send(Err(error));
                }
            })
            .map_err(|e| format!("with_webview: {e}"))?;
        match rx.recv_timeout(CALL_TIMEOUT) {
            Ok(result) => result,
            Err(_) => Err("The page did not answer.".to_owned()),
        }
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
        let mut facts: PageFacts = serde_json::from_str(&json)
            .map_err(|_| "The page answered with something unreadable.".to_owned())?;
        // An icon is only ever carried as data: a URL here would be persisted
        // and rendered by the app's own webview, which is the beacon this
        // whole path exists to avoid.
        facts.favicon = usable_icon(facts.favicon);
        Ok(facts)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Act, BrowserChord};
    use tauri::{AppHandle, Webview, Wry};

    /// Nothing to restrict: this target has no page to restrict.
    pub fn restrict(
        _webview: &Webview<Wry>,
        _on_download: impl Fn() + Send + 'static,
        _on_chord: impl Fn(BrowserChord) + Send + 'static,
    ) -> Result<(), String> {
        Ok(())
    }

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

/// The icon this app will carry, or None. Data only: a URL would be persisted
/// and then rendered by the app's own webview, which is the third party beacon
/// this whole path exists to avoid, and it is persisted across restarts.
fn usable_icon(icon: Option<String>) -> Option<String> {
    const MAX_CHARS: usize = 26_400; // 16 KiB of bytes as base64, plus the prefix
    icon.filter(|icon| icon.starts_with("data:image/") && icon.len() <= MAX_CHARS)
}

/// Which key, with which modifiers, is one of this app's two chords. The same
/// rules as the app's keymap: Ctrl with L or R, and Alt or Shift with it is a
/// different chord entirely.
fn chord_for(key: u16, control: bool, shift: bool, alt: bool) -> Option<BrowserChord> {
    if !control || shift || alt {
        return None;
    }
    match key {
        0x4C => Some(BrowserChord::FocusAddress),
        0x52 => Some(BrowserChord::Reload),
        _ => None,
    }
}

pub use imp::{act, facts, raise, restrict};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ctrl_with_l_or_r_is_a_chord() {
        assert_eq!(
            chord_for(0x4C, true, false, false),
            Some(BrowserChord::FocusAddress)
        );
        assert_eq!(
            chord_for(0x52, true, false, false),
            Some(BrowserChord::Reload)
        );
        // A letter on its own is text.
        assert_eq!(chord_for(0x4C, false, false, false), None);
        assert_eq!(chord_for(0x4B, true, false, false), None);
        // And Alt or Shift with it is a different chord, never this one.
        assert_eq!(chord_for(0x4C, true, true, false), None);
        assert_eq!(chord_for(0x52, true, false, true), None);
    }

    #[test]
    fn an_icon_is_data_or_it_is_nothing() {
        let icon = "data:image/png;base64,iVBORw0KGgo=";
        assert_eq!(usable_icon(Some(icon.to_owned())), Some(icon.to_owned()));
        // A page's own address is never carried: it would be fetched by the
        // app's webview, from a record on disk, on every start.
        assert_eq!(
            usable_icon(Some("https://example.com/icon.png".to_owned())),
            None
        );
        assert_eq!(usable_icon(Some("javascript:alert(1)".to_owned())), None);
        // Bigger than 16 KiB of bytes is dropped rather than persisted.
        assert_eq!(
            usable_icon(Some(format!(
                "data:image/png;base64,{}",
                "A".repeat(30_000)
            ))),
            None
        );
        assert_eq!(usable_icon(None), None);
    }
}
