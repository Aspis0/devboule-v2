//! The websocket half of the event layer: a reader's two streams drained into
//! the shared [`Listener`], and the close that stops it.
//!
//! The two streams carry different things — see `cdp_ws` — and both are read
//! here. A state event the reader had to drop is not forgotten: the loss is
//! counted, the drain is woken, and the listener treats the state as unknown.
//!
//! **The close never waits.** `Drop` marks [`Close`] and aborts the drain; the
//! drain checks the mark before every event and before every unknown-state
//! count, and the tab's reports check it before every callback. Nothing holds
//! a lock across a callback, so a report may drop its own watch and a runtime
//! thread may drop it while a report runs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::json;
use tokio::sync::Notify;

use super::{ask_for_events, listen, signal_for, Listener, Reports};
use crate::browser::cdp::{Bounded, Page as _};
use crate::browser::cdp_ws::{WsEvent, WsEvents};
use crate::browser::console;
use crate::browser::deadline::Deadline;
use crate::browser::frames::Frames;

/// Wire one websocket page's events into the ingestion the WebView2 path
/// feeds: the same counters, ring and reports, from the same normalized
/// payloads.
///
/// Called before the page is navigated. `Runtime` and `Log` are enabled for
/// its voice, `DOM` for the document updates a raw target reports only after
/// asking, and `Page` for the events that say where the tab is. What arrives
/// afterwards is counted exactly as a child webview's events are.
///
/// The drain runs from before the first setup call, so nothing the page says
/// while the domains are being switched on is lost to a queue holding it. It
/// reads the state stream only once the frame tree has been asked for: a
/// same-document move that arrived while the tree was still unknown is read
/// against the frame it happened in.
///
/// The guard is held for the tab's life: dropping it stops the drain task, and
/// the reader's own end stops it too.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn watch_ws(
    page: &dyn crate::browser::cdp::Page,
    id: &str,
    events: WsEvents,
    deadline: Deadline,
    reports: Reports,
) -> WsWatch {
    signal_for(id);
    console::open(id);
    let frames = Arc::new(Frames::default());
    let ready = Arc::new(AtomicBool::new(false));
    let close = Close::new();
    let notice = Arc::clone(&events.notice);
    let listener = Listener {
        id: id.to_owned(),
        frames: Arc::clone(&frames),
        reports: unclosed(&close, reports),
    };
    let drain = tauri::async_runtime::spawn(drain(
        listener,
        events,
        Arc::clone(&ready),
        Arc::clone(&close),
        Arc::clone(&notice),
    ));

    let page = Bounded::new(page, deadline);
    listen(&page).await;
    if let Err(error) = page.call("DOM.enable", json!({})).await {
        eprintln!("devboule: browser page {id} will not report a new document: {error}");
    }
    ask_for_events(&page, id, &frames).await;
    // The frame the move will be read against is only known now, so the stream
    // that says where the tab is opens here.
    ready.store(true, Ordering::SeqCst);
    notice.notify_one();
    WsWatch { drain, close }
}

/// One websocket page's event drain. Held for the tab's life.
#[cfg_attr(not(test), allow(dead_code))]
pub struct WsWatch {
    pub(super) drain: tauri::async_runtime::JoinHandle<()>,
    pub(super) close: Arc<Close>,
}

impl WsWatch {
    /// Whether the drain has ended. What a test watches to prove a close
    /// leaves no task behind.
    #[cfg(test)]
    pub(in crate::browser) fn finished(&self) -> bool {
        self.drain.inner().is_finished()
    }
}

impl Drop for WsWatch {
    /// Never waits, so it is safe from a report the drain is running and from
    /// a runtime thread. The event the drain is already inside may still
    /// finish; no event, count or callback starts after this.
    fn drop(&mut self) {
        self.close.begin();
        self.drain.inner().abort();
    }
}

/// Whether the watch has been dropped. A flag and not a lock: a close must
/// not wait for the callback the drain is running.
pub(in crate::browser) struct Close(AtomicBool);

impl Close {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Close(AtomicBool::new(false)))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn begin(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub(super) fn is_closed(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The tab's reports, each silent once the watch is dropped. The drain's own
/// check is per event; this one is per callback, so an event that was already
/// being counted when the close began still starts no report.
pub(super) fn unclosed(close: &Arc<Close>, reports: Reports) -> Reports {
    let Reports {
        within_document,
        committed,
    } = reports;
    Reports {
        within_document: Arc::new({
            let close = Arc::clone(close);
            move |url| {
                if !close.is_closed() {
                    within_document(url);
                }
            }
        }),
        committed: Arc::new({
            let close = Arc::clone(close);
            move || {
                if !close.is_closed() {
                    committed();
                }
            }
        }),
    }
}

/// Drain one page's two streams into its listener.
///
/// The state stream is read only once `ready` says the frame tree has been
/// asked for; voice events are read from the start, because what a page says
/// says nothing about where the tab is. A state event the reader dropped is
/// not forgotten: the loss is counted and this task is woken, and the listener
/// treats the state as unknown. A dropped watch hands the listener nothing
/// more.
async fn drain(
    listener: Listener,
    mut events: WsEvents,
    ready: Arc<AtomicBool>,
    close: Arc<Close>,
    notice: Arc<Notify>,
) {
    let mut state_open = true;
    let mut voice_open = true;
    let mut accounted_drops = 0u64;
    loop {
        if close.is_closed() || (!state_open && !voice_open) {
            return;
        }
        let mut arrived: Option<WsEvent> = None;
        tokio::select! {
            event = events.state.recv(), if state_open && ready.load(Ordering::SeqCst) => match event {
                Some(event) => arrived = Some(event),
                None => state_open = false,
            },
            event = events.voice.recv(), if voice_open => match event {
                Some(event) => arrived = Some(event),
                None => {
                    voice_open = false;
                    // One `Feed` holds both senders, so a voice stream that
                    // ended means the state stream is over too. Until `ready`
                    // its branch is disabled, and a reader that is gone will
                    // never make it ready: without this the drain would wait
                    // for a frame tree it can no longer ask for.
                    if !ready.load(Ordering::SeqCst) {
                        state_open = false;
                    }
                }
            },
            _ = notice.notified() => {}
        }
        if close.is_closed() {
            return;
        }
        if let Some(event) = arrived {
            listener.heard(&event.method, &event.params.to_string());
        }
        let dropped = events.dropped.state();
        if dropped > accounted_drops && !close.is_closed() {
            accounted_drops = dropped;
            listener.unknown_state();
        }
    }
}
