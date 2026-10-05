//! What dropping a websocket watch does while the drain is inside a report:
//! the drop never waits for it, from any thread, and after it nothing starts.

use std::sync::mpsc;
use std::time::Duration;

use serde_json::json;

use super::events_support::{before_navigation, burst, frame, say, until, watching, Said, MAIN};
use super::fake::{FakeServer, Step};
use super::{Drops, WsPage};
use crate::browser::cdp_events::{self, WsWatch};

/// How long a drop that must not wait is given before the test calls it stuck.
const PROMPT: Duration = Duration::from_secs(2);

fn a_new_document(url: &str) -> Step {
    Step::Event {
        method: "Page.frameNavigated".to_owned(),
        params: json!({ "frame": frame(MAIN, url), "type": "Navigation" }),
    }
}

/// A watch whose drain is held inside the first document's report, with
/// `behind` delivered to the socket while it is held.
struct Held {
    _server: FakeServer,
    page: WsPage,
    watch: WsWatch,
    said: Said,
    open: mpsc::Sender<()>,
    drops: Drops,
}

async fn held_in_the_first_report(id: &str, behind: Vec<Step>) -> Held {
    let behind_count = behind.len();
    let mut steps = vec![a_new_document("https://example.test/one")];
    steps.extend(before_navigation());
    steps.extend(behind);
    let (server, page, events) = FakeServer::start(steps).await.attached().await;
    let drops = events.drops();
    let said = Said::default();
    let (entered, open) = said.hold_the_next_commit();

    say(&page).await;
    let watch = watching(&page, id, events, &said).await;
    entered
        .recv_timeout(Duration::from_secs(5))
        .expect("the drain is held inside the first document's report");
    for _ in 0..behind_count {
        say(&page).await;
    }
    Held {
        _server: server,
        page,
        watch,
        said,
        open,
        drops,
    }
}

/// Run `closing` on its own thread and say whether it returned within
/// [`PROMPT`] while the report was still held. The report is released either
/// way, so a drop that does wait fails the test instead of hanging it.
fn returned_promptly(
    held_open: &mpsc::Sender<()>,
    closing: impl FnOnce(mpsc::Sender<()>) + Send + 'static,
) -> bool {
    let (done, returned) = mpsc::channel();
    let closer = std::thread::spawn(move || closing(done));
    let prompt = returned.recv_timeout(PROMPT).is_ok();
    held_open.send(()).expect("the drain is released");
    closer.join().expect("the closing thread");
    prompt
}

#[test]
fn a_report_that_drops_its_own_watch_does_not_deadlock_the_drain() {
    tauri::async_runtime::block_on(async {
        let mut steps = before_navigation();
        steps.push(a_new_document("https://example.test/self"));
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let id = "tab-ws-self-drop";
        let said = Said::default();
        let watch = watching(&page, id, events, &said).await;
        said.let_go_of_the_watch_on_commit(watch);

        say(&page).await;

        // The report drops the watch from inside the drain. A drop that waited
        // on the drain would wait on itself, so a regression is a count that
        // never arrives, not a test that hangs.
        until(|| said.commits() == 1).await;
        assert_eq!(cdp_events::moved(id), 1);

        page.close().await;
        cdp_events::forget(id);
    });
}

#[test]
fn a_drop_on_a_current_thread_runtime_does_not_wait_for_the_report_in_flight() {
    tauri::async_runtime::block_on(async {
        let id = "tab-ws-runtime-drop";
        let held = held_in_the_first_report(id, Vec::new()).await;

        let prompt = returned_promptly(&held.open, move |done| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            // The only worker of this runtime runs the drop: waiting would
            // leave nothing to run anything else.
            runtime.block_on(async move {
                tokio::spawn(async move {
                    drop(held.watch);
                    let _ = done.send(());
                })
                .await
                .expect("the dropping task");
            });
        });

        assert!(
            prompt,
            "a drop on a runtime thread returns while the drain is inside a report"
        );
        held.page.close().await;
        cdp_events::forget(id);
    });
}

#[test]
fn a_close_does_not_wait_for_the_report_in_flight_and_starts_nothing_after_it() {
    tauri::async_runtime::block_on(async {
        let id = "tab-ws-racing-close";
        let behind = vec![Step::Event {
            method: "Page.navigatedWithinDocument".to_owned(),
            params: json!({ "frameId": MAIN, "url": "https://example.test/late" }),
        }];
        let held = held_in_the_first_report(id, behind).await;
        let Held {
            page,
            watch,
            said,
            open,
            ..
        } = held;

        let prompt = returned_promptly(&open, move |done| {
            drop(watch);
            let _ = done.send(());
        });
        until(|| said.commits() == 1).await;
        // Nothing to wait on: the drain has left or is about to, and the only
        // thing that could start now is the event behind the report.
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(prompt, "the drop returns while the report is held");
        assert!(
            said.moved_to().is_empty(),
            "the event behind the held report never starts"
        );
        assert_eq!(
            cdp_events::moved(id),
            1,
            "only the event the drain was already inside was counted"
        );

        page.close().await;
        cdp_events::forget(id);
    });
}

#[test]
fn a_loss_noticed_after_the_close_does_not_move_the_counters() {
    tauri::async_runtime::block_on(async {
        // More state events than the queue holds arrive while the drain is held
        // in its report, so the loss is first met after the close began.
        let id = "tab-ws-loss-after-close";
        let extra = burst(super::STATE_QUEUE + 4, "DOM.documentUpdated", json!({}));
        let held = held_in_the_first_report(id, extra).await;
        assert!(held.drops.state() > 0, "the queue turned events away");
        let Held {
            page,
            watch,
            said,
            open,
            ..
        } = held;

        returned_promptly(&open, move |done| {
            drop(watch);
            let _ = done.send(());
        });
        until(|| said.commits() == 1).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(
            (cdp_events::moved(id), cdp_events::documents(id)),
            (1, 1),
            "the report in flight counted its own event, and the loss found \
             afterwards counted nothing"
        );

        page.close().await;
        cdp_events::forget(id);
    });
}
