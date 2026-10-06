//! What a screenshot answers: the image itself as an MCP image block, and the
//! one short line beside it that says how big it is.
//!
//! Every other browser command answers text, so what is proved here is both
//! halves — the bytes travel once, and a result that carries no image is still
//! read as text.

use serde_json::{json, Value};

use super::browser_tools_harness::{panel, FakeHost};
use super::tools::browser_args::parse;
use super::tools::browser_commands::spec_for;

/// A host's screenshot, field for field as `browser/commands/shot.rs` writes
/// it. The sizes are JSON **floats**: the host computes them as `f64` and
/// `serde_json` writes `1280.0`, so a line that reads them as integers finds
/// nothing and says `x px`.
fn shot(clip: Option<Value>) -> Value {
    json!({
        "mimeType": "image/jpeg",
        "data": "QUJDRA",
        "width": 1280.0,
        "height": 720.0,
        "cssWidth": 1024.0,
        "cssHeight": 768.0,
        "zoom": 1.25,
        "clip": clip,
    })
}

/// The sentence a refused screenshot answers, or a panic naming the call.
fn refusal(arguments: Value) -> String {
    match parse(
        spec_for("browser_screenshot").expect("browser_screenshot is served"),
        "browser_screenshot",
        &arguments,
    ) {
        Ok(_) => panic!("browser_screenshot: {arguments} must be refused"),
        Err(sentence) => sentence,
    }
}

#[test]
fn a_screenshot_answers_an_image_block_and_one_short_line() {
    let panel = panel("shot");
    let host = FakeHost::register(&panel.state, 5);
    let (body, request) = panel.call(
        &host,
        "browser_screenshot",
        json!({"browserId": "tab-1", "clip": {"x": 0, "y": 0, "width": 800, "height": 600}, "zoom": 2}),
        shot(Some(json!({"x": 0.0, "y": 0.0, "width": 800.0, "height": 600.0}))),
    );
    assert_eq!(request.command, "screenshot");
    let content = body["result"]["content"]
        .as_array()
        .expect("content blocks")
        .clone();
    assert_eq!(
        content.len(),
        4,
        "the daemon's head, the picture, its line, the daemon's tail: {body}"
    );
    assert_eq!(content[1]["type"], "image", "{body}");
    assert_eq!(content[1]["mimeType"], "image/jpeg", "{body}");
    assert_eq!(content[1]["data"], "QUJDRA", "{body}");
    let line = content[2]["text"].as_str().expect("text line").to_string();
    assert_eq!(content[2]["type"], "text", "{body}");
    assert_eq!(
        line, "image/jpeg 1280x720 px, viewport 1024x768 css px, 1.25 per point, clip 0,0,800,600",
        "every number the host sent is read, as the host sent it"
    );
    assert!(
        !line.contains("QUJDRA"),
        "the bytes are the image block, never the text: {line}"
    );
}

#[test]
fn the_bytes_travel_once_and_no_document_rides_beside_the_block() {
    let panel = panel("shot-bytes");
    let host = FakeHost::register(&panel.state, 5);
    let (body, _) = panel.call(
        &host,
        "browser_screenshot",
        json!({"browserId": "tab-1"}),
        shot(None),
    );
    assert!(
        body["result"].get("structuredContent").is_none(),
        "a document here would carry the same base64 twice: {body}"
    );
    let serialized = body.to_string();
    assert_eq!(
        serialized.matches("QUJDRA").count(),
        1,
        "the image is in the reply once: {serialized}"
    );
    let line = body["result"]["content"][2]["text"]
        .as_str()
        .expect("text line");
    assert_eq!(
        line, "image/jpeg 1280x720 px, viewport 1024x768 css px, 1.25 per point",
        "no clip was asked for, so none is named"
    );
}

#[test]
fn a_result_that_carries_no_image_is_still_read_as_text() {
    let panel = panel("shot-host");
    let host = FakeHost::register(&panel.state, 5);
    let result = json!({"mimeType": "image/jpeg", "width": 1280});
    let (body, _) = panel.call(
        &host,
        "browser_screenshot",
        json!({"browserId": "tab-1"}),
        result.clone(),
    );
    assert!(
        body["result"]["content"]
            .as_array()
            .expect("content blocks")
            .iter()
            .all(|block| block["type"] == "text"),
        "{body}"
    );
    assert_eq!(body["result"]["structuredContent"], result, "{body}");
}

/// The bounds the contract puts on the two ways to ask for a smaller picture.
#[test]
fn a_zoom_and_a_clip_outside_the_contract_are_refused_before_the_host_sees_it() {
    let cases = [
        (
            json!({"browserId": "t", "zoom": 0}),
            "1 to 3",
            "a zoom below one",
        ),
        (
            json!({"browserId": "t", "zoom": 4}),
            "1 to 3",
            "a zoom above three",
        ),
        (
            json!({"browserId": "t", "zoom": "2"}),
            "must be a number",
            "a zoom written as text",
        ),
        (
            json!({"browserId": "t", "clip": {"x": -1, "y": 0, "width": 10, "height": 10}}),
            "non-negative",
            "a clip before the viewport's own corner",
        ),
        (
            json!({"browserId": "t", "clip": {"x": 0, "y": 0, "width": 10}}),
            "needs 'height'",
            "a clip with three numbers",
        ),
        (
            json!({"browserId": "t", "clip": "0,0,10,10"}),
            "must be an object with x, y, width, height",
            "a clip that is not an object",
        ),
        (
            json!({"browserId": "t", "clip": {"x": 0, "y": 0, "width": 10, "height": 10, "z": 1}}),
            "has no 'z'",
            "a clip with a fifth number",
        ),
    ];
    for (arguments, expected, what) in cases {
        let refused = refusal(arguments.clone());
        assert!(refused.contains(expected), "{what}: {refused}");
    }
}

#[test]
fn a_clip_and_a_zoom_reach_the_host_as_written() {
    let panel = panel("shot-args");
    let host = FakeHost::register(&panel.state, 5);
    let arguments = json!({
        "browserId": "tab-1",
        "clip": {"x": 10.5, "y": 20.25, "width": 300.5, "height": 400},
        "zoom": 1.5,
    });
    let (body, request) = panel.call(&host, "browser_screenshot", arguments.clone(), shot(None));
    assert_eq!(request.args, arguments, "{body}");
    assert_eq!(body["result"]["isError"], json!(false), "{body}");
}

/// Geometry is a place on a scaled display, and the host reads it as a real
/// number: half a pixel is routine at 125% or 150%, and refusing it here would
/// make the daemon stricter than the page it drives.
#[test]
fn geometry_takes_the_fractional_numbers_the_host_takes() {
    let cases = [
        (
            "browser_click_at",
            json!({"browserId": "t", "x": 12.5, "y": 40.25}),
            "a point between two pixels",
        ),
        (
            "browser_screenshot",
            json!({"browserId": "t", "clip": {"x": 0.5, "y": 0, "width": 10.5, "height": 10}, "zoom": 1.5}),
            "half a clip edge and a fractional zoom",
        ),
        (
            "browser_scroll",
            json!({"browserId": "t", "direction": "down", "amount": 2.5}),
            "a third of a notch",
        ),
    ];
    for (tool, arguments, what) in cases {
        let spec = super::tools::browser_commands::spec_for(tool).expect(tool);
        assert!(
            parse(spec, tool, &arguments).is_ok(),
            "{tool}: {what}: {arguments}"
        );
    }
    let schema = |tool: &str| super::tools::browser_commands::schema_for(tool).expect(tool);
    assert_eq!(
        schema("browser_click_at")["properties"]["x"]["type"],
        json!("number")
    );
    assert_eq!(
        schema("browser_click_at")["properties"]["y"]["type"],
        json!("number")
    );
    assert_eq!(
        schema("browser_screenshot")["properties"]["zoom"]["type"],
        json!("number")
    );
    assert_eq!(
        schema("browser_screenshot")["properties"]["clip"]["properties"]["x"]["type"],
        json!("number"),
        "a clip edge is a place on a scaled display too"
    );
    assert_eq!(
        schema("browser_scroll")["properties"]["amount"]["type"],
        json!("number")
    );
    // A count is not a place: it stays a whole number.
    assert_eq!(
        schema("browser_click_at")["properties"]["clickCount"]["type"],
        json!("integer")
    );
}
