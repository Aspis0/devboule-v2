//! What a budget does: a call with none left is never written, a call that is
//! not answered in time takes its waiter with it, and the time left covers
//! queueing behind another writer rather than only the wait.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::fake::{FakeServer, Step};
use super::*;
use crate::browser::cdp::Bounded;
use crate::browser::deadline::Deadline;

#[test]
fn a_command_with_no_budget_left_is_never_written() {
    tauri::async_runtime::block_on(async {
        // One answer in the script and two calls: the first has no budget, so
        // it must leave the endpoint's script untouched on the way out.
        let (server, page, _events) = FakeServer::start(vec![Step::Echo]).await.attached().await;

        let refused = page
            .call_within(
                "Page.navigate",
                json!({ "url": "about:blank" }),
                Duration::ZERO,
            )
            .await
            .expect_err("no budget is no call");

        assert_eq!(
            refused,
            CdpError::Refused("Page.navigate: the page did not answer".to_owned())
        );
        assert_eq!(
            server.read().await,
            Vec::<String>::new(),
            "a call with no budget left must write no frame at all: a \
             navigation sent into a command that can never answer it is a side \
             effect nobody will ever see"
        );
        page.call("Runtime.evaluate", json!({ "expression": "1" }))
            .await
            .expect("the page still answers the next call");
    });
}

#[test]
fn a_spent_command_budget_is_refused_before_anything_is_written() {
    tauri::async_runtime::block_on(async {
        let (server, page, _events) = FakeServer::start(vec![Step::Echo]).await.attached().await;
        let bounded = Bounded::new(&page, Deadline::in_(Duration::from_millis(20)));
        std::thread::sleep(Duration::from_millis(50));

        bounded
            .call("Page.navigate", json!({ "url": "about:blank" }))
            .await
            .expect_err("a spent budget writes nothing");

        assert_eq!(server.read().await, Vec::<String>::new());
        page.call("Runtime.evaluate", json!({ "expression": "1" }))
            .await
            .expect("the page still answers the next call");
    });
}

#[test]
fn a_call_that_is_not_answered_runs_out_of_its_budget_and_leaves_no_waiter() {
    tauri::async_runtime::block_on(async {
        let (_server, page, _events) = FakeServer::start(vec![Step::Silence, Step::Echo])
            .await
            .attached()
            .await;

        let late = page
            .call_within(
                "Page.navigate",
                json!({ "url": "about:blank" }),
                Duration::from_millis(50),
            )
            .await
            .expect_err("no answer is no result");

        assert_eq!(
            late,
            CdpError::Refused("Page.navigate: the page did not answer".to_owned()),
            "the WebView2 text for a completion that never arrives"
        );
        assert_eq!(
            page.waiting().await,
            0,
            "a waiter whose budget is spent is taken out of the map, or the \
             map grows for the life of the tab"
        );

        page.call("Runtime.evaluate", json!({ "expression": "1" }))
            .await
            .expect("the page still answers the next call");
    });
}

#[test]
fn a_call_whose_budget_runs_out_while_the_writer_is_held_still_refuses() {
    tauri::async_runtime::block_on(async {
        let (_server, page, _events) = FakeServer::start(vec![Step::Echo]).await.attached().await;
        let page = Arc::new(page);
        let writer = page.hold_the_writer().await;

        let waiting = tauri::async_runtime::spawn({
            let page = Arc::clone(&page);
            async move {
                page.call_within(
                    "Page.navigate",
                    json!({ "url": "about:blank" }),
                    Duration::from_millis(60),
                )
                .await
            }
        });
        let given_up = tokio::time::timeout(Duration::from_millis(400), waiting).await;

        assert!(
            given_up.is_ok(),
            "the budget covers the queueing behind another writer, not only \
             the wait for the answer: this call is still held up while it \
             should have given up six budgets ago"
        );
        assert_eq!(
            given_up
                .expect("the call gave up inside its budget")
                .expect("the call task finished")
                .expect_err("the write never went out, so there is no answer to wait for"),
            CdpError::Refused("Page.navigate: the page did not answer".to_owned())
        );
        assert_eq!(page.waiting().await, 0);
        drop(writer);
    });
}

#[test]
fn a_bounded_command_cuts_the_wait_to_what_is_left_of_its_budget() {
    tauri::async_runtime::block_on(async {
        let (_server, page, _events) = FakeServer::start(vec![Step::Silence])
            .await
            .attached()
            .await;
        let bounded = Bounded::new(&page, Deadline::in_(Duration::from_millis(60)));

        let error = bounded
            .call("Page.navigate", json!({ "url": "about:blank" }))
            .await
            .expect_err("no answer is no result");

        assert!(
            error.message().contains("did not answer"),
            "the transport's own refusal, not a deadline the command layer \
             answers separately: {error}"
        );
        assert_eq!(page.waiting().await, 0);
    });
}
