//! `resize`: the override it sends, the size a tab keeps, and the tab on screen
//! that is left to its pane.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::browser::test_support::{parked_tab, FakePage};

use super::resize;

/// The window a phone-sized override makes, as the page reports it afterwards.
fn phone_viewport() -> Value {
    json!({
        "layoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": 390, "clientHeight": 844 },
        "cssLayoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": 390, "clientHeight": 844 },
    })
}

fn phone_page() -> FakePage {
    FakePage::new()
        .answering("Emulation.setDeviceMetricsOverride", json!({}))
        .answering("Page.getLayoutMetrics", phone_viewport())
}

#[test]
fn a_background_tab_is_laid_out_at_the_size_asked_and_answers_the_size_it_has() {
    let tab = parked_tab("tab-1");
    let page = phone_page();

    let answered = tauri::async_runtime::block_on(resize(
        &tab,
        &page,
        &json!({ "browserId": "tab-1", "width": 390, "height": 844 }),
    ))
    .expect("answered");

    let sent = page
        .last_params("Emulation.setDeviceMetricsOverride")
        .expect("the override was sent");
    assert_eq!(sent["width"], 390.0);
    assert_eq!(sent["height"], 844.0);
    assert_eq!(answered, json!({ "width": 390.0, "height": 844.0 }));
}

#[test]
fn the_size_asked_is_the_one_the_tabs_later_commands_are_measured_at() {
    let tab = parked_tab("tab-1");
    let page = phone_page();

    tauri::async_runtime::block_on(resize(
        &tab,
        &page,
        &json!({ "browserId": "tab-1", "width": 390, "height": 844 }),
    ))
    .expect("answered");

    let kept = tab.live.size();
    assert_eq!((kept.width, kept.height), (390.0, 844.0));
}

#[test]
fn a_tab_on_screen_is_left_to_its_pane_and_no_override_is_sent() {
    let tab = parked_tab("tab-1");
    tab.live.set_parked(false);
    let page = phone_page();

    let error = tauri::async_runtime::block_on(resize(
        &tab,
        &page,
        &json!({ "browserId": "tab-1", "width": 390, "height": 844 }),
    ))
    .expect_err("the pane sets the size of a tab on screen");

    assert!(error.message.contains("on screen"), "{}", error.message);
    assert_eq!(page.called("Emulation.setDeviceMetricsOverride"), 0);
}

#[test]
fn a_tab_the_pane_shows_during_the_resize_keeps_the_size_the_pane_gave_it() {
    let tab = parked_tab("tab-1");
    let before = tab.live.size();
    let presenting = Arc::clone(&tab.live);
    let page = phone_page().during("Emulation.setDeviceMetricsOverride", move || {
        presenting.set_parked(false)
    });

    let error = tauri::async_runtime::block_on(resize(
        &tab,
        &page,
        &json!({ "browserId": "tab-1", "width": 390, "height": 844 }),
    ))
    .expect_err("the pane showed the tab while it was resized");

    assert!(
        error.message.contains("shown in its pane"),
        "{}",
        error.message
    );
    let kept = tab.live.size();
    assert_eq!(
        (kept.width, kept.height),
        (before.width, before.height),
        "the remembered size is the pane's"
    );
}
