//! `screenshot` on a page that is scrolled: the clip is moved into the document's
//! pixels, what the viewport does not show is cut or refused, and a page that
//! moves while it is photographed is refused.

use serde_json::{json, Value};

use crate::browser::test_support::{ax_fixture, FakePage};

use super::tests::{base64_of, captures, page_answering, shoot};

/// The same window with the page scrolled down it: `pageX` and `pageY` are
/// where the visible band starts in the document, which is the pair a clip
/// written in viewport pixels has to be moved by before a capture.
fn scrolled_viewport(page_y: f64) -> Value {
    json!({
        "layoutViewport": { "pageX": 0, "pageY": page_y, "clientWidth": 1024, "clientHeight": 768 },
        "cssLayoutViewport": { "pageX": 0, "pageY": page_y, "clientWidth": 1024, "clientHeight": 768 },
        "cssContentSize": { "x": 0, "y": 0, "width": 1024, "height": 6000 },
    })
}

/// A capture's clip is in the document's pixels, so the viewport's own band —
/// what the caller asked for — is moved by where the page is scrolled to. A
/// clip left at the viewport's origin while the page is scrolled away from it
/// is a region the viewport is not showing, and the runtime answers that with
/// a blank picture rather than a refusal.
#[test]
fn a_scrolled_page_is_photographed_from_where_it_is_now() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", scrolled_viewport(2400.0))
        .answering("Page.captureScreenshot", json!({ "data": base64_of(1000) }));

    let answered = shoot(&page, json!({ "browserId": "tab-1" })).expect("answered");

    let clip = captures(&page).remove(0)["clip"].clone();
    assert_eq!(clip["x"], 0.0);
    assert_eq!(clip["y"], 2400.0, "the visible band in document pixels");
    assert_eq!(clip["width"], 1024.0);
    assert_eq!(clip["height"], 768.0);
    assert_eq!(
        answered["clip"]["y"], 0.0,
        "while a point read off the picture is still in the viewport's pixels"
    );
}

#[test]
fn a_clip_is_moved_by_the_scroll_it_was_written_against() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", scrolled_viewport(2400.0))
        .answering("Page.captureScreenshot", json!({ "data": base64_of(1000) }));

    let asked =
        json!({ "browserId": "tab-1", "clip": { "x": 20, "y": 40, "width": 200, "height": 120 } });
    let answered = shoot(&page, asked).expect("answered");

    let clip = captures(&page).remove(0)["clip"].clone();
    assert_eq!(clip["x"], 20.0);
    assert_eq!(clip["y"], 2440.0, "the same band, in the document's pixels");
    assert_eq!(
        answered["clip"],
        json!({ "x": 20.0, "y": 40.0, "width": 200.0, "height": 120.0 }),
        "and the answer says the clip the caller wrote"
    );
}

/// A clip of a band the viewport is not showing cannot be photographed: the
/// region is real but nothing of it is painted, and a capture of it is blank.
/// The refusal names the reason instead of answering with that blank.
#[test]
fn a_clip_the_viewport_does_not_show_is_refused_never_answered_blank() {
    let page = page_answering(base64_of(1000));

    let error = shoot(
        &page,
        json!({ "browserId": "tab-1", "clip": { "x": 5000, "y": 40, "width": 200, "height": 120 } }),
    )
    .expect_err("nothing of that clip is on screen");

    assert!(
        error.message.contains("shows none of that clip"),
        "{}",
        error.message
    );
    assert_eq!(
        page.called("Page.captureScreenshot"),
        0,
        "the renderer is never asked for a picture that would come back blank"
    );
}

/// The edge of the viewport is where a picture stops: a clip that reaches past
/// it is taken at the part that is painted, and the answer reports the clip the
/// picture really is, so a point read off it still divides back into the
/// viewport's pixels.
#[test]
fn a_clip_that_reaches_past_the_viewport_is_taken_at_the_part_it_shows() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", scrolled_viewport(2400.0))
        .answering("Page.captureScreenshot", json!({ "data": base64_of(1000) }));

    let answered = shoot(
        &page,
        json!({ "browserId": "tab-1", "clip": { "x": 20, "y": 700, "width": 200, "height": 600 } }),
    )
    .expect("answered");

    let clip = captures(&page).remove(0)["clip"].clone();
    assert_eq!(clip["y"], 3100.0, "in the document's pixels");
    assert_eq!(
        clip["height"], 68.0,
        "the 68 pixels of it the viewport shows"
    );
    assert_eq!(answered["clip"]["y"], 700.0);
    assert_eq!(answered["clip"]["height"], 68.0, "{answered}");
}

/// A page that scrolls while the renderer photographs it has been pictured at a
/// band the answer does not name, so the picture is refused, not sent.
#[test]
fn a_page_that_scrolls_while_it_is_photographed_is_refused_not_answered() {
    let scroll = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(2400));
    let read = scroll.clone();
    let moved = scroll.clone();
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering_with("Page.getLayoutMetrics", move |_| {
            let at = read.load(std::sync::atomic::Ordering::SeqCst);
            scrolled_viewport(f64::from(at))
        })
        .during("Page.captureScreenshot", move || {
            moved.store(2600, std::sync::atomic::Ordering::SeqCst)
        })
        .answering("Page.captureScreenshot", json!({ "data": base64_of(1000) }));

    let error = shoot(&page, json!({ "browserId": "tab-1" })).expect_err("the page moved");

    assert!(
        error.message.contains("scrolled while it was photographed"),
        "{}",
        error.message
    );
}
