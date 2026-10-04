//! How long a command may take and who it waits for: the deadline every call
//! is cut to, and the tab's lock an acting command takes.

use super::*;
use crate::browser::test_support::{ax_fixture, box_model, parked_tab, registry_with, FakePage};
use devboule_protocol::BrowserErrorCode;
use serde_json::json;
use std::time::{Duration, Instant};

fn form_page() -> FakePage {
    FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("DOM.getBoxModel", box_model())
}

fn spent() -> Deadline {
    let spent = Deadline::in_(Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(20));
    spent
}

#[test]
fn a_command_with_no_time_left_touches_nothing() {
    let page = form_page();
    let tab = parked_tab("tab-1");

    for (command, args) in [
        ("click", json!({ "browserId": "tab-1", "ref": "e15" })),
        ("snapshot", json!({ "browserId": "tab-1" })),
        (
            "navigate",
            json!({ "browserId": "tab-1", "action": "reload" }),
        ),
    ] {
        let error = tauri::async_runtime::block_on(on_tab(&tab, &page, command, &args, spent()))
            .expect_err(command);
        assert!(
            error.message.contains("ran out of time"),
            "{command}: {}",
            error.message
        );
    }

    assert!(
        page.calls().is_empty(),
        "not one call reached the page: {:?}",
        page.calls()
    );
}

#[test]
fn a_wait_puts_no_parked_page_on_screen_because_it_only_reads() {
    let page = form_page();
    let tab = parked_tab("tab-1");
    let args = json!({ "browserId": "tab-1", "text": "remember", "timeoutMs": 500 });

    tauri::async_runtime::block_on(on_tab(
        &tab,
        &page,
        "wait_for",
        &args,
        Deadline::in_(Duration::from_secs(10)),
    ))
    .expect("answered");

    assert_eq!(page.called("Emulation.setDeviceMetricsOverride"), 0);
}

#[test]
fn a_command_waits_for_a_tab_another_holds_and_then_takes_it() {
    let registry = registry_with("tab-1");
    let held = registry
        .guard_of("tab-1")
        .expect("the tab has a guard")
        .try_lock_owned()
        .expect("nobody holds it yet");
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(held);
    });

    let started = Instant::now();
    let taken = tauri::async_runtime::block_on(hold(
        &registry,
        "tab-1",
        Deadline::in_(Duration::from_secs(5)),
    ));

    assert!(taken.is_ok(), "the holder let go inside the deadline");
    assert!(
        started.elapsed() >= Duration::from_millis(100),
        "and it was waited for, not skipped: {:?}",
        started.elapsed()
    );
    release.join().expect("the holder ends");
}

#[test]
fn a_command_that_cannot_have_the_tab_in_time_says_it_is_busy() {
    let registry = registry_with("tab-1");
    let _held = registry
        .guard_of("tab-1")
        .expect("the tab has a guard")
        .try_lock_owned()
        .expect("nobody holds it yet");

    let started = Instant::now();
    let error = tauri::async_runtime::block_on(hold(
        &registry,
        "tab-1",
        Deadline::in_(Duration::from_millis(120)),
    ))
    .err()
    .expect("the tab never came free");

    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(error.message.contains("busy"), "{}", error.message);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the wait ended with the deadline: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_tab_that_is_gone_is_not_waited_for() {
    let registry = registry_with("tab-1");

    let error = tauri::async_runtime::block_on(hold(
        &registry,
        "tab-404",
        Deadline::in_(Duration::from_secs(5)),
    ))
    .err()
    .expect("no such tab");

    assert_eq!(error.code, BrowserErrorCode::TabNotFound);
}
