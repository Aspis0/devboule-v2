//! What installing a websocket watch does to the page, and what letting it go
//! does to the tasks behind it: the domains switched on before the first
//! navigation, the reader stopped by a close, and the drain stopped by a
//! dropped subscription.

use serde_json::json;

use super::events_support::{before_navigation, say, until, watching, Said};
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
        // One event the drain must see, then more than the channel holds after
        // the subscription is gone.
        let mut steps = before_navigation();
        steps.push(Step::Event {
            method: "Page.loadEventFired".to_owned(),
            params: json!({ "timestamp": 1.0 }),
        });
        for _ in 0..super::EVENT_QUEUE + 8 {
            steps.push(Step::Event {
                method: "Page.loadEventFired".to_owned(),
                params: json!({ "timestamp": 2.0 }),
            });
        }
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let id = "tab-ws-dropped";
        let said = Said::default();
        let watch = watching(&page, id, events, &said).await;

        say(&page).await;
        until(|| cdp_events::moved(id) == 1).await;

        drop(watch);
        for _ in 0..super::EVENT_QUEUE + 8 {
            say(&page).await;
        }
        // The events are already on the channel (or were dropped by it) by the
        // time the last call returned; a drain still running would have taken
        // every one of them long before this.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        assert_eq!(
            cdp_events::moved(id),
            1,
            "a dropped subscription's drain is stopped, so the events behind it \
             are dropped rather than counted late"
        );

        page.close().await;
        cdp_events::forget(id);
    });
}
