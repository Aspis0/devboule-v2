//! Which frame a node is in, over trees the fill's own fixtures do not have:
//! frames inside frames, and nodes inside shadow roots.

use serde_json::{json, Value};

use super::*;
use crate::browser::test_support::FakePage;

const MIDDLE_OWNER: u64 = 900;
const INNER_OWNER: u64 = 910;
/// A node in a shadow root, in a document two frames down.
const DEEP: u64 = 79;

/// `main` holds `middle`, which holds `inner`.
fn nested() -> Value {
    json!({"frameTree": {
        "frame": {"id": "main", "url": "https://shop.example.test/"},
        "childFrames": [{
            "frame": {"id": "middle", "url": "https://ads.example.test/"},
            "childFrames": [{"frame": {"id": "inner", "url": "https://deep.example.test/"}}],
        }],
    }})
}

fn inner_document() -> Value {
    json!({"backendNodeId": 76, "children": [{"backendNodeId": 77,
        "shadowRoots": [{"backendNodeId": 78, "children": [{"backendNodeId": DEEP}]}]}]})
}

fn page() -> FakePage {
    FakePage::new()
        .answering_with("DOM.getFrameOwner", |asked| {
            let owner = if asked["frameId"] == json!("middle") {
                MIDDLE_OWNER
            } else {
                INNER_OWNER
            };
            json!({ "backendNodeId": owner })
        })
        .answering_with("DOM.describeNode", |asked| {
            let inner = json!({"backendNodeId": INNER_OWNER, "contentDocument": inner_document()});
            match asked["backendNodeId"].as_u64() {
                Some(MIDDLE_OWNER) => json!({"node": {"backendNodeId": MIDDLE_OWNER,
                    "contentDocument": {"backendNodeId": 901, "children": [inner]}}}),
                Some(INNER_OWNER) => json!({"node": inner}),
                _ => json!({"node": {"backendNodeId": asked["backendNodeId"]}}),
            }
        })
}

fn frame_of_node(node: u64) -> String {
    tauri::async_runtime::block_on(frame_of(
        &page(),
        node,
        &frames_of(&nested()).expect("the tree is readable"),
    ))
    .expect("the node is in a frame")
}

#[test]
fn a_node_in_a_shadow_root_two_frames_down_is_in_the_innermost_frame() {
    assert_eq!(frame_of_node(DEEP), "inner");
}

#[test]
fn a_node_no_child_frame_holds_is_in_the_pages_own_frame() {
    assert_eq!(frame_of_node(5), "main");
}

#[test]
fn a_frame_whose_owner_cannot_be_named_refuses_instead_of_guessing() {
    let page = FakePage::new()
        .answering_with("DOM.describeNode", |_| json!({"node": {}}))
        .answering_with("DOM.getFrameOwner", |_| json!({}));

    let refused = tauri::async_runtime::block_on(frame_of(
        &page,
        5,
        &frames_of(&nested()).expect("the tree is readable"),
    ))
    .expect_err("the field might be inside the frame that has no owner");

    assert!(
        refused.message.contains("frame this app can name"),
        "{}",
        refused.message
    );
}

#[test]
fn an_address_no_site_can_be_compared_on_is_refused() {
    let every = vec![("main".to_owned(), "about:blank".to_owned())];

    let refused = origin_in(&every, "main").expect_err("about:blank has no origin");

    assert!(
        refused.message.contains("cannot be compared"),
        "{}",
        refused.message
    );
}

#[test]
fn a_frame_named_without_an_id_or_an_address_makes_the_tree_unreadable() {
    for frame in [
        json!({"id": "child"}),
        json!({"url": "https://ads.example.test/"}),
    ] {
        let tree = json!({"frameTree": {
            "frame": {"id": "main", "url": "https://shop.example.test/"},
            "childFrames": [{"frame": frame}],
        }});

        let refused = frames_of(&tree).expect_err("a frame that cannot be placed");

        assert!(
            refused.message.contains("without an id or an address"),
            "{}",
            refused.message
        );
    }
}

#[test]
fn an_answer_with_no_frame_tree_is_unreadable() {
    frames_of(&json!({})).expect_err("no frame of the page's own");
}
