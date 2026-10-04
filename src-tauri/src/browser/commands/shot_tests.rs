//! `screenshot`: what the page looks like, and what it costs to send.

use super::*;
use crate::browser::commands::{on_tab, BrowserError, Deadline};
use crate::browser::test_support::{ax_fixture, parked_tab, FakePage};
use serde_json::{json, Value};

/// The viewport the page reports, in the CSS pixels the clip and the answer are
/// both written in. `layoutViewport` is the same window in the device's own
/// pixels, which is how the display's scale is read.
fn viewport() -> Value {
    json!({
        "layoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": 1024, "clientHeight": 768 },
        "cssLayoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": 1024, "clientHeight": 768 },
        "cssContentSize": { "x": 0, "y": 0, "width": 1024, "height": 2400 },
    })
}

/// The same window on a display that scales by one and a half: the device's
/// own viewport is 1.5 times the CSS one, as a 150% display reports it.
fn scaled_viewport() -> Value {
    json!({
        "layoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": 1536, "clientHeight": 1152 },
        "visualViewport": { "offsetX": 0, "offsetY": 0, "pageX": 0, "pageY": 0, "clientWidth": 1536, "clientHeight": 1152, "scale": 1.0, "zoom": 1.0 },
        "cssLayoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": 1024, "clientHeight": 768 },
        "cssVisualViewport": { "offsetX": 0, "offsetY": 0, "pageX": 0, "pageY": 0, "clientWidth": 1024, "clientHeight": 768, "scale": 1.0, "zoom": 1.0 },
    })
}

/// A base64 run of `bytes` bytes, which is what a picture of that size costs.
fn base64_of(bytes: usize) -> String {
    let characters = bytes.div_ceil(3) * 4;
    "A".repeat(characters)
}

/// A JPEG whose header says `width` by `height`. The header is the only part
/// of a picture this host reads, and it is the only part a real answer's size
/// can come from.
fn jpeg(width: u16, height: u16) -> String {
    let mut bytes: Vec<u8> = vec![0xFF, 0xD8];
    // Start of frame: a two-byte length, the sample precision, then the height
    // and the width, big-endian, exactly as an encoder writes them.
    bytes.extend([0xFF, 0xC0, 0x00, 0x11, 0x08]);
    bytes.extend([(height >> 8) as u8]);
    bytes.extend([(height & 0xFF) as u8]);
    bytes.extend([(width >> 8) as u8]);
    bytes.extend([(width & 0xFF) as u8]);
    bytes.extend([0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    base64_bytes(&bytes)
}

fn base64_bytes(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let at = |index: usize| chunk.get(index).copied().unwrap_or(0) as u32;
        let triple = (at(0) << 16) | (at(1) << 8) | at(2);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((triple >> (18 - 6 * index)) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn page_answering(data: String) -> FakePage {
    FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", viewport())
        .answering("Page.captureScreenshot", json!({ "data": data }))
}

/// As [`page_answering`], on a display that scales.
fn page_scaled(data: String) -> FakePage {
    FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", scaled_viewport())
        .answering("Page.captureScreenshot", json!({ "data": data }))
}

/// A picture no amount of compressing makes small: the case that used to be
/// sent anyway and refused by the daemon on the way out.
fn too_big() -> Vec<u8> {
    vec![b'A'; 1 << 21]
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
    assert_eq!(
        answered["clip"],
        json!({ "x": 0.0, "y": 0.0, "width": 1024.0, "height": 768.0 }),
        "no clip was asked for, so the answer says which part it photographed"
    );
    assert_eq!(answered["zoom"], 1.0, "one picture pixel per point");
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

/// The answer's field names are a contract with the daemon, which reads them
/// to write the line that says how big the picture is
/// (`mcp_broker/tools/browser_tools.rs`, `picture`). A rename here makes that
/// line read `x px`, so the names are pinned rather than asserted one at a time.
#[test]
fn the_answer_names_the_fields_the_daemon_reads() {
    let plain = page_answering(base64_of(1000));
    let answered = shoot(&plain, json!({ "browserId": "tab-1" })).expect("answered");
    assert_eq!(
        keys_of(&answered),
        vec![
            "clip",
            "cssHeight",
            "cssWidth",
            "data",
            "height",
            "mimeType",
            "width",
            "zoom"
        ],
        "a viewport picture: the part photographed, its size and the factor"
    );

    let clipped = page_answering(base64_of(1000));
    let answered = shoot(
        &clipped,
        json!({
            "browserId": "tab-1",
            "clip": { "x": 20, "y": 40, "width": 200, "height": 120 },
        }),
    )
    .expect("answered");
    assert_eq!(
        keys_of(&answered),
        vec![
            "clip",
            "cssHeight",
            "cssWidth",
            "data",
            "height",
            "mimeType",
            "width",
            "zoom",
        ],
        "and a clipped picture carries the same set, with its own clip in it"
    );
    assert_eq!(
        keys_of(&answered["clip"]),
        vec!["height", "width", "x", "y"],
        "and the clip is the four numbers the daemon reads"
    );
    // The sizes are `f64` in this module, so they travel as JSON floats; a
    // reader that wants integers has to ask for them as numbers.
    assert!(answered["width"].is_f64(), "{}", answered["width"]);
    assert!(answered["clip"]["x"].is_f64(), "{}", answered["clip"]["x"]);
}

fn keys_of(value: &Value) -> Vec<&str> {
    let mut keys = value
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}

/// A zoom with no clip zooms the viewport: a whole-screen photograph at twice
/// the size, not an unzoomed picture labelled twice.
#[test]
fn a_zoom_without_a_clip_zooms_the_whole_viewport() {
    let page = page_answering(jpeg(2048, 1536));

    let answered = shoot(&page, json!({ "browserId": "tab-1", "zoom": 2 })).expect("answered");

    let asked = captures(&page);
    assert_eq!(asked.len(), 1, "{asked:?}");
    let clip = &asked[0]["clip"];
    assert_eq!(clip["x"], 0.0);
    assert_eq!(clip["y"], 0.0);
    assert_eq!(clip["width"], 1024.0, "the whole viewport is the clip");
    assert_eq!(clip["height"], 768.0);
    assert_eq!(clip["scale"], 2.0, "and the zoom is its scale");
    assert_eq!(answered["width"], 2048.0);
    assert_eq!(answered["height"], 1536.0);
    assert_eq!(answered["zoom"], 2.0, "the factor a point is divided by");
}

/// The picture is in CSS pixels times the zoom, whatever the display's own scale
/// is: the scale sent to the renderer divides it out, so a point read off the
/// picture lands where it was read from.
#[test]
fn the_picture_is_css_pixels_times_the_zoom_on_a_scaled_display() {
    let page = page_scaled(jpeg(1366, 1024));

    shoot(&page, json!({ "browserId": "tab-1", "zoom": 2 })).expect("answered");

    let asked = captures(&page);
    assert_eq!(
        asked[0]["clip"]["scale"],
        2.0 / 1.5,
        "the zoom divided by the display's own scale"
    );
    assert_eq!(
        asked[0]["clip"]["width"], 1024.0,
        "the clip is still CSS pixels"
    );
}

/// The size reported is the picture's own, read out of it, never computed: a
/// renderer that rounds is the only thing that knows how big it made one.
#[test]
fn the_answer_reports_the_pictures_own_size_not_a_computed_one() {
    // Asked for 800 by 600, the renderer answers 801 by 599.
    let page = page_answering(jpeg(801, 599));

    let answered = shoot(
        &page,
        json!({
            "browserId": "tab-1",
            "clip": { "x": 0, "y": 0, "width": 800, "height": 600 },
        }),
    )
    .expect("answered");

    assert_eq!(answered["width"], 801.0, "the header, not the request");
    assert_eq!(answered["height"], 599.0);
    assert_eq!(
        answered["zoom"], 1.00125,
        "the factor is what the picture is"
    );
}

/// The cap is on the encoded answer: base64 and the JSON around it, under what
/// the daemon and the wire allow. A page whose quality ladder bottoms out is
/// taken smaller instead, which is what an agent that asked for a picture can
/// be given.
#[test]
fn a_picture_too_big_for_an_answer_is_taken_smaller_rather_than_sent_over_the_cap() {
    // A renderer that loses bytes as the picture shrinks — as any real one
    // does, since a picture's bytes follow its area. At twice the size the
    // answer is over the cap at every quality, which is the case the quality
    // ladder alone cannot fix.
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", viewport())
        .answering_with("Page.captureScreenshot", |params| {
            let scale = params["clip"]["scale"].as_f64().unwrap_or(1.0);
            let bytes = (400_000.0 * scale * scale) as usize;
            let filler: Vec<u8> = vec![b'A'; bytes.min(1 << 21)];
            json!({ "data": base64_bytes(&filler) })
        });

    let answered = shoot(&page, json!({ "browserId": "tab-1", "zoom": 2 })).expect("answered");

    let frame = serde_json::to_vec(&answered).expect("encoded");
    assert!(
        frame.len() <= MAX_ANSWER_BYTES,
        "the answer is {} bytes and must fit {MAX_ANSWER_BYTES}",
        frame.len()
    );
    let scales = captures(&page)
        .iter()
        .map(|params| params["clip"]["scale"].as_f64().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        scales.windows(2).any(|pair| pair[1] < pair[0]),
        "a later capture is a smaller picture than an earlier one: {scales:?}"
    );
    assert_eq!(
        scales.last().copied(),
        Some(1.0),
        "and it stopped as soon as it fitted"
    );
    assert_eq!(
        answered["zoom"], 1.0,
        "the answer says the factor it really is in"
    );
}

/// Never an over-cap answer: a page that will not compress at any size this
/// tool will send is refused, not sent.
#[test]
fn a_picture_that_cannot_be_made_small_enough_is_refused_never_answered_over_the_cap() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", viewport())
        .answering(
            "Page.captureScreenshot",
            json!({ "data": base64_bytes(&too_big()) }),
        );

    // Asked for at three, so the whole ladder runs at three scales before the
    // floor is reached: the quality rungs and then smaller pictures.
    let error =
        shoot(&page, json!({ "browserId": "tab-1", "zoom": 3 })).expect_err("no answer fits");

    assert!(error.message.contains("too large"), "{}", error.message);
    assert!(
        captures(&page).len() > QUALITIES.len(),
        "the quality ladder and then smaller pictures were tried"
    );
}

#[test]
fn a_picture_too_big_for_an_answer_is_taken_again_more_compressed() {
    // A renderer that loses bytes as the quality drops: the best rungs are over
    // the cap and the fourth is not.
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("Page.getLayoutMetrics", viewport())
        .answering_with("Page.captureScreenshot", |params| {
            let quality = params["quality"].as_u64().unwrap_or(80);
            let filler: Vec<u8> = vec![b'A'; (700_000 - (80 - quality) * 8_000) as usize];
            json!({ "data": base64_bytes(&filler) })
        });

    let answered = shoot(&page, json!({ "browserId": "tab-1" })).expect("answered");

    let qualities: Vec<u64> = captures(&page)
        .iter()
        .map(|params| params["quality"].as_u64().unwrap())
        .collect();
    assert_eq!(
        qualities,
        [80, 65],
        "the ladder stops at the first rung that fits, best first"
    );
    let frame = serde_json::to_vec(&answered).expect("encoded");
    assert!(
        frame.len() <= MAX_ANSWER_BYTES,
        "and what is sent is inside the cap: {} bytes",
        frame.len()
    );
    assert_eq!(
        captures(&page)
            .iter()
            .filter(|params| params["clip"]["scale"] != 1.0)
            .count(),
        0,
        "and nothing was taken smaller: a smaller picture was not what this needed"
    );
}

#[test]
fn a_picture_that_fits_is_not_taken_again() {
    // Half the cap: the first rung answers, and the ladder stops there.
    let page = page_answering(base64_of(MAX_ANSWER_BYTES / 2));

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

    assert_eq!(
        answered["clip"]["width"], 1024.0,
        "a clip with no area is the whole viewport: {answered}"
    );
    assert_eq!(
        captures(&page)[0]["clip"]["width"],
        1024.0,
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
