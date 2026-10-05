//! One browser page's event ingestion, and the quiet it reports.
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
//! The two transports live beside this file and feed the same [`Listener`]:
//! `cdp_events_windows.rs` installs one receiver per event on a WebView2 child,
//! and `cdp_ws_events.rs` drains a websocket reader's streams. Nothing here
//! knows which one carried an event.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::console;
use super::deadline::Deadline;
use super::frames::{Frames, Observed};

/// What a page must go without before its state is believed.
pub const QUIET: Duration = Duration::from_millis(300);

/// How long a settle waits in total, however busy the page is. Past this the
/// answer is the page's state as far as it got, which is what a caller can
/// see anyway.
pub const SETTLE_CAP: Duration = Duration::from_secs(3);

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

fn bump(id: &str) {
    count(id, |counters| {
        counters.moved = counters.moved.wrapping_add(1);
    });
}

fn bump_document(id: &str) {
    count(id, |counters| {
        counters.documents = counters.documents.wrapping_add(1);
    });
}

fn count(id: &str, update: impl FnOnce(&mut Counters)) {
    let mut pages = QUIET_SIGNAL.lock().expect("browser page signals poisoned");
    if let Some(page) = pages.as_mut().and_then(|pages| pages.get_mut(id)) {
        update(page);
    }
}

/// `Page.enable` and the frame tree, into frames a listener already holds.
///
/// `Page.enable` is what makes `Page.*` arrive at all, and the frame tree is
/// what makes a same-document move the tab's address rather than an iframe's.
/// A failure is reported and nothing else is done: a settle without the event
/// falls back to its cap, and `frameNavigated` learns the frame a moment later.
async fn ask_for_events(page: &dyn super::cdp::Page, id: &str, frames: &Arc<Frames>) {
    if let Err(error) = page.call("Page.enable", json!({})).await {
        eprintln!("devboule: browser page {id} will not report its loads: {error}");
    }
    match page.call("Page.getFrameTree", json!({})).await {
        Ok(tree) => frames.learn_from_tree(&tree),
        Err(error) => eprintln!("devboule: browser page {id} has no known top frame: {error}"),
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

    /// A new document, confirmed by the page's own event: what the old one
    /// said is not what the page says now, and the pane is told.
    fn committed(&self) {
        bump_document(&self.id);
        console::clear(&self.id);
        (self.reports.committed)();
    }

    /// An event was dropped from the state stream, so the sequence this
    /// listener has counted is not the page's whole story: a navigation may be
    /// missing from it. What is known is only that the state is unknown, so
    /// every ref taken before is treated as stale — the document count moves,
    /// which is the delta's own "the page is not the one you measured" — and
    /// the loss is movement, so a settle does not read it as quiet. Nothing
    /// else is claimed: no commit is reported, and the ring keeps what the
    /// page really said.
    fn unknown_state(&self) {
        bump(&self.id);
        bump_document(&self.id);
    }
}

#[cfg(windows)]
#[path = "cdp_events_windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::watch;

/// No event stream on this target, so `moved` never moves and every settle
/// ends on its first quiet. Nothing here blocks a command from running.
#[cfg(not(windows))]
pub async fn watch(
    _app: &tauri::AppHandle,
    _id: &str,
    _label: &str,
    _deadline: Deadline,
    _reports: Reports,
) {
}

#[path = "cdp_ws_events.rs"]
mod ws;
// The published surface `tab.rs` and the slice-3 host call; the module itself
// stays private. Nothing in production constructs it yet, hence the same
// not-yet-wired allowance the items carry.
#[cfg_attr(not(test), allow(unused_imports))]
pub use ws::{watch_ws, WsWatch};

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
