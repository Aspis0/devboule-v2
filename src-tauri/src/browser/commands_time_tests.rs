//! How long a command may take: the deadline every call is cut to, and what a
//! wait is allowed to do with it.

use super::*;
use crate::browser::test_support::{ax_fixture, box_model, parked_tab, FakePage};
use serde_json::json;
use std::time::Duration;

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
