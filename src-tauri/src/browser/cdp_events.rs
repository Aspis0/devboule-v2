//! One event subscription per browser page, and the quiet it reports.
//!
//! Settling is the whole reason this exists. An action's answer is only worth
//! anything if the page it happened on has stopped moving, and the way to know
//! that is the runtime's own event stream rather than a guess at a delay: the
//! DOM counters (`DOM.documentUpdated`, which arrives with no enable) and the
//! load events (`Page.*`, which arrive only after `Page.enable`).
//!
//! The same stream says where the tab is: a new document in its own frame, and
//! a move of the address with no load at all (`pushState`, a hash change),
//! which the owner of the tab is told about through [`Reports`].
//!
//! One receiver per event per page, installed when the page is created and
//! never removed: the handler belongs to the child webview, which a close
//! disposes of, and a page watched twice would move its own counter twice. What
//! a close does drop is the counter, so the map holds only live tabs. A
//! websocket page has no receiver to install: its reader hands back one stream,
//! and [`watch_ws`] drains it into the same listener.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

use super::cdp::{Bounded, Page as _, WebviewPage};
use super::cdp_ws::{WsEvent, WsEvents};
use super::console;
use super::deadline::Deadline;
use super::frames::{Frames, Observed};

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
const WATCHED: [&str; 5] = [
    "DOM.documentUpdated",
    "Page.frameNavigated",
    "Page.navigatedWithinDocument",
    "Page.loadEventFired",
    "Page.frameStoppedLoading",
];

/// The domains whose events are a page's own voice, enabled on the blank
/// bootstrap before it loads anything by [`listen`].
const VOICED: [&str; 2] = ["Runtime", "Log"];

/// Turn on the events a page speaks through, so that the first thing it says is
/// heard. Asked for before the first navigation on purpose: an error thrown
/// while a page is still loading is one of the ones an agent most needs.
pub async fn listen(page: &dyn super::cdp::Page) {
    for domain in VOICED {
        if let Err(error) = page.call(&format!("{domain}.enable"), json!({})).await {
            eprintln!("devboule: a browser page will not report {domain}: {error}");
        }
    }
}

/// What a tab's owner is told when the page says where it is.
#[derive(Clone)]
pub struct Reports {
    /// The tab's own frame moved to this address without loading a document.
    pub within_document: Arc<dyn Fn(String) + Send + Sync>,
    /// The tab's own frame committed a new document.
    pub committed: Arc<dyn Fn() + Send + Sync>,
}

/// What one page's events have counted since the tab was opened.
#[derive(Default)]
struct Counters {
    /// Every event: how far the page has moved, which is what a settle waits out.
    moved: u64,
    /// New documents in the tab's own frame.
    documents: u64,
}

/// The counters of every watched page, per browser id.
static QUIET_SIGNAL: Mutex<Option<HashMap<String, Counters>>> = Mutex::new(None);

/// The counters one page's events bump, created when the page is watched.
fn signal_for(id: &str) {
    let mut pages = QUIET_SIGNAL.lock().expect("browser page signals poisoned");
    pages
        .get_or_insert_with(HashMap::new)
        .entry(id.to_owned())
        .or_default();
}

/// How far the page has moved since the tab was opened.
pub fn moved(id: &str) -> u64 {
    counted(id, |counters| counters.moved)
}

/// How many documents the tab's own frame has committed. A navigation that
/// loaded nothing leaves it where it was, which is how a delta tells a move
/// within a document from a new page.
pub fn documents(id: &str) -> u64 {
    counted(id, |counters| counters.documents)
}

fn counted(id: &str, read: impl Fn(&Counters) -> u64) -> u64 {
    let pages = QUIET_SIGNAL.lock().expect("browser page signals poisoned");
    pages
        .as_ref()
        .and_then(|pages| pages.get(id))
        .map(read)
        .unwrap_or(0)
}

/// Drop a page's signal and what its console said. The close is the only place
/// this happens, and every close goes through it, so the map holds only live
/// tabs.
pub fn forget(id: &str) {
    console::forget(id);
    if let Some(pages) = QUIET_SIGNAL
        .lock()
        .expect("browser page signals poisoned")
        .as_mut()
    {
        pages.remove(id);
    }
}

/// Ask for the page's own events and learn which frame is the tab's own.
///
/// `Page.enable` is what makes `Page.*` arrive at all, and the frame tree is
/// what makes a same-document move the tab's address rather than an iframe's.
/// A failure is reported and nothing else is done: the events that do arrive
/// are still counted, and `frameNavigated` learns the frame a moment later.
async fn page_events(page: &dyn super::cdp::Page, id: &str) -> Arc<Frames> {
    let frames = Arc::new(Frames::default());
    ask_for_events(page, id, &frames).await;
    frames
}

/// The same two calls, into frames a listener already holds. The websocket
/// listener is running before this is asked for, so the frames it reads are
/// the ones learned here.
async fn ask_for_events(page: &dyn super::cdp::Page, id: &str, frames: &Arc<Frames>) {
    if let Err(error) = page.call("Page.enable", json!({})).await {
        eprintln!("devboule: browser page {id} will not report its loads: {error}");
    }
    match page.call("Page.getFrameTree", json!({})).await {
        Ok(tree) => frames.learn_from_tree(&tree),
        Err(error) => eprintln!("devboule: browser page {id} has no known top frame: {error}"),
    }
}

/// Watch a page for as long as `deadline` allows. What is not installed in time
/// is not installed: the tab is already open, and a settle without the event
/// falls back to its cap.
#[cfg(windows)]
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
/// against the frame it happened in. A state event the reader had to drop
/// wakes the drain, which resynchronizes instead of forgetting it.
///
/// The guard is held for the tab's life: dropping it stops the drain task, and
/// the reader's own end stops it too.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn watch_ws(
    page: &dyn super::cdp::Page,
    id: &str,
    events: WsEvents,
    deadline: Deadline,
    reports: Reports,
) -> WsWatch {
    signal_for(id);
    console::open(id);
    let frames = Arc::new(Frames::default());
    let ready = Arc::new(AtomicBool::new(false));
    let closed = Arc::new(AtomicBool::new(false));
    let notice = Arc::clone(&events.notice);
    let listener = Listener {
        id: id.to_owned(),
        frames: Arc::clone(&frames),
        reports: unclosed(reports, Arc::clone(&closed)),
    };
    let drain = tauri::async_runtime::spawn(drain(
        listener,
        events,
        Arc::clone(&ready),
        Arc::clone(&closed),
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
    WsWatch { drain, closed }
}

/// One websocket page's event drain. Held for the tab's life.
#[cfg_attr(not(test), allow(dead_code))]
pub struct WsWatch {
    drain: tauri::async_runtime::JoinHandle<()>,
    closed: Arc<AtomicBool>,
}

impl WsWatch {
    /// Whether the drain has ended. What a test watches to prove a close
    /// leaves no task behind.
    #[cfg(test)]
    pub(super) fn finished(&self) -> bool {
        self.drain.inner().is_finished()
    }
}

impl Drop for WsWatch {
    fn drop(&mut self) {
        // The flag is what the reports check, so the event the drain is already
        // inside reaches no pane; the abort stops the events it has not read
        // yet, so a dropped subscription cannot outlive its tab with a task
        // still running.
        self.closed.store(true, Ordering::SeqCst);
        self.drain.inner().abort();
    }
}

/// The tab's reports, with every callback checking that the watch is still
/// open. A drain already inside `heard` when the close lands must not reach a
/// pane whose tab is gone.
fn unclosed(reports: Reports, closed: Arc<AtomicBool>) -> Reports {
    let within_document = Arc::clone(&reports.within_document);
    let committed = Arc::clone(&reports.committed);
    let within_closed = Arc::clone(&closed);
    Reports {
        within_document: Arc::new(move |url| {
            if !within_closed.load(Ordering::SeqCst) {
                within_document(url);
            }
        }),
        committed: Arc::new(move || {
            if !closed.load(Ordering::SeqCst) {
                committed();
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
/// resynchronizes. `closed` is checked before every event, so a subscription
/// that has been dropped reports nothing more even though its task may still
/// be finishing the event it was inside.
async fn drain(
    listener: Listener,
    mut events: WsEvents,
    ready: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    notice: Arc<Notify>,
) {
    let mut state_open = true;
    let mut voice_open = true;
    let mut resynced_at = 0u64;
    loop {
        if closed.load(Ordering::SeqCst) || (!state_open && !voice_open) {
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
        if closed.load(Ordering::SeqCst) {
            return;
        }
        if let Some(event) = arrived {
            listener.heard(&event.method, &event.params.to_string());
        }
        let dropped = events.dropped.state();
        if dropped > resynced_at {
            resynced_at = dropped;
            listener.resync();
        }
    }
}

/// What one event receiver needs to turn an event into a count and a report.
struct Listener {
    id: String,
    frames: Arc<Frames>,
    reports: Reports,
}

impl Listener {
    fn heard(&self, event: &str, params: &str) {
        // What a page says is its voice, not movement: a page logging on a
        // timer would otherwise hold every settle open to its cap.
        if console::is_voice(event) {
            console::record(&self.id, event, params);
            return;
        }
        bump(&self.id);
        match self.frames.observe(event, params) {
            Observed::NewDocument => self.committed(),
            Observed::SameDocument(url) => (self.reports.within_document)(url),
            Observed::Nothing => {}
        }
    }

    /// A new document, or a loss that could have hidden one: the old one's refs
    /// are stale and what it said is not what the page says now.
    fn committed(&self) {
        bump_document(&self.id);
        console::clear(&self.id);
        (self.reports.committed)();
    }

    /// An event was dropped from the state stream, so the sequence this
    /// listener has counted is not the page's whole story: a navigation may be
    /// missing from it. The document is invalidated as a committed one
    /// invalidates it — every ref taken before is now stale — and the loss is
    /// counted as movement, so a settle does not read it as quiet.
    fn resync(&self) {
        bump(&self.id);
        self.committed();
    }
}

#[cfg(windows)]
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

#[cfg(windows)]
fn bump(id: &str) {
    count(id, |counters| {
        counters.moved = counters.moved.wrapping_add(1);
    });
}

#[cfg(windows)]
fn bump_document(id: &str) {
    count(id, |counters| {
        counters.documents = counters.documents.wrapping_add(1);
    });
}

#[cfg(windows)]
fn count(id: &str, update: impl FnOnce(&mut Counters)) {
    let mut pages = QUIET_SIGNAL.lock().expect("browser page signals poisoned");
    if let Some(page) = pages.as_mut().and_then(|pages| pages.get_mut(id)) {
        update(page);
    }
}

#[cfg(windows)]
fn com(error: windows::core::Error) -> String {
    error.to_string()
}

#[cfg(not(windows))]
pub async fn watch(
    _app: &AppHandle,
    _id: &str,
    _label: &str,
    _deadline: Deadline,
    _reports: Reports,
) {
    // No event stream on this target, so `moved` never moves and every settle
    // ends on its first quiet. Nothing here blocks a command from running.
}

/// Wait without holding a runtime worker: this crate links no async timer, and
/// every CDP call already blocks its worker for the length of the call.
pub async fn nap(for_: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(for_)).await;
}

/// Wait until the page has gone `QUIET` without an event, or `SETTLE_CAP` has
/// passed, or the command has no time left. A navigation and a re-render are
/// the same signal here, which is what the contract asks for: one answer per
/// action, taken once the page has stopped.
pub async fn settle(id: &str, deadline: Deadline) {
    let end = std::time::Instant::now() + SETTLE_CAP.min(deadline.left());
    loop {
        let before = moved(id);
        let rest = end.saturating_duration_since(std::time::Instant::now());
        if rest.is_zero() {
            return;
        }
        nap(QUIET.min(rest)).await;
        if moved(id) == before || std::time::Instant::now() >= end {
            return;
        }
    }
}

#[cfg(test)]
#[path = "cdp_events_tests.rs"]
mod tests;
