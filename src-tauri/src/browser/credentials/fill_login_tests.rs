//! The two host commands a saved login answers, driven through a page that
//! answers from a table: what may be used where, and what typing one actually
//! types.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::browser::cdp::CdpError;
use crate::browser::credentials::secrets::fake::{Act, InMemory};
use crate::browser::test_support::{registry_with, FakePage};

/// A value that is obviously a test's.
const SECRET: &str = "SENTINEL-PW-7f3a";
const SITE: &str = "https://shop.example.test";
const OTHER: &str = "https://ads.example.test";
const USER: &str = "person@example.test";

/// One username field and one password field in the page's own frame, and a
/// third field in a child frame of another site.
const USERNAME: &str = "e13";
const PASSWORD: &str = "e14";
const FOREIGN: &str = "e21";

struct Over {
    _dir: tempfile::TempDir,
    vault: Vault,
    store: Arc<InMemory>,
}

impl Over {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(InMemory::empty());
        let vault = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)));
        Over {
            _dir: dir,
            vault,
            store,
        }
    }

    /// One saved login for `origin`, and the id its password is filed under.
    fn save(&self, label: &str, origin: &str) -> String {
        self.vault
            .create(label, &[origin.to_owned()], USER, SECRET)
            .expect("the login was saved")
            .id
    }
}

/// The tab the commands run on: parked, owned, at the fixture's address.
fn tab() -> TabInfo {
    registry_with("fill-login-tab")
        .tab_of("fill-login-tab")
        .expect("the fixture tab is claimed")
}

/// The `<iframe>` element that owns the child frame, whose document holds
/// `FOREIGN` (node 21).
const CHILD_OWNER: u64 = 900;

/// What the browser says of a node: an `<iframe>` carries the document it
/// owns, and nothing names the frame a plain input is in.
fn described(asked: &Value) -> Value {
    if asked["backendNodeId"] == json!(CHILD_OWNER) {
        return json!({"node": {"backendNodeId": CHILD_OWNER, "nodeName": "IFRAME",
            "contentDocument": {"backendNodeId": 901, "children": [
                {"backendNodeId": 21, "nodeName": "INPUT"}]}}});
    }
    json!({"node": {"backendNodeId": asked["backendNodeId"], "nodeName": "INPUT"}})
}

/// A page whose own frame is `SITE`, with one child frame of `OTHER`, and the
/// three fields above in one frame or the other.
fn sign_in_page() -> FakePage {
    FakePage::new()
        .answering_with("Page.getFrameTree", |_| {
            json!({"frameTree": {
                "frame": {"id": "main", "url": format!("{SITE}/sign-in")},
                "childFrames": [{"frame": {"id": "child", "url": format!("{OTHER}/widget")}}],
            }})
        })
        .answering_with(
            "DOM.getFrameOwner",
            |_| json!({"backendNodeId": CHILD_OWNER}),
        )
        .answering_with("DOM.describeNode", described)
        .answering_with(
            "DOM.resolveNode",
            |asked| json!({"object": {"objectId": format!("node-{}", asked["backendNodeId"])}}),
        )
        .answering_with(
            "Runtime.evaluate",
            |_| json!({ "result": { "type": "string", "value": "complete" } }),
        )
}

/// What the page was asked to type, in the order it was asked.
fn typed(page: &FakePage) -> Vec<Value> {
    page.calls()
        .into_iter()
        .filter(|(method, _)| method == "Input.insertText")
        .map(|(_, params)| params["text"].clone())
        .collect()
}

fn asking(args: Value) -> Value {
    let mut args = args;
    args["browserId"] = json!("fill-login-tab");
    args
}

#[test]
fn the_preview_answers_the_origin_and_the_labels_and_nothing_else() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);

    let answer = tauri::async_runtime::block_on(preview(
        &over.vault,
        &sign_in_page(),
        &asking(json!({"passwordRef": PASSWORD})),
    ))
    .expect("the preview answered");

    assert_eq!(answer["origin"], json!(SITE));
    assert_eq!(
        answer["entries"],
        json!([{ "id": saved, "label": "Shop account" }])
    );
    let words = answer.to_string();
    assert!(
        !words.contains(SECRET),
        "no password in the preview: {words}"
    );
    assert!(!words.contains(USER), "not the username either: {words}");
}

/// The origin the gate is checked against is the field's own frame's, which is
/// not the tab's address whenever the field is in a child frame.
#[test]
fn the_preview_names_the_origin_of_the_frame_the_field_lives_in() {
    let over = Over::new();
    over.save("Ad account", OTHER);

    let answer = tauri::async_runtime::block_on(preview(
        &over.vault,
        &sign_in_page(),
        &asking(json!({"passwordRef": FOREIGN})),
    ))
    .expect("a field of a child frame is a field of this page");

    assert_eq!(answer["origin"], json!(OTHER));
}

#[test]
fn a_site_with_no_saved_login_says_so() {
    let over = Over::new();
    over.save("Ad account", OTHER);

    let refused = tauri::async_runtime::block_on(preview(
        &over.vault,
        &sign_in_page(),
        &asking(json!({"passwordRef": PASSWORD})),
    ))
    .expect_err("this site has no saved login");

    assert!(
        refused.message.starts_with("no_saved_login:"),
        "{}",
        refused.message
    );
    assert!(refused.message.contains(SITE), "{}", refused.message);
}

/// One call is one site: two fields of two sites have no single origin for the
/// card to name, and so are two calls.
#[test]
fn two_fields_of_two_sites_are_refused_together() {
    let over = Over::new();
    over.save("Shop account", SITE);
    over.save("Ad account", OTHER);

    let refused = tauri::async_runtime::block_on(preview(
        &over.vault,
        &sign_in_page(),
        &asking(json!({"usernameRef": USERNAME, "passwordRef": FOREIGN})),
    ))
    .expect_err("one call names one site");

    assert!(
        refused.message.contains(SITE) && refused.message.contains(OTHER),
        "the refusal names both: {}",
        refused.message
    );
}

/// The preview and the fill are two looks at the page, and the second one is
/// what stands: a tab that moved to another site between the card and the
/// typing is a different site, and the entry is not typed there.
#[test]
fn a_page_that_moved_between_the_preview_and_the_fill_is_refused() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    tauri::async_runtime::block_on(preview(
        &over.vault,
        &sign_in_page(),
        &asking(json!({"passwordRef": PASSWORD})),
    ))
    .expect("the preview answered for the site that was there");

    // The same field id now lives in a frame of another site.
    let moved = FakePage::new()
        .answering_with("Page.getFrameTree", |_| {
            json!({ "frameTree": { "frame": { "id": "main", "url": format!("{OTHER}/sign-in") } } })
        })
        .answering_with("DOM.describeNode", described)
        .answering_with("DOM.resolveNode", |asked| {
            json!({"object": {"objectId": format!("node-{}", asked["backendNodeId"])}})
        })
        .answering_with("Runtime.evaluate", |_| {
            json!({ "result": { "type": "string", "value": "complete" } })
        });

    let refused = tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab(),
        &moved,
        &asking(json!({"entryId": saved, "passwordRef": PASSWORD})),
    ))
    .expect_err("the page is on another site now");

    assert!(
        refused.message.starts_with("origin_mismatch:"),
        "the gate judged the page the fill saw: {}",
        refused.message
    );
    assert_eq!(moved.called("Input.insertText"), 0);
}

#[test]
fn a_page_still_loading_is_refused_before_it_is_asked_anything() {
    let over = Over::new();
    over.save("Shop account", SITE);
    let page = sign_in_page().answering_with(
        "Runtime.evaluate",
        |_| json!({ "result": { "type": "string", "value": "loading" } }),
    );

    let refused = tauri::async_runtime::block_on(preview(
        &over.vault,
        &page,
        &asking(json!({"passwordRef": PASSWORD})),
    ))
    .expect_err("a loading page is not a settled one");

    assert!(
        refused.message.contains("still loading"),
        "{}",
        refused.message
    );
    assert_eq!(page.called("DOM.describeNode"), 0);
}

#[test]
fn the_fill_types_the_username_and_the_password_and_answers_only_that() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let page = sign_in_page();

    let answer = tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab(),
        &page,
        &asking(json!({"entryId": saved, "usernameRef": USERNAME, "passwordRef": PASSWORD})),
    ))
    .expect("the login was typed");

    assert_eq!(
        answer,
        json!({"filled": ["usernameRef", "passwordRef"]}),
        "a constant answer: no view, no delta, nothing read back"
    );
    assert_eq!(typed(&page), vec![json!(USER), json!(SECRET)]);
    scrub::forget("fill-login-tab");
}

#[test]
fn a_username_only_call_types_the_username_and_no_password() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let page = sign_in_page();

    let answer = tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab(),
        &page,
        &asking(json!({"entryId": saved, "usernameRef": USERNAME})),
    ))
    .expect("the first page of a two-step login was typed");

    assert_eq!(answer, json!({"filled": ["usernameRef"]}));
    assert_eq!(typed(&page), vec![json!(USER)], "the store was not read");
}

#[test]
fn a_ref_that_names_no_node_is_a_stale_ref() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let page = sign_in_page().refusing("DOM.describeNode", CdpError::StaleRef);

    let refused = tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab(),
        &page,
        &asking(json!({"entryId": saved, "passwordRef": PASSWORD})),
    ))
    .expect_err("the field is gone");

    assert!(
        refused.message.starts_with("stale_ref:"),
        "{}",
        refused.message
    );
    assert_eq!(page.called("Input.insertText"), 0);
}

/// The gate is read from the vault's own list of what a site may use, so an
/// entry this site does not allow is refused without the store being read.
#[test]
fn an_entry_the_site_does_not_allow_is_refused_without_reading_the_store() {
    let over = Over::new();
    let elsewhere = over.save("Ad account", OTHER);
    over.store.refuse(Act::Get);
    let page = sign_in_page();

    let refused = tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab(),
        &page,
        &asking(json!({"entryId": elsewhere, "passwordRef": PASSWORD})),
    ))
    .expect_err("this site is not one of that login's");

    assert!(
        refused.message.starts_with("origin_mismatch:"),
        "the refusal is the gate's own: {}",
        refused.message
    );
    assert!(
        !refused.message.contains("refuses to read"),
        "the store was never asked for a password: {}",
        refused.message
    );
    assert_eq!(page.called("Input.insertText"), 0);
}

/// A field that is gone by the time the value has been read is the check the
/// brief asks for: the value is never typed into whatever is there now.
#[test]
fn a_field_that_is_gone_when_the_value_is_read_stops_the_fill() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let page = sign_in_page().refusing_after(
        "Runtime.callFunctionOn",
        "DOM.describeNode",
        CdpError::StaleRef,
    );

    let refused = tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab(),
        &page,
        &asking(json!({"entryId": saved, "passwordRef": PASSWORD})),
    ))
    .expect_err("the field went away");

    assert!(
        refused.message.contains("changed"),
        "the refusal says the page moved: {}",
        refused.message
    );
    assert_eq!(page.called("Input.insertText"), 0);
    scrub::forget("fill-login-tab");
}

/// A document that committed between the checks and the typing replaces the
/// page's own frame id, which is the only document token a caller can see.
#[test]
fn a_new_document_under_the_field_stops_the_fill() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    let seen = AtomicUsize::new(0);
    let page = FakePage::new()
        .answering_with("Page.getFrameTree", move |_| {
            let first = seen.fetch_add(1, Ordering::SeqCst) == 0;
            json!({"frameTree": {"frame": {
                "id": if first { "main" } else { "next-document" },
                "url": format!("{SITE}/sign-in"),
            }}})
        })
        .answering_with("DOM.describeNode", described)
        .answering_with(
            "DOM.resolveNode",
            |asked| json!({"object": {"objectId": format!("node-{}", asked["backendNodeId"])}}),
        )
        .answering_with(
            "Runtime.evaluate",
            |_| json!({ "result": { "type": "string", "value": "complete" } }),
        );

    let refused = tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab(),
        &page,
        &asking(json!({"entryId": saved, "passwordRef": PASSWORD})),
    ))
    .expect_err("the page committed another document");

    assert!(refused.message.contains("changed"), "{}", refused.message);
    assert_eq!(page.called("Input.insertText"), 0);
    scrub::forget("fill-login-tab");
}

/// The value typed is remembered for the scrub, so what the page makes of it —
/// a show-password toggle, a console entry — does not reach an agent after it.
#[test]
fn a_typed_password_is_taken_out_of_what_the_tab_says_afterwards() {
    let over = Over::new();
    let saved = over.save("Shop account", SITE);
    tauri::async_runtime::block_on(fill(
        &over.vault,
        &tab(),
        &sign_in_page(),
        &asking(json!({"entryId": saved, "passwordRef": PASSWORD})),
    ))
    .expect("the login was typed");

    let mut afterwards = json!({
        "entries": [{"level": "info", "text": format!("the page logged {SECRET}")}],
        "view": format!("- textbox \"Password\" [value=\"{SECRET}\"]"),
    });
    scrub::clean(&mut afterwards);

    let words = afterwards.to_string();
    assert!(
        !words.contains(SECRET),
        "the page's own words carried it: {words}"
    );
    scrub::forget("fill-login-tab");
}
