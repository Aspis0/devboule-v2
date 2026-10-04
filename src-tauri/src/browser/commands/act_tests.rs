use super::*;
use crate::browser::test_support::FakePage;
use devboule_protocol::BrowserErrorCode;

#[test]
fn a_named_key_is_its_own_code_and_characters() {
    let enter = chord("Enter").expect("Enter is a key");
    assert_eq!(enter.virtual_key, 13);
    assert_eq!(enter.text, "\r", "Enter types a newline");
    assert_eq!(enter.modifiers, 0);

    let arrow = chord("arrowdown").expect("case does not matter");
    assert_eq!(arrow.key, "ArrowDown");
    assert!(arrow.text.is_empty(), "an arrow types nothing");

    let escape = chord("Escape").expect("Escape is a key");
    assert!(escape.text.is_empty());
}

#[test]
fn a_chord_is_the_modifiers_and_the_same_key() {
    let select_all = chord("Control+A").expect("a chord is a key");
    assert_eq!(select_all.modifiers, 2, "Control is bit 2");
    assert_eq!(select_all.key, "A", "the key is spelled as it was written");
    assert_eq!(select_all.virtual_key, 65);
    assert!(
        select_all.text.is_empty(),
        "a chord with Control types nothing: the page's shortcut runs instead"
    );

    let both = chord("Control+Shift+S").expect("two modifiers is a chord");
    assert_eq!(both.modifiers, 2 | 8);
    let alt = chord("alt+ArrowUp").expect("alt is a modifier");
    assert_eq!(alt.modifiers, 1);
}

#[test]
fn something_that_is_not_a_key_is_refused_rather_than_silently_pressed() {
    for written in [
        "",
        "Control+",
        "Hyper+a",
        "Control+Hyper+a",
        "F13",
        "Escape+",
    ] {
        let error = chord(written).expect_err(written);
        assert_eq!(error.code, BrowserErrorCode::HostError);
        assert!(
            !error.message.is_empty(),
            "{written} says why it will not press"
        );
    }
    // And an unknown modifier is named as it was written, so the caller can
    // see which half of the chord this app did not know.
    assert!(chord("Hyper+a").unwrap_err().message.contains("Hyper"));
}

#[test]
fn a_key_that_types_is_sent_with_its_characters() {
    let page = FakePage::new();

    tauri::async_runtime::block_on(async {
        press_key(&page, "Enter").await.expect("pressed");
    });

    let down = page
        .first_params("Input.dispatchKeyEvent")
        .expect("a key went down");
    assert_eq!(down["type"], "keyDown", "Enter carries a character itself");
    assert_eq!(down["windowsVirtualKeyCode"], 13);
    assert_eq!(page.called("Input.dispatchKeyEvent"), 2, "down and up");
}

#[test]
fn a_key_that_types_nothing_is_sent_raw() {
    let page = FakePage::new();

    tauri::async_runtime::block_on(async {
        press_key(&page, "Tab").await.expect("pressed");
    });

    let down = page
        .first_params("Input.dispatchKeyEvent")
        .expect("a key went down");
    assert_eq!(down["type"], "rawKeyDown");
    assert!(down.get("text").is_none(), "Tab types no character");
}

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
