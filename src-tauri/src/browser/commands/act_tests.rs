use super::*;
use crate::browser::test_support::FakePage;

#[test]
fn the_middle_of_a_box_is_the_middle_of_the_quad_the_runtime_answers() {
    // Recorded from a real `DOM.getBoxModel` on this WebView2: a flat array of
    // eight numbers, four corners, x then y. A parser that expects points or
    // objects reads none of them, and refuses every click on a link.
    let model = serde_json::json!({ "model": {
        "border":  [0.0, 0.0, 817.6, 0.0, 817.6, 716.0, 0.0, 716.0],
        "content": [0.0, 0.0, 802.4, 0.0, 802.4, 716.0, 0.0, 716.0],
        "height": 716.0,
        "margin":  [0.0, 0.0, 817.6, 0.0, 817.6, 716.0, 0.0, 716.0],
        "padding": [0.0, 0.0, 802.4, 0.0, 802.4, 716.0, 0.0, 716.0],
        "width": 802.4
    } });
    assert_eq!(box_centre(&model), Some((401.2, 358.0)));

    // An inline link: a small content box inside a page-sized one, which is
    // what the top bar of a page hands back.
    let link = serde_json::json!({ "model": {
        "border":  [24.0, 8.0, 96.0, 8.0, 96.0, 44.0, 24.0, 44.0],
        "content": [30.0, 15.0, 92.0, 15.0, 92.0, 38.0, 30.0, 38.0],
        "height": 23.0,
        "margin":  [24.0, 8.0, 96.0, 8.0, 96.0, 44.0, 24.0, 44.0],
        "padding": [30.0, 15.0, 92.0, 15.0, 92.0, 38.0, 30.0, 38.0],
        "width": 62.0
    }, "backendNodeId": 533 });
    assert_eq!(box_centre(&link), Some((61.0, 26.5)));

    // A node with no box (hidden, or gone between the snapshot and the click)
    // is not a point, and neither is a shape this runtime does not answer.
    assert_eq!(
        box_centre(&serde_json::json!({ "model": { "content": [] } })),
        None
    );
    assert_eq!(
        box_centre(&serde_json::json!({ "model": { "content": [1.0, 2.0] } })),
        None,
        "a pair is not a quad"
    );
    assert_eq!(box_centre(&serde_json::json!({})), None);
    assert_eq!(
        box_centre(&serde_json::json!({ "model": { "content": [
            "0", "0", "1", "1", "2", "2", "3", "3"
        ]}})),
        None,
        "and neither is a quad of words"
    );
}

#[test]
fn a_node_is_found_in_a_view_by_the_ref_the_caller_has() {
    let node = super::super::node_of("e14").expect("a ref is a node");
    assert_eq!(node, 14);
    for wrong in ["e", "Sign in", "E14", ""] {
        assert!(
            super::super::node_of(wrong).is_err(),
            "{wrong} is not a ref this app handed out"
        );
    }
}

/// The interleave a stalled command allows: it resolved the tab while parked,
/// the pane then waited out its moment and presented the page without the lock,
/// and only then does the command get to `ready`.
#[test]
fn a_command_that_resolved_the_tab_parked_does_not_override_a_page_the_pane_presented() {
    let registry = crate::browser::test_support::registry_with("tab-1");
    let tab = registry.tab_of("tab-1").expect("the fixture tab");
    assert!(
        tab.live.parked(),
        "it was parked when the command resolved it"
    );

    let pane = crate::browser::registry::LogicalRect {
        x: 455.0,
        y: 49.0,
        width: 770.0,
        height: 751.0,
    };
    registry.set_rect("tab-1", pane, false);
    let page = FakePage::new();

    tauri::async_runtime::block_on(ready(&tab, &page)).expect("nothing to do");

    assert_eq!(
        page.called("Emulation.setDeviceMetricsOverride"),
        0,
        "the page is in front of a person now"
    );
    assert!(!tab.live.overridden());
}

#[test]
fn a_parked_page_that_gets_an_override_says_so_and_presenting_it_takes_the_flag() {
    let registry = crate::browser::test_support::registry_with("tab-1");
    let tab = registry.tab_of("tab-1").expect("the fixture tab");
    let page = FakePage::new();

    tauri::async_runtime::block_on(ready(&tab, &page)).expect("overridden");
    assert_eq!(page.called("Emulation.setDeviceMetricsOverride"), 1);
    assert!(tab.live.overridden());

    let pane = crate::browser::registry::LogicalRect {
        x: 0.0,
        y: 0.0,
        width: 770.0,
        height: 751.0,
    };
    assert!(
        registry.set_rect("tab-1", pane, false),
        "presenting reports the override it found, for the pane to clear"
    );
    assert!(!tab.live.overridden());
    assert!(!registry.set_rect("tab-1", pane, false), "and only once");
}

#[test]
fn an_override_that_lands_while_the_pane_presents_is_cleared_by_the_command() {
    let registry = std::sync::Arc::new(crate::browser::test_support::registry_with("tab-1"));
    let tab = registry.tab_of("tab-1").expect("the fixture tab");
    let pane = crate::browser::registry::LogicalRect {
        x: 0.0,
        y: 0.0,
        width: 770.0,
        height: 751.0,
    };
    let presenting = std::sync::Arc::clone(&registry);
    let page = FakePage::new().during("Emulation.setDeviceMetricsOverride", move || {
        presenting.set_rect("tab-1", pane, false);
    });

    tauri::async_runtime::block_on(ready(&tab, &page)).expect("answered");

    assert_eq!(page.called("Emulation.clearDeviceMetricsOverride"), 1);
    assert!(
        !tab.live.overridden(),
        "no override is left on a page in front"
    );
    assert!(!tab.live.parked());
}
