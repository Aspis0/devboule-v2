//! What opens and ends a page: which addresses are opened at all, a socket
//! that goes away under a call, a frame past the cap, and the three ways a
//! page stops serving — close, an ended reader, and a reader that died.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::fake::{FakeServer, Step};
use super::*;

#[test]
fn only_a_loopback_websocket_is_opened() {
    tauri::async_runtime::block_on(async {
        for url in [
            // A name is not a loopback address: what it resolves to on this
            // machine is not this app's decision to make, and the address is
            // built from a port the browser wrote itself.
            "ws://localhost:9222/devtools/page/1",
            "ws://LOCALHOST:9222/devtools/page/1",
            "ws://localhost.localdomain:9222/devtools/page/1",
            "wss://127.0.0.1:9222/devtools/page/1",
            "ws://192.168.1.20:9222/devtools/page/1",
            "ws://127.0.0.1.evil.test:9222/devtools/page/1",
            "ws://[::2]:9222/devtools/page/1",
            "ws://0.0.0.0:9222/devtools/page/1",
            "ws://",
        ] {
            let refused = WsPage::connect(url)
                .await
                .err()
                .unwrap_or_else(|| panic!("{url} is not a debugger this app opens"));
            assert!(
                refused.message().contains("loopback ws://"),
                "{url} was refused as {refused}"
            );
        }
    });
}

#[test]
fn the_loopback_addresses_are_the_two_the_browser_prints() {
    tauri::async_runtime::block_on(async {
        for url in [
            "ws://127.0.0.1:9222/devtools/page/1",
            "ws://[::1]:9222/devtools/page/1",
            "ws://127.0.0.1:1/",
        ] {
            assert!(
                loopback(url).is_ok(),
                "{url} is a loopback literal and must be opened"
            );
        }
    });
}

#[test]
fn a_socket_that_closes_under_a_call_refuses_it_rather_than_hanging() {
    tauri::async_runtime::block_on(async {
        let (_server, page, _events) = FakeServer::start(vec![Step::Disconnect])
            .await
            .attached()
            .await;

        let refused = tokio::time::timeout(
            Duration::from_secs(2),
            page.call_within(
                "Page.navigate",
                json!({ "url": "about:blank" }),
                Duration::from_secs(5),
            ),
        )
        .await
        .expect("the reader's exit releases this waiter, not its own budget")
        .expect_err("a closed socket never answers");

        assert_eq!(
            refused,
            CdpError::Refused("Page.navigate: the page did not answer".to_owned())
        );
    });
}

#[test]
fn a_frame_past_the_cap_fails_its_own_call_and_releases_the_rest() {
    tauri::async_runtime::block_on(async {
        // One command, two calls: the endpoint answers the first with a
        // message past the cap. Nothing may be left waiting, so the caller
        // that is answered next to it is refused rather than held open.
        let (_server, page, _events) = FakeServer::start(vec![Step::Oversize])
            .await
            .attached()
            .await;
        let page = Arc::new(page);
        let second = tauri::async_runtime::spawn({
            let page = Arc::clone(&page);
            async move {
                page.call_within(
                    "Page.navigate",
                    json!({ "url": "about:blank" }),
                    Duration::from_secs(5),
                )
                .await
            }
        });

        let refused = page
            .call_within(
                "Page.captureScreenshot",
                json!({ "format": "png" }),
                Duration::from_secs(5),
            )
            .await
            .expect_err("a message past the cap is not answered");

        assert_eq!(
            refused,
            CdpError::Refused("Page.captureScreenshot: the page did not answer".to_owned()),
            "the reader cannot be resumed after an oversized frame, so the \
             call is refused like any other unanswered one"
        );
        assert_eq!(
            second
                .await
                .expect("the second call is finished")
                .expect_err("a call waiting beside an oversized frame is released, not held"),
            CdpError::Refused("Page.navigate: the page did not answer".to_owned())
        );
        assert_eq!(page.waiting().await, 0);
        let later = tokio::time::timeout(
            Duration::from_secs(2),
            page.call("Runtime.evaluate", json!({ "expression": "1" })),
        )
        .await
        .expect("a socket whose reader ended refuses the next call at once");
        assert_eq!(
            later.expect_err("the socket is gone, so a later call is refused"),
            CdpError::Refused("Runtime.evaluate: the page did not answer".to_owned()),
            "a page whose reader ended must not make every later call sit out \
             its own budget"
        );
    });
}

#[test]
fn closing_a_page_whose_writer_is_held_does_not_wait_for_it() {
    tauri::async_runtime::block_on(async {
        let (_server, page, _events) = FakeServer::start(vec![Step::Echo]).await.attached().await;
        let writer = page.hold_the_writer().await;

        let closed = tokio::time::timeout(Duration::from_secs(5), page.close()).await;

        assert!(
            closed.is_ok(),
            "a write that is not going to drain must not hold a closing tab \
             open; the waiters were released before the frame was attempted"
        );
        assert_eq!(page.waiting().await, 0);
        drop(writer);
    });
}

#[test]
fn closing_releases_the_waiter_in_flight_and_stops_the_reader() {
    tauri::async_runtime::block_on(async {
        let (_server, page, mut events) = FakeServer::start(vec![Step::Silence])
            .await
            .attached()
            .await;
        let page = Arc::new(page);
        let in_flight = tauri::async_runtime::spawn({
            let page = Arc::clone(&page);
            async move {
                page.call_within(
                    "Page.navigate",
                    json!({ "url": "about:blank" }),
                    Duration::from_secs(30),
                )
                .await
            }
        });
        // Long enough for the call to have put its waiter in the map and its
        // frame on the wire, which the endpoint answers with nothing.
        tokio::time::sleep(Duration::from_millis(80)).await;

        page.close().await;

        let outcome = in_flight.await.expect("the call task is finished");
        assert_eq!(
            outcome.expect_err(
                "a call waiting on a socket this page just closed \
                                is never answered"
            ),
            CdpError::Refused("Page.navigate: the page did not answer".to_owned())
        );
        assert_eq!(page.waiting().await, 0);
        assert!(
            events.recv().await.is_none(),
            "the reader ended: it dropped the event sender, so nothing can \
             hold the channel open behind it"
        );
        assert_eq!(
            page.call("Runtime.evaluate", json!({ "expression": "1" }))
                .await
                .expect_err("a closed page answers nothing"),
            CdpError::Refused("Runtime.evaluate: the page did not answer".to_owned())
        );
    });
}

#[test]
fn the_reader_task_releases_its_waiters_when_the_task_itself_is_cancelled() {
    tauri::async_runtime::block_on(async {
        let (server, page, _events) = FakeServer::start(vec![Step::Silence])
            .await
            .attached()
            .await;
        let page = Arc::new(page);
        let in_flight = tauri::async_runtime::spawn({
            let page = Arc::clone(&page);
            async move {
                page.call_within(
                    "Page.navigate",
                    json!({ "url": "about:blank" }),
                    Duration::from_secs(30),
                )
                .await
            }
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(page.waiting().await, 1, "the call is holding a waiter");

        // The production reader task, stopped the way a cancellation stops a
        // task: its future is dropped, and nothing else here drains anything.
        // If `read` stopped holding the guard, the waiter below would sit here
        // until its own thirty seconds ran out instead of being released.
        page.cancel_the_reader().await;
        let released = tokio::time::timeout(Duration::from_secs(5), in_flight).await;

        assert!(
            released.is_ok(),
            "the reader task must release the waiters it was holding as it ends"
        );
        assert_eq!(
            released
                .expect("the call task finished")
                .expect("the call task did not panic")
                .expect_err("a waiter whose reader ended is never answered"),
            CdpError::Refused("Page.navigate: the page did not answer".to_owned())
        );
        assert_eq!(page.waiting().await, 0);
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(2),
                page.call("Runtime.evaluate", json!({ "expression": "1" })),
            )
            .await
            .expect("an ended reader marks the page gone, so a later call is refused at once")
            .expect_err("the socket is gone"),
            CdpError::Refused("Runtime.evaluate: the page did not answer".to_owned())
        );
        assert_eq!(
            server.read().await,
            vec!["Page.navigate".to_owned()],
            "and the command the released caller had already written is the              only one the endpoint ever saw"
        );
    });
}

#[test]
fn a_command_queued_before_a_close_is_never_written_to_the_socket() {
    tauri::async_runtime::block_on(async {
        let (server, page, _events) = FakeServer::start(vec![Step::Echo]).await.attached().await;
        let page = Arc::new(page);
        let writer = page.hold_the_writer().await;

        // This call registers its waiter and queues on the writer, which the
        // test is holding. It is inside the transport and past its first
        // closed check when the close begins.
        let queued = tauri::async_runtime::spawn({
            let page = Arc::clone(&page);
            async move {
                page.call_within(
                    "Page.navigate",
                    json!({ "url": "about:blank" }),
                    Duration::from_secs(30),
                )
                .await
            }
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(page.waiting().await, 1, "the call is holding a waiter");

        let closing = tauri::async_runtime::spawn({
            let page = Arc::clone(&page);
            async move { page.close().await }
        });
        tokio::time::sleep(Duration::from_millis(80)).await;
        drop(writer);

        let outcome = tokio::time::timeout(Duration::from_secs(5), queued)
            .await
            .expect("the queued call is answered")
            .expect("the call task finished");
        assert_eq!(
            outcome.expect_err("the command was never written"),
            CdpError::Refused("Page.navigate: the page did not answer".to_owned())
        );
        closing.await.expect("the close task did not panic");
        assert_eq!(
            server.read().await,
            Vec::<String>::new(),
            "a command queued before the close began must not reach a socket              being torn down: the side effect nobody will ever see is the whole              reason the closed flag is read again under the writer lock"
        );
        assert_eq!(page.waiting().await, 0);
    });
}
