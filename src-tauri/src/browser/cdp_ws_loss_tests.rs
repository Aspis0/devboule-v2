//! What a full queue does: the page's own chatter cannot crowd out the event
//! that says where it moved, and a state event dropped all the same is counted
//! and leaves the state unknown rather than forgotten.
//!
//! A burst is sent before the watch exists, so nothing races the drain: every
//! event is on a stream the moment the command that carried it is answered,
//! and the drain meets the whole queue when it starts.

use serde_json::{json, Value};

use super::events_support::{before_navigation, burst, say, until, watching, Said, MAIN};
use super::fake::{FakeServer, Step};
use crate::browser::cdp_events;
use crate::browser::console::{self, Wanted};

/// One console line, in the shape `Runtime.consoleAPICalled` sends.
fn chatter() -> Value {
    json!({
        "type": "log",
        "args": [{ "type": "string", "value": "chatter" }],
        "timestamp": 2.0,
    })
}

#[test]
fn a_voice_burst_cannot_crowd_out_the_event_that_says_where_the_page_moved() {
    tauri::async_runtime::block_on(async {
        let mut steps = burst(
            super::VOICE_QUEUE + 8,
            "Runtime.consoleAPICalled",
            chatter(),
        );
        steps.push(Step::Event {
            method: "Page.navigatedWithinDocument".to_owned(),
            params: json!({
                "frameId": MAIN,
                "url": "https://example.test/after-the-chatter",
            }),
        });
        steps.extend(before_navigation());
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let drops = events.drops();
        let id = "tab-ws-voice-burst";
        let said = Said::default();

        for _ in 0..super::VOICE_QUEUE + 8 {
            say(&page).await;
        }
        say(&page).await;
        assert_eq!(
            drops.voice(),
            8,
            "the voice queue held what it could and counted the rest"
        );
        assert_eq!(
            drops.state(),
            0,
            "and the move behind it was not turned away"
        );

        let watch = watching(&page, id, events, &said).await;
        until(|| !said.moved_to().is_empty()).await;
        assert_eq!(
            said.moved_to(),
            ["https://example.test/after-the-chatter"],
            "a page that logs in a burst cannot crowd out where it moved to"
        );

        page.close().await;
        drop(watch);
        cdp_events::forget(id);
    });
}

#[test]
fn a_dropped_state_event_invalidates_the_document_without_claiming_a_commit() {
    tauri::async_runtime::block_on(async {
        // A line the page really said, then more state events than the queue
        // holds: the drop is uncertainty about the document, not a commit.
        let mut steps = vec![Step::Event {
            method: "Runtime.consoleAPICalled".to_owned(),
            params: chatter(),
        }];
        steps.extend(burst(
            super::STATE_QUEUE + 4,
            "DOM.documentUpdated",
            json!({}),
        ));
        steps.extend(before_navigation());
        let (_server, page, events) = FakeServer::start(steps).await.attached().await;
        let drops = events.drops();
        let id = "tab-ws-state-loss";
        let said = Said::default();

        say(&page).await;
        for _ in 0..super::STATE_QUEUE + 4 {
            say(&page).await;
        }
        assert_eq!(
            drops.state(),
            4,
            "the state queue held what it could and counted the rest"
        );

        let watch = watching(&page, id, events, &said).await;
        until(|| cdp_events::moved(id) as usize == super::STATE_QUEUE + 1).await;

        assert_eq!(
            cdp_events::documents(id),
            1,
            "a lost event may have been a navigation, so every ref taken \
             before is treated as stale"
        );
        assert_eq!(said.commits(), 0, "and no commit is claimed for a loss");
        assert!(said.moved_to().is_empty());
        assert_eq!(
            console::entries(id, Wanted::All, None).0.len(),
            1,
            "the ring keeps the line the page really said"
        );

        page.close().await;
        drop(watch);
        cdp_events::forget(id);
    });
}
