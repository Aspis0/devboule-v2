//! What installing a websocket watch does to the page, and what letting it go
//! does to the tasks behind it: the domains switched on before the first
//! navigation, the reader stopped by a close, and the drain stopped by a
//! dropped subscription.

use std::time::Duration;

use serde_json::json;

use super::events_support::{before_navigation, burst, frame, say, until, watching, Said, MAIN};
use super::fake::{FakeServer, Step};
use crate::browser::cdp_events;

#[test]
fn the_domains_the_ingestion_needs_are_enabled_before_the_page_is_navigated() {
    tauri::async_runtime::block_on(async {
        let (server, page, events) = FakeServer::start(before_navigation())
            .await
            .attached()
            .await;
        let id = "tab-ws-domains";
        let said = Said::default();
        let watch = watching(&page, id, events, &said).await;

        assert_eq!(
            server.read().await,
            [
                "Runtime.enable",
                "Log.enable",
                "DOM.enable",
                "Page.enable",
                "Page.getFrameTree"
            ],
            "the domains must be on before the first navigation, or a page's \
             first words are the ones nobody hears"
        );

        page.close().await;
        drop(watch);
        cdp_events::forget(id);
    });
}

#[test]
fn closing_the_page_ends_the_drain_task_with_the_reader() {
    tauri::async_runtime::block_on(async {
        let (server, page, events) = FakeServer::start(before_navigation())
            .await
            .attached()
            .await;
        let id = "tab-ws-close";
        let said = Said::default();
        let watch = watching(&page, id, events, &said).await;
        assert!(!watch.finished(), "the drain is running");

        page.close().await;

        until(|| watch.finished()).await;
        assert_eq!(
            page.waiting().await,
            0,
            "a closed page holds no waiter open"
        );
        assert_eq!(
            server.read().await.len(),
            5,
            "nothing is asked of a socket that is gone"
        );

        drop(watch);
        cdp_events::forget(id);
    });
}

#[test]
fn a_dropped_subscription_stops_the_drain_and_a_full_channel_never_stalls_the_reader() {
    tauri::async_runtime::block_on(async {
        // One event the drain must see, then more than any queue holds after
        // the subscription is gone.
        let mut steps = before_navigation();
        steps.push(Step::Event {
            method: "Page.loadEventFired".to_owned(),
            params: json!({ "timestamp": 1.0 }),
        });
        steps.extend(burst(
            super::STATE_QUEUE + 8,
            "Page.loadEventFired",
            json!({ "timestamp": 2.0 }),
        ));
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let drops = events.drops();
        let id = "tab-ws-dropped";
        let said = Said::default();
        let watch = watching(&page, id, events, &said).await;

        say(&page).await;
        until(|| cdp_events::moved(id) == 1).await;

        drop(watch);
        for _ in 0..super::STATE_QUEUE + 8 {
            say(&page).await;
        }
        // The events are already on the streams (or were turned away by them)
        // by the time the last call returned; a drain still running would have
        // taken every one of them long before this.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        assert_eq!(
            cdp_events::moved(id),
            1,
            "a dropped subscription's drain is stopped, so the events behind it \
             are dropped rather than counted late"
        );
        assert_eq!(
            (drops.state(), drops.voice()),
            (0, 0),
            "a stream whose watcher is gone is closed, not full: nothing was \
             lost to a queue, so nothing is counted as one"
        );

        page.close().await;
        cdp_events::forget(id);
    });
}

#[test]
fn an_event_that_arrives_before_the_frame_tree_is_reported_with_the_frame_it_names() {
    tauri::async_runtime::block_on(async {
        // The move is queued before the watch exists and read only after the
        // frame tree was asked for: the frame it happened in is learned in
        // between, and the report is read against that frame.
        let mut steps = vec![Step::Event {
            method: "Page.navigatedWithinDocument".to_owned(),
            params: json!({
                "frameId": MAIN,
                "url": "https://example.test/early",
            }),
        }];
        steps.extend(before_navigation());
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let id = "tab-ws-early";
        let said = Said::default();

        say(&page).await;

        let watch = watching(&page, id, events, &said).await;
        until(|| !said.moved_to().is_empty()).await;
        assert_eq!(
            said.moved_to(),
            ["https://example.test/early"],
            "the move arrived before the tree did, and the frame the tree \
             names is the one it is read against"
        );

        page.close().await;
        drop(watch);
        cdp_events::forget(id);
    });
}

#[test]
fn a_close_racing_an_in_flight_event_reports_nothing_more() {
    tauri::async_runtime::block_on(async {
        // Both events are queued before the watch exists, so the close races
        // the drain and not the socket; the first blocks in its own report
        // until the test has dropped the subscription.
        let mut steps = vec![
            Step::Event {
                method: "Page.frameNavigated".to_owned(),
                params: json!({
                    "frame": frame(MAIN, "https://example.test/one"),
                    "type": "Navigation",
                }),
            },
            Step::Event {
                method: "Page.navigatedWithinDocument".to_owned(),
                params: json!({
                    "frameId": MAIN,
                    "url": "https://example.test/late",
                }),
            },
        ];
        steps.extend(before_navigation());
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let id = "tab-ws-racing-close";
        let said = Said::default();
        let (entered, open) = said.hold_the_next_commit();

        say(&page).await;
        say(&page).await;
        let watch = watching(&page, id, events, &said).await;
        entered
            .recv_timeout(Duration::from_secs(5))
            .expect("the drain is held inside the first document's report");

        drop(watch);
        open.send(()).expect("the drain is released");
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(
            said.moved_to().is_empty(),
            "an event still in flight when the subscription drops is not \
             reported"
        );
        assert_eq!(
            cdp_events::moved(id),
            1,
            "only the event the drain was already inside was counted"
        );
        assert_eq!(said.commits(), 1, "and only its own commit was reported");

        page.close().await;
        cdp_events::forget(id);
    });
}
