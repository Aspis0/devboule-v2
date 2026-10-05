//! The WebView2 half of the event layer: one runtime receiver per event on a
//! browser child, each feeding the shared [`Listener`].
//!
//! One receiver per event per page, installed when the page is created and
//! never removed: the handler belongs to the child webview, which a close
//! disposes of, and a page watched twice would move its own counter twice.
//! What a close does drop is the counter, so the map holds only live tabs.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use super::{ask_for_events, signal_for, Listener, Reports};
use crate::browser::cdp::{Bounded, WebviewPage};
use crate::browser::console;
use crate::browser::deadline::Deadline;
use crate::browser::frames::Frames;

/// The events one page is watched for. `DOM.documentUpdated` needs no enable;
/// `Page.*` does, which is why [`watch`] enables the page before asking for
/// them.
const WATCHED: [&str; 5] = [
    "DOM.documentUpdated",
    "Page.frameNavigated",
    "Page.navigatedWithinDocument",
    "Page.loadEventFired",
    "Page.frameStoppedLoading",
];

/// The frames a still-blank page starts with, learned before any receiver is
/// installed so a same-document move is read against its own frame from the
/// first event.
async fn page_events(page: &dyn crate::browser::cdp::Page, id: &str) -> Arc<Frames> {
    let frames = Arc::new(Frames::default());
    ask_for_events(page, id, &frames).await;
    frames
}

/// Watch a page for as long as `deadline` allows. What is not installed in time
/// is not installed: the tab is already open, and a settle without the event
/// falls back to its cap.
pub async fn watch(app: &AppHandle, id: &str, label: &str, deadline: Deadline, reports: Reports) {
    signal_for(id);
    console::open(id);
    let webview = WebviewPage::new(app, label);
    let page = Bounded::new(&webview, deadline);
    let frames = page_events(&page, id).await;
    for event in WATCHED.iter().chain(console::VOICE.iter()).copied() {
        let listener = Listener {
            id: id.to_owned(),
            frames: Arc::clone(&frames),
            reports: reports.clone(),
        };
        if let Err(error) = subscribe(app, label, event, deadline.left(), listener).await {
            // Without this event the settle falls back to its cap, which is
            // slower and no worse than not settling at all.
            eprintln!("devboule: browser page {id} is not watched for {event}: {error}");
        }
    }
}

async fn subscribe(
    app: &AppHandle,
    label: &str,
    event: &str,
    limit: Duration,
    listener: Listener,
) -> Result<(), String> {
    use webview2_com::{take_pwstr, DevToolsProtocolEventReceivedEventHandler};
    use windows::core::HSTRING;

    let webview = app
        .get_webview(label)
        .ok_or_else(|| "This browser tab is no longer open.".to_owned())?;
    let (tx, rx) = mpsc::channel();
    let owned_event = event.to_owned();
    webview
        .with_webview(move |pw| {
            let outcome = (|| {
                let core = unsafe { pw.controller().CoreWebView2() }.map_err(com)?;
                let watched = HSTRING::from(owned_event.as_str());
                let receiver =
                    unsafe { core.GetDevToolsProtocolEventReceiver(&watched) }.map_err(com)?;
                let named = owned_event.clone();
                let handler =
                    DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                        let mut params = String::new();
                        if let Some(args) = args {
                            let mut buffer = windows::core::PWSTR::default();
                            if unsafe { args.ParameterObjectAsJson(&mut buffer) }.is_ok() {
                                params = take_pwstr(buffer);
                            }
                        }
                        listener.heard(&named, &params);
                        Ok(())
                    }));
                // The token the runtime hands back is not kept: nothing here
                // removes this handler, and the child webview it belongs to is
                // what a close disposes of.
                let mut token = 0i64;
                unsafe { receiver.add_DevToolsProtocolEventReceived(&handler, &mut token) }
                    .map_err(com)
            })();
            let _ = tx.send(outcome.map_err(|error| error.to_string()));
        })
        .map_err(|error| format!("with_webview: {error}"))?;
    rx.recv_timeout(limit)
        .unwrap_or(Err("The page did not answer.".to_owned()))
}

fn com(error: windows::core::Error) -> String {
    error.to_string()
}
