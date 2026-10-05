//! The websocket half of the event layer: a reader's two streams drained into
//! the shared [`Listener`], and the close that stops it.
//!
//! The two streams carry different things — see `cdp_ws` — and both are read
//! here. A state event the reader had to drop is not forgotten: the loss is
//! counted, the drain is woken, and the listener treats the state as unknown.
//!
//! **The close is an exclusion, not a flag.** The drain holds [`Close`]'s gate
//! around every event it hands to the listener, and `Drop` marks the closing
//! before it waits on that gate: a callback that has not started when the
//! close is marked can never start, and the one already in flight is waited
//! for.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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
        reports,
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

    /// The exclusion a test watches to know the close has begun. Test-only
    /// because the production holder drops the whole watch.
    #[cfg(test)]
    pub(in crate::browser) fn close_handle(&self) -> Arc<Close> {
        Arc::clone(&self.close)
    }
}

impl Drop for WsWatch {
    fn drop(&mut self) {
        // Stop new events, wait for the one in flight, then stop the task that
        // would read the next one. After this returns no callback can start
        // and none is still running.
        self.close.stop_and_wait();
        self.drain.inner().abort();
    }
}

/// The exclusion that makes a close total: the drain enters it around every
/// event it hands to the listener, and `Drop` marks the closing and then waits
/// on the same gate. A check-then-call on an atomic leaves a window between
/// the check and the callback; the gate has none.
pub(in crate::browser) struct Close {
    closing: AtomicBool,
    gate: Mutex<()>,
}

impl Close {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Close {
            closing: AtomicBool::new(false),
            gate: Mutex::new(()),
        })
    }

    /// Enter the exclusion for one event's whole ingestion, or `None` because
    /// the close has begun. The guard is held across `heard`, so every report
    /// inside it runs before a close that is waiting here.
    pub(super) fn entered(&self) -> Option<MutexGuard<'_, ()>> {
        let guard = self.gate.lock().unwrap_or_else(PoisonError::into_inner);
        if self.closing.load(Ordering::SeqCst) {
            return None;
        }
        Some(guard)
    }

    /// Mark the close first, so nothing new enters, and then wait for the
    /// event in flight to leave. The mark is what a test can watch to know the
    /// close has begun.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn stop_and_wait(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let _in_flight = self.gate.lock().unwrap_or_else(PoisonError::into_inner);
    }

    pub(in crate::browser) fn is_closed(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }
}

/// Drain one page's two streams into its listener.
///
/// The state stream is read only once `ready` says the frame tree has been
/// asked for; voice events are read from the start, because what a page says
/// says nothing about where the tab is. A state event the reader dropped is
/// not forgotten: the loss is counted and this task is woken, and the listener
/// treats the state as unknown. Every event is handed to the listener under
/// the close's exclusion, so a dropped watch starts no callback.
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
            let Some(_entered) = close.entered() else {
                return;
            };
            listener.heard(&event.method, &event.params.to_string());
        }
        let dropped = events.dropped.state();
        if dropped > accounted_drops {
            accounted_drops = dropped;
            listener.unknown_state();
        }
    }
}
