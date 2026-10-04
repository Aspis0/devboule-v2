//! `click_at`: a point read off a screenshot, pressed where it is.

use crate::browser::commands::{on_tab, BrowserError, Deadline};
use crate::browser::test_support::{ax_fixture, parked_tab, FakePage};
use devboule_protocol::BrowserErrorCode;
use serde_json::{json, Value};

/// What `Page.getLayoutMetrics` answers with, in the shape the protocol writes
/// it: `cssLayoutViewport` in CSS pixels beside `layoutViewport` in the
/// device's own. The two differ on a scaled display, which is why the CSS one
/// is the one a point is refused against.
fn metrics(width: f64, height: f64) -> Value {
    json!({
        "layoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": width, "clientHeight": height },
        "visualViewport": { "offsetX": 0, "offsetY": 0, "pageX": 0, "pageY": 0,
                            "clientWidth": width, "clientHeight": height, "scale": 1 },
        "contentSize": { "x": 0, "y": 0, "width": width, "height": height * 3.0 },
        "cssLayoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": width, "clientHeight": height },
        "cssContentSize": { "x": 0, "y": 0, "width": width, "height": height * 3.0 },
        "cssVisualViewport": { "offsetX": 0, "offsetY": 0, "pageX": 0, "pageY": 0,
                               "clientWidth": width, "clientHeight": height, "scale": 1 }
    })
}

/// A page with a viewport, a layout, and nothing else said of it.
fn form_page() -> FakePage {
    FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", metrics(1024.0, 768.0))
}

/// One point, pressed on a fresh page. The page is handed back with the answer
/// because every claim here is about what the page was asked to do.
fn press(x: f64, y: f64, extra: Value) -> (Result<Value, BrowserError>, FakePage) {
    let tab = parked_tab("tab-1");
    let page = form_page();
    let mut args = json!({ "browserId": "tab-1", "x": x, "y": y });
    if let (Some(object), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
        object.extend(
            extra
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    let answered = tauri::async_runtime::block_on(on_tab(
        &tab,
        &page,
        "click_at",
        &args,
        Deadline::in_(std::time::Duration::from_secs(10)),
    ));
    (answered, page)
}

fn events(page: &FakePage) -> Vec<Value> {
    page.calls()
        .into_iter()
        .filter(|(method, _)| method == "Input.dispatchMouseEvent")
        .map(|(_, params)| params)
        .collect()
}

#[test]
fn a_point_is_pressed_where_it_was_read_off_the_picture() {
    let (answered, page) = press(640.0, 300.0, json!({}));

    let answered = answered.expect("the page answers");
    let events = events(&page);
    assert_eq!(events.len(), 2, "a press and a release, as a click is");
    assert_eq!(events[0]["type"], "mousePressed");
    assert_eq!(events[1]["type"], "mouseReleased");
    for event in &events {
        assert_eq!(event["x"], 640.0, "in the CSS pixels the viewport is in");
        assert_eq!(event["y"], 300.0);
        assert_eq!(event["button"], "left", "a click, unless it says otherwise");
        assert_eq!(event["clickCount"], 1);
    }
    assert!(
        answered.get("delta").is_some(),
        "a click answers with a delta"
    );
}

#[test]
fn a_right_button_and_a_double_click_go_through_as_asked() {
    let (_, page) = press(10.0, 10.0, json!({ "button": "right", "clickCount": 2 }));

    let events = events(&page);
    assert_eq!(events.len(), 2);
    for event in &events {
        assert_eq!(event["button"], "right");
        assert_eq!(event["clickCount"], 2);
    }
}

#[test]
fn a_point_off_the_page_is_refused_before_anything_is_dispatched() {
    for (x, y) in [(-1.0, 10.0), (10.0, -1.0), (1024.5, 10.0), (10.0, 769.0)] {
        let (answered, page) = press(x, y, json!({}));

        let error = answered.expect_err("off the page");
        assert_eq!(error.code, BrowserErrorCode::HostError, "{x},{y}");
        assert!(
            error.message.contains("off the page"),
            "{x},{y}: {}",
            error.message
        );
        assert!(
            events(&page).is_empty(),
            "{x},{y}: a point off the page is pressed nowhere"
        );
    }
}

#[test]
fn the_very_edge_of_the_viewport_is_still_on_the_page() {
    let (answered, _) = press(1024.0, 768.0, json!({}));

    answered.expect("the last pixel of the last row is on the page");
}

#[test]
fn a_page_that_reports_no_viewport_cannot_be_pointed_at() {
    let tab = parked_tab("tab-1");
    let page = FakePage::new().answering("Accessibility.getFullAXTree", ax_fixture());

    let error = tauri::async_runtime::block_on(on_tab(
        &tab,
        &page,
        "click_at",
        &json!({ "browserId": "tab-1", "x": 1.0, "y": 1.0 }),
        Deadline::in_(std::time::Duration::from_secs(10)),
    ))
    .expect_err("no viewport, no point");

    assert!(
        error.message.contains("layout viewport"),
        "{}",
        error.message
    );
}
