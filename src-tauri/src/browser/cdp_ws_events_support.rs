//! The fixtures the websocket event tests share: the script a watch consumes
//! before the page is navigated anywhere, where its reports land, and the two
//! helpers a test drives the socket and waits on the drain with.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc::Receiver;

use super::fake::Step;
use super::WsPage;
use crate::browser::cdp::Page as _;
use crate::browser::cdp_events::{self, Reports};
use crate::browser::deadline::Deadline;

/// The frame the fake target calls its own, as `Page.getFrameTree` names it.
pub(super) const MAIN: &str = "F1";

/// The commands a watch sends before the page is navigated anywhere: `Runtime`
/// and `Log` for its voice, `DOM` for the document updates, then `Page` for
/// the loads. The fake endpoint answers one step per command, so a test that
/// speaks after the watch has to consume these five first.
pub(super) fn before_navigation() -> Vec<Step> {
    vec![
        Step::Echo,                                                        // Runtime.enable
        Step::Echo,                                                        // Log.enable
        Step::Echo,                                                        // DOM.enable
        Step::Echo,                                                        // Page.enable
        Step::Answer(json!({ "frameTree": { "frame": { "id": MAIN } } })), // Page.getFrameTree
    ]
}

/// Where a page's reports go in these tests.
#[derive(Default)]
pub(super) struct Said {
    moved_to: Arc<Mutex<Vec<String>>>,
    committed: Arc<AtomicUsize>,
}

impl Said {
    pub(super) fn reports(&self) -> Reports {
        let moved_to = Arc::clone(&self.moved_to);
        let committed = Arc::clone(&self.committed);
        Reports {
            within_document: Arc::new(move |url| {
                moved_to.lock().expect("reports poisoned").push(url);
            }),
            committed: Arc::new(move || {
                committed.fetch_add(1, Ordering::SeqCst);
            }),
        }
    }

    pub(super) fn moved_to(&self) -> Vec<String> {
        self.moved_to.lock().expect("reports poisoned").clone()
    }

    pub(super) fn commits(&self) -> usize {
        self.committed.load(Ordering::SeqCst)
    }
}

/// A watch over `page`'s reader, with a budget long enough that no test is
/// about the clock.
pub(super) async fn watching(
    page: &WsPage,
    id: &str,
    events: Receiver<super::WsEvent>,
    said: &Said,
) -> cdp_events::WsWatch {
    cdp_events::watch_ws(
        page,
        id,
        events,
        Deadline::in_(Duration::from_secs(5)),
        said.reports(),
    )
    .await
}

/// Send one scripted event by answering a command with it. The endpoint writes
/// the event before the answer, so it is on the reader's channel by the time
/// this returns.
pub(super) async fn say(page: &WsPage) {
    page.call("Runtime.evaluate", json!({ "expression": "1" }))
        .await
        .expect("the command carrying the event is answered");
}

/// Wait for the drain task to catch up with what the socket already delivered.
pub(super) async fn until(condition: impl Fn() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("nothing caught up within two seconds");
}
