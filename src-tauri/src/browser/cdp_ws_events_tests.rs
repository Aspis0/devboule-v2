//! What a websocket page's events do to the counters, the ring and the
//! reports once they leave the socket: the same ingestion the WebView2 path
//! feeds, from the payloads a real target sends. The endpoint is the
//! transport's own fake, so what is proven here is the reader's channel
//! feeding the shared listener, not a stand-in for either half.

use serde_json::json;

use super::events_support::{before_navigation, say, until, watching, Said, MAIN};
use super::fake::{FakeServer, Step};
use crate::browser::cdp::{CdpError, Page as _};
use crate::browser::cdp_events;
use crate::browser::console::{self, Wanted};

#[test]
fn the_events_a_websocket_page_sends_are_what_a_settle_waits_on() {
    tauri::async_runtime::block_on(async {
        let mut steps = before_navigation();
        steps.push(Step::Event {
            method: "Page.loadEventFired".to_owned(),
            params: json!({ "timestamp": 10.0 }),
        });
        steps.push(Step::Event {
            method: "Page.frameStoppedLoading".to_owned(),
            params: json!({ "frameId": MAIN }),
        });
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let id = "tab-ws-settle";
        let said = Said::default();
        let watch = watching(&page, id, events, &said).await;
        assert_eq!(cdp_events::moved(id), 0, "nothing has moved yet");

        say(&page).await;
        say(&page).await;

        until(|| cdp_events::moved(id) == 2).await;

        page.close().await;
        drop(watch);
        cdp_events::forget(id);
    });
}

#[test]
fn a_move_within_the_document_over_the_socket_is_reported_as_the_tabs_address() {
    tauri::async_runtime::block_on(async {
        let mut steps = before_navigation();
        steps.push(Step::Event {
            method: "Page.navigatedWithinDocument".to_owned(),
            params: json!({ "frameId": MAIN, "url": "https://example.test/next" }),
        });
        steps.push(Step::Event {
            method: "Page.navigatedWithinDocument".to_owned(),
            params: json!({ "frameId": "advert", "url": "https://ads.test/click" }),
        });
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let id = "tab-ws-same-document";
        let said = Said::default();
        let watch = watching(&page, id, events, &said).await;

        say(&page).await;
        say(&page).await;

        until(|| cdp_events::moved(id) == 2).await;
        assert_eq!(
            said.moved_to(),
            ["https://example.test/next"],
            "the tab's address is its own frame's move; an iframe's is the iframe's"
        );
        assert_eq!(
            cdp_events::moved(id),
            2,
            "both moves are movement, whoever's frame they happened in"
        );

        page.close().await;
        drop(watch);
        cdp_events::forget(id);
    });
}

#[test]
fn a_new_document_over_the_socket_invalidates_what_the_old_one_held() {
    tauri::async_runtime::block_on(async {
        let mut steps = before_navigation();
        steps.push(Step::Event {
            method: "Runtime.consoleAPICalled".to_owned(),
            params: json!({
                "type": "log",
                "args": [{ "type": "string", "value": "before the document changed" }],
                "timestamp": 1.0,
            }),
        });
        steps.push(Step::Event {
            method: "Page.frameNavigated".to_owned(),
            params: json!({ "frame": { "id": MAIN } }),
        });
        steps.push(Step::Error {
            code: -32000,
            message: "No node with given id found".to_owned(),
        });
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let id = "tab-ws-document";
        let said = Said::default();
        let watch = watching(&page, id, events, &said).await;

        say(&page).await;
        until(|| console::entries(id, Wanted::All, None).0.len() == 1).await;

        say(&page).await;
        until(|| said.commits() == 1).await;
        assert_eq!(
            cdp_events::documents(id),
            1,
            "the tab's own frame committing is what the pane is told and what \
             the delta reads as a navigation"
        );
        assert!(
            console::entries(id, Wanted::All, None).0.is_empty(),
            "what the page said about the page it left is not what it says now"
        );

        // The refs a caller holds were taken against the old document: the
        // runtime refuses one by name, and the transport answers a stale ref.
        let refused = page
            .call("DOM.getBoxModel", json!({ "backendNodeId": 15 }))
            .await
            .expect_err("a node of the old document is gone");
        assert_eq!(refused, CdpError::StaleRef);

        page.close().await;
        drop(watch);
        cdp_events::forget(id);
    });
}

#[test]
fn the_console_and_exception_ring_is_the_one_the_webview2_path_fills() {
    tauri::async_runtime::block_on(async {
        let mut steps = before_navigation();
        steps.push(Step::Event {
            method: "Runtime.consoleAPICalled".to_owned(),
            params: json!({
                "type": "warning",
                "args": [{ "type": "string", "value": "slow response" }],
                "timestamp": 4.0,
            }),
        });
        steps.push(Step::Event {
            method: "Runtime.exceptionThrown".to_owned(),
            params: json!({
                "timestamp": 5.0,
                "exceptionDetails": {
                    "exceptionId": 1,
                    "text": "Uncaught",
                    "exception": {
                        "type": "object",
                        "className": "TypeError",
                        "description": "TypeError: x is not a function",
                    },
                },
            }),
        });
        steps.push(Step::Event {
            method: "Log.entryAdded".to_owned(),
            params: json!({
                "entry": {
                    "source": "network",
                    "level": "error",
                    "text": "a resource failed",
                    "timestamp": 6.0,
                },
            }),
        });
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let id = "tab-ws-console";
        let said = Said::default();
        let watch = watching(&page, id, events, &said).await;

        say(&page).await;
        say(&page).await;
        say(&page).await;
        until(|| console::entries(id, Wanted::All, None).0.len() == 3).await;

        let (all, dropped) = console::entries(id, Wanted::All, None);
        assert_eq!(dropped, 0);
        assert_eq!(
            all.iter()
                .map(|entry| (
                    entry.level.as_str(),
                    entry.source.as_deref(),
                    entry.text.as_str()
                ))
                .collect::<Vec<_>>(),
            [
                ("warning", None, "slow response"),
                ("error", Some("exception"), "TypeError: x is not a function"),
                ("error", Some("network"), "a resource failed"),
            ]
        );
        assert_eq!(
            console::entries(id, Wanted::Error, None).0.len(),
            2,
            "errors only is the throw and the failed resource"
        );
        assert_eq!(
            cdp_events::moved(id),
            0,
            "what a page says is not where it is: a page logging on a timer \
             must not hold a settle open"
        );

        page.close().await;
        drop(watch);
        cdp_events::forget(id);
    });
}
