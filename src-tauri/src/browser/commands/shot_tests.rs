//! `screenshot`: what the page looks like, and what it costs to send.

use super::*;
use crate::browser::commands::{on_tab, BrowserError, Deadline};
use crate::browser::test_support::{ax_fixture, parked_tab, FakePage};
use serde_json::{json, Value};

/// The viewport the page reports, in the CSS pixels the clip and the answer are
/// both written in.
fn viewport() -> Value {
    json!({
        "cssLayoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": 1024, "clientHeight": 768 },
        "cssContentSize": { "x": 0, "y": 0, "width": 1024, "height": 2400 },
    })
}

/// A base64 run of `bytes` bytes, which is what a picture of that size costs.
fn base64_of(bytes: usize) -> String {
    let characters = bytes.div_ceil(3) * 4;
    "A".repeat(characters)
}

fn page_answering(data: String) -> FakePage {
    FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", viewport())
        .answering("Page.captureScreenshot", json!({ "data": data }))
}

fn shoot(page: &FakePage, args: Value) -> Result<Value, BrowserError> {
    let tab = parked_tab("tab-1");
    tauri::async_runtime::block_on(on_tab(
        &tab,
        page,
        "screenshot",
        &args,
        Deadline::in_(std::time::Duration::from_secs(10)),
    ))
}

fn captures(page: &FakePage) -> Vec<Value> {
    page.calls()
        .into_iter()
        .filter(|(method, _)| method == "Page.captureScreenshot")
        .map(|(_, params)| params)
        .collect()
}

#[test]
fn the_viewport_is_taken_as_jpeg_at_the_size_the_page_reports() {
    let page = page_answering(base64_of(1000));

    let answered = shoot(&page, json!({ "browserId": "tab-1" })).expect("answered");

    assert_eq!(answered["mimeType"], "image/jpeg");
    assert_eq!(
        answered["cssWidth"], 1024.0,
        "what a point would be read against"
    );
    assert_eq!(answered["cssHeight"], 768.0);
    assert_eq!(answered["width"], 1024.0);
    assert_eq!(answered["height"], 768.0);
    assert!(answered.get("clip").is_none(), "no clip was asked for");
    assert_eq!(answered["data"], base64_of(1000));

    let asked = captures(&page);
    assert_eq!(asked.len(), 1, "a picture that fits is taken once");
    assert_eq!(asked[0]["format"], "jpeg");
    assert_eq!(asked[0]["quality"], QUALITIES[0], "the best rung first");
    assert_eq!(asked[0]["captureBeyondViewport"], false);
}

#[test]
fn a_clip_is_the_same_css_pixels_a_point_is_and_the_zoom_is_its_scale() {
    let page = page_answering(base64_of(1000));

    let answered = shoot(
        &page,
        json!({
            "browserId": "tab-1",
            "clip": { "x": 20, "y": 40, "width": 200, "height": 120 },
            "zoom": 2,
        }),
    )
    .expect("answered");

    let clip = captures(&page).remove(0)["clip"].clone();
    assert_eq!(clip["x"], 20.0);
    assert_eq!(clip["y"], 40.0);
    assert_eq!(clip["width"], 200.0);
    assert_eq!(clip["height"], 120.0);
    assert_eq!(
        clip["scale"], 2.0,
        "the zoom is the one number that changes"
    );
    assert_eq!(answered["width"], 400.0, "200 CSS pixels at twice the size");
    assert_eq!(answered["height"], 240.0);
    assert_eq!(
        answered["clip"]["x"], 20.0,
        "and the clip comes back as asked"
    );
    assert_eq!(
        answered["cssWidth"], 1024.0,
        "the viewport is still the viewport"
    );
}

#[test]
fn a_picture_too_big_for_an_answer_is_taken_again_more_compressed() {
    // Every rung of the ladder answers as though it were the whole page.
    let page = page_answering(base64_of(MAX_BYTES + 1));

    let answered = shoot(&page, json!({ "browserId": "tab-1" })).expect("answered");

    let asked = captures(&page);
    assert_eq!(
        asked.len(),
        QUALITIES.len(),
        "nothing at that size fits, so every rung is tried"
    );
    let qualities: Vec<u64> = asked
        .iter()
        .map(|params| params["quality"].as_u64().unwrap())
        .collect();
    assert_eq!(
        qualities,
        QUALITIES.map(u64::from).to_vec(),
        "best first, lowest last"
    );
    assert!(
        answered["data"].as_str().unwrap_or_default().len() / 4 * 3 > MAX_BYTES,
        "and the last one is sent anyway: a page that will not compress is still a page to look at"
    );
}

#[test]
fn a_picture_that_fits_is_not_taken_again() {
    // Half the cap: the first rung answers, and the ladder stops there.
    let page = page_answering(base64_of(MAX_BYTES / 2));

    shoot(&page, json!({ "browserId": "tab-1" })).expect("answered");

    assert_eq!(captures(&page).len(), 1);
}

#[test]
fn a_zoom_outside_one_to_three_is_refused_before_the_page_is_asked() {
    for zoom in [0.5, 0.0, 4.0, -1.0] {
        let page = page_answering(base64_of(1000));

        let error =
            shoot(&page, json!({ "browserId": "tab-1", "zoom": zoom })).expect_err("not a zoom");

        assert!(
            error.message.contains("zoom is a number from 1 to 3"),
            "{zoom}: {}",
            error.message
        );
        assert!(page.calls().is_empty(), "{zoom}: {:?}", page.calls());
    }
}

#[test]
fn a_clip_with_no_area_is_no_clip() {
    let page = page_answering(base64_of(1000));

    let answered = shoot(
        &page,
        json!({ "browserId": "tab-1", "clip": { "x": 20, "y": 40, "width": 0, "height": 120 } }),
    )
    .expect("answered");

    assert!(answered.get("clip").is_none(), "{answered}");
    assert!(
        captures(&page)[0].get("clip").is_none(),
        "and the whole viewport was taken instead of nothing"
    );
}

#[test]
fn a_parked_page_is_put_on_screen_before_it_is_photographed() {
    let page = page_answering(base64_of(1000));

    shoot(&page, json!({ "browserId": "tab-1" })).expect("answered");

    let called = page.calls();
    let methods: Vec<&str> = called.iter().map(|(method, _)| method.as_str()).collect();
    let override_at = methods
        .iter()
        .position(|method| *method == "Emulation.setDeviceMetricsOverride")
        .expect("a parked page is overridden");
    let shot_at = methods
        .iter()
        .position(|method| *method == "Page.captureScreenshot")
        .expect("photographed");
    assert!(override_at < shot_at, "two pixels of page is not a picture");
}

#[test]
fn a_runtime_that_returns_no_picture_is_refused_rather_than_answered_blank() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", viewport())
        .answering("Page.captureScreenshot", json!({}));

    let error = shoot(&page, json!({ "browserId": "tab-1" })).expect_err("no picture");

    assert!(error.message.contains("no picture"), "{}", error.message);
}
