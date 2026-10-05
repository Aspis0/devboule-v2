//! The fixtures the websocket event tests share: the script a watch consumes
//! before the page is navigated anywhere, where its reports land, and the two
//! helpers a test drives the socket and waits on the drain with.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::fake::Step;
use super::{WsEvents, WsPage};
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

/// `count` copies of one event, one per command: what a test fills a queue
/// with.
pub(super) fn burst(count: usize, method: &str, params: Value) -> Vec<Step> {
    (0..count)
        .map(|_| Step::Event {
            method: method.to_owned(),
            params: params.clone(),
        })
        .collect()
}

/// One `Page.Frame` as the protocol sends it, not only the id the ingestion
/// reads.
pub(super) fn frame(id: &str, url: &str) -> Value {
    json!({
        "id": id,
        "loaderId": "LOADER",
        "url": url,
        "domainAndRegistry": "example.test",
        "securityOrigin": "https://example.test",
        "mimeType": "text/html",
        "secureContextType": "Secure",
        "crossOriginIsolatedContextType": "NotIsolated",
        "gpcEnabled": false,
    })
}

/// A gate the next commit report waits on.
struct Gate {
    entered: mpsc::Sender<()>,
    open: mpsc::Receiver<()>,
}

/// Where a page's reports go in these tests.
#[derive(Default)]
pub(super) struct Said {
    moved_to: Arc<Mutex<Vec<String>>>,
    committed: Arc<AtomicUsize>,
    gate: Arc<Mutex<Option<Gate>>>,
    owned: Arc<Mutex<Option<cdp_events::WsWatch>>>,
}

impl Said {
    pub(super) fn reports(&self) -> Reports {
        let moved_to = Arc::clone(&self.moved_to);
        let committed = Arc::clone(&self.committed);
        let gate = Arc::clone(&self.gate);
        let owned = Arc::clone(&self.owned);
        Reports {
            within_document: Arc::new(move |url| {
                moved_to.lock().expect("reports poisoned").push(url);
            }),
            committed: Arc::new(move || {
                let gate = gate.lock().expect("gate poisoned").take();
                if let Some(gate) = gate {
                    let _ = gate.entered.send(());
                    // A bound on a hung test, not on the burst a test holds the
                    // drain for: the test releases the gate itself.
                    let _ = gate.open.recv_timeout(Duration::from_secs(60));
                }
                let own = owned.lock().expect("owned poisoned").take();
                drop(own);
                committed.fetch_add(1, Ordering::SeqCst);
            }),
        }
    }

    /// Give the watch to the reports: the next commit report drops it, from
    /// inside the drain, the way a tab host closing itself on a report would.
    pub(super) fn let_go_of_the_watch_on_commit(&self, watch: cdp_events::WsWatch) {
        *self.owned.lock().expect("owned poisoned") = Some(watch);
    }

    /// Hold the next commit report until the open end fires, and say when it
    /// is holding: a test keeps the drain in a report while events pile up
    /// behind it.
    pub(super) fn hold_the_next_commit(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, entered_rx) = mpsc::channel();
        let (open, open_rx) = mpsc::channel();
        *self.gate.lock().expect("gate poisoned") = Some(Gate {
            entered,
            open: open_rx,
        });
        (entered_rx, open)
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
    events: WsEvents,
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
/// the event before the answer, so it is on the reader's stream by the time
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
