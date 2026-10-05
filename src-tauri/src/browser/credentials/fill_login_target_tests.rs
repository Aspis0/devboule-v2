//! What a fill refuses to type into: a ref that is not a field of the kind the
//! call named, a field the page takes the focus from, and a frame tree this
//! app cannot read to the end. Each refuses before anything is typed, and the
//! first kind before the store is read at all.

use serde_json::{json, Value};

use super::fixtures::{
    asking, page_that_takes_the_focus_when_a_field_is_emptied, sign_in_page, tab_named, typed,
    with_node, Over, PASSWORD, SITE, USERNAME,
};
use super::*;
use crate::browser::cdp::CdpError;
use crate::browser::test_support::FakePage;

/// Run a fill of `args` over `page` and hand back the refusal, with the
/// guarantees every refusal here shares: nothing was typed, and nothing is
/// left in the scrub.
fn refused(page: &FakePage, over: &Over, entry: &str, args: Value) -> BrowserError {
    let tab = tab_named("fill-refused-target");
    let mut args = asking(args);
    args["entryId"] = json!(entry);
    let error = tauri::async_runtime::block_on(fill(&over.vault, &tab, page, &args))
        .expect_err("this fill is refused");
    assert!(typed(page).is_empty(), "something was typed");
    assert_eq!(scrub::typed_of(&tab.browser_id), 0);
    error
}

/// A refusal before any password is read, and one that names which argument
/// was wrong.
fn refused_before_the_store(page: &FakePage, args: Value, argument: &str, mentioning: &str) {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let error = refused(page, &over, &saved, args);
    assert!(over.store.readers().is_empty(), "the store was read");
    assert!(error.message.contains(argument), "{}", error.message);
    assert!(error.message.contains(mentioning), "{}", error.message);
}

fn input(attributes: Value) -> Value {
    json!({"nodeName": "INPUT", "attributes": attributes})
}

#[test]
fn a_password_ref_that_is_not_a_password_input_is_refused() {
    let page = with_node(14, input(json!(["type", "text"])));

    refused_before_the_store(
        &page,
        json!({"passwordRef": PASSWORD}),
        "passwordRef",
        "`text`",
    );
}

#[test]
fn a_username_ref_that_is_a_password_input_is_refused() {
    let page = with_node(13, input(json!(["type", "password"])));

    refused_before_the_store(
        &page,
        json!({"usernameRef": USERNAME}),
        "usernameRef",
        "`password`",
    );
}

#[test]
fn a_ref_that_is_not_an_input_is_refused() {
    let page = with_node(14, json!({"nodeName": "DIV"}));

    refused_before_the_store(
        &page,
        json!({"passwordRef": PASSWORD}),
        "passwordRef",
        "not an input",
    );
}

#[test]
fn a_disabled_input_is_refused() {
    let page = with_node(14, input(json!(["type", "password", "disabled", ""])));

    refused_before_the_store(
        &page,
        json!({"passwordRef": PASSWORD}),
        "passwordRef",
        "disabled",
    );
}

#[test]
fn a_read_only_input_is_refused() {
    let page = with_node(14, input(json!(["type", "password", "readonly", ""])));

    refused_before_the_store(
        &page,
        json!({"passwordRef": PASSWORD}),
        "passwordRef",
        "read-only",
    );
}

#[test]
fn a_hidden_input_is_refused() {
    let page = with_node(13, input(json!(["type", "hidden"])));

    refused_before_the_store(
        &page,
        json!({"usernameRef": USERNAME}),
        "usernameRef",
        "`hidden`",
    );
}

#[test]
fn an_input_with_no_box_on_the_page_is_refused() {
    let page = sign_in_page().refusing(
        "DOM.getBoxModel",
        CdpError::Refused("Could not compute box model.".to_owned()),
    );

    refused_before_the_store(
        &page,
        json!({"passwordRef": PASSWORD}),
        "passwordRef",
        "not shown",
    );
}

#[test]
fn an_input_with_an_empty_box_is_refused() {
    let page = sign_in_page().answering_with(
        "DOM.getBoxModel",
        |_| json!({"model": {"width": 0, "height": 24}}),
    );

    refused_before_the_store(
        &page,
        json!({"passwordRef": PASSWORD}),
        "passwordRef",
        "not shown",
    );
}

/// A username field with no `type` is a text field, and so is one the page
/// spells in capitals.
#[test]
fn a_username_field_with_no_type_or_a_capitalised_one_is_typed_into() {
    for attributes in [json!([]), json!(["type", "EMAIL"]), json!(["type", "tel"])] {
        let over = Over::new();
        let saved = over.save("Shop account", SITE);
        let page = with_node(13, input(attributes));
        let tab = tab_named("fill-text-like");

        tauri::async_runtime::block_on(fill(
            &over.vault,
            &tab,
            &page,
            &asking(json!({"entryId": saved, "usernameRef": USERNAME})),
        ))
        .expect("a text-like field takes a username");

        assert_eq!(typed(&page).len(), 1);
    }
}

/// The page's own `input` handler can move the focus while a field is emptied,
/// and the value would then go to whatever holds it.
#[test]
fn a_page_that_takes_the_focus_when_the_field_is_emptied_gets_nothing() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);

    let error = refused(
        &page_that_takes_the_focus_when_a_field_is_emptied(),
        &over,
        &saved,
        json!({"passwordRef": PASSWORD}),
    );

    assert!(error.message.contains("focus"), "{}", error.message);
}

/// The clear step answers whether it could empty the field.
#[test]
fn a_field_that_could_not_be_emptied_is_refused() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let page = sign_in_page().answering_with(
        "Runtime.callFunctionOn",
        |_| json!({"result": {"value": false}}),
    );

    let error = refused(&page, &over, &saved, json!({"passwordRef": PASSWORD}));

    assert!(error.message.contains("emptied"), "{}", error.message);
}

/// A frame the tree names without an address could hold the field, and is not
/// quietly left out.
#[test]
fn a_frame_tree_with_an_unreadable_frame_refuses_instead_of_assuming_the_page_own() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let page = sign_in_page().answering_with("Page.getFrameTree", |_| {
        json!({"frameTree": {
            "frame": {"id": "main", "url": format!("{SITE}/sign-in")},
            "childFrames": [{"frame": {"id": "child"}}],
        }})
    });

    let error = refused(&page, &over, &saved, json!({"passwordRef": PASSWORD}));

    assert!(
        error.message.contains("without an id or an address"),
        "{}",
        error.message
    );
    assert!(over.store.readers().is_empty());
}
