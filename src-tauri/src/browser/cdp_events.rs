//! One event subscription per browser page, and the quiet it reports.
//!
//! Settling is the whole reason this exists. An action's answer is only worth
//! anything if the page it happened on has stopped moving, and the way to know
//! that is the runtime's own event stream rather than a guess at a delay: the
//! DOM counters (`DOM.documentUpdated`, which arrives with no enable) and the
//! load events (`Page.*`, which arrive only after `Page.enable`).
//!
//! A handler is installed per page and never removed — this runtime has no
//! unsubscribe on the path `GetDevToolsProtocolEventReceiver` opens — so a
//! page is watched exactly once, when it is created, and its counter is
//! dropped when the tab closes.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Manager};

use super::cdp::{Page as _, WebviewPage};

/// What a page must go without before its state is believed.
pub const QUIET: Duration = Duration::from_millis(300);

/// How long a settle waits in total, however busy the page is. Past this the
/// answer is the page's state as far as it got, which is what a caller can
/// see anyway.
pub const SETTLE_CAP: Duration = Duration::from_secs(3);

/// The events one page is watched for. `DOM.documentUpdated` needs no enable;
/// `Page.*` does, which is why [`watch`] enables the page before asking for
/// them.
#[cfg(windows)]
const WATCHED: [&str; 4] = [
    "DOM.documentUpdated",
    "Page.frameNavigated",
    "Page.loadEventFired",
    "Page.frameStoppedLoading",
];

/// How much the page has moved since the tab was opened, per browser id.
static QUIET_SIGNAL: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// The signal one page's events bump, created when the page is watched.
fn signal_for(id: &str) {
    let mut pages = QUIET_SIGNAL.lock().expect("browser page signals poisoned");
    pages
        .get_or_insert_with(HashMap::new)
        .entry(id.to_owned())
        .or_insert(0);
}

/// How far the page has moved since the tab was opened.
pub fn moved(id: &str) -> u64 {
    let pages = QUIET_SIGNAL.lock().expect("browser page signals poisoned");
    pages
        .as_ref()
        .and_then(|pages| pages.get(id).copied())
        .unwrap_or(0)
}

/// Drop a page's signal. The close is the only place this happens, and every
/// close goes through it, so the map holds only live tabs.
pub fn forget(id: &str) {
    if let Some(pages) = QUIET_SIGNAL
        .lock()
        .expect("browser page signals poisoned")
        .as_mut()
    {
        pages.remove(id);
    }
}

#[cfg(windows)]
pub async fn watch(app: &AppHandle, id: &str, label: &str) {
    signal_for(id);
    let page = WebviewPage::new(app, label);
    // `Page.*` events arrive only after this, so it is asked for before any
    // receiver is installed.
    if let Err(error) = page.call("Page.enable", json!({})).await {
        eprintln!("devboule: browser page {id} will not report its loads: {error}");
    }
    for event in WATCHED {
        if let Err(error) = subscribe(app, label, id, event).await {
            // Without this event the settle falls back to its cap, which is
            // slower and no worse than not settling at all.
            eprintln!("devboule: browser page {id} is not watched for {event}: {error}");
        }
    }
}

#[cfg(windows)]
async fn subscribe(app: &AppHandle, label: &str, id: &str, event: &str) -> Result<(), String> {
    use webview2_com::{take_pwstr, DevToolsProtocolEventReceivedEventHandler};
    use windows::core::HSTRING;

    let webview = app
        .get_webview(label)
        .ok_or_else(|| "This browser tab is no longer open.".to_owned())?;
    let (tx, rx) = mpsc::channel();
    let owned_id = id.to_owned();
    let owned_event = event.to_owned();
    webview
        .with_webview(move |pw| {
            let outcome = (|| {
                let core = unsafe { pw.controller().CoreWebView2() }.map_err(com)?;
                let watched = HSTRING::from(owned_event.as_str());
                let receiver =
                    unsafe { core.GetDevToolsProtocolEventReceiver(&watched) }.map_err(com)?;
                let handler =
                    DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                        // The parameters are read only to prove an event
                        // arrived; nothing here needs what is in them.
                        if let Some(args) = args {
                            let mut buffer = windows::core::PWSTR::default();
                            if unsafe { args.ParameterObjectAsJson(&mut buffer) }.is_ok() {
                                take_pwstr(buffer);
                            }
                        }
                        bump(&owned_id);
                        Ok(())
                    }));
                let mut token = 0i64;
                unsafe { receiver.add_DevToolsProtocolEventReceived(&handler, &mut token) }
                    .map_err(com)
            })();
            let _ = tx.send(outcome.map_err(|error| error.to_string()));
        })
        .map_err(|error| format!("with_webview: {error}"))?;
    rx.recv()
        .unwrap_or(Err("The page did not answer.".to_owned()))
}

#[cfg(windows)]
fn bump(id: &str) {
    let mut pages = QUIET_SIGNAL.lock().expect("browser page signals poisoned");
    if let Some(page) = pages.as_mut().and_then(|pages| pages.get_mut(id)) {
        *page = page.wrapping_add(1);
    }
}

#[cfg(windows)]
fn com(error: windows::core::Error) -> String {
    error.to_string()
}

#[cfg(not(windows))]
pub async fn watch(_app: &AppHandle, _id: &str, _label: &str) {
    // No event stream on this target, so `moved` never moves and every settle
    // ends on its first quiet. Nothing here blocks a command from running.
}

/// Wait without holding a runtime worker: this crate links no async timer, and
/// every CDP call already blocks its worker for the length of the call.
pub async fn nap(for_: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(for_)).await;
}

/// Wait until the page has gone `QUIET` without an event, or `SETTLE_CAP` has
/// passed. A navigation and a re-render are the same signal here, which is
/// what the contract asks for: one answer per action, taken once the page has
/// stopped.
pub async fn settle(id: &str) {
    let deadline = std::time::Instant::now() + SETTLE_CAP;
    loop {
        let before = moved(id);
        nap(QUIET).await;
        if moved(id) == before || std::time::Instant::now() >= deadline {
            return;
        }
    }
}

#[cfg(test)]
#[path = "cdp_events_tests.rs"]
mod tests;
