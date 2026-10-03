//! The 4b spike's window onto WebView2's DevTools Protocol: forward one call to
//! one browser child, and record the events a subscription asked for. Its only
//! purpose is to make the numbers in `scout/browser-tabs/SPIKE-REPORT-cdp.md`
//! measurements of this machine's runtime rather than assumptions about it.

#![cfg(debug_assertions)]

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager, State};
use webview2_com::{
    take_pwstr, CallDevToolsProtocolMethodCompletedHandler,
    DevToolsProtocolEventReceivedEventHandler,
};
use windows::core::{HSTRING, PWSTR};

use super::registry::BrowserRegistry;

/// Every WebView2 completion arrives through the UI thread's message pump, so
/// waiting past this is waiting on a thread that is already gone.
const CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// Emptied by every [`browser_cdp_events`] call. The cap is a backstop for a
/// caller that never polls, and each record says how many older ones it took
/// with it, so a gap never passes for a quiet page.
const MAX_RECORDED: usize = 4_096;
static RECORDED: Mutex<Vec<serde_json::Value>> = Mutex::new(Vec::new());

/// Forward one CDP call to a browser child's WebView2, and answer with its
/// result JSON or with the failure text. Method and parameters go through
/// verbatim, including for a method nothing here has heard of: which methods
/// this runtime answers is the question, and a typed signature would answer
/// "no such command" for everything it did not name.
#[tauri::command]
pub async fn browser_cdp_probe(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    id: String,
    method: String,
    params_json: String,
) -> Result<String, String> {
    on_child(&app, &registry, &id, move |core, tx| unsafe {
        let label = method.clone();
        let (name, params) = (HSTRING::from(method), HSTRING::from(params_json));
        let handler =
            CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |hr, result| {
                let _ = tx.send(
                    hr.map(|()| result)
                        .map_err(|error| format!("{label}: {error}")),
                );
                Ok(())
            }));
        core.CallDevToolsProtocolMethod(&name, &params, &handler)
            .map_err(com)
    })
    .await
}

/// Subscribe a browser child to the named CDP events, then hand back every
/// event recorded so far. There is no matching unsubscribe: a probe that has
/// to be stopped is a probe whose process is about to be killed.
#[tauri::command]
pub async fn browser_cdp_events(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    id: String,
    watch: Vec<String>,
) -> Result<String, String> {
    for event in watch {
        let (tab, named) = (id.clone(), event.clone());
        on_child(&app, &registry, &id, move |core, tx| unsafe {
            let receiver = core
                .GetDevToolsProtocolEventReceiver(&HSTRING::from(event))
                .map_err(com)?;
            let handler =
                DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                    let params = args
                        .map(|args| {
                            let mut buffer = PWSTR::default();
                            args.ParameterObjectAsJson(&mut buffer)
                                .map(|()| take_pwstr(buffer))
                        })
                        .unwrap_or_else(|| Ok(String::from("null")));
                    record(
                        &tab,
                        &named,
                        &params.unwrap_or_else(|error| format!("\"{error}\"")),
                    );
                    Ok(())
                }));
            let mut token = 0i64;
            receiver
                .add_DevToolsProtocolEventReceived(&handler, &mut token)
                .map_err(com)?;
            let _ = tx.send(Ok(String::new()));
            Ok(())
        })
        .await?;
    }
    let recorded = std::mem::take(&mut *RECORDED.lock().expect("cdp probe events poisoned"));
    Ok(serde_json::Value::Array(recorded).to_string())
}

/// Add one event to the record. `params` stays the JSON text WebView2 handed
/// over rather than being parsed, so a malformed event is still visible.
fn record(tab: &str, event: &str, params: &str) {
    let mut recorded = RECORDED.lock().expect("cdp probe events poisoned");
    let mut dropped = 0;
    if recorded.len() >= MAX_RECORDED {
        dropped = recorded.len() + 1 - MAX_RECORDED;
        recorded.drain(..dropped);
    }
    recorded.push(serde_json::json!({
        "tab": tab, "event": event, "params": params, "dropped": dropped,
    }));
}

fn com(error: windows::core::Error) -> String {
    error.to_string()
}

/// Run `f` against a browser child's `ICoreWebView2` on the UI thread and wait
/// for its answer off it; `with_webview` returns before the closure has run.
/// Copy of `page_host`'s, because widening that one's visibility would leave
/// it dead, and a clippy warning, in every release build from here on.
async fn on_child<F>(
    app: &AppHandle,
    registry: &BrowserRegistry,
    id: &str,
    f: F,
) -> Result<String, String>
where
    F: FnOnce(
            webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
            mpsc::Sender<Result<String, String>>,
        ) -> Result<(), String>
        + Send
        + 'static,
{
    let label = registry.label_of(id)?;
    let webview = app
        .get_webview(&label)
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
