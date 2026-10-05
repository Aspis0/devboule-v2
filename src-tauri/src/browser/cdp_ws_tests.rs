//! What a reply does: the waiter its own id reaches, a reply that turns up
//! after its caller gave up, a notification, and the two refusals the
//! WebView2 adapter also answers with.

use futures_util::future::join;
use serde_json::json;

use super::fake::{FakeServer, Step};
use super::*;

#[test]
fn each_call_is_answered_by_the_reply_carrying_its_own_id() {
    tauri::async_runtime::block_on(async {
        let (_server, page, _events) = FakeServer::start(vec![Step::Echo, Step::Echo])
            .await
            .attached()
            .await;

        let (read, navigate) = join(
            page.call("Runtime.evaluate", json!({ "expression": "1" })),
            page.call("Page.navigate", json!({ "url": "about:blank" })),
        )
        .await;

        assert_eq!(
            read.expect("the read is answered")["method"],
            "Runtime.evaluate"
        );
        assert_eq!(
            navigate.expect("the navigation is answered")["method"],
            "Page.navigate",
            "two calls in flight must not be told they were each other"
        );
    });
}

#[test]
fn a_reply_that_arrives_late_still_reaches_the_waiter_that_asked_for_it() {
    tauri::async_runtime::block_on(async {
        // The first command's answer is held back until the second command has
        // been read, so it lands after the reply to a later call.
        let (_server, page, _events) = FakeServer::start(vec![Step::Hold, Step::Release])
            .await
            .attached()
            .await;

        let (first, second) = join(
            page.call("Runtime.evaluate", json!({ "expression": "1" })),
            page.call("Page.navigate", json!({ "url": "about:blank" })),
        )
        .await;

        assert_eq!(
            first.expect("the held answer is released")["method"],
            "Runtime.evaluate"
        );
        assert_eq!(
            second.expect("the release is answered")["method"],
            "Page.navigate"
        );
    });
}

#[test]
fn a_reply_that_arrives_after_its_caller_gave_up_is_dropped() {
    tauri::async_runtime::block_on(async {
        // The first command's answer is held back until the second command has
        // been read, and it is written FIRST, while the second call is still
        // waiting for its own. A reader that routed by arrival rather than by
        // id would hand the stale answer to the live caller.
        let (_server, page, _events) =
            FakeServer::start(vec![Step::Hold, Step::ReleaseHeldFirst, Step::Echo])
                .await
                .attached()
                .await;

        let abandoned = page
            .call_within(
                "Runtime.evaluate",
                json!({ "expression": "1" }),
                Duration::from_millis(60),
            )
            .await
            .expect_err("the held answer comes after this budget");
        assert_eq!(
            abandoned,
            CdpError::Refused("Runtime.evaluate: the page did not answer".to_owned())
        );
        assert_eq!(page.waiting().await, 0, "and it left no waiter");

        let late = page
            .call("Page.navigate", json!({ "url": "about:blank" }))
            .await
            .expect("the next call is answered");
        assert_eq!(
            late["method"], "Page.navigate",
            "the answer held for the caller that gave up must not land here"
        );

        page.call("Runtime.evaluate", json!({ "expression": "1" }))
            .await
            .expect("and the page keeps answering afterwards");
        assert_eq!(page.waiting().await, 0);
    });
}

#[test]
fn an_event_the_reader_saw_reaches_the_subscriber_and_is_not_an_answer() {
    tauri::async_runtime::block_on(async {
        let (_server, page, mut events) = FakeServer::start(vec![Step::Event {
            method: "Page.loadEventFired".to_owned(),
            params: json!({ "timestamp": 12.5 }),
        }])
        .await
        .attached()
        .await;

        page.call("Page.navigate", json!({ "url": "about:blank" }))
            .await
            .expect("the command that carried the event is answered");

        let event = events
            .state
            .try_recv()
            .expect("the event reached its subscriber");
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.params["timestamp"], 12.5);
        assert!(
            events.state.try_recv().is_err() && events.voice.try_recv().is_err(),
            "an event is not a second answer to the command that carried it"
        );
    });
}

#[test]
fn a_frame_that_is_not_a_protocol_message_is_not_an_event() {
    tauri::async_runtime::block_on(async {
        let (_server, page, mut events) = FakeServer::start(vec![Step::NotAMessage])
            .await
            .attached()
            .await;

        page.call("Page.navigate", json!({ "url": "about:blank" }))
            .await
            .expect("the command is answered");

        assert!(
            events.state.try_recv().is_err() && events.voice.try_recv().is_err(),
            "an object with neither an id nor a method is not a notification, \
             and a subscriber with an empty name cannot act on one"
        );
    });
}
#[test]
fn a_protocol_error_object_names_the_method_that_was_refused() {
    tauri::async_runtime::block_on(async {
        let (_server, page, _events) = FakeServer::start(vec![Step::Error {
            code: -32601,
            message: "'Runtime.nosuch' wasn't found".to_owned(),
        }])
        .await
        .attached()
        .await;

        let refused = page
            .call("Runtime.nosuch", json!({}))
            .await
            .expect_err("an unknown method is refused");

        assert_eq!(
            refused,
            CdpError::Refused("Runtime.nosuch: 'Runtime.nosuch' wasn't found".to_owned()),
            "the WebView2 shape: the method, then the runtime's own text"
        );
    });
}

#[test]
fn a_refusal_of_a_call_that_named_a_node_is_a_stale_ref() {
    tauri::async_runtime::block_on(async {
        let (_server, page, _events) = FakeServer::start(vec![Step::Error {
            code: -32602,
            message: "No node with given id found".to_owned(),
        }])
        .await
        .attached()
        .await;

        let refused = page
            .call("DOM.getBoxModel", json!({ "backendNodeId": 15 }))
            .await
            .expect_err("a dead node is refused");

        assert_eq!(refused, CdpError::StaleRef);
    });
}
