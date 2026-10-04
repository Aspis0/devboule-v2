//! The payloads here are the protocol's own shapes for `Page.getFrameTree`,
//! `Page.frameNavigated` and `Page.navigatedWithinDocument` (a pushState on a
//! single-page app's top frame, and one in a child frame), not shapes invented
//! for the test.

use super::*;
use serde_json::json;

const TOP: &str = "B5D4E2F0C6A64E3B9E0A7C0B2D2A7F11";
const CHILD: &str = "6A1E9C27F3B04D58A9D2E41C7B3F8A02";

fn frame_tree() -> Value {
    json!({ "frameTree": { "frame": {
        "id": TOP,
        "loaderId": "9A3B7F5C1E2D4A68B0C9D8E7F6A5B4C3",
        "url": "about:blank",
        "domainAndRegistry": "",
        "securityOrigin": "://",
        "mimeType": "text/html",
        "adFrameStatus": { "adFrameType": "none" },
        "secureContextType": "InsecureScheme",
        "crossOriginIsolatedContextType": "NotIsolated",
        "gatedAPIFeatures": []
    }}})
}

fn moved(frame: &str, url: &str, kind: &str) -> String {
    json!({ "frameId": frame, "url": url, "navigationType": kind }).to_string()
}

fn committed(id: &str, parent: Option<&str>, url: &str) -> String {
    let mut frame = json!({
        "id": id,
        "loaderId": "1F2E3D4C5B6A79880123456789ABCDEF",
        "url": url,
        "domainAndRegistry": "react.dev",
        "securityOrigin": "https://react.dev",
        "mimeType": "text/html",
        "secureContextType": "Secure",
        "crossOriginIsolatedContextType": "NotIsolated",
        "gatedAPIFeatures": []
    });
    if let Some(parent) = parent {
        frame["parentId"] = json!(parent);
    }
    json!({ "frame": frame, "type": "Navigation" }).to_string()
}

fn known() -> Frames {
    let frames = Frames::default();
    frames.learn_from_tree(&frame_tree());
    frames
}

#[test]
fn a_push_state_in_the_top_frame_is_the_tabs_new_address() {
    let frames = known();

    assert_eq!(
        frames.observe(
            "Page.navigatedWithinDocument",
            &moved(TOP, "https://react.dev/learn", "historyApi")
        ),
        Observed::SameDocument("https://react.dev/learn".to_owned())
    );
}

#[test]
fn a_hash_change_and_a_replace_state_are_the_same_thing() {
    let frames = known();

    for (url, kind) in [
        ("https://react.dev/learn#installation", "fragment"),
        ("https://react.dev/learn?tab=2", "historyApi"),
        ("https://react.dev/reference", "other"),
    ] {
        assert_eq!(
            frames.observe("Page.navigatedWithinDocument", &moved(TOP, url, kind)),
            Observed::SameDocument(url.to_owned()),
            "{kind}"
        );
    }
}

#[test]
fn a_push_state_in_a_child_frame_is_not_the_tabs_address() {
    let frames = known();

    assert_eq!(
        frames.observe(
            "Page.navigatedWithinDocument",
            &moved(CHILD, "https://ads.example/slot/2", "historyApi")
        ),
        Observed::Nothing
    );
}

#[test]
fn until_the_top_frame_is_known_no_same_document_move_is_believed() {
    let frames = Frames::default();

    assert_eq!(
        frames.observe(
            "Page.navigatedWithinDocument",
            &moved(TOP, "https://react.dev/learn", "historyApi")
        ),
        Observed::Nothing
    );
}

#[test]
fn a_new_document_in_the_top_frame_is_one_and_teaches_which_frame_is_the_top() {
    let frames = Frames::default();

    assert_eq!(
        frames.observe(
            "Page.frameNavigated",
            &committed(TOP, None, "https://react.dev/")
        ),
        Observed::NewDocument
    );
    assert_eq!(
        frames.observe(
            "Page.navigatedWithinDocument",
            &moved(TOP, "https://react.dev/learn", "historyApi")
        ),
        Observed::SameDocument("https://react.dev/learn".to_owned()),
        "the commit named the top frame, so its later moves count"
    );
}

#[test]
fn a_new_document_in_a_child_frame_is_not_the_tabs() {
    let frames = known();

    assert_eq!(
        frames.observe(
            "Page.frameNavigated",
            &committed(CHILD, Some(TOP), "https://ads.example/slot")
        ),
        Observed::Nothing
    );
}

#[test]
fn what_is_not_one_of_the_two_events_or_not_json_is_nothing() {
    let frames = known();

    assert_eq!(
        frames.observe("Page.loadEventFired", r#"{"timestamp":1234.5}"#),
        Observed::Nothing
    );
    assert_eq!(
        frames.observe("Page.navigatedWithinDocument", "not json"),
        Observed::Nothing
    );
    assert_eq!(
        frames.observe("Page.navigatedWithinDocument", r#"{"frameId":"x"}"#),
        Observed::Nothing,
        "no address, no move"
    );
    assert_eq!(
        frames.observe("Page.navigatedWithinDocument", &moved(TOP, "", "other")),
        Observed::Nothing
    );
}
