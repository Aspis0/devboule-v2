//! The platform calls a browser page needs that Tauri 2.11 does not expose:
//! history, stop, and the page's favicon. WebView2 answers all three over
//! COM; every other target refuses them by name rather than pretending, so a
//! caller there sees a disabled control instead of one that silently does
//! nothing.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

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
  use super::{Act, AppHandle, Manager, PageFacts};
  use std::sync::mpsc;
  use std::time::Duration;

  use tauri::{Wry, Webview};
  use webview2_com::Microsoft::Web::WebView2::Win32::*;
  use webview2_com::{
    ExecuteScriptCompletedHandler, PermissionRequestedEventHandler,
    COREWEBVIEW2_PERMISSION_STATE_DENY,
  };
  use windows::core::{BOOL, HSTRING};

  const CALL_TIMEOUT: Duration = Duration::from_secs(10);

  /// The page's declared icon as a JSON value, or `null` when it has none.
  /// The history answers are spliced around it, so one round trip carries the
  /// whole of `PageFacts`.
  const FAVICON_SCRIPT: &str =
    "(function(){var l=document.querySelector('link[rel~=\"icon\"]');return JSON.stringify(l?l.href:null);})()";

  /// Refuse every permission the page asks for, the sensors included: an
  /// embedded page cannot show a prompt the user can attribute to the site
  /// they meant, so there is nothing to ask about.
  pub fn deny_permissions(webview: &Webview<Wry>) -> Result<(), String> {
    let mut token: i64 = 0;
    webview
      .with_webview(move |pw| unsafe {
        pw.controller()
          .CoreWebView2()?
          .add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(|_, args| {
              if let Some(args) = args {
                args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
              }
              Ok(())
            })),
            &mut token,
          )
      })
      .map_err(|e| format!("permission denial: {e}"))
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
          let core = unsafe { pw.controller().CoreWebView2() }?;
          f(core, tx)
        })();
        if let Err(error) = outcome {
          let _ = err_tx.send(Err(error.to_string()));
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
          Act::Back => core.GoBack()?,
          Act::Forward => core.GoForward()?,
          Act::Stop => core.Stop()?,
        }
      }
      let _ = tx.send(Ok(String::new()));
      Ok(())
    })
    .await
  }

  pub async fn facts(app: &AppHandle, label: &str) -> Result<PageFacts, String> {
    let json = on_ui(app, label, move |core, tx| unsafe {
      let (mut back, mut forward) = (BOOL(0), BOOL(0));
      core.CanGoBack(&mut back)?;
      core.CanGoForward(&mut forward)?;
      let script = HSTRING::from(FAVICON_SCRIPT);
      let handler = ExecuteScriptCompletedHandler::create(Box::new(move |hr, favicon| {
        if let Err(error) = hr {
          let _ = tx.send(Err(format!("The page refused to be read: {error}")));
          return Ok(());
        }
        let _ = tx.send(Ok(format!(
          r#"{{"canGoBack":{},"canGoForward":{},"favicon":{favicon}}}"#,
          back.as_bool(),
          forward.as_bool()
        )));
        Ok(())
      }));
      core.ExecuteScript(&script, &handler)?;
      Ok(())
    })
    .await?;
    serde_json::from_str(&json).map_err(|_| "The page answered with something unreadable.".to_owned())
  }
}

#[cfg(not(windows))]
mod imp {
  use super::{Act, AppHandle, PageFacts};
  use tauri::Wry;

  pub fn deny_permissions(_webview: &Webview<Wry>) -> Result<(), String> {
    Ok(())
  }

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

pub use imp::{act, deny_permissions, facts};
