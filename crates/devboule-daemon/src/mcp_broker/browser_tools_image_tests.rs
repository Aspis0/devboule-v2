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

/// A host's screenshot, in the contract's shape.
fn shot(clip: Option<Value>) -> Value {
    json!({
        "mimeType": "image/jpeg",
        "data": "QUJDRA",
        "width": 1280,
        "height": 720,
        "cssWidth": 1280,
        "cssHeight": 720,
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
        shot(Some(json!({"x": 0, "y": 0, "width": 800, "height": 600}))),
    );
    assert_eq!(request.command, "screenshot");
    let content = body["result"]["content"]
        .as_array()
        .expect("content blocks")
        .clone();
    assert_eq!(content.len(), 2, "{body}");
    assert_eq!(content[0]["type"], "image", "{body}");
    assert_eq!(content[0]["mimeType"], "image/jpeg", "{body}");
    assert_eq!(content[0]["data"], "QUJDRA", "{body}");
    let line = content[1]["text"].as_str().expect("text line").to_string();
    assert_eq!(content[1]["type"], "text", "{body}");
    assert!(line.contains("1280x720"), "{line}");
    assert!(line.contains("1280x720 css"), "{line}");
    assert!(line.contains("clip 0,0,800,600"), "{line}");
    assert!(line.len() < 200, "the line is short: {line}");
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
    let line = body["result"]["content"][1]["text"]
        .as_str()
        .expect("text line");
    assert!(line.contains("1280x720"), "{line}");
    assert!(!line.contains("clip"), "no clip was asked for: {line}");
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
    assert_eq!(body["result"]["content"][0]["type"], "text", "{body}");
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
            json!({"browserId": "t", "zoom": 1.5}),
            "1 to 3",
            "a zoom that is not a whole number",
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
        "clip": {"x": 10, "y": 20, "width": 300, "height": 400},
        "zoom": 3,
    });
    let (body, request) = panel.call(&host, "browser_screenshot", arguments.clone(), shot(None));
    assert_eq!(request.args, arguments, "{body}");
    assert_eq!(body["result"]["isError"], json!(false), "{body}");
}
